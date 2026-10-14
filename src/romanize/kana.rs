//! Kana to Hepburn romaji.
//!
//! The table and the rules for small kana, っ, ん and ー are ported from cutlet
//! (polm/cutlet, MIT, © 2020 Paul O'Leary McCann): `mapping.py` and
//! `Cutlet.get_single_mapping`. Reading kanji is not here — that needs a
//! dictionary, and lives in `japanese` behind the `cjk` feature.

use super::{Part, Word};

const TABLE: &[(char, &str)] = &[
    ('あ', "a"),
    ('い', "i"),
    ('う', "u"),
    ('え', "e"),
    ('お', "o"),
    ('か', "ka"),
    ('き', "ki"),
    ('く', "ku"),
    ('け', "ke"),
    ('こ', "ko"),
    ('が', "ga"),
    ('ぎ', "gi"),
    ('ぐ', "gu"),
    ('げ', "ge"),
    ('ご', "go"),
    ('さ', "sa"),
    ('し', "shi"),
    ('す', "su"),
    ('せ', "se"),
    ('そ', "so"),
    ('ざ', "za"),
    ('じ', "ji"),
    ('ず', "zu"),
    ('ぜ', "ze"),
    ('ぞ', "zo"),
    ('た', "ta"),
    ('ち', "chi"),
    ('つ', "tsu"),
    ('て', "te"),
    ('と', "to"),
    ('だ', "da"),
    ('ぢ', "ji"),
    ('づ', "zu"),
    ('で', "de"),
    ('ど', "do"),
    ('な', "na"),
    ('に', "ni"),
    ('ぬ', "nu"),
    ('ね', "ne"),
    ('の', "no"),
    ('は', "ha"),
    ('ひ', "hi"),
    ('ふ', "fu"),
    ('へ', "he"),
    ('ほ', "ho"),
    ('ば', "ba"),
    ('び', "bi"),
    ('ぶ', "bu"),
    ('べ', "be"),
    ('ぼ', "bo"),
    ('ぱ', "pa"),
    ('ぴ', "pi"),
    ('ぷ', "pu"),
    ('ぺ', "pe"),
    ('ぽ', "po"),
    ('ま', "ma"),
    ('み', "mi"),
    ('む', "mu"),
    ('め', "me"),
    ('も', "mo"),
    ('や', "ya"),
    ('ゆ', "yu"),
    ('よ', "yo"),
    ('ら', "ra"),
    ('り', "ri"),
    ('る', "ru"),
    ('れ', "re"),
    ('ろ', "ro"),
    ('わ', "wa"),
    ('ゐ', "wi"),
    ('ゑ', "we"),
    ('を', "wo"),
    ('ん', "n"),
    ('ゔ', "vu"),
    ('ゃ', "ya"),
    ('ゅ', "yu"),
    ('ょ', "yo"),
    ('ぁ', "a"),
    ('ぃ', "i"),
    ('ぅ', "u"),
    ('ぇ', "e"),
    ('ぉ', "o"),
    ('ゕ', "ka"),
    ('ゖ', "ke"),
    ('ゎ', "wa"),
];

/// Pairs Hepburn spells as one sound rather than kana by kana.
const DIGRAPHS: &[(&str, &str)] = &[
    ("しゃ", "sha"),
    ("しゅ", "shu"),
    ("しょ", "sho"),
    ("じゃ", "ja"),
    ("じゅ", "ju"),
    ("じょ", "jo"),
    ("ちゃ", "cha"),
    ("ちゅ", "chu"),
    ("ちょ", "cho"),
];

const SMALL: &str = "ゃゅょぁぃぅぇぉ";

fn table(c: char) -> Option<&'static str> {
    TABLE.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

fn digraph(a: char, b: char) -> Option<&'static str> {
    let mut key = String::with_capacity(6);
    key.push(a);
    key.push(b);
    DIGRAPHS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Katakana to hiragana, leaving everything else — the prolonged-sound mark
/// included — as it was.
pub(super) fn to_hiragana(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0x30A1..=0x30F6 => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

pub(super) fn is_kana(c: char) -> bool {
    ('\u{3041}'..='\u{30FF}').contains(&c) && c != '・'
}

fn macron(v: char) -> Option<char> {
    Some(match v {
        'a' => 'ā',
        'i' => 'ī',
        'u' => 'ū',
        'e' => 'ē',
        'o' => 'ō',
        _ => return None,
    })
}

/// Kana (either script) in Hepburn. `macrons` writes ー as a macron over the
/// vowel it lengthens; otherwise the vowel is written twice, as cutlet does.
pub(super) fn romaji(kana: &str, macrons: bool) -> String {
    let k: Vec<char> = to_hiragana(kana).chars().collect();
    let mut out = String::new();
    for (i, &c) in k.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| k[j]);
        let next = k.get(i + 1).copied();
        // Iteration marks: ゝ repeats the kana before it, ゞ voices it.
        if c == 'ゝ' || c == 'ゞ' {
            if let Some(p) = prev.and_then(table) {
                out.push_str(p);
            }
            continue;
        }
        if let Some(p) = prev {
            if let Some(d) = digraph(p, c) {
                out.push_str(d);
                continue;
            }
        }
        if let Some(n) = next {
            if digraph(c, n).is_some() {
                continue;
            }
            if SMALL.contains(n) {
                if c == 'っ' {
                    continue;
                }
                if let (Some(base), Some(small)) = (table(c), table(n)) {
                    // きゃ → k + ya; ふぁ → f + a.
                    out.push_str(&base[..base.len() - 1]);
                    out.push_str(small);
                }
                continue;
            }
        }
        if SMALL.contains(c) && prev.is_some_and(|p| table(p).is_some()) {
            continue;
        }
        match c {
            'ー' => {
                let last = out.chars().last();
                match (macrons, last.and_then(macron)) {
                    (true, Some(m)) => {
                        out.pop();
                        out.push(m);
                    }
                    (false, Some(_)) => out.push(last.expect("matched")),
                    _ => {}
                }
            }
            'っ' => {
                // Doubles the next consonant; っち is tch, as Hepburn writes it.
                if let Some(n) = next.and_then(table) {
                    if n.starts_with("ch") {
                        out.push('t');
                    } else if let Some(f) = n.chars().next().filter(|f| !"aiueo".contains(*f)) {
                        out.push(f);
                    }
                }
            }
            'ん' => {
                out.push('n');
                if next.is_some_and(|n| "あいうえおやゆよ".contains(n)) {
                    out.push('\'');
                }
            }
            _ => match table(c) {
                Some(r) => out.push_str(r),
                None => out.push(c),
            },
        }
    }
    out
}

/// A run of Japanese with no dictionary to read its kanji: kana are
/// romanized, kanji are carried as written. Used by the build without the
/// `cjk` feature, where a partial answer that shows its gaps beats a guess.
#[cfg_attr(feature = "cjk", allow(dead_code))]
pub(super) fn words_without_dictionary(run: &str, macrons: bool) -> Vec<Word> {
    let mut parts: Vec<Part> = Vec::new();
    let mut buf = String::new();
    let mut kana_buf = true;
    for c in run.chars() {
        let k = is_kana(c);
        if !buf.is_empty() && k != kana_buf {
            let r = if kana_buf { romaji(&buf, macrons) } else { buf.clone() };
            parts.push(Part { o: std::mem::take(&mut buf), r, lit: !kana_buf });
        }
        kana_buf = k;
        buf.push(c);
    }
    if !buf.is_empty() {
        let r = if kana_buf { romaji(&buf, macrons) } else { buf.clone() };
        parts.push(Part { o: buf, r, lit: !kana_buf });
    }
    vec![Word { parts, syl: false }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hepburn_as_cutlet_writes_it() {
        assert_eq!(romaji("かつかれー", false), "katsukaree");
        assert_eq!(romaji("カツカレー", true), "katsukarē");
        assert_eq!(romaji("きょう", false), "kyou");
        assert_eq!(romaji("しゃしん", false), "shashin");
        assert_eq!(romaji("まっちゃ", false), "matcha");
        assert_eq!(romaji("きって", false), "kitte");
        assert_eq!(romaji("ほんや", false), "hon'ya");
        assert_eq!(romaji("ファン", false), "fan");
        assert_eq!(romaji("ワクワク", false), "wakuwaku");
    }
}
