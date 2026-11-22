//! Sync with the pic_yomi cloud hub (a Cloudflare Worker, see `cloud/`).
//!
//! The Mac is one client of the hub, like the phone: it pushes cards and
//! review logs changed here since the last push, pulls what other devices
//! wrote, and uploads the card pictures and pronunciation clips the phone
//! needs (the phone can't make those itself). Same merge rules as everywhere:
//! cards are last-write-wins by `updated_at`, logs are deduplicated by id.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::cards::{self, Card, SyncRequest};
use crate::db::Db;
use crate::service::Service;
use crate::srs::ReviewLog;

pub const URL: &str = "cloud.url";
pub const TOKEN: &str = "cloud.token";
/// Hub cursor: the hub's change number we have seen.
const SEQ: &str = "cloud.seq";
/// Local change number pushed so far.
const PUSHED: &str = "cloud.pushed";
const SETTINGS_SENT: &str = "cloud.settings_sent";
pub const LAST_SYNC: &str = "cloud.last_sync";
pub const LAST_ERROR: &str = "cloud.last_error";

/// The hub accepts at most this many cards + logs per request.
const CHUNK: usize = 40;
/// Uploads per sync round, so one round stays short.
const UPLOADS_PER_ROUND: usize = 24;
const DEVICE_ID: &str = "mac";

#[derive(Debug, Clone)]
pub struct CloudConfig {
    pub url: String,
    pub token: String,
}

impl CloudConfig {
    pub fn load(db: &Db) -> Result<Option<Self>> {
        let url = db.get_setting(URL)?.unwrap_or_default();
        let token = db.get_setting(TOKEN)?.unwrap_or_default();
        Ok((!url.is_empty() && !token.is_empty()).then(|| Self { url: url.trim_end_matches('/').to_string(), token }))
    }

    /// Link that pairs the phone (opens the web app with the token).
    pub fn pair_url(&self) -> String {
        format!("{}/app/#pair={}", self.url, self.token)
    }
}

/// Point this Mac at a hub. Keeps an existing token (the hub has it as a
/// secret) unless there is none yet. Returns the token.
pub fn configure(db: &Db, url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1") && !url.starts_with("http://localhost") {
        bail!("the hub URL must be https://…");
    }
    let token = match db.get_setting(TOKEN)?.filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => {
            use base64::Engine;
            use rand::Rng;
            let bytes: [u8; 24] = rand::rng().random();
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        }
    };
    if db.get_setting(URL)?.as_deref() != Some(url) {
        // A different hub: start over (push everything, pull from the start).
        for k in [SEQ, PUSHED, SETTINGS_SENT, LAST_SYNC, LAST_ERROR] {
            db.set_setting(k, None)?;
        }
        db.lock().execute("DELETE FROM cloud_uploads", [])?;
    }
    db.set_setting(URL, Some(url))?;
    db.set_setting(TOKEN, Some(&token))?;
    nudge();
    Ok(token)
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub pushed_cards: usize,
    pub pushed_logs: usize,
    pub pulled_cards: usize,
    pub pulled_logs: usize,
    pub uploaded: usize,
    /// Pictures/clips still to upload (done in later rounds).
    pub pending_uploads: usize,
}

#[derive(Deserialize)]
struct HubResponse {
    seq: i64,
    cards: Vec<Card>,
    logs: Vec<ReviewLog>,
}

fn int(db: &Db, key: &str) -> Result<i64> {
    Ok(db.get_setting(key)?.and_then(|v| v.parse().ok()).unwrap_or(0))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(60)).user_agent(concat!("pic_yomi/", env!("CARGO_PKG_VERSION"))).build()?)
}

/// One full round: push, pull, settings, uploads.
pub async fn sync_once(svc: &Service) -> Result<Report> {
    let db = &svc.db;
    let Some(cfg) = CloudConfig::load(db)? else { bail!("cloud sync is not set up") };
    let http = client()?;
    let mut report = Report::default();

    // Local changes since the last push (reading them through the local sync merge).
    let pushed = int(db, PUSHED)?;
    let local = cards::sync(db, &SyncRequest { since: pushed, ..Default::default() })?;
    report.pushed_cards = local.cards.len();
    report.pushed_logs = local.logs.len();

    let mut items: Vec<(Option<Card>, Option<ReviewLog>)> = Vec::new();
    items.extend(local.cards.into_iter().map(|c| (Some(c), None)));
    items.extend(local.logs.into_iter().map(|l| (None, Some(l))));
    let mut chunks: Vec<Vec<(Option<Card>, Option<ReviewLog>)>> = items.chunks(CHUNK).map(|c| c.to_vec()).collect();
    if chunks.is_empty() {
        chunks.push(vec![]); // still pull
    }
    for chunk in chunks {
        let since = int(db, SEQ)?;
        let body = serde_json::json!({
            "device_id": DEVICE_ID,
            "since": since,
            "cards": chunk.iter().filter_map(|(c, _)| c.clone()).collect::<Vec<_>>(),
            "logs": chunk.iter().filter_map(|(_, l)| l.clone()).collect::<Vec<_>>(),
        });
        let resp = http.post(format!("{}/api/sync", cfg.url)).bearer_auth(&cfg.token).json(&body).send().await.context("cloud unreachable")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("cloud sync {status}: {}", resp.text().await.unwrap_or_default());
        }
        let hub: HubResponse = resp.json().await?;
        // Our own echo is skipped by the merge (same updated_at / same log id).
        report.pulled_cards += hub.cards.len();
        report.pulled_logs += hub.logs.len();
        cards::sync(db, &SyncRequest { since: i64::MAX, cards: hub.cards, logs: hub.logs, ..Default::default() })?;
        db.set_setting(SEQ, Some(&hub.seq.to_string()))?;
    }
    db.set_setting(PUSHED, Some(&local.seq.to_string()))?;

    // Review settings for the phone (only when they changed).
    let settings = svc.srs_settings_json()?;
    let text = settings.to_string();
    if db.get_setting(SETTINGS_SENT)?.as_deref() != Some(text.as_str()) {
        let r = http.put(format!("{}/api/settings/srs", cfg.url)).bearer_auth(&cfg.token).json(&settings).send().await?;
        if r.status().is_success() {
            db.set_setting(SETTINGS_SENT, Some(&text))?;
        }
    }

    let (uploaded, pending) = upload_missing(svc, &cfg, &http, &settings).await?;
    report.uploaded = uploaded;
    report.pending_uploads = pending;
    db.set_setting(LAST_SYNC, Some(&crate::now_ms().to_string()))?;
    db.set_setting(LAST_ERROR, None)?;
    Ok(report)
}

fn uploaded(db: &Db, key: &str) -> bool {
    db.lock().query_row("SELECT 1 FROM cloud_uploads WHERE key=?1", [key], |_| Ok(())).is_ok()
}

fn mark_uploaded(db: &Db, key: &str) -> Result<()> {
    db.lock().execute("INSERT OR IGNORE INTO cloud_uploads(key, at) VALUES(?1, ?2)", params![key, crate::now_ms()])?;
    Ok(())
}

/// Upload card pictures and pronunciation clips the hub doesn't have yet.
/// Returns (uploaded now, still missing).
async fn upload_missing(svc: &Service, cfg: &CloudConfig, http: &reqwest::Client, settings: &serde_json::Value) -> Result<(usize, usize)> {
    let db = &svc.db;
    let voice = settings["voice"].as_str().unwrap_or("").to_string();
    let rev = crate::audio::TTS_REV;
    // The web app asks for clips with exactly these parameters (web/src/audio.js).
    enum Job {
        Media(String),
        Audio { e: String, r: String, s: String },
    }
    let mut jobs: Vec<(String, Job)> = Vec::new();
    for c in cards::all_cards(db, &svc.scheduler())?.into_iter().map(|c| c.card).filter(|c| !c.deleted) {
        if let Some(m) = &c.media_id {
            jobs.push((format!("media|{m}"), Job::Media(m.clone())));
        }
        let mut sentences = vec![String::new()];
        if !c.sentence.is_empty() {
            sentences.push(c.sentence.clone());
        }
        for s in sentences {
            let key = format!("audio|{voice}|{rev}|{s}|{}|{}", c.expression, c.reading);
            jobs.push((key, Job::Audio { e: c.expression.clone(), r: c.reading.clone(), s }));
        }
    }
    let missing: Vec<(String, Job)> = jobs.into_iter().filter(|(k, _)| !uploaded(db, k)).collect();
    let total = missing.len();
    let mut done = 0;
    for (key, job) in missing.into_iter().take(UPLOADS_PER_ROUND) {
        let res: Result<()> = async {
            let (url, bytes, mime) = match job {
                Job::Media(id) => {
                    let Some((bytes, mime)) = cards::load_media(db, &id)? else { return Ok(()) };
                    (format!("{}/api/media/{id}", cfg.url), bytes, mime)
                }
                Job::Audio { e, r, s } => {
                    let a = if s.is_empty() { crate::audio::word(db, &e, &r).await? } else { crate::audio::sentence(db, &s, &e, &r).await? };
                    let q = reqwest::Url::parse_with_params(
                        &format!("{}/api/audio", cfg.url),
                        &[("e", e.as_str()), ("r", r.as_str()), ("s", s.as_str()), ("v", voice.as_str()), ("a", rev)],
                    )?;
                    (q.to_string(), a.bytes, a.mime)
                }
            };
            let resp = http.put(url).bearer_auth(&cfg.token).header("content-type", mime).body(bytes).send().await?;
            if !resp.status().is_success() {
                bail!("upload {}", resp.status());
            }
            Ok(())
        }
        .await;
        match res {
            Ok(()) => {
                mark_uploaded(db, &key)?;
                done += 1;
            }
            Err(e) => eprintln!("cloud upload {key}: {e:#}"),
        }
    }
    Ok((done, total - done))
}

fn notify() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// Something changed locally: sync soon instead of waiting for the next round.
pub fn nudge() {
    notify().notify_one();
}

/// Background sync: every 30 s, sooner after local changes, faster while
/// uploads are pending. Does nothing until the hub is set up.
pub fn spawn(svc: Arc<Service>, rt: &tokio::runtime::Handle) {
    rt.spawn(async move {
        loop {
            let mut wait = Duration::from_secs(30);
            if CloudConfig::load(&svc.db).ok().flatten().is_some() {
                // Let a burst of local changes settle into one round.
                tokio::time::sleep(Duration::from_millis(800)).await;
                match sync_once(&svc).await {
                    Ok(r) if r.pending_uploads > 0 => wait = Duration::from_secs(2),
                    Ok(_) => {}
                    Err(e) => {
                        let _ = svc.db.set_setting(LAST_ERROR, Some(&format!("{e:#}")));
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = notify().notified() => {}
            }
        }
    });
}
