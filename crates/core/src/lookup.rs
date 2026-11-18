//! Yomitan-style scanning lookup against imported dictionaries.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::params_from_iter;
use serde::Serialize;
use serde_json::Value;

use crate::db::Db;
use crate::deinflect::{deinflect, rules_match};
use crate::kana::kata_to_hira;

pub const MAX_SCAN: usize = 20;
pub const MAX_ENTRIES: usize = 12;

#[derive(Debug, Clone, Serialize)]
pub struct Frequency {
    pub dict: String,
    pub value: Option<f64>,
    pub display: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub dict: String,
    pub dict_priority: i64,
    pub expression: String,
    pub reading: String,
    pub def_tags: String,
    pub rules: String,
    pub score: f64,
    pub sequence: Option<i64>,
    pub glossary: Value,
    /// The scanned source text this entry matched (e.g. 食べなかった).
    pub source: String,
    /// Length of `source` in characters.
    pub source_length: usize,
    pub reasons: Vec<&'static str>,
    pub frequencies: Vec<Frequency>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LookupResult {
    pub entries: Vec<Entry>,
    /// Characters matched by the best entry, for highlighting.
    pub match_length: usize,
}

struct Dict {
    title: String,
    priority: i64,
}

fn enabled_dicts(db: &Db) -> Result<HashMap<i64, Dict>> {
    let c = db.lock();
    let mut st = c.prepare("SELECT id, title, priority FROM dictionaries WHERE enabled=1")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, Dict { title: r.get(1)?, priority: r.get(2)? })))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

struct Cand {
    len: usize,
    term: String,
    rules: u16,
    reasons: Vec<&'static str>,
}

fn candidates(text: &str, offset: usize, extra: &[(usize, String)]) -> Vec<Cand> {
    let chars: Vec<char> = text.chars().skip(offset).take(MAX_SCAN).collect();
    let mut out = Vec::new();
    for len in (1..=chars.len()).rev() {
        let src: String = chars[..len].iter().collect();
        let hira = kata_to_hira(&src);
        let variants: Vec<&str> = if hira != src { vec![&src, &hira] } else { vec![&src] };
        for v in variants {
            for c in deinflect(v) {
                out.push(Cand { len, term: c.term, rules: c.rules, reasons: c.reasons });
            }
        }
    }
    for (len, base) in extra {
        out.push(Cand { len: *len, term: base.clone(), rules: 0, reasons: vec!["tokenizer"] });
    }
    out
}

fn frequencies(db: &Db, dicts: &HashMap<i64, Dict>, expression: &str, reading: &str) -> Result<Vec<Frequency>> {
    let c = db.lock();
    let mut st = c.prepare_cached(
        "SELECT dict_id, reading, value, display FROM term_meta WHERE mode='freq' AND term IN (?1, ?2)",
    )?;
    let rows = st.query_map([expression, reading], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<f64>>(2)?, r.get::<_, Option<String>>(3)?))
    })?;
    let mut best: HashMap<i64, Frequency> = HashMap::new();
    for row in rows {
        let (did, rd, value, display) = row?;
        let Some(d) = dicts.get(&did) else { continue };
        if rd.as_deref().is_some_and(|r| r != reading) {
            continue;
        }
        let better = match best.get(&did) {
            None => true,
            Some(cur) => value.unwrap_or(1e12) < cur.value.unwrap_or(1e12),
        };
        if better {
            best.insert(did, Frequency { dict: d.title.clone(), value, display });
        }
    }
    let mut v: Vec<(i64, Frequency)> = best.into_iter().collect();
    v.sort_by_key(|(did, _)| dicts.get(did).map(|d| d.priority).unwrap_or(0));
    Ok(v.into_iter().map(|(_, f)| f).collect())
}

/// Look up the word starting at character `offset` of `text`.
///
/// `tokenizer_bases` are extra (length, dictionary form) candidates, e.g. from a
/// morphological analyzer, used when rule-based deinflection misses.
pub fn lookup(db: &Db, text: &str, offset: usize, tokenizer_bases: &[(usize, String)]) -> Result<LookupResult> {
    let n = text.chars().count();
    let dicts = enabled_dicts(db)?;
    if dicts.is_empty() || offset >= n {
        return Ok(LookupResult::default());
    }
    let cands = candidates(text, offset, tokenizer_bases);
    let terms: Vec<String> = cands.iter().map(|c| c.term.clone()).collect::<HashSet<_>>().into_iter().collect();

    struct Row {
        id: i64,
        dict_id: i64,
        expression: String,
        reading: String,
        def_tags: String,
        rules: String,
        score: f64,
        glossary: String,
        sequence: Option<i64>,
    }
    let mut by_term: HashMap<String, Vec<usize>> = HashMap::new();
    let mut rows: Vec<Row> = Vec::new();
    {
        let c = db.lock();
        for chunk in terms.chunks(400) {
            let ph = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT id, dict_id, expression, reading, def_tags, rules, score, glossary, sequence FROM terms
                 WHERE expression IN ({ph}) OR reading IN ({ph})"
            );
            let mut st = c.prepare(&sql)?;
            let args = chunk.iter().chain(chunk.iter());
            let mut q = st.query(params_from_iter(args))?;
            while let Some(r) = q.next()? {
                let dict_id: i64 = r.get(1)?;
                if !dicts.contains_key(&dict_id) {
                    continue;
                }
                let row = Row {
                    id: r.get(0)?,
                    dict_id,
                    expression: r.get(2)?,
                    reading: r.get(3)?,
                    def_tags: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    rules: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    score: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                    glossary: r.get(7)?,
                    sequence: r.get(8)?,
                };
                let i = rows.len();
                by_term.entry(row.expression.clone()).or_default().push(i);
                if row.reading != row.expression {
                    by_term.entry(row.reading.clone()).or_default().push(i);
                }
                rows.push(row);
            }
        }
    }

    // Best candidate per entry: longer source, then an expression (not reading) match, then fewer steps.
    let mut found: HashMap<usize, (usize, bool, usize, &Cand)> = HashMap::new();
    for c in &cands {
        let Some(idxs) = by_term.get(&c.term) else { continue };
        for &i in idxs {
            let row = &rows[i];
            if !rules_match(c.rules, &row.rules) {
                continue;
            }
            let key = (c.len, row.expression == c.term, usize::MAX - c.reasons.len());
            let better = match found.get(&i) {
                None => true,
                Some(prev) => (prev.0, prev.1, prev.2) < key,
            };
            if better {
                found.insert(i, (key.0, key.1, key.2, c));
            }
        }
    }

    let chars: Vec<char> = text.chars().collect();
    // (entry, matched its written form, deinflection steps incl. tokenizer guesses)
    let mut entries: Vec<(Entry, bool, usize)> = Vec::with_capacity(found.len());
    for (i, (len, expr_match, _, c)) in found {
        let row = &rows[i];
        let d = &dicts[&row.dict_id];
        entries.push((
            Entry {
                id: row.id,
                dict: d.title.clone(),
                dict_priority: d.priority,
                expression: row.expression.clone(),
                reading: row.reading.clone(),
                def_tags: row.def_tags.clone(),
                rules: row.rules.clone(),
                score: row.score,
                sequence: row.sequence,
                glossary: serde_json::from_str(&row.glossary).unwrap_or(Value::Null),
                source: chars[offset..(offset + len).min(n)].iter().collect(),
                source_length: len,
                reasons: c.reasons.iter().copied().filter(|r| *r != "tokenizer").collect(),
                frequencies: frequencies(db, &dicts, &row.expression, &row.reading)?,
            },
            expr_match,
            c.reasons.len(),
        ));
    }

    let rank = |e: &Entry| e.frequencies.iter().filter_map(|f| f.value).fold(1e12, f64::min);
    let affix = |e: &Entry| e.def_tags.split_whitespace().any(|t| t == "造");
    // Longest source first, then dictionary priority. Among words that fit the
    // same text: the one needing fewer conjugation steps (嗜み before 嗜む,
    // 宿した → 宿す before 宿＋した), a written-form match before a reading
    // match, real words before kanji/affix entries (催す before 催), and only
    // then the more common word (行った → 行く before 行う).
    entries.sort_by(|(a, am, asteps), (b, bm, bsteps)| {
        b.source_length
            .cmp(&a.source_length)
            .then(a.dict_priority.cmp(&b.dict_priority))
            .then(asteps.cmp(bsteps))
            .then(bm.cmp(am))
            .then(affix(a).cmp(&affix(b)))
            .then(rank(a).total_cmp(&rank(b)))
            .then(b.score.total_cmp(&a.score))
            .then(a.id.cmp(&b.id))
    });
    let entries = merge_same_word(entries.into_iter().map(|(e, _, _)| e).collect());
    let entries: Vec<Entry> = entries.into_iter().take(MAX_ENTRIES).collect();
    let match_length = entries.first().map(|e| e.source_length).unwrap_or(0);
    Ok(LookupResult { entries, match_length })
}

/// One dictionary can have several entries for the same word (新明解 has a
/// kanji entry 「［劇］」 and a word entry 「劇」). Show them as one result:
/// the word entry first, kanji/affix entries (tag 造) after it.
fn merge_same_word(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(entries.len());
    let mut parts: Vec<Vec<Entry>> = Vec::new();
    for e in entries {
        match out.iter().position(|o| o.dict == e.dict && o.expression == e.expression && o.reading == e.reading) {
            Some(i) => parts[i].push(e),
            None => {
                out.push(e.clone());
                parts.push(vec![e]);
            }
        }
    }
    for (o, mut group) in out.iter_mut().zip(parts) {
        if group.len() < 2 {
            continue;
        }
        group.sort_by_key(|e| e.def_tags.split_whitespace().any(|t| t == "造"));
        let mut glossary = Vec::new();
        for e in &group {
            match &e.glossary {
                Value::Array(a) => glossary.extend(a.iter().cloned()),
                v => glossary.push(v.clone()),
            }
        }
        let first = &group[0];
        o.id = first.id;
        o.def_tags = first.def_tags.clone();
        o.rules = first.rules.clone();
        o.sequence = first.sequence;
        o.glossary = Value::Array(glossary);
    }
    out
}

/// The sentence around character `offset` in `text`, and the offset within it.
/// Splits at 。！？ and line breaks, and trims very long runs to a window.
pub fn sentence_at(text: &str, offset: usize) -> (String, usize) {
    const ENDS: &[char] = &['。', '！', '？', '!', '?', '\n'];
    const MAX: usize = 80;
    let chars: Vec<char> = text.chars().collect();
    let offset = offset.min(chars.len().saturating_sub(1));
    let mut start = offset;
    while start > 0 && !ENDS.contains(&chars[start - 1]) {
        start -= 1;
    }
    let mut end = offset;
    while end < chars.len() && !ENDS.contains(&chars[end]) {
        end += 1;
    }
    if end < chars.len() && chars[end] != '\n' {
        end += 1; // keep the closing punctuation
    }
    // Leading whitespace and closing brackets left over from the previous sentence.
    while start < offset && (chars[start].is_whitespace() || "」』）)".contains(chars[start])) {
        start += 1;
    }
    if end - start > MAX {
        start = start.max(offset.saturating_sub(MAX / 3));
        end = end.min(start + MAX);
    }
    (chars[start..end].iter().collect(), offset - start)
}

/// `word_span` for a card's word, also when the sentence spells it in kana
/// (うなだれる for 項垂れる).
pub fn find_word(text: &str, expression: &str, reading: &str) -> Option<std::ops::Range<usize>> {
    let hira = crate::kana::kata_to_hira(reading);
    word_span(text, expression).or_else(|| word_span(text, &hira)).or_else(|| word_span(text, &crate::kana::hira_to_kata(&hira)))
}

/// Byte range of `word` in `sentence`, also when it appears inflected:
/// the longest stretch from its kanji stem that deinflects to the word
/// (喫したい → 喫する, 催していた → 催す), like the lookup popup highlights it.
pub fn word_span(sentence: &str, word: &str) -> Option<std::ops::Range<usize>> {
    if word.is_empty() {
        return None;
    }
    if let Some(a) = sentence.find(word) {
        return Some(a..a + word.len());
    }
    let is_kana = |c: char| ('\u{3041}'..='\u{309f}').contains(&c);
    let stem: String = {
        let mut chars: Vec<char> = word.chars().collect();
        while chars.len() > 1 && chars.last().is_some_and(|c| is_kana(*c)) {
            chars.pop();
        }
        chars.into_iter().collect()
    };
    if stem == word || stem.is_empty() {
        return None;
    }
    let mut fallback = None;
    for (a, _) in sentence.match_indices(stem.as_str()) {
        let after = a + stem.len();
        let ends: Vec<usize> = sentence[after..].char_indices().take(12).map(|(i, c)| after + i + c.len_utf8()).collect();
        for &end in ends.iter().rev() {
            if deinflect(&sentence[a..end]).iter().any(|c| c.term == word) {
                return Some(a..end);
            }
        }
        // Not a known inflection: the kanji stem plus as many kana as the word has.
        if fallback.is_none() && crate::kana::has_kanji(&stem) {
            let mut end = after;
            let okurigana = word.chars().count() - stem.chars().count();
            for c in sentence[after..].chars().take(okurigana.max(1)) {
                if !is_kana(c) {
                    break;
                }
                end += c.len_utf8();
            }
            fallback = Some(a..end);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Db {
        let db = Db::in_memory().unwrap();
        {
            let c = db.lock();
            c.execute_batch(
                r#"
                INSERT INTO dictionaries(id, title, priority, enabled) VALUES (1, 'Test', 1, 1), (2, 'Freq', 2, 1);
                INSERT INTO terms(dict_id, expression, reading, rules, score, glossary) VALUES
                  (1, '食べる', 'たべる', 'v1', 0, '["to eat"]'),
                  (1, '行く', 'いく', 'v5', 0, '["to go"]'),
                  (1, '行う', 'おこなう', 'v5', 0, '["to do"]'),
                  (1, '静か', 'しずか', '', 0, '["quiet"]'),
                  (1, '部屋', 'へや', '', 0, '["room"]'),
                  (1, '勉強', 'べんきょう', '', 0, '["study"]'),
                  (1, 'やばい', 'やばい', 'adj-i', 0, '["risky"]');
                INSERT INTO term_meta(dict_id, term, mode, reading, value, display) VALUES
                  (2, '行く', 'freq', 'いく', 44, '44'),
                  (2, '行う', 'freq', 'おこなう', 361, '361');
                "#,
            )
            .unwrap();
        }
        db
    }

    #[test]
    fn deinflected_lookup() {
        let db = fixture();
        let r = lookup(&db, "食べなかった", 0, &[]).unwrap();
        assert_eq!(r.entries[0].expression, "食べる");
        assert_eq!(r.match_length, 6);
    }

    #[test]
    fn frequency_breaks_ties() {
        let db = fixture();
        let r = lookup(&db, "行ってしまった", 0, &[]).unwrap();
        assert_eq!(r.entries[0].expression, "行く");
        assert_eq!(r.entries[1].expression, "行う");
    }

    #[test]
    fn prefix_scan_and_katakana() {
        let db = fixture();
        assert_eq!(lookup(&db, "静かな部屋", 0, &[]).unwrap().entries[0].expression, "静か");
        assert_eq!(lookup(&db, "静かな部屋", 3, &[]).unwrap().entries[0].expression, "部屋");
        assert_eq!(lookup(&db, "ヤバい", 0, &[]).unwrap().entries[0].expression, "やばい");
        let r = lookup(&db, "勉強している", 0, &[]).unwrap();
        assert_eq!((r.entries[0].expression.as_str(), r.match_length), ("勉強", 6));
    }

    #[test]
    fn sentence_window() {
        let t = "Isla · 4時間\nゴジラの話は酷いと思った。劇伴にはムカついた！次";
        let off = t.chars().position(|c| c == '劇').unwrap();
        let (s, o) = sentence_at(t, off);
        assert_eq!(s, "劇伴にはムカついた！");
        assert_eq!(o, 0);
        let (s, o) = sentence_at(t, 2);
        assert_eq!(s, "Isla · 4時間");
        assert_eq!(o, 2);
    }

    #[test]
    fn fewer_steps_and_real_words_rank_first() {
        let db = fixture();
        db.lock()
            .execute_batch(
                r#"
                INSERT INTO terms(dict_id, expression, reading, def_tags, rules, score, glossary) VALUES
                  (1, '嗜み', 'たしなみ', '', '', 0, '["taste"]'),
                  (1, '嗜む', 'たしなむ', '', 'v5', 0, '["enjoy"]'),
                  (1, '宿', 'やど', '', '', 0, '["inn"]'),
                  (1, '宿す', 'やどす', '', 'v5', 0, '["harbor"]'),
                  (1, '催', 'さい', '造', '', 0, '["host (kanji)"]'),
                  (1, '催す', 'もよおす', '', 'v5', 0, '["feel"]');
                INSERT INTO term_meta(dict_id, term, mode, reading, value, display) VALUES
                  (2, '嗜む', 'freq', 'たしなむ', 100, '100'),
                  (2, '嗜み', 'freq', 'たしなみ', 900, '900'),
                  (2, '宿', 'freq', 'やど', 50, '50'),
                  (2, '催', 'freq', 'さい', 10, '10');
                "#,
            )
            .unwrap();
        // The noun needs no conjugation, even though the verb is more common.
        assert_eq!(lookup(&db, "嗜みよ", 0, &[]).unwrap().entries[0].expression, "嗜み");
        // 宿した is the past of 宿す, not 宿 + した.
        assert_eq!(lookup(&db, "宿した", 0, &[]).unwrap().entries[0].expression, "宿す");
        // A kanji/affix entry never comes before the word.
        assert_eq!(lookup(&db, "催していた", 0, &[]).unwrap().entries[0].expression, "催す");
    }

    #[test]
    fn same_word_entries_merge() {
        let db = fixture();
        db.lock()
            .execute_batch("INSERT INTO terms(dict_id, expression, reading, def_tags, rules, score, glossary) VALUES (1, '部屋', 'へや', '造', '', 0, '[\"kanji\"]');")
            .unwrap();
        let r = lookup(&db, "部屋", 0, &[]).unwrap();
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].glossary, serde_json::json!(["room", "kanji"]));
    }
}
