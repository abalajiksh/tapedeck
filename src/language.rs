//! Lightweight language classification for listens.
//!
//! Two tiers live here:
//!   1. `script_language` — instant Unicode-script detection (no network). It
//!      resolves non-Latin scripts (Tamil, Devanagari, Hangul, …) with certainty.
//!   2. `iso_to_display` — maps MusicBrainz ISO 639-3 codes to display names for
//!      the work-language path (see `musicbrainz::fetch_recording_language`).
//!
//! Latin-script titles are left to the MusicBrainz work-language lookup, then a
//! plain "English/Other" default.

/// The scripts this file can name, and the block each one occupies.
///
/// **One table, two readers.** `script_language` asks whether a script appears
/// at all, which is right for a title; `dominant_script` asks which script the
/// text is mostly *in*, which is the only sane question about a thousand
/// characters of lyric. Splitting the ranges out means the two cannot come to
/// disagree about where Hangul starts.
///
/// Ordered most-specific first: kana before the CJK ideographs, because a
/// Japanese lyric contains both and only the kana are decisive.
const SCRIPTS: &[(u32, u32, &str)] = &[
    (0x0B80, 0x0BFF, "Tamil"),
    (0x0D00, 0x0D7F, "Malayalam"),
    (0x0980, 0x09FF, "Bengali"),
    (0x0C00, 0x0C7F, "Telugu"),
    (0x0C80, 0x0CFF, "Kannada"),
    (0x0A00, 0x0A7F, "Punjabi"), // Gurmukhi
    (0x0A80, 0x0AFF, "Gujarati"),
    (0x0900, 0x097F, "Hindi"),    // Devanagari (Hindi/Marathi/Sanskrit)
    (0xAC00, 0xD7A3, "Korean"),   // Hangul syllables
    (0x3040, 0x30FF, "Japanese"), // Hiragana + Katakana
    (0x0600, 0x06FF, "Arabic"),
    (0x0400, 0x04FF, "Russian"), // Cyrillic
    (0x0E00, 0x0E7F, "Thai"),
    (0x0590, 0x05FF, "Hebrew"),
    (0x0370, 0x03FF, "Greek"),
    // CJK ideographs without kana are ambiguous JP/ZH — call it Chinese.
    (0x4E00, 0x9FFF, "Chinese"),
];

/// Which of the named scripts a character belongs to, if any.
fn script_of(c: char) -> Option<&'static str> {
    let u = c as u32;
    SCRIPTS.iter().find(|(lo, hi, _)| u >= *lo && u <= *hi).map(|(_, _, name)| *name)
}

/// Detect a language purely from the script of the title/album text. Returns
/// `None` for Latin-script text (which needs MusicBrainz to disambiguate).
///
/// **An existence test, and that is right for a title.** A title is a handful
/// of words, so a single Hangul character in it really is decisive. It is
/// emphatically *not* right for a lyric — see `dominant_script`, which exists
/// because one loanword in a thousand characters would flip this.
pub fn script_language(title: &str, album: Option<&str>) -> Option<&'static str> {
    let mut text = title.to_string();
    if let Some(a) = album {
        text.push(' ');
        text.push_str(a);
    }
    // Most-specific first, so a Japanese title with kana and ideographs is
    // Japanese rather than Chinese.
    for (lo, hi, name) in SCRIPTS {
        if text.chars().any(|c| {
            let u = c as u32;
            u >= *lo && u <= *hi
        }) {
            return Some(name);
        }
    }
    None
}

/// The share of a text's letters that must be in one script before that script
/// names the language.
///
/// **Not a tuned parameter, and the measurement says so.** A Korean lyric
/// measured against the live API is 88% Hangul; an English lyric quoting one
/// name is under 1%. Two orders of magnitude apart, so anything in the middle
/// separates them and the exact value carries no weight — which is the test
/// this codebase applies to a number before letting it decide anything.
const SCRIPT_SHARE: f64 = 0.20;

/// The script a *body of text* is written in, by weight rather than presence.
///
/// Latin is deliberately not a possible answer: it means "some European
/// language", which is not a language, and telling French from English needs a
/// statistical detector rather than a codepoint range. `None` therefore means
/// *unresolved*, never *English*.
pub fn dominant_script(text: &str) -> Option<&'static str> {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    let mut letters = 0usize;
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if let Some(name) = script_of(c) {
            match counts.iter_mut().find(|(n, _)| *n == name) {
                Some((_, n)) => *n += 1,
                None => counts.push((name, 1)),
            }
        }
    }
    if letters == 0 {
        return None;
    }
    let (name, n) = counts.into_iter().max_by_key(|(_, n)| *n)?;
    (n as f64 / letters as f64 >= SCRIPT_SHARE).then_some(name)
}

/// The label used for Latin-script listens MusicBrainz couldn't classify.
pub const DEFAULT_LANGUAGE: &str = "English/Other";

/// A track with no words, which is not the same as a track whose words nobody
/// could read.
///
/// The vocabulary had no way to say this: every listen was either classified or
/// shrugged at, so an instrumental landed in `English/Other` beside songs that
/// merely defeated the detector. LRCLIB answers it directly — `instrumental:
/// true` — and MusicBrainz already had it as ISO `zxx`, which `iso_to_display`
/// has been mapping to this word all along.
pub const INSTRUMENTAL: &str = "Instrumental";

/// Map a MusicBrainz ISO 639-3 language code to a display name.
/// "zxx" (no linguistic content) → Instrumental. Unknown → capitalised code.
pub fn iso_to_display(iso: &str) -> String {
    let name = match iso.to_lowercase().as_str() {
        "eng" => "English",
        "fra" | "fre" => "French",
        "spa" => "Spanish",
        "deu" | "ger" => "German",
        "ita" => "Italian",
        "por" => "Portuguese",
        "nld" | "dut" => "Dutch",
        "swe" => "Swedish",
        "nor" | "nob" => "Norwegian",
        "dan" => "Danish",
        "fin" => "Finnish",
        "pol" => "Polish",
        "ces" | "cze" => "Czech",
        "tam" => "Tamil",
        "mal" => "Malayalam",
        "hin" => "Hindi",
        "tel" => "Telugu",
        "kan" => "Kannada",
        "ben" => "Bengali",
        "pan" => "Punjabi",
        "mar" => "Marathi",
        "guj" => "Gujarati",
        "urd" => "Urdu",
        "san" => "Sanskrit",
        "ori" => "Odia",
        "kor" => "Korean",
        "jpn" => "Japanese",
        "zho" | "chi" | "cmn" => "Chinese",
        "yue" => "Cantonese",
        "tha" => "Thai",
        "vie" => "Vietnamese",
        "ind" => "Indonesian",
        "ara" => "Arabic",
        "tur" => "Turkish",
        "heb" => "Hebrew",
        "ell" | "gre" => "Greek",
        "rus" => "Russian",
        "ukr" => "Ukrainian",
        "zxx" => "Instrumental",
        other => {
            return {
                let mut c = other.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => "Unknown".into(),
                }
            }
        }
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_non_latin_scripts() {
        assert_eq!(script_language("நட்பே", None), Some("Tamil"));
        assert_eq!(script_language("うたかたの歌", None), Some("Japanese"));
        assert_eq!(script_language("사랑", None), Some("Korean"));
        assert_eq!(script_language("Привет", None), Some("Russian"));
        assert_eq!(script_language("नमस्ते", None), Some("Hindi"));
    }

    #[test]
    fn latin_is_left_for_musicbrainz() {
        assert_eq!(script_language("Teardrop", Some("Mezzanine")), None);
        assert_eq!(script_language("La Vie en Rose", None), None);
    }

    #[test]
    fn iso_mapping() {
        assert_eq!(iso_to_display("eng"), "English");
        assert_eq!(iso_to_display("tam"), "Tamil");
        assert_eq!(iso_to_display("zxx"), "Instrumental");
        assert_eq!(iso_to_display("xyz"), "Xyz"); // unknown → capitalised
    }
}
