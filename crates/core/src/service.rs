//! Application facade shared by the desktop UI and the HTTP API.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use serde::{Deserialize, Serialize};

use crate::cards::{self, Card, CardWithSchedule, Definition, Explanation, Stats};
use crate::db::Db;
use crate::lookup::{self, LookupResult};
use crate::ocr::{OcrEngine, OcrPage};
use crate::srs::{ReviewLog, Schedule, Scheduler, SrsConfig, format_interval};
use crate::tokenize::{self, Ruby};
use crate::{models, now_ms};

const IMAGE_CACHE: usize = 12;
const OCR_CACHE: usize = 128;

/// Setting key: "1" shows all four rating buttons.
pub const FOUR_BUTTONS: &str = "review.four_buttons";

pub struct Service {
    pub db: Db,
    ocr: Mutex<Option<Arc<OcrEngine>>>,
    ocr_cache: Mutex<Lru<OcrPage>>,
    images: Mutex<Lru<Arc<DynamicImage>>>,
}

struct Lru<T> {
    cap: usize,
    order: VecDeque<String>,
    map: HashMap<String, T>,
}

impl<T: Clone> Lru<T> {
    fn new(cap: usize) -> Self {
        Self { cap, order: VecDeque::new(), map: HashMap::new() }
    }
    fn get(&mut self, k: &str) -> Option<T> {
        let v = self.map.get(k)?.clone();
        self.order.retain(|x| x != k);
        self.order.push_back(k.to_string());
        Some(v)
    }
    fn put(&mut self, k: String, v: T) {
        self.order.retain(|x| x != &k);
        self.order.push_back(k.clone());
        self.map.insert(k, v);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LookupResponse {
    #[serde(flatten)]
    pub result: LookupResult,
    /// Furigana for the whole text (the sentence), for display.
    pub furigana: Vec<Ruby>,
    /// Card ids already saved for each entry (same order as `entries`; None if not saved).
    pub saved: Vec<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AddCardRequest {
    pub expression: String,
    pub reading: String,
    #[serde(default)]
    pub sentence: String,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub source_title: Option<String>,
    /// OCR image key + region to crop for the card picture.
    #[serde(default)]
    pub image_key: Option<String>,
    #[serde(default)]
    pub crop: Option<[f32; 4]>,
    /// Or an already-encoded picture (base64 / data URL).
    #[serde(default)]
    pub image_b64: Option<String>,
    #[serde(default)]
    pub explanation: Option<Explanation>,
    #[serde(default)]
    pub frequency: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    #[serde(flatten)]
    pub card: CardWithSchedule,
    /// Due time for again / hard / good / easy.
    pub preview: [i64; 4],
    pub preview_labels: [String; 4],
}

impl Service {
    pub fn open() -> Result<Arc<Self>> {
        Ok(Arc::new(Self::with_db(Db::open_default()?)))
    }

    pub fn with_db(db: Db) -> Self {
        Self { db, ocr: Mutex::new(None), ocr_cache: Mutex::new(Lru::new(OCR_CACHE)), images: Mutex::new(Lru::new(IMAGE_CACHE)) }
    }

    pub fn scheduler(&self) -> Scheduler {
        let mut cfg = SrsConfig::default();
        if let Ok(Some(v)) = self.db.get_setting("srs.desired_retention") {
            if let Ok(r) = v.parse::<f32>() {
                cfg.desired_retention = r.clamp(0.7, 0.99);
            }
        }
        Scheduler::new(cfg)
    }

    /// Review with 重来/困难/良好/简单 instead of the default 忘了/记得.
    pub fn four_buttons(&self) -> bool {
        self.db.get_setting(FOUR_BUTTONS).ok().flatten().as_deref() == Some("1")
    }

    pub fn new_cards_per_day(&self) -> usize {
        self.db.get_setting("srs.new_per_day").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(20)
    }

    // --- OCR -------------------------------------------------------------------

    pub fn ocr_ready(&self) -> bool {
        models::all_present()
    }

    pub fn ocr_engine(&self) -> Result<Arc<OcrEngine>> {
        let mut g = self.ocr.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.as_ref() {
            return Ok(e.clone());
        }
        if !models::all_present() {
            bail!("OCR models are not downloaded yet");
        }
        let e = Arc::new(OcrEngine::load()?);
        *g = Some(e.clone());
        Ok(e)
    }

    /// OCR an image; results and the decoded image are cached under `key` (e.g. its URL).
    pub fn ocr(&self, key: &str, img: Arc<DynamicImage>) -> Result<OcrPage> {
        if let Some(hit) = self.ocr_cache.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
            self.images.lock().unwrap_or_else(|e| e.into_inner()).put(key.to_string(), img);
            return Ok(hit);
        }
        let page = self.ocr_engine()?.read_page(&img)?;
        self.ocr_cache.lock().unwrap_or_else(|e| e.into_inner()).put(key.to_string(), page.clone());
        self.images.lock().unwrap_or_else(|e| e.into_inner()).put(key.to_string(), img);
        Ok(page)
    }

    pub fn ocr_cached(&self, key: &str) -> Option<OcrPage> {
        self.ocr_cache.lock().unwrap_or_else(|e| e.into_inner()).get(key)
    }

    pub fn ocr_bytes(&self, key: &str, bytes: &[u8]) -> Result<OcrPage> {
        if let Some(hit) = self.ocr_cached(key) {
            if self.images.lock().unwrap_or_else(|e| e.into_inner()).get(key).is_some() {
                return Ok(hit);
            }
        }
        let img = Arc::new(crate::ocr::decode(bytes).context("decode image")?);
        self.ocr(key, img)
    }

    // --- lookup ----------------------------------------------------------------

    pub fn lookup(&self, text: &str, offset: usize) -> Result<LookupResponse> {
        let bases = tokenize::bases_at(text, offset);
        let result = lookup::lookup(&self.db, text, offset, &bases)?;
        let saved = result
            .entries
            .iter()
            .map(|e| cards::find_card(&self.db, &e.expression, &e.reading).ok().flatten().map(|c| c.id))
            .collect();
        Ok(LookupResponse { furigana: tokenize::furigana(text), result, saved })
    }

    /// Definitions of a word from every enabled dictionary (snapshot for a card).
    pub fn definitions(&self, expression: &str, reading: &str) -> Result<(Vec<Definition>, Option<String>)> {
        let r = lookup::lookup(&self.db, expression, 0, &[])?;
        let mut defs = Vec::new();
        let mut freq = None;
        for e in r.entries.iter().filter(|e| e.expression == expression && (reading.is_empty() || e.reading == reading)) {
            if e.source_length != expression.chars().count() {
                continue;
            }
            freq = freq.or_else(|| e.frequencies.first().and_then(|f| f.display.clone()));
            defs.push(Definition { dict: e.dict.clone(), def_tags: e.def_tags.clone(), glossary: e.glossary.clone() });
        }
        Ok((defs, freq))
    }

    // --- cards -----------------------------------------------------------------

    pub fn add_card(&self, req: &AddCardRequest) -> Result<Card> {
        if req.expression.trim().is_empty() {
            bail!("empty expression");
        }
        if let Some(existing) = cards::find_card(&self.db, &req.expression, &req.reading)? {
            return Ok(existing);
        }
        let (definitions, freq) = self.definitions(&req.expression, &req.reading)?;
        let mut card = Card::new(&req.expression, &req.reading);
        card.definitions = definitions;
        card.frequency = req.frequency.clone().or(freq);
        card.sentence = req.sentence.trim().to_string();
        card.sentence_furigana = if card.sentence.is_empty() { vec![] } else { tokenize::furigana(&card.sentence) };
        card.source_url = req.source_url.clone();
        card.source_title = req.source_title.clone();
        card.explanation = req.explanation.clone();
        if let Some(b64) = &req.image_b64 {
            use base64::Engine;
            let data = b64.split_once(',').map(|(_, d)| d).unwrap_or(b64);
            let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim())?;
            card.media_id = Some(cards::save_media(&self.db, &bytes, "image/jpeg")?);
        } else if let (Some(key), Some(bbox)) = (&req.image_key, req.crop) {
            let img = self.images.lock().unwrap_or_else(|e| e.into_inner()).get(key);
            if let Some(img) = img {
                card.media_id = Some(cards::save_crop(&self.db, &img, bbox, 12.0)?);
            }
        }
        cards::save_card(&self.db, &card)?;
        Ok(card)
    }

    /// Generate the Chinese explanation for a saved card that has none (runs
    /// after "add to word list"; lookups no longer call the LLM by themselves).
    /// Returns the updated card, or None when nothing was needed or no LLM is set up.
    pub async fn fill_explanation(&self, card_id: &str) -> Result<Option<Card>> {
        let Some(card) = cards::get_card(&self.db, card_id)? else { return Ok(None) };
        if card.explanation.is_some() || card.deleted || !crate::explain::LlmConfig::load(&self.db)?.is_configured() {
            return Ok(None);
        }
        let req = crate::explain::ExplainRequest {
            expression: card.expression.clone(),
            reading: card.reading.clone(),
            sentence: card.sentence.clone(),
            definition_hint: card.definitions.first().map(|d| crate::explain::gloss_text(&d.glossary, 200)).unwrap_or_default(),
        };
        let explanation = crate::explain::explain(&self.db, &req).await?;
        // Re-read: the card may have changed while the LLM was answering.
        let Some(mut card) = cards::get_card(&self.db, card_id)? else { return Ok(None) };
        if card.explanation.is_some() || card.deleted {
            return Ok(None);
        }
        card.explanation = Some(explanation);
        Ok(Some(self.update_card(card)?))
    }

    pub fn update_card(&self, mut card: Card) -> Result<Card> {
        card.updated_at = now_ms();
        cards::save_card(&self.db, &card)?;
        Ok(card)
    }

    pub fn delete_card(&self, id: &str) -> Result<()> {
        cards::delete_card(&self.db, id)
    }

    pub fn cards(&self) -> Result<Vec<CardWithSchedule>> {
        cards::all_cards(&self.db, &self.scheduler())
    }

    pub fn queue(&self) -> Result<Vec<QueueItem>> {
        let sched = self.scheduler();
        let all = cards::all_cards(&self.db, &sched)?;
        let now = now_ms();
        let new_today = self.new_cards_started_today(&all)?;
        let limit = self.new_cards_per_day().saturating_sub(new_today);
        Ok(cards::review_queue(&all, now, limit)
            .into_iter()
            .map(|c| {
                let preview = sched.preview(&c.schedule, now);
                let preview_labels = preview.map(|due| format_interval(due - now));
                QueueItem { card: c, preview, preview_labels }
            })
            .collect())
    }

    fn day_start(&self, now: i64) -> i64 {
        // Local midnight, shifted by the "day starts at" hour (default 4:00 like Anki).
        let hour: i64 = self.db.get_setting("srs.day_start_hour").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(4);
        let offset = local_utc_offset_ms();
        let local = now + offset - hour * 3_600_000;
        let start_local = local - local.rem_euclid(86_400_000);
        start_local - offset + hour * 3_600_000
    }

    fn new_cards_started_today(&self, all: &[CardWithSchedule]) -> Result<usize> {
        let start = self.day_start(now_ms());
        let c = self.db.lock();
        let mut st = c.prepare("SELECT card_id, MIN(reviewed_at) FROM review_logs GROUP BY card_id HAVING MIN(reviewed_at) >= ?1")?;
        let n = st.query_map([start], |r| r.get::<_, String>(0))?.count();
        let _ = all;
        Ok(n)
    }

    pub fn review(&self, card_id: &str, rating: u8, device_id: &str, duration_ms: Option<i64>) -> Result<Schedule> {
        let log = ReviewLog {
            id: uuid::Uuid::now_v7().to_string(),
            card_id: card_id.into(),
            rating,
            reviewed_at: now_ms(),
            device_id: device_id.into(),
            duration_ms,
        };
        cards::add_review(&self.db, &log)?;
        Ok(self.scheduler().replay(&cards::logs_for(&self.db, card_id)?))
    }

    pub fn stats(&self) -> Result<Stats> {
        let sched = self.scheduler();
        let all = cards::all_cards(&self.db, &sched)?;
        let now = now_ms();
        let start = self.day_start(now);
        let c = self.db.lock();
        let reviewed_today: i64 = c.query_row("SELECT COUNT(*) FROM review_logs WHERE reviewed_at >= ?1", [start], |r| r.get(0))?;
        // Streak: consecutive days (ending today or yesterday) with at least one review.
        let mut st = c.prepare("SELECT reviewed_at FROM review_logs ORDER BY reviewed_at DESC")?;
        let mut days = std::collections::BTreeSet::new();
        for t in st.query_map([], |r| r.get::<_, i64>(0))? {
            days.insert((start - t?).div_euclid(86_400_000) + 1);
        }
        let mut streak = 0;
        let first = if days.contains(&0) { 0 } else { 1 };
        while days.contains(&(first + streak as i64)) {
            streak += 1;
        }
        let queue_due = all.iter().filter(|x| x.schedule.phase != crate::srs::Phase::New && x.schedule.due <= now).count();
        Ok(Stats {
            total: all.len(),
            new: all.iter().filter(|x| x.schedule.phase == crate::srs::Phase::New).count(),
            due_now: queue_due,
            reviewed_today: reviewed_today as usize,
            streak_days: streak,
        })
    }
}

/// Local timezone offset in ms (macOS / unix via libc-free `date +%z`, cached).
fn local_utc_offset_ms() -> i64 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if s.len() == 5 {
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let h: i64 = s[1..3].parse().unwrap_or(0);
            let m: i64 = s[3..5].parse().unwrap_or(0);
            sign * (h * 60 + m) * 60_000
        } else {
            0
        }
    })
}
