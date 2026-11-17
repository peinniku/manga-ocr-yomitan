//! Word cards, review logs, media, and the sync merge.

use std::collections::HashMap;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{Db, current_seq, data_dir, next_seq};
use crate::srs::{ReviewLog, Schedule, Scheduler};
use crate::tokenize::Ruby;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// One definition snapshot taken from a dictionary at the time the card was made.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Definition {
    pub dict: String,
    #[serde(default)]
    pub def_tags: String,
    pub glossary: Value,
}

/// Chinese helper explanation produced by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Explanation {
    #[serde(default)]
    pub meaning_zh: String,
    #[serde(default)]
    pub gloss_zh: String,
    #[serde(default)]
    pub sentence_zh: String,
    #[serde(default)]
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Example {
    pub ja: String,
    #[serde(default)]
    pub zh: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: String,
    pub expression: String,
    pub reading: String,
    #[serde(default)]
    pub definitions: Vec<Definition>,
    #[serde(default)]
    pub frequency: Option<String>,
    #[serde(default)]
    pub explanation: Option<Explanation>,
    /// The sentence the word was found in.
    #[serde(default)]
    pub sentence: String,
    #[serde(default)]
    pub sentence_furigana: Vec<Ruby>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub source_title: Option<String>,
    /// Crop of the speech bubble / screen region.
    #[serde(default)]
    pub media_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub deleted: bool,
}

impl Card {
    pub fn new(expression: &str, reading: &str) -> Self {
        let now = now_ms();
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            expression: expression.into(),
            reading: reading.into(),
            definitions: vec![],
            frequency: None,
            explanation: None,
            sentence: String::new(),
            sentence_furigana: vec![],
            source_url: None,
            source_title: None,
            media_id: None,
            tags: vec![],
            note: String::new(),
            created_at: now,
            updated_at: now,
            deleted: false,
        }
    }
}

/// A card together with its derived schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardWithSchedule {
    #[serde(flatten)]
    pub card: Card,
    pub schedule: Schedule,
}

fn upsert_card(c: &Connection, card: &Card) -> Result<()> {
    let seq = next_seq(c)?;
    c.execute(
        "INSERT INTO cards(id, data, updated_at, deleted, seq) VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET data=excluded.data, updated_at=excluded.updated_at,
           deleted=excluded.deleted, seq=excluded.seq",
        params![card.id, serde_json::to_string(card)?, card.updated_at, card.deleted as i64, seq],
    )?;
    Ok(())
}

fn insert_log(c: &Connection, log: &ReviewLog) -> Result<bool> {
    let exists: Option<i64> = c.query_row("SELECT 1 FROM review_logs WHERE id=?1", [&log.id], |r| r.get(0)).optional()?;
    if exists.is_some() {
        return Ok(false);
    }
    let seq = next_seq(c)?;
    c.execute(
        "INSERT INTO review_logs(id, card_id, data, reviewed_at, seq) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![log.id, log.card_id, serde_json::to_string(log)?, log.reviewed_at, seq],
    )?;
    Ok(true)
}

pub fn save_card(db: &Db, card: &Card) -> Result<()> {
    let mut c = db.lock();
    let tx = c.transaction()?;
    upsert_card(&tx, card)?;
    tx.commit()?;
    Ok(())
}

pub fn get_card(db: &Db, id: &str) -> Result<Option<Card>> {
    let c = db.lock();
    let data: Option<String> = c.query_row("SELECT data FROM cards WHERE id=?1", [id], |r| r.get(0)).optional()?;
    data.map(|d| serde_json::from_str(&d).context("card json")).transpose()
}

/// A live (not deleted) card with the same expression and reading, if any.
pub fn find_card(db: &Db, expression: &str, reading: &str) -> Result<Option<Card>> {
    let c = db.lock();
    let mut st = c.prepare_cached("SELECT data FROM cards WHERE deleted=0 AND json_extract(data, '$.expression')=?1")?;
    let rows = st.query_map([expression], |r| r.get::<_, String>(0))?;
    for row in rows {
        let card: Card = serde_json::from_str(&row?)?;
        if card.reading == reading || reading.is_empty() {
            return Ok(Some(card));
        }
    }
    Ok(None)
}

pub fn delete_card(db: &Db, id: &str) -> Result<()> {
    if let Some(mut card) = get_card(db, id)? {
        card.deleted = true;
        card.updated_at = now_ms();
        save_card(db, &card)?;
    }
    Ok(())
}

pub fn add_review(db: &Db, log: &ReviewLog) -> Result<()> {
    let mut c = db.lock();
    let tx = c.transaction()?;
    insert_log(&tx, log)?;
    tx.commit()?;
    Ok(())
}

pub fn logs_for(db: &Db, card_id: &str) -> Result<Vec<ReviewLog>> {
    let c = db.lock();
    let mut st = c.prepare_cached("SELECT data FROM review_logs WHERE card_id=?1")?;
    let rows = st.query_map([card_id], |r| r.get::<_, String>(0))?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}

/// All live cards with schedules derived from their logs.
pub fn all_cards(db: &Db, sched: &Scheduler) -> Result<Vec<CardWithSchedule>> {
    let (cards, logs) = {
        let c = db.lock();
        let mut st = c.prepare("SELECT data FROM cards WHERE deleted=0")?;
        let cards: Vec<Card> =
            st.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect();
        let mut st = c.prepare("SELECT data FROM review_logs")?;
        let logs: Vec<ReviewLog> =
            st.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect();
        (cards, logs)
    };
    let mut by_card: HashMap<&str, Vec<ReviewLog>> = HashMap::new();
    for l in &logs {
        by_card.entry(l.card_id.as_str()).or_default().push(l.clone());
    }
    let mut out: Vec<CardWithSchedule> = cards
        .into_iter()
        .map(|card| {
            let schedule = by_card.get(card.id.as_str()).map(|l| sched.replay(l)).unwrap_or_default();
            CardWithSchedule { card, schedule }
        })
        .collect();
    out.sort_by(|a, b| b.card.created_at.cmp(&a.card.created_at));
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stats {
    pub total: usize,
    pub new: usize,
    pub due_now: usize,
    pub reviewed_today: usize,
    pub streak_days: u32,
}

/// Review queue at `now`: due learning/review cards first (oldest due first), then up to `new_limit` new cards.
pub fn review_queue(cards: &[CardWithSchedule], now: i64, new_limit: usize) -> Vec<CardWithSchedule> {
    use crate::srs::Phase;
    let mut due: Vec<&CardWithSchedule> =
        cards.iter().filter(|c| c.schedule.phase != Phase::New && c.schedule.due <= now).collect();
    due.sort_by_key(|c| c.schedule.due);
    let mut new: Vec<&CardWithSchedule> = cards.iter().filter(|c| c.schedule.phase == Phase::New).collect();
    new.sort_by_key(|c| c.card.created_at);
    due.into_iter().chain(new.into_iter().take(new_limit)).cloned().collect()
}

// --- media -------------------------------------------------------------------

pub fn save_media(db: &Db, bytes: &[u8], mime: &str) -> Result<String> {
    let id = uuid::Uuid::now_v7().simple().to_string();
    let ext = match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let filename = format!("{id}.{ext}");
    std::fs::write(data_dir().join("media").join(&filename), bytes)?;
    db.lock().execute(
        "INSERT INTO media(id, mime, filename, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![id, mime, filename, now_ms() as f64 / 1000.0],
    )?;
    Ok(id)
}

pub fn load_media(db: &Db, id: &str) -> Result<Option<(Vec<u8>, String)>> {
    let row: Option<(String, String)> = db
        .lock()
        .query_row("SELECT mime, filename FROM media WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((mime, filename)) = row else { return Ok(None) };
    let path = data_dir().join("media").join(filename);
    Ok(std::fs::read(path).ok().map(|b| (b, mime)))
}

/// Save a cropped region of an image as a small JPEG for a card.
pub fn save_crop(db: &Db, img: &image::DynamicImage, bbox: [f32; 4], margin: f32) -> Result<String> {
    use image::GenericImageView;
    let (w, h) = img.dimensions();
    let x0 = (bbox[0] - margin).max(0.0) as u32;
    let y0 = (bbox[1] - margin).max(0.0) as u32;
    let x1 = ((bbox[2] + margin) as u32).min(w).max(x0 + 1);
    let y1 = ((bbox[3] + margin) as u32).min(h).max(y0 + 1);
    let mut crop = img.crop_imm(x0, y0, x1 - x0, y1 - y0);
    if crop.width() > 720 || crop.height() > 720 {
        crop = crop.resize(720, 720, image::imageops::FilterType::Triangle);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82);
    crop.to_rgb8().write_with_encoder(enc)?;
    save_media(db, &buf.into_inner(), "image/jpeg")
}

// --- sync --------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncRequest {
    #[serde(default)]
    pub device_id: String,
    /// Last server cursor this device has seen.
    #[serde(default)]
    pub since: i64,
    #[serde(default)]
    pub cards: Vec<Card>,
    #[serde(default)]
    pub logs: Vec<ReviewLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncResponse {
    /// New cursor to send as `since` next time.
    pub seq: i64,
    pub cards: Vec<Card>,
    pub logs: Vec<ReviewLog>,
}

/// Merge a device's changes and return everything newer than its cursor.
/// Cards: last writer (by `updated_at`) wins. Logs: union by id.
pub fn sync(db: &Db, req: &SyncRequest) -> Result<SyncResponse> {
    let mut c = db.lock();
    let tx = c.transaction()?;
    for card in &req.cards {
        let existing: Option<i64> =
            tx.query_row("SELECT updated_at FROM cards WHERE id=?1", [&card.id], |r| r.get(0)).optional()?;
        if existing.is_none_or(|t| t < card.updated_at) {
            upsert_card(&tx, card)?;
        }
    }
    for log in &req.logs {
        insert_log(&tx, log)?;
    }
    let seq = current_seq(&tx)?;
    let cards: Vec<Card> = {
        let mut st = tx.prepare("SELECT data FROM cards WHERE seq > ?1 ORDER BY seq")?;
        st.query_map([req.since], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect()
    };
    let logs: Vec<ReviewLog> = {
        let mut st = tx.prepare("SELECT data FROM review_logs WHERE seq > ?1 ORDER BY seq")?;
        st.query_map([req.since], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect()
    };
    tx.commit()?;
    Ok(SyncResponse { seq, cards, logs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srs::SrsConfig;

    fn log(id: &str, card: &str, rating: u8, at: i64, dev: &str) -> ReviewLog {
        ReviewLog { id: id.into(), card_id: card.into(), rating, reviewed_at: at, device_id: dev.into(), duration_ms: None }
    }

    #[test]
    fn offline_reviews_on_two_devices_converge() {
        let db = Db::in_memory().unwrap();
        let sched = Scheduler::new(SrsConfig::default());
        let card = Card::new("守宮", "やもり");
        save_card(&db, &card).unwrap();
        let t0 = 1_700_000_000_000;

        // Phone and Mac both start from cursor 0 and review offline.
        let phone = sync(&db, &SyncRequest { device_id: "phone".into(), since: 0, cards: vec![], logs: vec![log("p1", &card.id, 3, t0, "phone")] }).unwrap();
        let mac = sync(&db, &SyncRequest { device_id: "mac".into(), since: 0, cards: vec![], logs: vec![log("m1", &card.id, 1, t0 + 3_600_000, "mac")] }).unwrap();
        assert_eq!(mac.logs.len(), 2, "mac receives the phone's review too");

        // Re-sending is idempotent.
        let again = sync(&db, &SyncRequest { since: mac.seq, logs: vec![log("m1", &card.id, 1, t0 + 3_600_000, "mac")], ..Default::default() }).unwrap();
        assert!(again.logs.is_empty());

        let server = sched.replay(&logs_for(&db, &card.id).unwrap());
        let phone_view = sched.replay(&[phone.logs, sync(&db, &SyncRequest { since: 0, ..Default::default() }).unwrap().logs].concat());
        assert_eq!(server.reps, 2);
        assert_eq!(server.due, phone_view.due);
    }

    #[test]
    fn last_writer_wins_for_cards() {
        let db = Db::in_memory().unwrap();
        let mut card = Card::new("猫", "ねこ");
        card.updated_at = 100;
        save_card(&db, &card).unwrap();
        let mut stale = card.clone();
        stale.note = "old".into();
        stale.updated_at = 50;
        let mut fresh = card.clone();
        fresh.note = "new".into();
        fresh.updated_at = 200;
        sync(&db, &SyncRequest { cards: vec![stale], ..Default::default() }).unwrap();
        assert_eq!(get_card(&db, &card.id).unwrap().unwrap().note, "");
        sync(&db, &SyncRequest { cards: vec![fresh], ..Default::default() }).unwrap();
        assert_eq!(get_card(&db, &card.id).unwrap().unwrap().note, "new");
    }
}
