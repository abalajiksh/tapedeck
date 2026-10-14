//! Japanese in Hepburn, with kanji read by a dictionary.
//!
//! The reading comes from lindera (MIT) with IPADIC embedded in the binary —
//! chosen over UniDic on 2026-10-14 by measurement: a quarter of the size, less
//! memory, and fewer misreadings on a real lyric. The rules on top are cutlet's
//! (polm/cutlet, MIT): particles は へ を as wa e wo, っ carried across a token
//! boundary, and which tokens are written together. cutlet reads UniDic's
//! part-of-speech labels; IPADIC's differ (記号 not 補助記号, 接頭詞 not 接頭辞),
//! and they are mapped here.

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

use super::kana::{is_kana, romaji, to_hiragana};
use super::{Part, Word};

/// IPADIC's fields: part of speech ×4, conjugation ×2, base form, reading,
/// pronunciation. The reading spells long vowels out (トウキョウ), the
/// pronunciation marks them (トーキョー) — which is the difference between
/// the two Hepburn styles.
const READING: usize = 7;
const PRONUNCIATION: usize = 8;

fn segmenter() -> Option<&'static Segmenter> {
    static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEG.get_or_init(|| match load_dictionary("embedded://ipadic") {
        Ok(d) => Some(Segmenter::new(Mode::Normal, d, None)),
        Err(e) => {
            tracing::warn!(
                "🎤 Japanese dictionary unavailable, kanji will be left as written: {e}"
            );
            None
        }
    })
    .as_ref()
}

struct Tok {
    surface: String,
    pos1: String,
    pos2: String,
    /// In katakana; `None` for a word the dictionary does not know.
    reading: Option<String>,
}

fn tokens(text: &str, macrons: bool) -> Option<Vec<Tok>> {
    let seg = segmenter()?;
    let mut toks = seg.segment(Cow::Borrowed(text)).ok()?;
    Some(
        toks.iter_mut()
            .map(|t| {
                let surface = t.surface.to_string();
                let d = t.details();
                let field = |i: usize| {
                    d.get(i).filter(|v| !v.is_empty() && **v != "*").map(|v| v.to_string())
                };
                Tok {
                    pos1: field(0).unwrap_or_default(),
                    pos2: field(1).unwrap_or_default(),
                    reading: field(if macrons { PRONUNCIATION } else { READING })
                        .or_else(|| field(READING)),
                    surface,
                }
            })
            .collect(),
    )
}

/// Full-width Latin and punctuation in plain ASCII, and the few marks Japanese
/// typesetting has its own forms of.
fn punct(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '。' => '.',
            '、' => ',',
            '「' | '」' | '『' | '』' => '"',
            '・' => '-',
            '〜' | '～' => '~',
            c if ('\u{FF01}'..='\u{FF5E}').contains(&c) => {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            }
            c => c,
        })
        .collect()
}

fn is_kanji(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF) || c == '々'
}

/// Whether there is no space between this token and the next, after cutlet.
fn joins_next(t: &Tok, n: &Tok) -> bool {
    // お酒 → osake: a prefix is part of its word.
    if t.pos1 == "接頭詞" {
        return true;
    }
    // Punctuation and suffixes attach to what they follow.
    if n.pos1 == "記号"
        || n.pos2 == "接尾"
        || n.reading.is_none() && !n.surface.chars().any(|c| c.is_alphanumeric())
    {
        return true;
    }
    // こ・ん・な・の is spelled out letter by letter: ko-n-na-no.
    if t.surface == "・" {
        return true;
    }
    // 思えば → omoeba, 食べて → tabete. Only these: cutlet joins every
    // conjunctive particle, but と, が, けど, から are words a singer sets
    // apart (くれると → kureru to).
    if n.pos1 == "助詞"
        && n.pos2 == "接続助詞"
        && matches!(n.surface.as_str(), "て" | "で" | "ば" | "ちゃ" | "じゃ")
    {
        return true;
    }
    // The contracted auxiliaries a verb carries: 過ごしてた → sugoshiteta,
    // かわいちゃう → kawaichau. A full いる stays a word (mite iru).
    if n.pos1 == "動詞"
        && n.pos2 == "非自立"
        && n.surface.starts_with(['て', 'で', 'ち', 'じ', 'と', 'ど'])
    {
        return true;
    }
    // そうでした → sou deshita, but 遊ぼう → asobou. The copula in any form
    // (です, でし, でしょ) is its own word.
    if matches!(t.pos1.as_str(), "動詞" | "助動詞" | "形容詞")
        && n.pos1 == "助動詞"
        && !n.surface.starts_with('で')
    {
        return true;
    }
    // 333 → 333.
    t.surface.chars().all(|c| c.is_ascii_digit()) && n.surface.chars().all(|c| c.is_ascii_digit())
}

pub(super) fn words(run: &str, macrons: bool) -> Vec<Word> {
    let Some(toks) = tokens(run, macrons) else {
        return super::kana::words_without_dictionary(run, macrons);
    };
    let mut out: Vec<Word> = Vec::new();
    let mut join = false;
    // A reading that ended on っ doubles the next token's first consonant
    // (乗っ|た → notta).
    let mut sokuon = false;
    // Kana the previous token's reading already said (晴|レタ: 晴 reads ハレ,
    // so the レ written after it in katakana is not said twice).
    let mut said_already: Option<char> = None;

    for (i, t) in toks.iter().enumerate() {
        let mut part = if t.surface.is_ascii() || t.pos1 == "記号" {
            Part { o: t.surface.clone(), r: punct(&t.surface), lit: true }
        } else if t.pos1 == "助詞" && matches!(t.surface.as_str(), "は" | "へ" | "を") {
            let r = match t.surface.as_str() {
                "は" => "wa",
                "へ" => "e",
                _ => "wo",
            };
            Part { o: t.surface.clone(), r: r.into(), lit: false }
        } else {
            let source = match &t.reading {
                Some(r) => Some(r.clone()),
                None if t.surface.chars().all(is_kana) => Some(t.surface.clone()),
                None => None,
            };
            match source {
                Some(mut kana) => {
                    if let Some(c) = said_already.take() {
                        let h = to_hiragana(&kana);
                        if h.starts_with(c) {
                            kana = h.chars().skip(1).collect();
                        }
                    }
                    Part { o: t.surface.clone(), r: romaji(&kana, macrons), lit: false }
                }
                // Kanji the dictionary has no reading for: carried as written
                // rather than guessed.
                None => Part { o: t.surface.clone(), r: t.surface.clone(), lit: true },
            }
        };
        said_already = None;

        if std::mem::take(&mut sokuon) && !part.lit {
            if let Some(first) = part.r.chars().next().filter(|c| !"aiueo".contains(*c)) {
                let double = if part.r.starts_with("ch") { 't' } else { first };
                part.r.insert(0, double);
            }
        }
        if let Some(r) = &t.reading {
            let h = to_hiragana(r);
            sokuon = h.ends_with('っ');
            // The surface ends on a kanji but its reading on kana: that kana is
            // okurigana the dictionary attached, and if the next token begins
            // with it the song wrote it out (in katakana, here) separately.
            if t.surface.chars().last().is_some_and(is_kanji) {
                said_already = h.chars().last().filter(|_| h.chars().count() > 1);
            }
        }

        if join {
            if let Some(w) = out.last_mut() {
                w.parts.push(part);
            } else {
                out.push(Word { parts: vec![part], syl: false });
            }
        } else {
            out.push(Word { parts: vec![part], syl: false });
        }
        // The okurigana continuation is one word with what it continues.
        join = toks.get(i + 1).is_some_and(|n| {
            joins_next(t, n) || said_already.is_some_and(|c| to_hiragana(&n.surface).starts_with(c))
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str, macrons: bool) -> String {
        words(s, macrons)
            .iter()
            .map(|w| w.parts.iter().map(|p| p.r.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn cutlets_own_examples() {
        // cutlet's README, with foreign spellings off — "katsu karee" rather
        // than "cutlet curry", which is UniDic's lemma and not used here.
        assert_eq!(text("カツカレーは美味しい", false), "katsu karee wa oishii");
        assert_eq!(
            text("彼女は王への手紙を読み上げた。", false),
            "kanojo wa ou e no tegami wo yomiageta."
        );
    }

    #[test]
    fn macrons_come_from_the_pronunciation() {
        assert_eq!(text("東京", true), "tōkyō");
        assert_eq!(text("東京", false), "toukyou");
    }

    /// Short phrases from the sample lyric (Hare Hare Yukai), each the rule it
    /// pins.
    #[test]
    fn the_lyric_cases() {
        assert_eq!(
            text("何もかもを巻き込んだ想像で遊ぼう", false),
            "nanimokamo wo makikonda souzou de asobou"
        );
        // 晴 reads ハレ and the song writes the レ again in katakana: said once.
        assert_eq!(text("晴レタ", false), "hareta");
        // In the whole phrase IPADIC takes 日 for a suffix ("bi", joined) where
        // the song says "hi". A dictionary misread, pinned as what it is
        // rather than hidden — it is what the reader will show.
        assert_eq!(text("アル晴レタ日ノ事", false), "aru haretabi no koto");
        assert_eq!(text("明日また会うとき", false), "ashita mata au toki");
    }
}
