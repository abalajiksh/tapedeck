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

/// Detect a language purely from the script of the title/album text. Returns
/// `None` for Latin-script text (which needs MusicBrainz to disambiguate).
pub fn script_language(title: &str, album: Option<&str>) -> Option<&'static str> {
    let mut text = title.to_string();
    if let Some(a) = album {
        text.push(' ');
        text.push_str(a);
    }

    let has = |lo: u32, hi: u32| {
        text.chars().any(|c| {
            let u = c as u32;
            u >= lo && u <= hi
        })
    };

    // Order: most-specific / unambiguous blocks first.
    if has(0x0B80, 0x0BFF) {
        return Some("Tamil");
    }
    if has(0x0D00, 0x0D7F) {
        return Some("Malayalam");
    }
    if has(0x0980, 0x09FF) {
        return Some("Bengali");
    }
    if has(0x0C00, 0x0C7F) {
        return Some("Telugu");
    }
    if has(0x0C80, 0x0CFF) {
        return Some("Kannada");
    }
    if has(0x0A00, 0x0A7F) {
        return Some("Punjabi");
    } // Gurmukhi
    if has(0x0A80, 0x0AFF) {
        return Some("Gujarati");
    }
    if has(0x0900, 0x097F) {
        return Some("Hindi");
    } // Devanagari (Hindi/Marathi/Sanskrit)
    if has(0xAC00, 0xD7A3) {
        return Some("Korean");
    } // Hangul syllables
    if has(0x3040, 0x30FF) {
        return Some("Japanese");
    } // Hiragana + Katakana
    if has(0x0600, 0x06FF) {
        return Some("Arabic");
    }
    if has(0x0400, 0x04FF) {
        return Some("Russian");
    } // Cyrillic
    if has(0x0E00, 0x0E7F) {
        return Some("Thai");
    }
    if has(0x0590, 0x05FF) {
        return Some("Hebrew");
    }
    if has(0x0370, 0x03FF) {
        return Some("Greek");
    }
    // CJK ideographs without kana are ambiguous JP/ZH — call it Chinese.
    if has(0x4E00, 0x9FFF) {
        return Some("Chinese");
    }
    None
}

/// The label used for Latin-script listens MusicBrainz couldn't classify.
pub const DEFAULT_LANGUAGE: &str = "English/Other";

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
