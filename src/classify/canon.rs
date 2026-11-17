//! The classical canon: who counts as a classical composer before MusicBrainz
//! is asked anything.
//!
//! Keyed by MBID because MusicBrainz's own names cannot be matched by an
//! English list (Пётр Ильич Чайковский, "Fryderyk Chopin", "Bartók Béla", a
//! U+2010 hyphen in Saint‐Saëns). The aliases are for listens that carry names
//! only — Spotify, Pano Scrobbler, Starling — and match a *whole* credited name
//! after [`name_key`], never a substring.

use std::collections::HashMap;
use std::sync::OnceLock;

const DATA: &str = include_str!("composers.tsv");

#[derive(Debug, Clone, PartialEq)]
pub struct Composer {
    pub mbid: &'static str,
    pub name: &'static str,
    pub born: i32,
    /// `YYYY[-MM[-DD]]`, as MusicBrainz records them.
    pub born_date: &'static str,
    pub died_date: &'static str,
    pub gender: &'static str,
    /// ISO 3166-1 alpha-2.
    pub country: &'static str,
}

struct Canon {
    composers: Vec<Composer>,
    by_mbid: HashMap<&'static str, usize>,
    by_alias: HashMap<String, usize>,
}

fn canon() -> &'static Canon {
    static CANON: OnceLock<Canon> = OnceLock::new();
    CANON.get_or_init(|| {
        let mut composers = Vec::new();
        let mut by_mbid = HashMap::new();
        let mut by_alias = HashMap::new();
        for line in DATA.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let cols: Vec<&'static str> = line.split('\t').collect();
            let [mbid, name, born, aliases, born_date, died_date, gender, country] = cols[..]
            else {
                continue;
            };
            let i = composers.len();
            composers.push(Composer {
                mbid,
                name,
                born: born.parse().unwrap_or(0),
                born_date,
                died_date,
                gender,
                country,
            });
            by_mbid.insert(mbid, i);
            by_alias.insert(name_key(name), i);
            for a in aliases.split('|').filter(|a| !a.is_empty()) {
                by_alias.insert(name_key(a), i);
            }
        }
        Canon { composers, by_mbid, by_alias }
    })
}

/// A spelling-insensitive key for a person's name: letters and digits only,
/// lowercased, so "J.S. Bach", "J. S. Bach" and "JS Bach" agree, and
/// "Saint‐Saëns" (U+2010) agrees with "Saint-Saëns". Accents are kept — the
/// aliases list the unaccented spellings sources send.
pub fn name_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn by_mbid(mbid: &str) -> Option<&'static Composer> {
    let c = canon();
    c.by_mbid.get(mbid.trim()).map(|&i| &c.composers[i])
}

/// A whole credited name, as a source sent it.
pub fn by_name(name: &str) -> Option<&'static Composer> {
    let key = name_key(name);
    if key.is_empty() {
        return None;
    }
    let c = canon();
    c.by_alias.get(&key).map(|&i| &c.composers[i])
}

#[cfg(test)]
pub fn all() -> &'static [Composer] {
    &canon().composers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_composer_from_the_list_is_loaded() {
        assert_eq!(all().len(), 66);
        assert!(all().iter().all(|c| c.mbid.len() == 36 && c.born > 1500));
        assert!(all().iter().all(|c| c.born_date.starts_with(&c.born.to_string())));
        assert!(all().iter().all(|c| !c.died_date.is_empty() && c.country.len() == 2));
    }

    #[test]
    fn names_sources_send_resolve_to_one_composer() {
        let tchaikovsky = "9ddd7abc-9e1b-471d-8031-583bc6bc8be9";
        for n in [
            "Pyotr Ilyich Tchaikovsky",
            "Tchaikovsky",
            "Пётр Ильич Чайковский",
            "P. I. Tchaikovsky",
        ] {
            assert_eq!(by_name(n).map(|c| c.mbid), Some(tchaikovsky), "{n}");
        }
        assert_eq!(by_name("J.S. Bach").unwrap().name, "Johann Sebastian Bach");
        assert_eq!(by_name("J. S. Bach").unwrap().name, "Johann Sebastian Bach");
        assert_eq!(by_name("Camille Saint‐Saëns").unwrap().name, "Camille Saint-Saëns");
        assert_eq!(by_name("Saint-Saens").unwrap().name, "Camille Saint-Saëns");
    }

    #[test]
    fn a_namesake_is_not_the_composer() {
        assert!(by_name("Sebastian Bach").is_none(), "Skid Row's singer");
        assert!(by_name("Johann Strauss II").is_none(), "not on the list");
        assert!(by_name("Strauss").is_none(), "ambiguous, so not an alias");
        assert!(by_name("").is_none());
    }
}
