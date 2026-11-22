-- pic_yomi cloud sync hub (Cloudflare D1). Same shape as the Mac's tables.
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

CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);

-- Card pictures and pronunciation clips (key: media/<id> or audio/<hash>).
CREATE TABLE IF NOT EXISTS files (key TEXT PRIMARY KEY, type TEXT, data BLOB NOT NULL);
