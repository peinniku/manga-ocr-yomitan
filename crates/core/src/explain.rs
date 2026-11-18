//! Chinese helper explanations from an OpenAI-compatible chat endpoint
//! (DeepSeek, Qwen, Kimi, OpenAI, a local Ollama ...). Results are cached in SQLite.

use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cards::{Example, Explanation};
use crate::db::Db;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmConfig {
    /// e.g. https://api.deepseek.com/v1
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl LlmConfig {
    pub fn load(db: &Db) -> Result<Self> {
        Ok(Self {
            base_url: db.get_setting("llm.base_url")?.unwrap_or_default(),
            api_key: db.get_setting("llm.api_key")?.unwrap_or_default(),
            model: db.get_setting("llm.model")?.unwrap_or_default(),
        })
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.set_setting("llm.base_url", Some(self.base_url.trim()))?;
        db.set_setting("llm.api_key", Some(self.api_key.trim()))?;
        db.set_setting("llm.model", Some(self.model.trim()))?;
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty()
    }

    /// Same config with the key masked, for display.
    pub fn redacted(&self) -> Self {
        let k = &self.api_key;
        let api_key = if k.len() > 8 { format!("{}…{}", &k[..3], &k[k.len() - 4..]) } else if k.is_empty() { String::new() } else { "…".into() };
        Self { api_key, ..self.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainRequest {
    pub expression: String,
    #[serde(default)]
    pub reading: String,
    #[serde(default)]
    pub sentence: String,
    /// First lines of the Japanese definition, to keep the explanation on the right sense.
    #[serde(default)]
    pub definition_hint: String,
}

const SYSTEM: &str = "你是日语老师，学生母语是中文，正在读日本漫画。根据给出的日语单词和它出现的句子，用简体中文解释。\
只输出一个 JSON 对象，不要输出其他文字。字段：\
meaning_zh: 这个词在本句中的意思（一句话，简洁）；\
gloss_zh: 这个词的常见义项，用；分隔，最多 4 个；\
sentence_zh: 整句的自然中文翻译（没有句子时为空字符串）；\
examples: 2 个简短常用的日语例句，每个为 {\"ja\": 例句, \"zh\": 中文翻译}。";

/// Every reading the enabled dictionaries give for a word written with kanji
/// (皺 → しわ, しぼ). Fewer than two means there is nothing to choose.
pub fn readings_of(db: &Db, expression: &str) -> Vec<String> {
    if !crate::kana::has_kanji(expression) {
        return vec![];
    }
    let c = db.lock();
    let Ok(mut st) = c.prepare_cached(
        "SELECT DISTINCT t.reading FROM terms t JOIN dictionaries d ON d.id = t.dict_id
         WHERE d.enabled = 1 AND t.expression = ?1 AND t.reading <> '' ORDER BY t.reading",
    ) else {
        return vec![];
    };
    let rows = st.query_map([expression], |r| r.get::<_, String>(0));
    let mut out: Vec<String> = rows.map(|r| r.filter_map(|x| x.ok()).map(|x| crate::kana::kata_to_hira(&x)).collect()).unwrap_or_default();
    out.dedup();
    out
}

fn cache_key(r: &ExplainRequest, model: &str, readings: &[String]) -> String {
    let mut h = Sha256::new();
    let readings = readings.join("/");
    for part in [model, &r.expression, &r.reading, &r.sentence, &readings] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    format!("{:x}", h.finalize())
}

pub fn cached(db: &Db, r: &ExplainRequest) -> Result<Option<Explanation>> {
    let cfg = LlmConfig::load(db)?;
    let key = cache_key(r, &cfg.model, &readings_of(db, &r.expression));
    let v: Option<String> = db.lock().query_row("SELECT value FROM explain_cache WHERE key=?1", [key], |row| row.get(0)).optional()?;
    Ok(v.and_then(|s| serde_json::from_str(&s).ok()))
}

/// Pull the first JSON object out of a model reply (tolerates ```json fences and chatter).
fn extract_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

fn to_explanation(v: &serde_json::Value) -> Explanation {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let examples = v
        .get("examples")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let ja = e.get("ja")?.as_str()?.trim().to_string();
                    let zh = e.get("zh").and_then(|z| z.as_str()).unwrap_or("").trim().to_string();
                    (!ja.is_empty()).then_some(Example { ja, zh })
                })
                .take(3)
                .collect()
        })
        .unwrap_or_default();
    Explanation { meaning_zh: s("meaning_zh"), gloss_zh: s("gloss_zh"), sentence_zh: s("sentence_zh"), examples, reading: crate::kana::kata_to_hira(&s("reading")) }
}

pub async fn explain(db: &Db, r: &ExplainRequest) -> Result<Explanation> {
    let cfg = LlmConfig::load(db)?;
    if !cfg.is_configured() {
        bail!("LLM not configured");
    }
    if let Some(hit) = cached(db, r)? {
        return Ok(hit);
    }
    let readings = readings_of(db, &r.expression);
    let choose = readings.len() > 1 && !r.sentence.is_empty();
    let mut user = format!(
        "单词：{}（{}）\n句子：{}\n日日词典释义（参考）：{}",
        r.expression,
        r.reading,
        if r.sentence.is_empty() { "（无）" } else { &r.sentence },
        r.definition_hint.chars().take(300).collect::<String>()
    );
    let mut system = SYSTEM.to_string();
    if choose {
        // The reading above is only the lookup's first guess.
        user.push_str(&format!("\n这个写法有几种读音：{}。上面括号里的读音只是猜测，请根据句子判断。", readings.join("、")));
        system.push_str("另加字段 reading: 这个词在本句中的读音（平假名，必须是给出的读音之一），其余字段按这个读音来解释。");
    }
    let content = complete(&cfg, &system, &user).await?;
    let content = content.as_str();
    let mut parsed = extract_json(content).map(|j| to_explanation(&j)).unwrap_or_else(|| Explanation {
        meaning_zh: content.trim().to_string(),
        ..Default::default()
    });
    if !choose || !readings.contains(&parsed.reading) {
        parsed.reading.clear();
    }
    db.lock().execute(
        "INSERT OR REPLACE INTO explain_cache(key, value, created_at) VALUES(?1, ?2, strftime('%s','now'))",
        params![cache_key(r, &cfg.model, &readings), serde_json::to_string(&parsed)?],
    )?;
    Ok(parsed)
}

/// Gemini's native API (its keys may not work on the OpenAI-compatible endpoint).
fn is_gemini_native(base_url: &str) -> bool {
    base_url.contains("generativelanguage.googleapis.com") && !base_url.contains("/openai")
}

/// One JSON-mode completion; returns the model's text.
pub async fn complete(cfg: &LlmConfig, system: &str, user: &str) -> Result<String> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;
    let base = cfg.base_url.trim_end_matches('/');
    let (req, gemini) = if is_gemini_native(base) {
        let model = cfg.model.trim_start_matches("models/");
        let body = json!({
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": [{"role": "user", "parts": [{"text": user}]}],
            "generationConfig": {"temperature": 0.2, "responseMimeType": "application/json"},
        });
        let req = client.post(format!("{base}/models/{model}:generateContent")).header("x-goog-api-key", &cfg.api_key).json(&body);
        (req, true)
    } else {
        let body = json!({
            "model": cfg.model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
            "temperature": 0.2,
            "response_format": {"type": "json_object"},
        });
        let mut req = client.post(format!("{base}/chat/completions")).json(&body);
        if !cfg.api_key.is_empty() {
            req = req.bearer_auth(&cfg.api_key);
        }
        (req, false)
    };
    let resp = req.send().await.context("LLM request failed")?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.context("LLM response is not JSON")?;
    if !status.is_success() {
        bail!("LLM error {status}: {}", v.get("error").map(|e| e.to_string()).unwrap_or_default());
    }
    let text = if gemini {
        v["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|p| p.iter().filter_map(|x| x["text"].as_str()).collect::<String>())
            .unwrap_or_default()
    } else {
        v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string()
    };
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let text = "```json\n{\"meaning_zh\":\"壁虎\",\"gloss_zh\":\"壁虎；守宫\",\"sentence_zh\":\"\",\"examples\":[{\"ja\":\"ヤモリがいる\",\"zh\":\"有壁虎\"}]}\n```";
        let e = to_explanation(&extract_json(text).unwrap());
        assert_eq!(e.meaning_zh, "壁虎");
        assert_eq!(e.examples.len(), 1);
    }
}

/// Plain text of a (structured-content) glossary, up to `max` characters.
pub fn gloss_text(v: &serde_json::Value, max: usize) -> String {
    fn walk(v: &serde_json::Value, out: &mut String, max: usize) {
        if out.chars().count() >= max {
            return;
        }
        match v {
            serde_json::Value::String(s) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(s.trim());
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out, max)),
            serde_json::Value::Object(o) => {
                // Skip images and the like; text lives under "content" or "text".
                if let Some(c) = o.get("content").or_else(|| o.get("text")) {
                    walk(c, out, max);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(v, &mut out, max);
    out.chars().take(max).collect()
}

