//! SQLite storage: dictionaries, settings, cards, review logs, media.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE IF NOT EXISTS dictionaries (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL UNIQUE,
    revision TEXT,
    attribution TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    term_count INTEGER NOT NULL DEFAULT 0,
    meta_count INTEGER NOT NULL DEFAULT 0,
    imported_at REAL
);

CREATE TABLE IF NOT EXISTS terms (
    id INTEGER PRIMARY KEY,
    dict_id INTEGER NOT NULL,
    expression TEXT NOT NULL,
    reading TEXT NOT NULL,
    def_tags TEXT,
    rules TEXT,
    score REAL,
    glossary TEXT NOT NULL,
    sequence INTEGER,
    term_tags TEXT
);
CREATE INDEX IF NOT EXISTS terms_expression ON terms(expression);
CREATE INDEX IF NOT EXISTS terms_reading ON terms(reading);

CREATE TABLE IF NOT EXISTS term_meta (
    dict_id INTEGER NOT NULL,
    term TEXT NOT NULL,
    mode TEXT NOT NULL,
    reading TEXT,
    value REAL,
    display TEXT
);
CREATE INDEX IF NOT EXISTS term_meta_term ON term_meta(term);

CREATE TABLE IF NOT EXISTS tags (
    dict_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    category TEXT,
    ord INTEGER,
    notes TEXT,
    score REAL
);

CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE IF NOT EXISTS explain_cache (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    created_at REAL
);

CREATE TABLE IF NOT EXISTS cards (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    seq INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS cards_seq ON cards(seq);

CREATE TABLE IF NOT EXISTS review_logs (
    id TEXT PRIMARY KEY,
    card_id TEXT NOT NULL,
    data TEXT NOT NULL,
    reviewed_at INTEGER NOT NULL,
    seq INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS review_logs_seq ON review_logs(seq);
CREATE INDEX IF NOT EXISTS review_logs_card ON review_logs(card_id);

CREATE TABLE IF NOT EXISTS audio_cache (
    key TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    mime TEXT NOT NULL,
    filename TEXT NOT NULL,
    created_at INTEGER
);

CREATE TABLE IF NOT EXISTS media (
    id TEXT PRIMARY KEY,
    mime TEXT NOT NULL,
    filename TEXT NOT NULL,
    created_at REAL
);
"#;

/// Where all local data lives: `$PICYOMI_DATA`, else `~/Library/Application Support/pic_yomi`.
pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("PICYOMI_DATA").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join("Library/Application Support/pic_yomi")
    });
    let _ = std::fs::create_dir_all(dir.join("media"));
    let _ = std::fs::create_dir_all(dir.join("models"));
    dir
}

/// One shared connection behind a mutex; cheap to clone.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    pub path: PathBuf,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA cache_size=-32000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path })
    }

    pub fn open_default() -> Result<Self> {
        Self::open(data_dir().join("picyomi.db"))
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: PathBuf::from(":memory:") })
    }

    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .lock()
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        let c = self.lock();
        match value {
            Some(v) => c.execute(
                "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, v],
            )?,
            None => c.execute("DELETE FROM settings WHERE key=?1", [key])?,
        };
        Ok(())
    }
}

/// Monotonic change counter used as the sync cursor (immune to device clock skew).
pub fn next_seq(c: &Connection) -> Result<i64> {
    let cur: Option<String> = c.query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0)).optional()?;
    let n = cur.and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) + 1;
    c.execute(
        "INSERT INTO meta(key, value) VALUES('seq', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [n.to_string()],
    )?;
    Ok(n)
}

pub fn current_seq(c: &Connection) -> Result<i64> {
    let cur: Option<String> = c.query_row("SELECT value FROM meta WHERE key='seq'", [], |r| r.get(0)).optional()?;
    Ok(cur.and_then(|v| v.parse().ok()).unwrap_or(0))
}
