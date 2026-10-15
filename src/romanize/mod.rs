//! Lyrics in Latin letters, for reading along with a song in a script you do
//! not read.
//!
//! Native Rust throughout, by decision: no Python, no subprocess. The rules are
//! ported from reference implementations, each credited in its module —
//! Aksharamukha for the Indic scripts, cutlet for Japanese, korean-romanizer
//! for Korean (with its pronunciation bugs fixed rather than copied), iuliia
//! for Russian.
//!
//! ## Line for line, always
//!
//! A `[[line:N]]` quote is an index, so a romanization is produced **per
//! line** of the lyric as `lyrics::parse_lines` numbered it, and never by
//! re-splitting the text. A line with nothing to romanize (English, a stanza
//! break) has no romanization rather than a copy of itself.
//!
//! ## Words and parts
//!
//! A line comes back as words, and a word as *parts*: one Hangul block, one
//! Japanese or Chinese token, one Indic or Russian word. The reader draws the parts
//! three ways — joined (plain text), with a dot between syllables (Korean,
//! sung syllable by syllable), and under their original (the interlinear
//! "both" view) — so the split is data rather than something a client has to
//! work out again.

mod cyrillic;
mod indic;
#[allow(clippy::all)]
mod indic_tables;
mod kana;
mod korean;

#[cfg(feature = "cjk")]
mod chinese;
#[cfg(feature = "cjk")]
mod japanese;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Bumped whenever any romanizer's output changes, so stored romanizations
/// are recomputed rather than served stale.
pub const VERSION: i64 = 3;

/// One unit drawn under its original in the interlinear view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Part {
    /// The original text.
    pub o: String,
    /// Its romanization. Equal to `o` for punctuation and Latin text.
    pub r: String,
    /// Punctuation or text that was already Latin: carried along, never
    /// separated from its neighbours by a syllable dot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lit: bool,
}

/// A word: parts written together, words separated by a space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Word {
    pub parts: Vec<Part>,
    /// The parts are sung syllables (Korean), so the reader marks the
    /// boundaries between them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub syl: bool,
}

/// One lyric line in Latin letters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RomanLine {
    /// The whole line as plain text.
    pub text: String,
    pub words: Vec<Word>,
}

/// How Indic scripts are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndicStyle {
    /// No diacritics, spelled the way lyric sites spell it: Tamil voicing,
    /// Hindi schwa deletion, `aa`/`ee`/`oo` for long vowels.
    #[default]
    Readable,
    /// ISO 15919, strict and reversible.
    Iso,
}

impl IndicStyle {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("iso") {
            Self::Iso
        } else {
            Self::Readable
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Readable => "readable",
            Self::Iso => "iso",
        }
    }
}

/// How Russian is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CyrillicStyle {
    /// Wikipedia's scheme: no diacritics, the spelling lyric sites use
    /// (Yuliya, kazhetsya, marmeladny).
    #[default]
    Readable,
    /// ISO 9, one Latin letter per Cyrillic one (Ûliâ, kažetsâ).
    Iso,
}

impl CyrillicStyle {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("iso") {
            Self::Iso
        } else {
            Self::Readable
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Readable => "readable",
            Self::Iso => "iso",
        }
    }
}

/// The choices a reader can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub indic: IndicStyle,
    pub cyrillic: CyrillicStyle,
    /// Japanese long vowels with macrons (Tōkyō) rather than spelled out
    /// (toukyou).
    pub macrons: bool,
}

impl Options {
    /// A short stable name, part of the key a romanization is stored under.
    ///
    /// Each choice is in the key only when it is not the default, so a key
    /// written before a choice existed still names the same romanization.
    pub fn key(self) -> String {
        format!(
            "{}{}{}",
            self.indic.as_str(),
            if self.cyrillic == CyrillicStyle::Iso { "+cyr-iso" } else { "" },
            if self.macrons { "+macrons" } else { "" }
        )
    }
}

/// Which romanizer a run of text belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Hangul,
    /// Kana, kanji/hanzi and the CJK marks that travel with them.
    Cjk,
    Indic(indic::Script),
    Cyrillic,
    /// Whitespace, including the ideographic space.
    Space,
    /// Anything else: Latin, digits, punctuation. Carried as it is.
    Other,
}

fn class_of(c: char) -> Class {
    let u = c as u32;
    if c.is_whitespace() || u == 0x3000 {
        return Class::Space;
    }
    if (0xAC00..=0xD7A3).contains(&u) {
        return Class::Hangul;
    }
    if (0x3040..=0x30FF).contains(&u)
        || (0x31F0..=0x31FF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || u == 0x3005
        || u == 0x3006
    {
        return Class::Cjk;
    }
    if let Some(s) = indic::Script::of(c) {
        return Class::Indic(s);
    }
    if (0x0400..=0x04FF).contains(&u) {
        return Class::Cyrillic;
    }
    Class::Other
}

/// Whether a lyric's language is one this module romanizes.
pub fn supports(language: Option<&str>) -> bool {
    matches!(
        language,
        Some(
            "Korean"
                | "Japanese"
                | "Chinese"
                | "Tamil"
                | "Malayalam"
                | "Telugu"
                | "Kannada"
                | "Hindi"
                | "Bengali"
                | "Gujarati"
                | "Punjabi"
                | "Russian"
        )
    )
}

/// The language to romanize a lyric as, or `None` when there is nothing this
/// module should romanize in it.
///
/// `language` is the lyric-level reading, which needs a script to be a fifth
/// of the letters before it names one — right for *what language is this
/// song*, wrong for *is there anything here to romanize*. "APT." is mostly
/// English with forty lines of 아파트; its language is unresolved and every
/// one of those lines still wants Latin letters. So when the lyric's language
/// is not one romanized here, the most frequent script that is wins, however
/// small its share.
///
/// Russian is checked the other way: Cyrillic is named "Russian" by codepoint,
/// and a lyric with a letter only Ukrainian, Serbian or another neighbour uses
/// is not romanized with Russian's rules.
pub fn language_for<S: AsRef<str>>(language: Option<&str>, lines: &[S]) -> Option<String> {
    let chars = || lines.iter().flat_map(|l| l.as_ref().chars());
    let chosen = if supports(language) {
        language.map(str::to_string)
    } else {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for name in chars().filter_map(crate::language::script_of) {
            match counts.iter_mut().find(|(n, _)| *n == name) {
                Some((_, n)) => *n += 1,
                None => counts.push((name, 1)),
            }
        }
        counts.retain(|(n, _)| supports(Some(n)));
        // Kana settles Japanese against Chinese however many kanji there are:
        // Chinese has none.
        let kana = counts.iter().any(|(n, _)| *n == "Japanese");
        counts.retain(|(n, _)| !(kana && *n == "Chinese"));
        counts.into_iter().max_by_key(|(_, n)| *n).map(|(n, _)| n.to_string())
    }?;
    if chosen == "Russian" && chars().any(cyrillic::not_russian) {
        return None;
    }
    Some(chosen)
}

/// Romanize a lyric, line for line.
///
/// `language` is the lyric-level reading (`lyrics_cache.language`). It decides
/// what a run of CJK ideographs is — a Japanese lyric has lines of kanji alone,
/// which per line would look Chinese. Returns one entry per input line: `None`
/// where there was nothing to romanize.
pub fn lyric(lines: &[&str], language: Option<&str>, opts: Options) -> Vec<Option<RomanLine>> {
    let cjk = CjkMode::for_language(language);
    lines.iter().map(|l| line(l, cjk, opts)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CjkMode {
    Japanese,
    Chinese,
}

impl CjkMode {
    fn for_language(language: Option<&str>) -> Self {
        if language == Some("Chinese") {
            Self::Chinese
        } else {
            Self::Japanese
        }
    }
}

/// One line. `None` when nothing in it needed romanizing.
fn line(text: &str, cjk: CjkMode, opts: Options) -> Option<RomanLine> {
    use unicode_normalization::UnicodeNormalization;
    // NFC so a decomposed vowel sign or a Hangul block spelled as jamo is the
    // same text as its composed form. NFKC would be wrong here: it rewrites
    // full-width punctuation the reader should see as written.
    let text: String = text.nfc().collect();
    if !text.chars().any(|c| {
        matches!(class_of(c), Class::Hangul | Class::Cjk | Class::Indic(_) | Class::Cyrillic)
    }) {
        return None;
    }

    // Whitespace separates words in Korean and Indic text and in any Latin
    // run; inside one whitespace-free chunk, runs of different scripts are
    // romanized by their own rules and joined.
    let mut words: Vec<Word> = Vec::new();
    for chunk in text.split(|c: char| class_of(c) == Class::Space).filter(|c| !c.is_empty()) {
        let mut chunk_words: Vec<Word> = Vec::new();
        for (class, run) in runs(chunk) {
            let produced: Vec<Word> = match class {
                Class::Hangul => vec![korean::word(&run)],
                Class::Indic(script) => vec![indic::word(&run, script, opts.indic)],
                Class::Cyrillic => vec![cyrillic::word(&run, opts.cyrillic)],
                Class::Cjk => cjk_words(&run, cjk, opts),
                Class::Other | Class::Space => {
                    // Carried as written. Bare punctuation is glued to what it
                    // touches — "괜찮아," is one word with a comma on the end —
                    // but Latin words are words: "までBoooon" is two.
                    let part = Part { o: run.clone(), r: ascii_punct(&run), lit: true };
                    let wordy = run.chars().any(char::is_alphanumeric);
                    match chunk_words.last_mut() {
                        Some(w) if !wordy => {
                            w.parts.push(part);
                            continue;
                        }
                        _ => vec![Word { parts: vec![part], syl: false }],
                    }
                }
            };
            // Text right after punctuation, with no space between, is the same
            // word: "(사랑" opens on a bracket, कुछ-कुछ is one word with a
            // hyphen in it — not "kuchh- kuchh".
            let mut produced = produced.into_iter();
            if let Some(last) = chunk_words.last_mut() {
                // Only marks that join: a hyphen, an apostrophe, an opening
                // bracket or quote. After a colon or a comma the next word is
                // a new word ("cí: Zhōu Jiélún", not "cí:Zhōu…").
                let after_punct = last.parts.last().is_some_and(|p| {
                    p.lit
                        && p.o.ends_with([
                            '-', '‐', '‑', '–', '\'', '’', '(', '（', '[', '「', '『', '“', '"',
                        ])
                });
                if after_punct {
                    if let Some(first) = produced.next() {
                        last.syl = first.syl;
                        last.parts.extend(first.parts);
                    }
                }
            }
            chunk_words.extend(produced);
        }
        words.extend(chunk_words);
    }

    let text = words
        .iter()
        .map(|w| w.parts.iter().map(|p| p.r.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    Some(RomanLine { text, words })
}

/// CJK and full-width punctuation in the ASCII a romanized line is written
/// in: ？ → ?, 、 → ",", 「」 → quotes, Ａ → A.
fn ascii_punct(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '。' => '.',
            '、' => ',',
            '「' | '」' | '『' | '』' | '“' | '”' => '"',
            '〜' | '～' => '~',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            c => c,
        })
        .collect()
}

/// Split a chunk into maximal runs of one class. Indic combining marks and
/// joiners stay with the script they modify, and so does a stress mark on a
/// Russian vowel (голова́), which NFC has no precomposed letter for.
fn runs(chunk: &str) -> Vec<(Class, String)> {
    let mut out: Vec<(Class, String)> = Vec::new();
    for c in chunk.chars() {
        let mut class = class_of(c);
        // ZWJ / ZWNJ and the prolonged-sound mark belong to the run they sit in.
        if matches!(c, '\u{200C}' | '\u{200D}' | 'ー') {
            if let Some((prev, _)) = out.last() {
                class = *prev;
            }
        }
        if matches!(c, '\u{0300}' | '\u{0301}')
            && out.last().is_some_and(|(prev, _)| *prev == Class::Cyrillic)
        {
            class = Class::Cyrillic;
        }
        match out.last_mut() {
            Some((prev, s)) if *prev == class => s.push(c),
            _ => out.push((class, c.to_string())),
        }
    }
    out
}

#[cfg(feature = "cjk")]
fn cjk_words(run: &str, mode: CjkMode, opts: Options) -> Vec<Word> {
    // Kana settles it whatever the lyric was called: Chinese has none.
    let has_kana = run.chars().any(|c| ('\u{3040}'..='\u{30FF}').contains(&c));
    if mode == CjkMode::Chinese && !has_kana {
        chinese::words(run)
    } else {
        japanese::words(run, opts.macrons)
    }
}

/// Without the dictionaries (`--no-default-features`, the small-memory build),
/// kana is still romanized and ideographs are carried as written — a partial
/// answer that says so rather than a guess.
#[cfg(not(feature = "cjk"))]
fn cjk_words(run: &str, _mode: CjkMode, opts: Options) -> Vec<Word> {
    kana::words_without_dictionary(run, opts.macrons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roman(s: &str, lang: &str) -> Option<String> {
        lyric(&[s], Some(lang), Options::default()).remove(0).map(|l| l.text)
    }

    #[test]
    fn a_line_with_nothing_to_romanize_has_no_romanization() {
        assert_eq!(roman("You and me", "Korean"), None);
        assert_eq!(roman("", "Korean"), None);
    }

    #[test]
    fn latin_and_punctuation_are_carried_and_glued() {
        let l = lyric(&["난 괜찮아, 넌 괜찮을까?"], Some("Korean"), Options::default())
            .remove(0)
            .unwrap();
        assert_eq!(l.text, "nan gwaenchana, neon gwaenchaneulkka?");
        let w = &l.words[1];
        assert!(w.syl);
        assert_eq!(w.parts.last().unwrap(), &Part { o: ",".into(), r: ",".into(), lit: true });
    }

    #[test]
    fn a_colon_or_comma_is_followed_by_a_new_word() {
        assert_eq!(roman("词：周杰伦", "Chinese").as_deref(), Some("cí: Zhōu Jiélún"));
    }

    #[test]
    fn a_hyphen_joins_rather_than_splits() {
        assert_eq!(roman("कुछ-कुछ होता है", "Hindi").as_deref(), Some("kuchh-kuchh hota hai"));
    }

    #[test]
    fn english_inside_a_korean_line_is_left_alone() {
        assert_eq!(roman("이젠 안녕 goodbye", "Korean").as_deref(), Some("ijen annyeong goodbye"));
    }

    #[test]
    fn russian_in_both_styles() {
        assert_eq!(
            roman("Мой мармеладный, я не права", "Russian").as_deref(),
            Some("Moy marmeladny, ya ne prava")
        );
        let iso = Options { cyrillic: CyrillicStyle::Iso, ..Options::default() };
        let l = lyric(&["Опять мне кажется"], Some("Russian"), iso).remove(0).unwrap();
        assert_eq!(l.text, "Opâtʹ mne kažetsâ");
        // A hyphenated word stays one word, as कुछ-कुछ does.
        assert_eq!(roman("муа-муа", "Russian").as_deref(), Some("mua-mua"));
        // A stress mark stays on the original and off the Latin letters.
        let l = lyric(&["голова́"], Some("Russian"), Options::default()).remove(0).unwrap();
        assert_eq!((l.words[0].parts[0].o.as_str(), l.text.as_str()), ("голова́", "golova"));
    }

    /// "APT.": mostly English, so the lyric has no language, and its lines of
    /// Hangul still want romanizing.
    #[test]
    fn a_lyric_with_no_language_is_romanized_by_the_script_it_has() {
        let lines = [
            "아파트, 아파트",
            "Kissy face, kissy face, sent to your phone, but",
            "I'm trying to kiss your lips for real",
            "Don't you want me like I want you, baby?",
        ];
        assert_eq!(language_for(None, &lines).as_deref(), Some("Korean"));
        assert_eq!(language_for(None, &["Only English here"]), None);
        assert_eq!(language_for(Some("English/Other"), &["사랑"]).as_deref(), Some("Korean"));
        // Kana makes it Japanese however many kanji there are.
        assert_eq!(language_for(None, &["東京都庁の前で", "ね"]).as_deref(), Some("Japanese"));
    }

    #[test]
    fn ukrainian_is_not_read_with_russian_rules() {
        assert_eq!(language_for(Some("Russian"), &["Я їду до Києва"]), None);
        assert_eq!(language_for(Some("Russian"), &["Я не права"]).as_deref(), Some("Russian"));
    }

    #[test]
    fn one_entry_per_line_blanks_included() {
        let out = lyric(&["꽃", "", "flower"], Some("Korean"), Options::default());
        assert_eq!(out.len(), 3);
        assert!(out[0].is_some() && out[1].is_none() && out[2].is_none());
    }
}

/// Prints the sample songs, romanized, for a person to read: the lyric file
/// and the two LRC files in the gitignored `temp/romanizer/` (third-party
/// text, so never in the repository). `cargo test print_samples -- --ignored
/// --nocapture`.
#[cfg(test)]
#[test]
#[ignore = "reads local, gitignored sample lyrics and prints them"]
fn print_samples() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/temp/romanizer/");
    let mut songs: Vec<(String, String)> = Vec::new();
    if let Ok(t) = std::fs::read_to_string(format!("{dir}lyricfile.txt")) {
        let mut cur: Option<(String, String)> = None;
        for l in t.lines() {
            let head = l.split(':').next().unwrap_or("");
            if ["Tamil", "Korean", "Japanese", "Hindi", "Russian"].contains(&head) {
                if let Some(c) = cur.take() {
                    songs.push(c);
                }
                cur = Some((head.to_string(), String::new()));
            } else if let Some((_, body)) = cur.as_mut() {
                body.push_str(&l.replace(" — ", "\n"));
                body.push('\n');
            }
        }
        songs.extend(cur);
    }
    if let Ok(t) = std::fs::read_to_string(format!("{dir}ru_marmeladny.txt")) {
        songs.push(("Russian".to_string(), t));
    }
    for (lang, file) in [("Korean", "ko_flower.lrc"), ("Chinese", "zh_qingtian.lrc")] {
        if let Ok(t) = std::fs::read_to_string(format!("{dir}{file}")) {
            songs.push((lang.to_string(), crate::lyrics::strip_cues(&t)));
        }
    }
    for (lang, body) in songs {
        let lines: Vec<&str> = body.lines().collect();
        for opts in [
            Options::default(),
            Options { indic: IndicStyle::Iso, cyrillic: CyrillicStyle::Iso, macrons: true },
        ] {
            println!("==== {lang} {}", opts.key());
            for (l, r) in lines.iter().zip(lyric(&lines, Some(&lang), opts)) {
                match r {
                    Some(r) => println!("{l}\n    {}", r.text),
                    None => println!("{l}"),
                }
            }
        }
    }
}
