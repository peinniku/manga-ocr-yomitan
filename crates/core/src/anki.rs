//! Import vocabulary notes and their review history from an Anki collection.
//!
//! Works with note types that have `Expression` and `Reading` fields (the
//! usual Yomitan card format), optionally `Sentence` and `Meaning`. Definitions
//! are taken from the dictionaries imported here when the word is found, so the
//! cards look like ones added in pic_yomi; otherwise Anki's own text is kept.
//! Anki's review log is copied over and FSRS replays it, so intervals carry on.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::cards::{self, Card, Definition};
use crate::service::Service;
use crate::srs::ReviewLog;
use crate::tokenize;

#[derive(Debug, Default, Serialize)]
pub struct AnkiReport {
    pub added: usize,
    /// Already in the word list (same word and reading).
    pub skipped: usize,
    pub reviews: usize,
    /// Words not found in the dictionaries here; Anki's definition text was kept.
    pub kept_anki_definition: Vec<String>,
    pub decks: Vec<String>,
}

/// Field indexes of a usable note type.
struct Layout {
    expression: usize,
    reading: usize,
    sentence: Option<usize>,
    meaning: Option<usize>,
}

fn find_field(names: &[String], wanted: &[&str]) -> Option<usize> {
    names.iter().position(|n| wanted.iter().any(|w| n.trim().eq_ignore_ascii_case(w)))
}

/// Plain text of an Anki field: no HTML, entities decoded, `[sound:…]` removed.
pub fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let mut out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    while let Some(start) = out.find("[sound:") {
        let end = out[start..].find(']').map(|e| start + e + 1).unwrap_or(out.len());
        out.replace_range(start..end, "");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `漢字[かんじ]` furigana notation → `漢字`.
fn strip_furigana(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            ' ' if depth == 0 => {}
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Copy the collection into memory, so a running Anki is never disturbed.
fn open_snapshot(path: &Path) -> Result<Connection> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .with_context(|| format!("open {}", path.display()))?;
    let mut mem = Connection::open_in_memory()?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut mem)?;
        backup.run_to_completion(256, std::time::Duration::from_millis(5), None)?;
    }
    // Anki's own collation; needed by some of its indexes.
    mem.create_collation("unicase", |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase()))?;
    Ok(mem)
}

/// Import notes from decks named `deck` (and its subdecks), or from every deck.
pub fn import(svc: &Service, path: &Path, deck: Option<&str>) -> Result<AnkiReport> {
    let col = open_snapshot(path)?;

    let decks: HashMap<i64, String> =
        col.prepare("SELECT id, name FROM decks")?.query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?.collect::<Result<_, _>>()?;
    // Anki stores subdeck paths with \x1f separators.
    let decks: HashMap<i64, String> = decks.into_iter().map(|(id, n)| (id, n.replace('\x1f', "::"))).collect();
    let wanted = |did: i64| match deck {
        None => true,
        Some(d) => decks.get(&did).is_some_and(|n| n == d || n.starts_with(&format!("{d}::"))),
    };

    let mut fields: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
    for row in col.prepare("SELECT ntid, ord, name FROM fields")?.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))? {
        let (nt, ord, name) = row?;
        fields.entry(nt).or_default().push((ord, name));
    }
    let layouts: HashMap<i64, Layout> = fields
        .into_iter()
        .filter_map(|(nt, mut f)| {
            f.sort();
            let names: Vec<String> = f.into_iter().map(|(_, n)| n).collect();
            Some((
                nt,
                Layout {
                    expression: find_field(&names, &["Expression", "Word", "Vocab", "Term", "単語", "単語/語彙"])?,
                    reading: find_field(&names, &["Reading", "Kana", "読み", "読み方"])?,
                    sentence: find_field(&names, &["Sentence", "Example", "例文"]),
                    meaning: find_field(&names, &["Meaning", "Glossary", "Definition", "意味"]),
                },
            ))
        })
        .collect();
    if layouts.is_empty() {
        bail!("这个 Anki 合集里没有带 Expression / Reading 字段的笔记类型");
    }

    // One card per note (the first template), with its deck.
    let mut st = col.prepare("SELECT c.id, c.did, n.id, n.mid, n.flds FROM cards c JOIN notes n ON n.id = c.nid ORDER BY n.id, c.ord")?;
    let rows: Vec<(i64, i64, i64, i64, String)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
    let mut revlog = col.prepare("SELECT id, ease, time FROM revlog WHERE cid = ?1 AND ease BETWEEN 1 AND 4 AND type IN (0, 1, 2) ORDER BY id")?;

    let mut report = AnkiReport::default();
    let mut seen_notes = std::collections::HashSet::new();
    for (cid, did, nid, mid, flds) in rows {
        let Some(layout) = layouts.get(&mid) else { continue };
        if !wanted(did) || !seen_notes.insert(nid) {
            continue;
        }
        let f: Vec<&str> = flds.split('\x1f').collect();
        let get = |i: Option<usize>| i.and_then(|i| f.get(i)).map(|s| plain_text(s)).unwrap_or_default();
        let expression = strip_furigana(&get(Some(layout.expression)));
        let reading = strip_furigana(&get(Some(layout.reading)));
        if expression.is_empty() {
            continue;
        }
        if cards::find_card(&svc.db, &expression, &reading)?.is_some() {
            report.skipped += 1;
            continue;
        }
        let deck_name = decks.get(&did).cloned().unwrap_or_default();
        if !report.decks.contains(&deck_name) {
            report.decks.push(deck_name.clone());
        }

        let (mut definitions, frequency) = svc.definitions(&expression, &reading)?;
        if definitions.is_empty() {
            let text = get(layout.meaning);
            if !text.is_empty() {
                definitions.push(Definition { dict: "Anki".into(), def_tags: String::new(), glossary: serde_json::json!([text]) });
                report.kept_anki_definition.push(expression.clone());
            }
        }
        let mut card = Card::new(&expression, &reading);
        card.definitions = definitions;
        card.frequency = frequency;
        card.sentence = get(layout.sentence);
        card.sentence_furigana = if card.sentence.is_empty() { vec![] } else { tokenize::furigana(&card.sentence) };
        card.source_title = Some(format!("Anki · {deck_name}"));
        card.tags = vec!["anki".into()];
        card.created_at = nid; // note ids are creation times in ms
        cards::save_card(&svc.db, &card)?;
        report.added += 1;

        let logs: Vec<(i64, i64, i64)> = revlog.query_map([cid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        for (id, ease, time) in logs {
            cards::add_review(
                &svc.db,
                &ReviewLog {
                    id: format!("anki-{id}"),
                    card_id: card.id.clone(),
                    rating: ease as u8,
                    reviewed_at: id, // revlog ids are review times in ms
                    device_id: "anki".into(),
                    duration_ms: Some(time),
                },
            )?;
            report.reviews += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_and_furigana() {
        assert_eq!(plain_text("<b>清掃</b>&nbsp;を行う [sound:a.mp3]"), "清掃 を行う");
        assert_eq!(strip_furigana("食[た]べる"), "食べる");
        assert_eq!(strip_furigana(" 清掃"), "清掃");
    }
}
