//! Morphological analysis (lindera + embedded IPADIC): readings, dictionary forms, furigana.

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use serde::{Deserialize, Serialize};

use crate::kana::{has_kanji, is_kanji, kata_to_hira};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub surface: String,
    /// Character offsets into the analysed text.
    pub start: usize,
    pub end: usize,
    /// Hiragana reading, when the dictionary knows it.
    pub reading: Option<String>,
    /// Dictionary form (原形).
    pub base: String,
    pub pos: String,
}

/// A run of text with an optional hiragana reading (ruby).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ruby {
    pub t: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
}

fn segmenter() -> Option<&'static Segmenter> {
    static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEG.get_or_init(|| {
        let dict = load_dictionary("embedded://ipadic").ok()?;
        Some(Segmenter::new(Mode::Normal, dict, None))
    })
    .as_ref()
}

pub fn tokenize(text: &str) -> Vec<Token> {
    let Some(seg) = segmenter() else { return Vec::new() };
    let Ok(mut tokens) = seg.segment(Cow::Borrowed(text)) else { return Vec::new() };
    let mut out = Vec::with_capacity(tokens.len());
    for tok in tokens.iter_mut() {
        let surface = tok.surface.to_string();
        let start = text[..tok.byte_start].chars().count();
        let end = start + surface.chars().count();
        let d = tok.details();
        // IPADIC: 品詞,細分類1,細分類2,細分類3,活用型,活用形,原形,読み,発音
        let field = |i: usize| d.get(i).copied().filter(|v| *v != "*" && !v.is_empty());
        let pos = field(0).unwrap_or("").to_string();
        let base = field(6).unwrap_or(&surface).to_string();
        let reading = field(7).map(kata_to_hira);
        out.push(Token { surface, start, end, reading, base, pos });
    }
    out
}

/// Split `text` into ruby segments: kanji-bearing tokens get their reading,
/// with kana shared at the start / end of the token moved out of the ruby
/// (食べた → 食[た]べた).
pub fn furigana(text: &str) -> Vec<Ruby> {
    let mut out: Vec<Ruby> = Vec::new();
    let push_plain = |out: &mut Vec<Ruby>, s: &str| {
        if s.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.r.is_none() {
                last.t.push_str(s);
                return;
            }
        }
        out.push(Ruby { t: s.to_string(), r: None });
    };
    // The tokenizer drops whitespace; copy anything it skipped back in, so the
    // segments spell out exactly `text`.
    let mut at = 0;
    for tok in tokenize(text) {
        if let Some(i) = text[at..].find(tok.surface.as_str()) {
            push_plain(&mut out, &text[at..at + i]);
            at += i + tok.surface.len();
        }
        let s: Vec<char> = tok.surface.chars().collect();
        let Some(reading) = tok.reading.as_ref().filter(|r| has_kanji(&tok.surface) && **r != kata_to_hira(&tok.surface))
        else {
            push_plain(&mut out, &tok.surface);
            continue;
        };
        let r: Vec<char> = reading.chars().collect();
        let same = |a: char, b: char| kata_to_hira(&a.to_string()) == b.to_string();
        let mut head = 0;
        while head < s.len() - 1 && head < r.len() && !is_kanji(s[head]) && same(s[head], r[head]) {
            head += 1;
        }
        let mut tail = 0;
        while tail < s.len() - head - 1
            && tail < r.len() - head
            && !is_kanji(s[s.len() - 1 - tail])
            && same(s[s.len() - 1 - tail], r[r.len() - 1 - tail])
        {
            tail += 1;
        }
        let core: String = s[head..s.len() - tail].iter().collect();
        let core_r: String = r[head..r.len() - tail].iter().collect();
        push_plain(&mut out, &s[..head].iter().collect::<String>());
        if core_r.is_empty() {
            push_plain(&mut out, &core);
        } else {
            out.push(Ruby { t: core, r: Some(core_r) });
        }
        push_plain(&mut out, &s[s.len() - tail..].iter().collect::<String>());
    }
    push_plain(&mut out, &text[at..]);
    out
}

/// Dictionary-form candidates for the token starting at `offset` (fallback for lookup).
pub fn bases_at(text: &str, offset: usize) -> Vec<(usize, String)> {
    tokenize(text)
        .into_iter()
        .filter(|t| t.start == offset && t.base != t.surface)
        .map(|t| (t.end - t.start, t.base))
        .collect()
}

/// Furigana for a card's sentence: the tokenizer's readings, except for the
/// card's own word, which gets the card's reading (躊躇い is ためらい here,
/// not ちゅうちょ＋い as the tokenizer guesses).
pub fn furigana_for_word(text: &str, expression: &str, reading: &str) -> Vec<Ruby> {
    let segs = furigana(text);
    let Some(span) = crate::lookup::find_word(text, expression, reading) else { return segs };
    let Some(word_segs) = word_ruby(&text[span.clone()], expression, reading) else { return segs };
    let (s, e) = (text[..span.start].chars().count(), text[..span.end].chars().count());
    let plain = |t: String| Ruby { t, r: None };
    let mut out: Vec<Ruby> = Vec::new();
    let mut inserted = false;
    let mut pos = 0;
    for seg in segs {
        let chars: Vec<char> = seg.t.chars().collect();
        let (a, b) = (pos, pos + chars.len());
        pos = b;
        if b <= s || a >= e {
            if a >= e && !inserted {
                out.extend(word_segs.iter().cloned());
                inserted = true;
            }
            out.push(seg);
            continue;
        }
        // Overlaps the word: keep the outside parts as plain text.
        if a < s {
            out.push(plain(chars[..s - a].iter().collect()));
        }
        if !inserted {
            out.extend(word_segs.iter().cloned());
            inserted = true;
        }
        if b > e {
            out.push(plain(chars[e - a..].iter().collect()));
        }
    }
    if !inserted {
        out.extend(word_segs);
    }
    // Merge neighbouring plain runs.
    let mut merged: Vec<Ruby> = Vec::with_capacity(out.len());
    for seg in out.into_iter().filter(|s| !s.t.is_empty()) {
        match merged.last_mut() {
            Some(last) if last.r.is_none() && seg.r.is_none() => last.t.push_str(&seg.t),
            _ => merged.push(seg),
        }
    }
    merged
}

/// Ruby for the word as written in the sentence (`shown`, maybe inflected):
/// the expression's kanji part gets the matching part of the reading.
fn word_ruby(shown: &str, expression: &str, reading: &str) -> Option<Vec<Ruby>> {
    if !has_kanji(shown) {
        return Some(vec![Ruby { t: shown.to_string(), r: None }]);
    }
    let e: Vec<char> = expression.chars().collect();
    let r: Vec<char> = kata_to_hira(reading).chars().collect();
    let hira = |c: char| kata_to_hira(&c.to_string());
    // Leading and trailing kana shared by the written form and the reading (お茶, 躊躇う).
    let mut head = 0;
    while head < e.len() && head < r.len() && !is_kanji(e[head]) && hira(e[head]) == r[head].to_string() {
        head += 1;
    }
    let mut tail = 0;
    while tail < e.len() - head && tail < r.len() - head && !is_kanji(e[e.len() - 1 - tail]) && hira(e[e.len() - 1 - tail]) == r[r.len() - 1 - tail].to_string() {
        tail += 1;
    }
    let core: String = e[head..e.len() - tail].iter().collect();
    let core_r: String = r[head..r.len() - tail].iter().collect();
    let lead: String = e[..head].iter().collect();
    if core.is_empty() || core_r.is_empty() || !shown.starts_with(&format!("{lead}{core}")) {
        return None;
    }
    let rest = &shown[lead.len() + core.len()..];
    let mut out = vec![Ruby { t: lead, r: None }];
    // Kana inside the word (嗅ぎ付け, 暴れん坊) splits the ruby: 嗅【か】ぎ付【つ】.
    out.extend(split_core(&core, &core_r).unwrap_or_else(|| vec![Ruby { t: core.clone(), r: Some(core_r.clone()) }]));
    out.push(Ruby { t: rest.to_string(), r: None });
    Some(out.into_iter().filter(|s| !s.t.is_empty()).collect())
}

/// Align a written form that mixes kanji and kana with its reading, one ruby
/// per kanji run. None when there is no inner kana or no unique alignment.
fn split_core(core: &str, reading: &str) -> Option<Vec<Ruby>> {
    let mut runs: Vec<(bool, String)> = Vec::new(); // (is kanji run, text)
    for c in core.chars() {
        let k = is_kanji(c);
        match runs.last_mut() {
            Some((lk, t)) if *lk == k => t.push(c),
            _ => runs.push((k, c.to_string())),
        }
    }
    if runs.len() < 2 {
        return None;
    }
    fn go(runs: &[(bool, String)], r: &[char], out: &mut Vec<Vec<Ruby>>, acc: &mut Vec<Ruby>) {
        let Some(((kanji, t), rest)) = runs.split_first() else {
            if r.is_empty() {
                out.push(acc.clone());
            }
            return;
        };
        if !kanji {
            let k: Vec<char> = kata_to_hira(t).chars().collect();
            if r.starts_with(&k) {
                acc.push(Ruby { t: t.clone(), r: None });
                go(rest, &r[k.len()..], out, acc);
                acc.pop();
            }
            return;
        }
        for n in 1..=r.len() {
            acc.push(Ruby { t: t.clone(), r: Some(r[..n].iter().collect()) });
            go(rest, &r[n..], out, acc);
            acc.pop();
        }
    }
    let r: Vec<char> = reading.chars().collect();
    let mut found = Vec::new();
    go(&runs, &r, &mut found, &mut Vec::new());
    (found.len() == 1).then(|| found.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_reading_wins_for_the_word() {
        let segs = furigana_for_word("躊躇いもなく僕の股間に頭をうずめる。", "躊躇う", "ためらう");
        let text: String = segs.iter().map(|s| s.t.as_str()).collect();
        assert_eq!(text, "躊躇いもなく僕の股間に頭をうずめる。");
        assert_eq!(segs[0], Ruby { t: "躊躇".into(), r: Some("ためら".into()) });
        assert!(segs[1].t.starts_with('い') && segs[1].r.is_none());
        let segs = furigana_for_word("匂いを嗅ぎ付けた", "嗅ぎ付ける", "かぎつける");
        assert!(segs.contains(&Ruby { t: "嗅".into(), r: Some("か".into()) }) && segs.contains(&Ruby { t: "付".into(), r: Some("つ".into()) }));
        let segs = furigana_for_word("顔には皺があった", "皺", "しわ");
        assert!(segs.contains(&Ruby { t: "皺".into(), r: Some("しわ".into()) }));
    }

    #[test]
    fn furigana_keeps_spaces() {
        let text = "試験内容 試験室で 忍耐力を試す。 ";
        let segs = furigana(text);
        assert_eq!(segs.iter().map(|s| s.t.as_str()).collect::<String>(), text);
    }

    #[test]
    fn furigana_splits_okurigana() {
        let f = furigana("ヤモリを食べた");
        let ruby: Vec<_> = f.iter().filter(|r| r.r.is_some()).collect();
        assert_eq!(ruby.len(), 1);
        assert_eq!(ruby[0].t, "食");
        assert_eq!(ruby[0].r.as_deref(), Some("た"));
        let joined: String = f.iter().map(|r| r.t.as_str()).collect();
        assert_eq!(joined, "ヤモリを食べた");
    }

    #[test]
    fn base_forms() {
        let toks = tokenize("書かせられた");
        assert_eq!(toks[0].base, "書く");
    }
}
