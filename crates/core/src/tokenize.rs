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
    for tok in tokenize(text) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
