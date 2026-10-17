//! Shoutcast/Icecast in-stream metadata ("ICY"), and what a line of it means.
//!
//! Asked for with `Icy-MetaData: 1`, a server that supports it answers with
//! `icy-metaint: N` and then interleaves the audio: N bytes of audio, one
//! length byte L, L×16 bytes of metadata (`StreamTitle='…';`, NUL-padded),
//! N bytes of audio, and so on. The audio is thrown away here — Tapedeck only
//! wants the names.
//!
//! What a German public station puts in `StreamTitle`, measured on ARD
//! streams on 2026-10-17: `Teddy Swims - Mr. Know it all` during music, and
//! anything at all otherwise — `Infos und Playlist auch im Netz:
//! 1LIVEDIGGI.de`, `Anruf im Verkehrszentrum: 0800 637 637 8`. So a line is
//! classified before it is believed, and even a line shaped like a track is
//! only a guess until something confirms it (see `radio::settle`).

/// Splits an ICY stream into metadata blocks. Feed it whatever the socket
/// hands over; it carries partial blocks across calls.
#[derive(Debug)]
pub struct IcyDemux {
    metaint: usize,
    state: State,
}

#[derive(Debug)]
enum State {
    /// Audio bytes still to skip before the next length byte.
    Audio(usize),
    /// Metadata bytes still to read, and what has been read so far.
    Meta(usize, Vec<u8>),
}

impl IcyDemux {
    pub fn new(metaint: usize) -> Self {
        IcyDemux { metaint, state: State::Audio(metaint) }
    }

    /// Every complete metadata block in `data`, decoded. Empty blocks — the
    /// usual case, a zero length byte meaning "unchanged" — are not returned.
    pub fn feed(&mut self, mut data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        while !data.is_empty() {
            match &mut self.state {
                State::Audio(0) => {
                    let len = data[0] as usize * 16;
                    data = &data[1..];
                    self.state = if len == 0 {
                        State::Audio(self.metaint)
                    } else {
                        State::Meta(len, Vec::with_capacity(len))
                    };
                }
                State::Audio(n) => {
                    let take = (*n).min(data.len());
                    *n -= take;
                    data = &data[take..];
                }
                State::Meta(n, buf) => {
                    let take = (*n).min(data.len());
                    buf.extend_from_slice(&data[..take]);
                    *n -= take;
                    data = &data[take..];
                    if *n == 0 {
                        out.push(decode(buf));
                        self.state = State::Audio(self.metaint);
                    }
                }
            }
        }
        out
    }
}

/// UTF-8 when it is valid UTF-8, otherwise Latin-1 — older Shoutcast servers
/// send the latter, and every Latin-1 byte maps to the code point of the same
/// number.
fn decode(bytes: &[u8]) -> String {
    let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    let bytes = &bytes[..end];
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|b| *b as char).collect(),
    }
}

/// `StreamTitle='…';` out of a metadata block. The title may itself contain
/// apostrophes (`Ain't`), so it ends at the `';` that closes the field, not
/// at the first quote.
pub fn stream_title(block: &str) -> Option<String> {
    let start = block.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &block[start..];
    let end = rest.find("';").unwrap_or_else(|| rest.trim_end_matches('\'').len());
    Some(rest[..end].trim().to_string())
}

/// What one line of station text is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Shaped like `Artist - Title`. Still unconfirmed.
    Track { artist: String, title: String },
    /// News, traffic, weather, ads.
    Break,
    /// Anything else: a slogan, a web address, a phone number.
    Other,
}

/// Words that mark a station's own non-music items in German and English
/// public radio. A line naming one of these is a break even when it happens
/// to contain a dash ("Nachrichten - 18 Uhr").
const BREAK_WORDS: &[&str] = &[
    "nachrichten",
    "news",
    "verkehr",
    "traffic",
    "wetter",
    "weather",
    "werbung",
    "sport am",
    "börse",
    "stau",
    "blitzer",
];

/// Separators stations use between artist and title. The spaced forms only:
/// a bare hyphen is part of too many names (`Jay-Z`, `Sigur-Rós`).
const SEPARATORS: &[&str] = &[" - ", " – ", " — ", " / "];

fn looks_like_address(s: &str) -> bool {
    let l = s.to_lowercase();
    if l.contains("http") || l.contains("www.") || l.contains('@') {
        return true;
    }
    // A token ending in a dot and two-to-four letters: `1LIVEDIGGI.de`.
    l.split_whitespace().any(|tok| {
        let tok = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '.');
        match tok.rsplit_once('.') {
            Some((name, tld)) => {
                !name.is_empty()
                    && (2..=4).contains(&tld.len())
                    && tld.chars().all(|c| c.is_ascii_alphabetic())
            }
            None => false,
        }
    })
}

/// Classify a `StreamTitle`. `station_names` are the station's own names,
/// which a line made only of them is not a track by ("MDR KULTUR - Das Radio").
pub fn classify(raw: &str, station_names: &[&str]) -> Line {
    let line = raw.trim();
    let lower = line.to_lowercase();
    if line.is_empty() {
        return Line::Other;
    }
    // German compounds the words: "Verkehrsmeldungen", "Wetterbericht",
    // "Nachrichtenüberblick". So a long word matches as a prefix; a short one
    // ("news", "stau") only whole, or "Staubsauger" would be a traffic jam.
    let is_break = BREAK_WORDS.iter().any(|w| {
        if w.contains(' ') {
            return lower.contains(w);
        }
        lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|t| t == *w || (w.chars().count() >= 5 && t.starts_with(w)))
    });
    if is_break {
        return Line::Break;
    }
    if looks_like_address(line) {
        return Line::Other;
    }
    // Long runs of digits are phone numbers and hotlines, never song titles
    // in practice ("0800 637 637 8").
    if line.chars().filter(|c| c.is_ascii_digit()).count() >= 7 {
        return Line::Other;
    }
    let Some((artist, title)) = SEPARATORS.iter().find_map(|sep| line.split_once(sep)) else {
        return Line::Other;
    };
    let (artist, title) = (artist.trim(), title.trim());
    if artist.is_empty() || title.is_empty() {
        return Line::Other;
    }
    let is_station =
        |s: &str| station_names.iter().any(|n| !n.is_empty() && s.eq_ignore_ascii_case(n.trim()));
    if is_station(artist) || is_station(title) || is_station(line) {
        return Line::Other;
    }
    Line::Track { artist: artist.to_string(), title: title.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> Vec<u8> {
        raw_block(text.as_bytes())
    }

    fn raw_block(bytes: &[u8]) -> Vec<u8> {
        let mut meta = bytes.to_vec();
        let len = meta.len().div_ceil(16);
        meta.resize(len * 16, 0);
        let mut out = vec![len as u8];
        out.extend(meta);
        out
    }

    #[test]
    fn metadata_is_found_between_audio_whatever_the_chunking() {
        let metaint = 8;
        let mut stream = Vec::new();
        stream.extend([1u8; 8]);
        stream.extend(block("StreamTitle='Teddy Swims - Mr. Know it all';"));
        stream.extend([2u8; 8]);
        stream.push(0); // "unchanged"
        stream.extend([3u8; 8]);
        stream.extend(block("StreamTitle='ADÉLA - Ain't It';"));
        stream.extend([4u8; 3]);

        for chunk in [1, 2, 3, 7, 64] {
            let mut d = IcyDemux::new(metaint);
            let titles: Vec<_> = stream
                .chunks(chunk)
                .flat_map(|c| d.feed(c))
                .filter_map(|b| stream_title(&b))
                .collect();
            assert_eq!(
                titles,
                vec!["Teddy Swims - Mr. Know it all", "ADÉLA - Ain't It"],
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn latin1_metadata_is_read_as_latin1() {
        let mut d = IcyDemux::new(1);
        let mut s = vec![0u8];
        s.extend(raw_block(b"StreamTitle='Bj\xf6rk - J\xf3ga';"));
        let titles: Vec<_> = d.feed(&s).iter().filter_map(|b| stream_title(b)).collect();
        assert_eq!(titles, vec!["Björk - Jóga"]);
    }

    #[test]
    fn real_ard_lines_classify_the_way_a_listener_would() {
        let names = ["1LIVE diggi", "WDR 2"];
        assert_eq!(
            classify("Teddy Swims - Mr. Know it all", &names),
            Line::Track { artist: "Teddy Swims".into(), title: "Mr. Know it all".into() }
        );
        assert_eq!(
            classify("ADÉLA - Ain't It", &names),
            Line::Track { artist: "ADÉLA".into(), title: "Ain't It".into() }
        );
        assert_eq!(classify("Infos und Playlist auch im Netz: 1LIVEDIGGI.de", &names), Line::Other);
        assert_eq!(classify("Anruf im Verkehrszentrum: 0800 637 637 8", &names), Line::Break);
        assert_eq!(classify("Nachrichten - 18 Uhr", &names), Line::Break);
        assert_eq!(classify("WDR 2 - Der Sender", &names), Line::Other);
        assert_eq!(classify("", &names), Line::Other);
        assert_eq!(
            classify("Jay-Z", &names),
            Line::Other,
            "a hyphen inside a name is not a separator"
        );
        assert_eq!(
            classify("Sigur Rós – Hoppípolla", &names),
            Line::Track { artist: "Sigur Rós".into(), title: "Hoppípolla".into() }
        );
        assert_eq!(classify("Hotline 0221 567 89 123 - jetzt anrufen", &names), Line::Other);
    }
}
