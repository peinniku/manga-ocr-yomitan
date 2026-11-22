//! Flatten Yomitan glossary entries (plain strings and structured-content trees)
//! into display lines for the native UI.

use serde_json::Value;

const BLOCK_TAGS: [&str; 12] = ["div", "p", "li", "ol", "ul", "table", "tr", "details", "summary", "thead", "tbody", "dl"];

fn walk(v: &Value, buf: &mut String, lines: &mut Vec<String>) {
    match v {
        Value::String(s) => buf.push_str(s),
        Value::Array(items) => {
            for it in items {
                walk(it, buf, lines);
            }
        }
        Value::Object(o) => {
            if let Some(t) = o.get("type").and_then(|t| t.as_str()) {
                match t {
                    "text" => {
                        if let Some(s) = o.get("text").and_then(|s| s.as_str()) {
                            buf.push_str(s);
                        }
                        return;
                    }
                    "structured-content" => {
                        if let Some(c) = o.get("content") {
                            walk(c, buf, lines);
                        }
                        return;
                    }
                    "image" => return,
                    _ => {}
                }
            }
            let tag = o.get("tag").and_then(|t| t.as_str()).unwrap_or("");
            match tag {
                "br" => flush(buf, lines),
                "img" | "rp" => {}
                "rt" => {
                    let mut inner = String::new();
                    if let Some(c) = o.get("content") {
                        walk(c, &mut inner, lines);
                    }
                    if !inner.is_empty() {
                        buf.push('（');
                        buf.push_str(&inner);
                        buf.push('）');
                    }
                }
                _ => {
                    let block = BLOCK_TAGS.contains(&tag);
                    if block {
                        flush(buf, lines);
                    }
                    if tag == "li" {
                        buf.push_str("・");
                    }
                    if let Some(c) = o.get("content") {
                        walk(c, buf, lines);
                    }
                    if block {
                        flush(buf, lines);
                    } else if tag == "td" || tag == "th" {
                        buf.push(' ');
                    }
                }
            }
        }
        _ => {}
    }
}

fn flush(buf: &mut String, lines: &mut Vec<String>) {
    let line = buf.trim().to_string();
    if !line.is_empty() {
        lines.push(line);
    }
    buf.clear();
}

/// Display lines for one entry's glossary.
pub fn lines(glossary: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let items: Vec<&Value> = match glossary {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    for item in items {
        let mut buf = String::new();
        walk(item, &mut buf, &mut lines);
        flush(&mut buf, &mut lines);
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// The dictionary's own headword line (＊もよお・す③⓪【催す】), redundant with the card header.
    Headword,
    /// Starts a numbered sense: ㊀ ① ❶ ⑴ …
    Sense,
    /// A usage example: 「…」『…』
    Example,
    /// Grammar / usage notes in brackets: （他五）〈なにヲ―〉
    Note,
    Text,
}

pub fn kind(line: &str, index: usize) -> LineKind {
    let first = line.chars().next().unwrap_or(' ');
    if index == 0 && (first == '＊' || first == '*' || line.contains('【')) && line.chars().count() < 40 {
        return LineKind::Headword;
    }
    match first {
        '「' | '『' => LineKind::Example,
        '\u{3280}'..='\u{3289}' | '\u{2460}'..='\u{2473}' | '\u{2776}'..='\u{277f}' | '\u{2474}'..='\u{2487}' | '\u{24b6}'..='\u{24e9}' => {
            LineKind::Sense
        }
        '（' | '(' | '〈' | '〔' | '［' if line.chars().count() < 24 => LineKind::Note,
        _ => LineKind::Text,
    }
}

/// A glossary laid out for reading: senses stand out, examples are indented
/// and muted, and the dictionary's repeated headword line is dropped.
pub fn view(glossary: &Value, limit: Option<usize>, cx: &gpui_kit::App) -> gpui_kit::Div {
    use gpui_kit::component::{ActiveTheme, v_flex};
    use gpui_kit::prelude::FluentBuilder as _;
    use gpui_kit::*;
    let theme = cx.theme();
    let mut col = v_flex().gap(px(3.)).text_sm().line_height(relative(1.55));
    let all = lines(glossary);
    let mut shown = 0;
    for (i, line) in all.iter().enumerate() {
        let k = kind(line, i);
        if k == LineKind::Headword {
            continue;
        }
        if limit.is_some_and(|n| shown >= n) {
            col = col.child(div().text_xs().text_color(theme.muted_foreground).child("…"));
            break;
        }
        shown += 1;
        col = col.child(
            div()
                .when(k == LineKind::Sense, |d| d.mt_1().font_weight(FontWeight::MEDIUM))
                .when(k == LineKind::Example, |d| d.pl_4().text_color(theme.muted_foreground))
                .when(k == LineKind::Note, |d| d.text_color(theme.muted_foreground))
                .child(line.clone()),
        );
    }
    col
}

/// A short plain-text summary (first lines), e.g. for list rows and LLM hints.
pub fn summary(glossary: &Value, max_chars: usize) -> String {
    let all = lines(glossary);
    let joined = all
        .iter()
        .enumerate()
        .filter(|(i, l)| !matches!(kind(l, *i), LineKind::Headword | LineKind::Note | LineKind::Example))
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut s: String = joined.chars().take(max_chars).collect();
    if joined.chars().count() > max_chars {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattens_structured_content() {
        let g = json!([{"type": "structured-content", "content": [
            {"tag": "span", "content": "見出し"},
            {"tag": "div", "content": ["一つ目"]},
            {"tag": "ul", "content": [{"tag": "li", "content": "a"}, {"tag": "li", "content": "b"}]},
            {"tag": "ruby", "content": ["漢", {"tag": "rt", "content": "かん"}]}
        ]}, "plain"]);
        assert_eq!(lines(&g), vec!["見出し", "一つ目", "・a", "・b", "漢（かん）", "plain"]);
    }

    #[test]
    fn classifies_lines() {
        assert_eq!(kind("＊もよお・す③⓪(モヨホス)【催す】", 0), LineKind::Headword);
        assert_eq!(kind("㊀計画して、会などを開く。", 3), LineKind::Sense);
        assert_eq!(kind("「涙を―」", 4), LineKind::Example);
        assert_eq!(kind("（他五）〈なにヲ―〉", 1), LineKind::Note);
        assert_eq!(kind("特定の問題について、集まった人が意見を述べあって議論をすること。", 2), LineKind::Text);
    }
}
