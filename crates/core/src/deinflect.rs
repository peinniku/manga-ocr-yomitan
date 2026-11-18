//! Japanese deinflection, Yomitan style.
//!
//! Each rule rewrites a suffix (`kana_in` -> `kana_out`). `rules_in` names the
//! word types the *current* form must have for the rule to apply; the original
//! text is untyped and accepts every rule. `rules_out` is the type of the
//! result. A deinflected candidate only matches a dictionary entry whose
//! `rules` field (v1 / v5 / vs / vk / adj-i) shares a type with it.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Word-type bit flags.
pub mod t {
    pub const V1: u16 = 1 << 0;
    pub const V5: u16 = 1 << 1;
    pub const VS: u16 = 1 << 2;
    pub const VK: u16 = 1 << 3;
    pub const ADJ_I: u16 = 1 << 4;
    pub const MASU: u16 = 1 << 5;
    pub const TE: u16 = 1 << 6;
    pub const TA: u16 = 1 << 7;
    /// する-noun: 勉強する → 勉強 matches entries regardless of their rules.
    pub const N_SURU: u16 = 1 << 8;
    pub const TERMINAL: u16 = V1 | V5 | VS | VK | ADJ_I;
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub reason: &'static str,
    pub kana_in: String,
    pub kana_out: String,
    pub rules_in: u16,
    pub rules_out: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub term: String,
    /// 0 = untyped original text.
    pub rules: u16,
    pub reasons: Vec<&'static str>,
}

/// Godan rows: dictionary ending -> (a, i, e, o, te, ta).
const GODAN: [(&str, [&str; 6]); 9] = [
    ("う", ["わ", "い", "え", "お", "って", "った"]),
    ("く", ["か", "き", "け", "こ", "いて", "いた"]),
    ("ぐ", ["が", "ぎ", "げ", "ご", "いで", "いだ"]),
    ("す", ["さ", "し", "せ", "そ", "して", "した"]),
    ("つ", ["た", "ち", "て", "と", "って", "った"]),
    ("ぬ", ["な", "に", "ね", "の", "んで", "んだ"]),
    ("ぶ", ["ば", "び", "べ", "ぼ", "んで", "んだ"]),
    ("む", ["ま", "み", "め", "も", "んで", "んだ"]),
    ("る", ["ら", "り", "れ", "ろ", "って", "った"]),
];

struct B(Vec<Rule>);

impl B {
    fn r(&mut self, reason: &'static str, kin: &str, kout: &str, rin: u16, rout: u16) {
        self.0.push(Rule { reason, kana_in: kin.into(), kana_out: kout.into(), rules_in: rin, rules_out: rout });
    }

    /// One inflection across verb classes. `v5` maps a godan row to the suffix
    /// replacing its dictionary ending; `vs` / `vk` replace する / くる; `adj` replaces い.
    #[allow(clippy::too_many_arguments)]
    fn forms(
        &mut self,
        reason: &'static str,
        rin: u16,
        v1: Option<&str>,
        v5: Option<&dyn Fn(&[&str; 6]) -> String>,
        vs: &[&str],
        vk: &[&str],
        adj: Option<&str>,
    ) {
        if let Some(s) = v1 {
            self.r(reason, s, "る", rin, t::V1);
        }
        if let Some(f) = v5 {
            for (base, row) in GODAN.iter() {
                self.r(reason, &f(row), base, rin, t::V5);
            }
        }
        for s in vs {
            self.r(reason, s, "する", rin, t::VS);
        }
        for s in vk {
            self.r(reason, s, "くる", rin, t::VK);
            if s.starts_with('こ') || s.starts_with('き') || s.starts_with('く') {
                let rest: String = s.chars().skip(1).collect();
                self.r(reason, &format!("来{rest}"), "来る", rin, t::VK);
            }
        }
        if let Some(s) = adj {
            self.r(reason, s, "い", rin, t::ADJ_I);
        }
    }
}

fn build_rules() -> Vec<Rule> {
    use t::*;
    let mut b = B(Vec::new());
    let row = |i: usize| move |r: &[&str; 6]| r[i].to_string();
    let row_plus = |i: usize, s: &'static str| move |r: &[&str; 6]| format!("{}{}", r[i], s);

    // past / te
    b.forms("past", TA, Some("た"), Some(&row(5)), &["した"], &["きた"], Some("かった"));
    b.r("past", "行った", "行く", TA, V5);
    b.r("past", "いった", "いく", TA, V5);
    b.forms("te", TE, Some("て"), Some(&row(4)), &["して"], &["きて"], Some("くて"));
    b.r("te", "行って", "行く", TE, V5);
    b.r("te", "いって", "いく", TE, V5);
    for (kin, kout) in [("たら", "た"), ("だら", "だ")] {
        b.r("-tara", kin, kout, 0, TA);
    }
    for (kin, kout) in [("たり", "た"), ("だり", "だ")] {
        b.r("-tari", kin, kout, 0, TA);
    }

    // negatives (ない inflects like an adjective)
    b.forms("negative", ADJ_I, Some("ない"), Some(&row_plus(0, "ない")), &["しない"], &["こない"], None);
    b.forms("negative", 0, Some("ず"), Some(&row_plus(0, "ず")), &["せず"], &["こず"], None);
    b.forms("negative", 0, Some("ずに"), Some(&row_plus(0, "ずに")), &["せずに"], &["こずに"], None);
    b.forms("negative", 0, Some("ぬ"), Some(&row_plus(0, "ぬ")), &["せぬ"], &[], None);
    b.forms("negative", 0, Some("ん"), Some(&row_plus(0, "ん")), &["せん"], &[], None);
    b.r("negative", "くない", "い", ADJ_I, ADJ_I);

    // polite
    b.forms("polite", MASU, Some("ます"), Some(&row_plus(1, "ます")), &["します"], &["きます"], None);
    for kin in ["ました", "ません", "ませんでした", "ましょう", "まして", "ませ"] {
        b.r("polite", kin, "ます", 0, MASU);
    }

    // desire / appearance / adverbial / noun
    b.forms("-tai", ADJ_I, Some("たい"), Some(&row_plus(1, "たい")), &["したい"], &["きたい"], None);
    b.forms("-sou", 0, Some("そう"), Some(&row_plus(1, "そう")), &[], &[], Some("そう"));
    b.r("adv", "く", "い", 0, ADJ_I);
    b.r("noun", "さ", "い", 0, ADJ_I);
    b.r("conditional", "ければ", "い", 0, ADJ_I);
    for sugi in ["すぎる", "過ぎる"] {
        let vs = format!("し{sugi}");
        b.forms("-sugiru", V1, Some(sugi), Some(&row_plus(1, sugi)), &[vs.as_str()], &[], Some(sugi));
    }
    b.forms("-nagara", 0, Some("ながら"), Some(&row_plus(1, "ながら")), &["しながら"], &[], None);

    // conditional / volitional / imperative
    b.forms("conditional", 0, Some("れば"), Some(&row_plus(2, "ば")), &["すれば"], &["くれば"], None);
    b.forms("volitional", 0, Some("よう"), Some(&row_plus(3, "う")), &["しよう"], &["こよう"], None);
    b.forms("imperative", 0, Some("ろ"), Some(&row(2)), &["しろ", "せよ"], &["こい"], None);
    b.r("imperative", "よ", "る", 0, V1);

    // potential / passive / causative produce new ichidan verbs
    b.forms("potential", V1, Some("られる"), Some(&row_plus(2, "る")), &[], &["こられる"], None);
    b.r("potential", "できる", "する", V1, VS);
    b.forms("passive", V1, None, Some(&row_plus(0, "れる")), &["される"], &[], None);
    b.forms("causative", V1, Some("させる"), Some(&row_plus(0, "せる")), &["させる"], &["こさせる"], None);
    b.forms("causative", V5, None, Some(&row_plus(0, "す")), &[], &[], None);

    // auxiliaries attached to the te-form
    for (kin, kout, rin) in [
        ("ている", "て", V1),
        ("でいる", "で", V1),
        ("てる", "て", V1),
        ("でる", "で", V1),
        ("てしまう", "て", V5),
        ("でしまう", "で", V5),
        ("ちゃう", "て", V5),
        ("じゃう", "で", V5),
        ("ておく", "て", V5),
        ("でおく", "で", V5),
        ("とく", "て", V5),
        ("どく", "で", V5),
        ("てある", "て", V5),
        ("てくる", "て", VK),
        ("ていく", "て", V5),
        ("てみる", "て", V1),
    ] {
        b.r("te-aux", kin, kout, rin, TE);
    }

    // masu stem (連用形) on its own, e.g. 書き / 食べ
    b.forms("stem", 0, Some(""), Some(&row(1)), &["し"], &[], None);

    // colloquial contractions
    b.r("colloquial", "んない", "らない", 0, ADJ_I);
    b.r("colloquial", "んねえ", "らない", 0, ADJ_I);
    b.r("colloquial", "ねえ", "ない", 0, ADJ_I);
    b.r("colloquial", "ねー", "ない", 0, ADJ_I);
    b.r("colloquial", "なきゃ", "なければ", 0, 0);
    b.r("colloquial", "なくちゃ", "なくては", 0, 0);
    b.r("negative", "なければ", "ない", 0, ADJ_I);
    b.r("negative", "なくて", "ない", 0, ADJ_I);
    b.r("negative", "なくては", "ない", 0, ADJ_I);

    b.r("suru", "する", "", VS, N_SURU);
    b.0
}

struct Index {
    by_suffix: HashMap<String, Vec<Rule>>,
    max_suffix: usize,
}

fn index() -> &'static Index {
    static IDX: OnceLock<Index> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut by_suffix: HashMap<String, Vec<Rule>> = HashMap::new();
        for r in build_rules() {
            by_suffix.entry(r.kana_in.clone()).or_default().push(r);
        }
        let max_suffix = by_suffix.keys().map(|k| k.chars().count()).max().unwrap_or(0);
        Index { by_suffix, max_suffix }
    })
}

const MAX_DEPTH: usize = 6;

/// The original text plus every reachable deinflected form.
pub fn deinflect(text: &str) -> Vec<Candidate> {
    let idx = index();
    let mut results = vec![Candidate { term: text.to_string(), rules: 0, reasons: vec![] }];
    let mut seen: HashSet<(String, u16)> = HashSet::from([(text.to_string(), 0)]);
    let mut i = 0;
    while i < results.len() {
        let cur = results[i].clone();
        i += 1;
        if cur.reasons.len() >= MAX_DEPTH {
            continue;
        }
        let chars: Vec<char> = cur.term.chars().collect();
        for n in 0..=idx.max_suffix.min(chars.len()) {
            let suffix: String = chars[chars.len() - n..].iter().collect();
            let Some(rules) = idx.by_suffix.get(&suffix) else { continue };
            let stem: String = chars[..chars.len() - n].iter().collect();
            for rule in rules {
                if cur.rules != 0 && cur.rules & rule.rules_in == 0 {
                    continue;
                }
                let term = format!("{stem}{}", rule.kana_out);
                if term.is_empty() || (stem.is_empty() && rule.kana_out.chars().count() < 2) {
                    continue;
                }
                if !seen.insert((term.clone(), rule.rules_out)) {
                    continue;
                }
                let mut reasons = Vec::with_capacity(cur.reasons.len() + 1);
                reasons.push(rule.reason);
                reasons.extend(cur.reasons.iter().copied());
                results.push(Candidate { term, rules: rule.rules_out, reasons });
            }
        }
    }
    results
}

/// Untyped candidates match anything; typed ones need a shared word type.
pub fn rules_match(candidate: u16, entry_rules: &str) -> bool {
    if candidate == 0 || candidate & t::N_SURU != 0 {
        return true;
    }
    let terminal = candidate & t::TERMINAL;
    if terminal == 0 {
        return false;
    }
    entry_rules.split_whitespace().any(|r| {
        let bit = match r {
            "v1" => t::V1,
            "v5" => t::V5,
            "vs" => t::VS,
            "vk" => t::VK,
            "adj-i" => t::ADJ_I,
            _ => 0,
        };
        bit & terminal != 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(text: &str, term: &str, rule: u16) -> bool {
        deinflect(text).iter().any(|c| c.term == term && c.rules & rule != 0)
    }

    #[test]
    fn common_forms() {
        assert!(has("食べなかった", "食べる", t::V1));
        assert!(has("書かせられた", "書く", t::V5));
        assert!(has("行ってしまった", "行く", t::V5));
        assert!(has("読みました", "読む", t::V5));
        assert!(has("美しくない", "美しい", t::ADJ_I));
        assert!(has("来なかった", "来る", t::VK));
        assert!(has("泳げる", "泳ぐ", t::V5));
        assert!(has("言われて", "言う", t::V5));
        assert!(has("待ってて", "待つ", t::V5));
        assert!(has("わかんない", "わかる", t::V5));
        assert!(has("走れ", "走る", t::V5));
        assert!(has("食べ過ぎた", "食べる", t::V1));
        assert!(has("勉強している", "勉強", t::N_SURU));
    }

    #[test]
    fn rule_matching() {
        assert!(rules_match(0, ""));
        assert!(rules_match(t::V5, "v5"));
        assert!(!rules_match(t::V5, "v1"));
        assert!(!rules_match(t::TE, "v1"));
        assert!(rules_match(t::N_SURU, ""));
    }
}
