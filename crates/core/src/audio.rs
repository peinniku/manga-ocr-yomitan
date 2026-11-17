//! Pronunciation audio.
//!
//! Words: a native speaker's recording when one exists (JapanesePod101, then
//! Lingua Libre on Wikimedia Commons), otherwise neural TTS. Sentences: neural
//! TTS. TTS is Azure (natural voices) when a key is set, else macOS `say`.
//!
//! The reading is only forced on the TTS voice when the word is written the
//! same way as another word (橋/はし vs 橋/きょう); otherwise the voice reads
//! the written form, which keeps its own pitch accent for the word.
//!
//! Every result is cached on disk, so each word is fetched once.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::Db;
use crate::kana;

pub const AZURE_KEY: &str = "tts.azure_key";
pub const AZURE_REGION: &str = "tts.azure_region";
pub const VOICE: &str = "tts.voice";
/// "0" turns off playing the word when a review card is turned over.
pub const AUTOPLAY: &str = "audio.autoplay";

pub const DEFAULT_VOICE: &str = "ja-JP-NanamiNeural";

/// Azure Japanese voices offered in settings: (id, label).
pub const VOICES: &[(&str, &str)] = &[
    ("ja-JP-NanamiNeural", "七海（女声，最自然）"),
    ("ja-JP-KeitaNeural", "圭太（男声）"),
    ("ja-JP-MayuNeural", "真由（女声，偏年轻）"),
    ("ja-JP-AoiNeural", "葵（女声，明亮）"),
    ("ja-JP-ShioriNeural", "栞（女声，平静）"),
    ("ja-JP-NaokiNeural", "直樹（男声）"),
    ("ja-JP-DaichiNeural", "大地（男声，低沉）"),
    ("ja-JP-MasaruMultilingualNeural", "勝（男声，多语种）"),
];

/// JapanesePod101 answers unknown words with this "no audio" clip.
const JPOD_MISSING_SHA256: &str = "ae6398b5a27bc8c0a771df6c907ade794be15518174773c58c7c7ddd17098906";
const USER_AGENT: &str = concat!("pic_yomi/", env!("CARGO_PKG_VERSION"), " (local Japanese dictionary app)");

#[derive(Debug, Clone)]
pub struct Audio {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// "jpod101", "lingualibre", "azure", "macos".
    pub source: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TtsConfig {
    #[serde(default)]
    pub azure_key: String,
    #[serde(default)]
    pub azure_region: String,
    #[serde(default)]
    pub voice: String,
    #[serde(default = "yes")]
    pub autoplay: bool,
}

fn yes() -> bool {
    true
}

impl TtsConfig {
    pub fn load(db: &Db) -> Result<Self> {
        Ok(Self {
            azure_key: db.get_setting(AZURE_KEY)?.unwrap_or_default(),
            azure_region: db.get_setting(AZURE_REGION)?.unwrap_or_default(),
            voice: db.get_setting(VOICE)?.filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_VOICE.into()),
            autoplay: db.get_setting(AUTOPLAY)?.as_deref() != Some("0"),
        })
    }

    /// Save; a masked key (from `redacted`) keeps the stored one.
    /// Changing the voice drops the cached TTS clips.
    pub fn save(&self, db: &Db) -> Result<()> {
        let old = Self::load(db)?;
        if !self.azure_key.contains('…') {
            db.set_setting(AZURE_KEY, Some(self.azure_key.trim()))?;
        }
        db.set_setting(AZURE_REGION, Some(self.azure_region.trim()))?;
        let voice = if self.voice.trim().is_empty() { DEFAULT_VOICE } else { self.voice.trim() };
        db.set_setting(VOICE, Some(voice))?;
        db.set_setting(AUTOPLAY, Some(if self.autoplay { "1" } else { "0" }))?;
        let now = Self::load(db)?;
        if now.voice != old.voice || now.azure_key != old.azure_key || now.azure_region != old.azure_region {
            clear_tts(db)?;
        }
        Ok(())
    }

    pub fn redacted(mut self) -> Self {
        if self.azure_key.chars().count() > 8 {
            let tail: String = self.azure_key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            self.azure_key = format!("…{tail}");
        }
        self
    }

    pub fn azure_ready(&self) -> bool {
        !self.azure_key.is_empty() && !self.azure_region.is_empty()
    }
}

fn audio_dir(db: &Db) -> PathBuf {
    let dir = db.path.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.join("audio")).unwrap_or_else(|| crate::db::data_dir().join("audio"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn ext(mime: &str) -> &'static str {
    match mime {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mp4" => "m4a",
        _ => "mp3",
    }
}

fn cached(db: &Db, key: &str) -> Result<Option<Audio>> {
    let row: Option<(String, String, String)> = db
        .lock()
        .query_row("SELECT source, mime, filename FROM audio_cache WHERE key=?1", [key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((source, mime, filename)) = row else { return Ok(None) };
    let path = audio_dir(db).join(filename);
    Ok(std::fs::read(&path).ok().map(|bytes| Audio { bytes, mime, source, path }))
}

fn store(db: &Db, key: &str, bytes: Vec<u8>, mime: &str, source: &str) -> Result<Audio> {
    let hash = hex(&Sha256::digest(key.as_bytes()))[..32].to_string();
    let filename = format!("{hash}.{}", ext(mime));
    let path = audio_dir(db).join(&filename);
    std::fs::write(&path, &bytes)?;
    db.lock().execute(
        "INSERT OR REPLACE INTO audio_cache(key, source, mime, filename, created_at) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![key, source, mime, filename, crate::now_ms()],
    )?;
    Ok(Audio { bytes, mime: mime.into(), source: source.into(), path })
}

/// Forget TTS clips (after the voice or key changed); recordings stay.
pub fn clear_tts(db: &Db) -> Result<()> {
    let files: Vec<String> = db
        .lock()
        .prepare("SELECT filename FROM audio_cache WHERE source IN ('azure', 'macos')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for f in files {
        let _ = std::fs::remove_file(audio_dir(db).join(f));
    }
    db.lock().execute("DELETE FROM audio_cache WHERE source IN ('azure', 'macos')", [])?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn client(timeout: u64) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(timeout)).user_agent(USER_AGENT).build()?)
}

/// The word's audio: a recording if there is one, else TTS.
pub async fn word(db: &Db, expression: &str, reading: &str) -> Result<Audio> {
    let expression = expression.trim();
    let reading = kana::kata_to_hira(reading.trim());
    if expression.is_empty() {
        bail!("no word");
    }
    let key = format!("w|{expression}|{reading}");
    if let Some(a) = cached(db, &key)? {
        return Ok(a);
    }
    let reading = if reading.is_empty() { kana::kata_to_hira(expression) } else { reading };
    // A recording source that could not be reached (offline, rate-limited)
    // may still have the word: then the TTS stand-in is not cached.
    let mut unsure = false;
    match jpod101(expression, &reading).await {
        Ok(Some(bytes)) => return store(db, &key, bytes, "audio/mpeg", "jpod101"),
        Ok(None) => {}
        Err(e) => {
            unsure = true;
            eprintln!("jpod101 {expression}: {e:#}");
        }
    }
    match lingua_libre(expression).await {
        Ok(Some(bytes)) => return store(db, &key, bytes, "audio/wav", "lingualibre"),
        Ok(None) => {}
        Err(e) => {
            unsure = true;
            eprintln!("lingua libre {expression}: {e:#}");
        }
    }
    let homograph = has_other_readings(db, expression, &reading);
    let (bytes, mime, source) = tts(db, &Speech::Word { expression, reading: &reading, homograph }).await?;
    if unsure {
        let path = std::env::temp_dir().join(format!("picyomi-{}.{}", uuid::Uuid::new_v4().simple(), ext(mime)));
        std::fs::write(&path, &bytes)?;
        return Ok(Audio { bytes, mime: mime.into(), source: source.into(), path });
    }
    store(db, &key, bytes, mime, source)
}

/// The sentence read aloud, with the card's word in its dictionary reading.
pub async fn sentence(db: &Db, sentence: &str, expression: &str, reading: &str) -> Result<Audio> {
    let sentence = sentence.trim();
    if sentence.is_empty() {
        bail!("no sentence");
    }
    let reading = kana::kata_to_hira(reading.trim());
    let voice = TtsConfig::load(db)?.voice;
    let key = format!("s|{voice}|{sentence}|{expression}|{reading}");
    if let Some(a) = cached(db, &key)? {
        return Ok(a);
    }
    let homograph = !expression.is_empty() && !reading.is_empty() && has_other_readings(db, expression, &reading);
    let (bytes, mime, source) = tts(db, &Speech::Sentence { sentence, expression, reading: &reading, homograph }).await?;
    store(db, &key, bytes, mime, source)
}

/// Fetch a new card's audio ahead of time, so review starts instantly.
pub async fn prefetch(db: &Db, expression: &str, reading: &str, sentence: &str) {
    if let Err(e) = word(db, expression, reading).await {
        eprintln!("audio for {expression}: {e:#}");
    }
    if !sentence.trim().is_empty() {
        if let Err(e) = self::sentence(db, sentence, expression, reading).await {
            eprintln!("sentence audio for {expression}: {e:#}");
        }
    }
}

/// The written form also has a different reading in the dictionaries.
fn has_other_readings(db: &Db, expression: &str, reading: &str) -> bool {
    if !kana::has_kanji(expression) {
        return false;
    }
    let n: i64 = db
        .lock()
        .query_row("SELECT COUNT(DISTINCT reading) FROM terms WHERE expression=?1 AND reading<>?2", params![expression, reading], |r| r.get(0))
        .unwrap_or(0);
    n > 0
}

async fn jpod101(expression: &str, reading: &str) -> Result<Option<Vec<u8>>> {
    let url = reqwest::Url::parse_with_params(
        "https://assets.languagepod101.com/dictionary/japanese/audiomp3.php",
        &[("kanji", expression), ("kana", reading)],
    )?;
    let resp = client(6)?.get(url).send().await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let bytes = resp.bytes().await?.to_vec();
    if bytes.len() < 500 || hex(&Sha256::digest(&bytes)) == JPOD_MISSING_SHA256 {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// Escape for a CirrusSearch `intitle:/…/` regex.
fn regex_escape(s: &str) -> String {
    s.chars().fold(String::new(), |mut out, c| {
        if r#"\.^$|?*+()[]{}/<>#@&~""#.contains(c) {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

async fn lingua_libre(expression: &str) -> Result<Option<Vec<u8>>> {
    let api = "https://commons.wikimedia.org/w/api.php";
    let c = client(8)?;
    let search = format!(r"intitle:/LL-Q5287 \(jpn\)-.*-{}\.wav/", regex_escape(expression));
    let v: serde_json::Value = c
        .get(reqwest::Url::parse_with_params(
            api,
            &[("action", "query"), ("format", "json"), ("list", "search"), ("srnamespace", "6"), ("srlimit", "5"), ("srsearch", &search)],
        )?)
        .send()
        .await?
        .json()
        .await?;
    if let Some(e) = v.get("error") {
        bail!("Commons search: {}", e["code"].as_str().unwrap_or("error"));
    }
    // The search is fuzzy; keep exact file names only.
    let wanted = format!("-{expression}.wav");
    let Some(title) = v["query"]["search"].as_array().into_iter().flatten().filter_map(|r| r["title"].as_str()).find(|t| t.ends_with(&wanted))
    else {
        return Ok(None);
    };
    let v: serde_json::Value = c
        .get(reqwest::Url::parse_with_params(api, &[("action", "query"), ("format", "json"), ("prop", "imageinfo"), ("iiprop", "url"), ("titles", title)])?)
        .send()
        .await?
        .json()
        .await?;
    let Some(url) = v["query"]["pages"].as_object().and_then(|p| p.values().next()).and_then(|p| p["imageinfo"][0]["url"].as_str()) else {
        return Ok(None);
    };
    // Tracking parameters make the file server rate-limit the request.
    let url = url.split('?').next().unwrap_or(url);
    let resp = c.get(url).send().await?;
    if !resp.status().is_success() {
        bail!("Commons file {}", resp.status());
    }
    Ok(Some(resp.bytes().await?.to_vec()))
}

enum Speech<'a> {
    Word { expression: &'a str, reading: &'a str, homograph: bool },
    Sentence { sentence: &'a str, expression: &'a str, reading: &'a str, homograph: bool },
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// SSML for Azure.
fn ssml(voice: &str, speech: &Speech) -> String {
    let say = |word: &str, reading: &str, homograph: bool| {
        if homograph {
            format!("<sub alias=\"{}\">{}</sub>", xml_escape(reading), xml_escape(word))
        } else {
            xml_escape(word)
        }
    };
    let body = match speech {
        Speech::Word { expression, reading, homograph } => say(expression, reading, *homograph),
        Speech::Sentence { sentence, expression, reading, homograph } => {
            match (*homograph).then(|| crate::lookup::word_span(sentence, expression)).flatten() {
                // Inflected forms keep the sentence's own okurigana; only a
                // plain occurrence of the word gets its reading forced.
                Some(span) if &sentence[span.clone()] == *expression => format!(
                    "{}{}{}",
                    xml_escape(&sentence[..span.start]),
                    say(expression, reading, true),
                    xml_escape(&sentence[span.end..])
                ),
                _ => xml_escape(sentence),
            }
        }
    };
    format!(
        "<speak version=\"1.0\" xml:lang=\"ja-JP\" xmlns=\"http://www.w3.org/2001/10/synthesis\"><voice name=\"{}\">{body}</voice></speak>",
        xml_escape(voice)
    )
}

async fn tts(db: &Db, speech: &Speech<'_>) -> Result<(Vec<u8>, &'static str, &'static str)> {
    let cfg = TtsConfig::load(db)?;
    if cfg.azure_ready() {
        match azure(&cfg, speech).await {
            Ok(b) => return Ok((b, "audio/mpeg", "azure")),
            Err(e) => eprintln!("azure tts: {e:#}"),
        }
    }
    let text = match speech {
        Speech::Word { expression, reading, homograph } => (if *homograph { reading } else { expression }).to_string(),
        Speech::Sentence { sentence, .. } => sentence.to_string(),
    };
    let bytes = tokio::task::spawn_blocking(move || macos_say(&text)).await??;
    Ok((bytes, "audio/mp4", "macos"))
}

async fn azure(cfg: &TtsConfig, speech: &Speech<'_>) -> Result<Vec<u8>> {
    let url = format!("https://{}.tts.speech.microsoft.com/cognitiveservices/v1", cfg.azure_region);
    let resp = client(15)?
        .post(url)
        .header("Ocp-Apim-Subscription-Key", &cfg.azure_key)
        .header("Content-Type", "application/ssml+xml")
        .header("X-Microsoft-OutputFormat", "audio-48khz-192kbitrate-mono-mp3")
        .body(ssml(&cfg.voice, speech))
        .send()
        .await
        .context("Azure TTS request failed")?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() || bytes.is_empty() {
        bail!("Azure TTS {status}");
    }
    Ok(bytes.to_vec())
}

/// Offline fallback: the system Japanese voice, as AAC.
fn macos_say(text: &str) -> Result<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("picyomi-say-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir)?;
    let aiff = dir.join("a.aiff");
    let m4a = dir.join("a.m4a");
    let ok = std::process::Command::new("say").args(["-v", "Kyoko", "-o"]).arg(&aiff).arg(text).status()?.success()
        && std::process::Command::new("afconvert").args(["-f", "m4af", "-d", "aac"]).arg(&aiff).arg(&m4a).status()?.success();
    let bytes = std::fs::read(&m4a);
    let _ = std::fs::remove_dir_all(&dir);
    if !ok {
        bail!("say failed");
    }
    Ok(bytes?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssml_forces_reading_only_for_homographs() {
        let w = ssml("v", &Speech::Word { expression: "清掃", reading: "せいそう", homograph: false });
        assert!(w.contains(">清掃</voice>"), "{w}");
        let w = ssml("v", &Speech::Word { expression: "橋", reading: "はし", homograph: true });
        assert!(w.contains("<sub alias=\"はし\">橋</sub>"), "{w}");
        let s = ssml("v", &Speech::Sentence { sentence: "上手に話す", expression: "上手", reading: "じょうず", homograph: true });
        assert!(s.contains("<sub alias=\"じょうず\">上手</sub>に話す"), "{s}");
        let s = ssml("v", &Speech::Sentence { sentence: "A&B<c>", expression: "", reading: "", homograph: false });
        assert!(s.contains("A&amp;B&lt;c&gt;"), "{s}");
    }

    #[test]
    fn cache_round_trip_and_clear() {
        let dir = std::env::temp_dir().join(format!("picyomi-audio-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("t.db")).unwrap();
        store(&db, "w|a|a", vec![1, 2, 3], "audio/mpeg", "jpod101").unwrap();
        store(&db, "s|v|x||", vec![4], "audio/mpeg", "azure").unwrap();
        assert_eq!(cached(&db, "w|a|a").unwrap().unwrap().bytes, vec![1, 2, 3]);
        clear_tts(&db).unwrap();
        assert!(cached(&db, "s|v|x||").unwrap().is_none());
        assert!(cached(&db, "w|a|a").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Hits the network: `cargo test -p picyomi-core audio -- --ignored`.
    #[test]
    #[ignore]
    fn fetches_recordings() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(jpod101("清掃", "せいそう")).unwrap().is_some());
        assert!(rt.block_on(jpod101("ほげふがぜ", "ほげふがぜ")).unwrap().is_none());
        assert!(rt.block_on(lingua_libre("箸")).unwrap().is_some());
    }
}
