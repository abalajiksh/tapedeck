//! What a title alone can say: the catalogue number of a classical piece, where
//! the piece ends and the movement begins, and the words that mark score or a
//! season. Hand-written token scans rather than a regex dependency; every rule
//! here matches whole words, because "Santa Monica" is not Christmas.

/// Lowercased words: letters and digits, everything else a separator.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// The words joined by single spaces and padded, so a phrase test is a
/// substring test on word boundaries: `" silent night "`.
fn padded(s: &str) -> String {
    format!(" {} ", words(s).join(" "))
}

fn has_phrase(hay: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| hay.contains(&format!(" {p} ")))
}

/// Catalogue systems, as written before the number. The single letters (K, D,
/// S, B, L, M, Z) only count when followed by a number, which every system
/// requires anyway.
const CATALOGUES: &[&str] = &[
    "bwv", "k", "kv", "op", "opus", "hob", "rv", "hwv", "d", "s", "twv", "wq", "woo", "bb", "sz",
    "l", "kk", "wab", "trv", "jw", "fp", "b", "m", "z", "swv", "buxwv", "lwv",
];

/// The catalogue number in a title, as `("BWV", "1004")`. `Op. 64`, `K. 504`,
/// `BWV 1004`, `D 960`, `Hob. XVI:52` (roman numerals count as the number).
pub fn catalogue(title: &str) -> Option<(String, String)> {
    let w = words(title);
    for pair in w.windows(2) {
        let (sys, num) = (&pair[0], &pair[1]);
        if !CATALOGUES.contains(&sys.as_str()) {
            continue;
        }
        let numeric = num.chars().next().is_some_and(|c| c.is_ascii_digit());
        let roman = sys == "hob" && num.chars().all(|c| "ivxl".contains(c));
        // A lone letter needs a digit after it — "Vol. D" is not Schubert.
        if numeric || roman {
            let sys = match sys.as_str() {
                "op" | "opus" => "Op".to_string(),
                "kv" => "K".to_string(),
                s => s.to_uppercase(),
            };
            return Some((sys, num.clone()));
        }
    }
    None
}

/// Is the text after a separator a movement? "V. Ciaccona", "I. Adagio",
/// "No. 3", "3. Presto", "Allegro molto", "Act 2".
fn looks_like_movement(rest: &str) -> bool {
    let rest = rest.trim_start();
    let first: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    let bare = first.trim_end_matches(['.', ')', ':']);
    let roman = !bare.is_empty() && bare.chars().all(|c| "IVXLivxl".contains(c));
    let numeric = !bare.is_empty() && bare.chars().all(|c| c.is_ascii_digit());
    let marked = first.len() > bare.len();
    if (roman || numeric) && marked {
        return true;
    }
    let lower = padded(rest);
    has_phrase(
        &lower,
        &[
            "no",
            "act",
            "scene",
            "allegro",
            "adagio",
            "andante",
            "presto",
            "largo",
            "lento",
            "moderato",
            "allegretto",
            "vivace",
            "menuetto",
            "minuet",
            "scherzo",
            "rondo",
            "finale",
            "prelude",
            "prélude",
            "fugue",
            "aria",
            "recitative",
            "chorus",
            "variation",
            "variations",
            "theme",
            "gigue",
            "sarabande",
            "courante",
            "allemande",
            "ciaccona",
            "chaconne",
            "gavotte",
            "bourrée",
            "intermezzo",
        ],
    )
}

/// Split a classical track title into the piece and the movement.
///
/// "Violin Partita No. 2 in D minor, BWV 1004: V. Ciaccona" →
/// ("Violin Partita No. 2 in D minor, BWV 1004", Some("V. Ciaccona")).
/// The left side must look like a piece (a catalogue number, or a movement on
/// the right) — a colon in "Bach: Toccata" is a composer prefix, handled by
/// [`strip_composer_prefix`], not a movement.
pub fn split_piece(title: &str) -> (String, Option<String>) {
    for sep in [": ", " - ", " – ", " — "] {
        if let Some(i) = title.find(sep) {
            let (left, right) = (&title[..i], &title[i + sep.len()..]);
            if left.trim().is_empty() || right.trim().is_empty() {
                continue;
            }
            if catalogue(left).is_some() || looks_like_movement(right) {
                return (left.trim().to_string(), Some(right.trim().to_string()));
            }
        }
    }
    (title.trim().to_string(), None)
}

/// "Johann Sebastian Bach: Toccata and Fugue…" → the composer and the rest.
pub fn strip_composer_prefix(title: &str) -> Option<(&'static super::canon::Composer, String)> {
    let i = title.find(':')?;
    let composer = super::canon::by_name(title[..i].trim())?;
    let rest = title[i + 1..].trim();
    (!rest.is_empty()).then(|| (composer, rest.to_string()))
}

/// A key that groups spellings of one piece: words only, and the catalogue
/// number, when there is one, stands for the whole title — "Partita No. 2 in
/// D minor, BWV 1004" and "Violin Partita No.2, BWV 1004" are one piece.
pub fn piece_key(piece: &str) -> String {
    match catalogue(piece) {
        Some((sys, num)) => format!("{}{}", sys.to_lowercase(), num),
        None => words(piece).join(" "),
    }
}

/// Words that put an instrumental cue on a soundtrack — never enough alone,
/// only on a release that *is* a soundtrack.
pub fn has_score_track_words(title: &str) -> bool {
    has_phrase(
        &padded(title),
        &[
            "theme",
            "main title",
            "main titles",
            "end title",
            "end titles",
            "end credits",
            "bgm",
            "instrumental",
            "karaoke",
            "score",
            "suite",
            "underscore",
            "cue",
        ],
    )
}

/// Words that make an album a score rather than a soundtrack of songs.
pub fn has_score_album_words(album: &str) -> bool {
    has_phrase(
        &padded(album),
        &["score", "background score", "bgm", "instrumental", "instrumentals", "original music"],
    )
}

/// Words that say an album is a soundtrack at all, for listens with no
/// release MBID to ask MusicBrainz about.
pub fn has_soundtrack_album_words(album: &str) -> bool {
    has_phrase(
        &padded(album),
        &["soundtrack", "ost", "motion picture", "original score", "background score", "bgm"],
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    Christmas,
    Halloween,
    Easter,
}

/// Season words in a title or an album name. Whole words, so "Santa Monica"
/// and "Holiday" (Madonna) are neither.
pub fn season_words(text: &str) -> Option<Season> {
    let p = padded(text);
    let christmas = has_phrase(
        &p,
        &[
            "christmas",
            "xmas",
            "x mas",
            "navidad",
            "noël",
            "noel",
            "jingle bell",
            "jingle bells",
            "sleigh ride",
            "mistletoe",
            "rudolph",
            "santa claus",
            "silent night",
            "stille nacht",
            "o holy night",
            "deck the halls",
            "feliz navidad",
            "joy to the world",
            "little drummer boy",
            "frosty the snowman",
            "hark the herald",
            "o tannenbaum",
            "let it snow",
            "white christmas",
            "weihnachten",
            "weihnachtsoratorium",
        ],
    ) || words(text).iter().any(|w| w.starts_with("weihnacht"));
    if christmas {
        return Some(Season::Christmas);
    }
    if has_phrase(&p, &["halloween", "trick or treat"]) {
        return Some(Season::Halloween);
    }
    if has_phrase(&p, &["easter", "ostern", "pâques"]) {
        return Some(Season::Easter);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_numbers_in_the_shapes_sources_send() {
        let c = |t| catalogue(t);
        assert_eq!(
            c("Violin Partita No. 2 in D Minor, BWV 1004: V. Ciaccona"),
            Some(("BWV".into(), "1004".into()))
        );
        assert_eq!(
            c("Symphony no. 38 in D major, K. 504 \"Prague\": I. Adagio - Allegro"),
            Some(("K".into(), "504".into()))
        );
        assert_eq!(c("Etude Op. 25 No. 11 (Winter Wind)"), Some(("Op".into(), "25".into())));
        assert_eq!(c("Piano Sonata in B-flat, D. 960"), Some(("D".into(), "960".into())));
        assert_eq!(c("Sonata Hob. XVI:52"), Some(("HOB".into(), "xvi".into())));
        assert_eq!(c("Vol. D"), None);
        assert_eq!(c("New York Nagaram"), None);
    }

    #[test]
    fn movements_split_off_the_piece() {
        assert_eq!(
            split_piece("Violin Partita No. 2 in D Minor, BWV 1004: V. Ciaccona"),
            ("Violin Partita No. 2 in D Minor, BWV 1004".into(), Some("V. Ciaccona".into()))
        );
        assert_eq!(
            split_piece("Symphony No. 5 in C Minor, Op. 67 - I. Allegro con brio").0,
            "Symphony No. 5 in C Minor, Op. 67"
        );
        // A song with a dash is not a piece with a movement.
        assert_eq!(split_piece("Love Me Do - Remastered").1, None);
        assert_eq!(split_piece("Swan Lake").0, "Swan Lake");
    }

    #[test]
    fn a_catalogue_number_keys_the_piece_whatever_the_spelling() {
        assert_eq!(
            piece_key("Violin Partita No. 2 in D minor, BWV 1004"),
            piece_key("Partita for Violin No.2, BWV 1004")
        );
    }

    #[test]
    fn a_composer_prefix_is_not_a_movement() {
        let (c, rest) =
            strip_composer_prefix("Johann Sebastian Bach: Toccata and Fugue in d-minor").unwrap();
        assert_eq!(c.name, "Johann Sebastian Bach");
        assert_eq!(rest, "Toccata and Fugue in d-minor");
        assert!(strip_composer_prefix("Interlude: Home").is_none());
    }

    #[test]
    fn season_words_are_whole_words() {
        assert_eq!(season_words("Last Christmas"), Some(Season::Christmas));
        assert_eq!(season_words("Weihnachtsoratorium, BWV 248"), Some(Season::Christmas));
        assert_eq!(season_words("A Jolly Christmas from Frank Sinatra"), Some(Season::Christmas));
        assert_eq!(season_words("Santa Monica"), None);
        assert_eq!(season_words("Holiday"), None, "Madonna");
        assert_eq!(season_words("This Is Halloween"), Some(Season::Halloween));
        assert_eq!(season_words("Easter Parade"), Some(Season::Easter));
    }

    #[test]
    fn soundtrack_words() {
        assert!(has_score_album_words("Vikram (Original Background Score)"));
        assert!(has_score_album_words("Interstellar (Original Motion Picture Score)"));
        assert!(!has_score_album_words("The Breakfast Club (Original Motion Picture Soundtrack)"));
        assert!(has_soundtrack_album_words(
            "The Breakfast Club (Original Motion Picture Soundtrack)"
        ));
        assert!(!has_score_track_words("Don't You (Forget About Me)"));
        assert!(has_score_track_words("Main Title"));
    }
}
