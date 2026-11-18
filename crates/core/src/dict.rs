//! Yomitan-format dictionary import (format 1–3) and dictionary management.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::db::Db;

#[derive(Debug, Clone, Serialize)]
pub struct DictInfo {
    pub id: i64,
    pub title: String,
    pub revision: Option<String>,
    pub attribution: Option<String>,
    pub priority: i64,
    pub enabled: bool,
    pub term_count: i64,
    pub meta_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub id: i64,
    pub title: String,
    pub terms: usize,
    pub meta: usize,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Bank {
    Tag,
    Term,
    TermMeta,
}

/// Yomitan freq payloads: number | string | {value, displayValue} | {reading, frequency}.
pub(crate) fn parse_freq(data: &Value) -> (Option<String>, Option<f64>, Option<String>) {
    let mut reading = None;
    let mut data = data;
    if let Some(obj) = data.as_object() {
        if let Some(f) = obj.get("frequency") {
            reading = obj.get("reading").and_then(|v| v.as_str()).map(String::from);
            data = f;
        }
    }
    let (value, display) = match data {
        Value::Object(o) => (o.get("value").cloned(), o.get("displayValue").and_then(|v| v.as_str()).map(String::from)),
        other => (Some(other.clone()), None),
    };
    let num = match &value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => {
            let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            digits.parse().ok()
        }
        _ => None,
    };
    let display = display.or_else(|| match &value {
        Some(Value::String(s)) => Some(s.clone()),
        _ => num.map(|n| if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }),
    });
    (reading, num, display)
}

fn s(v: Option<&Value>) -> String {
    v.and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// Import one Yomitan zip. An existing dictionary with the same title is replaced.
pub fn import_zip(db: &Db, path: &Path, enabled: bool, mut progress: impl FnMut(&str, usize, usize)) -> Result<ImportResult> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a zip archive")?;

    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().and_then(|f| f.name().ok().map(|n| n.into_owned())))
        .collect();
    let index_name = names
        .iter()
        .find(|n| n.rsplit('/').next() == Some("index.json"))
        .cloned()
        .context("index.json not found (not a Yomitan dictionary?)")?;
    let index: Value = {
        let mut buf = Vec::new();
        zip.by_name(&index_name)?.read_to_end(&mut buf)?;
        serde_json::from_slice(&buf)?
    };
    let title = index["title"].as_str().map(String::from).unwrap_or_else(|| {
        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    });
    let format = index["format"].as_i64().or_else(|| index["version"].as_i64()).unwrap_or(0);
    if !(1..=3).contains(&format) {
        bail!("{title}: unsupported dictionary format {format}");
    }

    let mut banks: Vec<(Bank, u32, String)> = names
        .iter()
        .filter_map(|n| {
            let base = n.rsplit('/').next()?;
            let stem = base.strip_suffix(".json")?;
            let (kind, num) = stem.rsplit_once('_')?;
            let num: u32 = num.parse().ok()?;
            let kind = match kind {
                "term_bank" => Bank::Term,
                "term_meta_bank" => Bank::TermMeta,
                "tag_bank" => Bank::Tag,
                _ => return None,
            };
            Some((kind, num, n.clone()))
        })
        .collect();
    banks.sort();

    let mut conn = db.lock();
    let tx = conn.transaction()?;
    let existing: Option<i64> =
        tx.query_row("SELECT id FROM dictionaries WHERE title=?1", [&title], |r| r.get(0)).optional()?;
    let did = match existing {
        Some(id) => {
            for table in ["terms", "term_meta", "tags"] {
                tx.execute(&format!("DELETE FROM {table} WHERE dict_id=?1"), [id])?;
            }
            id
        }
        None => {
            let prio: i64 = tx.query_row("SELECT COALESCE(MAX(priority), 0) + 1 FROM dictionaries", [], |r| r.get(0))?;
            tx.execute(
                "INSERT INTO dictionaries(title, priority, enabled) VALUES(?1, ?2, ?3)",
                params![title, prio, enabled as i64],
            )?;
            tx.last_insert_rowid()
        }
    };

    let (mut terms, mut meta) = (0usize, 0usize);
    {
        let mut ins_term = tx.prepare(
            "INSERT INTO terms(dict_id, expression, reading, def_tags, rules, score, glossary, sequence, term_tags)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        let mut ins_meta =
            tx.prepare("INSERT INTO term_meta(dict_id, term, mode, reading, value, display) VALUES(?1,?2,?3,?4,?5,?6)")?;
        let mut ins_tag =
            tx.prepare("INSERT INTO tags(dict_id, name, category, ord, notes, score) VALUES(?1,?2,?3,?4,?5,?6)")?;

        for (kind, _, name) in &banks {
            let mut buf = Vec::new();
            zip.by_name(name)?.read_to_end(&mut buf)?;
            let rows: Vec<Vec<Value>> = serde_json::from_slice(&buf).with_context(|| format!("parse {name}"))?;
            for r in rows {
                match kind {
                    Bank::Term => {
                        let expr = s(r.first());
                        let reading = s(r.get(1));
                        let reading = if reading.is_empty() { expr.clone() } else { reading };
                        let score = r.get(4).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let (gloss, seq, ttags) = if format == 1 {
                            (Value::Array(r.iter().skip(5).cloned().collect()), None, String::new())
                        } else {
                            (r.get(5).cloned().unwrap_or(Value::Array(vec![])), r.get(6).and_then(|v| v.as_i64()), s(r.get(7)))
                        };
                        ins_term.execute(params![
                            did,
                            expr,
                            reading,
                            s(r.get(2)),
                            s(r.get(3)),
                            score,
                            serde_json::to_string(&gloss)?,
                            seq,
                            ttags
                        ])?;
                        terms += 1;
                    }
                    Bank::TermMeta => {
                        if r.get(1).and_then(|v| v.as_str()) != Some("freq") {
                            continue; // pitch / ipa not used yet
                        }
                        let (reading, value, display) = parse_freq(r.get(2).unwrap_or(&Value::Null));
                        ins_meta.execute(params![did, s(r.first()), "freq", reading, value, display])?;
                        meta += 1;
                    }
                    Bank::Tag => {
                        ins_tag.execute(params![
                            did,
                            s(r.first()),
                            s(r.get(1)),
                            r.get(2).and_then(|v| v.as_i64()),
                            s(r.get(3)),
                            r.get(4).and_then(|v| v.as_f64())
                        ])?;
                    }
                }
            }
            progress(name, terms, meta);
        }
    }
    tx.execute(
        "UPDATE dictionaries SET revision=?1, attribution=?2, term_count=?3, meta_count=?4, imported_at=strftime('%s','now') WHERE id=?5",
        params![
            index["revision"].as_str(),
            index["attribution"].as_str().or(index["author"].as_str()),
            terms as i64,
            meta as i64,
            did
        ],
    )?;
    tx.commit()?;
    Ok(ImportResult { id: did, title, terms, meta })
}

pub fn list(db: &Db) -> Result<Vec<DictInfo>> {
    let c = db.lock();
    let mut st = c.prepare(
        "SELECT id, title, revision, attribution, priority, enabled, term_count, meta_count FROM dictionaries ORDER BY priority, id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(DictInfo {
            id: r.get(0)?,
            title: r.get(1)?,
            revision: r.get(2)?,
            attribution: r.get(3)?,
            priority: r.get(4)?,
            enabled: r.get::<_, i64>(5)? != 0,
            term_count: r.get(6)?,
            meta_count: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn set_enabled(db: &Db, id: i64, enabled: bool) -> Result<()> {
    db.lock().execute("UPDATE dictionaries SET enabled=?1 WHERE id=?2", params![enabled as i64, id])?;
    Ok(())
}

pub fn set_priority(db: &Db, id: i64, priority: i64) -> Result<()> {
    db.lock().execute("UPDATE dictionaries SET priority=?1 WHERE id=?2", params![priority, id])?;
    Ok(())
}

pub fn delete(db: &Db, id: i64) -> Result<()> {
    let mut c = db.lock();
    let tx = c.transaction()?;
    for table in ["terms", "term_meta", "tags"] {
        tx.execute(&format!("DELETE FROM {table} WHERE dict_id=?1"), [id])?;
    }
    tx.execute("DELETE FROM dictionaries WHERE id=?1", [id])?;
    tx.commit()?;
    Ok(())
}
