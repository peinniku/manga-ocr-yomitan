//! Kana / kanji helpers.

pub fn kata_to_hira(s: &str) -> String {
    s.chars()
        .map(|c| if ('ァ'..='ヶ').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c })
        .collect()
}

pub fn hira_to_kata(s: &str) -> String {
    s.chars()
        .map(|c| if ('ぁ'..='ゖ').contains(&c) { char::from_u32(c as u32 + 0x60).unwrap_or(c) } else { c })
        .collect()
}

pub fn is_kanji(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF) || matches!(c, '々' | '〆' | 'ヵ' | 'ヶ')
}

pub fn has_kanji(s: &str) -> bool {
    s.chars().any(is_kanji)
}

pub fn is_japanese(c: char) -> bool {
    is_kanji(c) || matches!(c as u32, 0x3040..=0x30FF | 0xFF66..=0xFF9F) || c == 'ー'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(kata_to_hira("ヤバイ"), "やばい");
        assert_eq!(hira_to_kata("たべる"), "タベル");
        assert!(has_kanji("食べる"));
        assert!(!has_kanji("たべる"));
    }
}
