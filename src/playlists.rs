//! Pushing a list of tracks to a media server as a playlist.
//!
//! Tapedeck generates lists — Rediscovery does today, the genre-map page will —
//! and none of them are playable here: this app has no playback and is not going
//! to grow any. What it *can* do is hand the list to the thing that does play
//! music, which for a self-hosted setup is the same Plex / Jellyfin / Subsonic
//! server the listens came from in the first place.
//!
//! Three things decide the shape of this module.
//!
//! **The target is whatever the user already connected.** There is no new
//! credential, no new settings page: a `user_sources` row that can poll can also
//! be written to, so the button's options are exactly the servers they set up.
//! Subsonic is spelled through the Navidrome source because that source *is* a
//! Subsonic client — anything else speaking OpenSubsonic works through the same
//! two calls.
//!
//! **A match is checked, never trusted.** Tapedeck stores names, not the
//! server's item ids — a Rediscovery row has a title and an artist and nothing
//! else — so every track has to be found by searching, and every search engine
//! here is fuzzy. Plex's `/hubs/search` answers "Arkells" when asked for "Cœur",
//! which is the trap `find_artist` already documents. So the search result is a
//! *suggestion* and [`pick`] decides, on the artist and the title both.
//!
//! **A partial push is reported, not hidden.** A history spans more than one
//! library, so some tracks legitimately are not on the server being pushed to.
//! The response names every track that did not land. Silently sending nine of
//! twelve and calling it done is the failure this codebase keeps finding in
//! other forms — a number that is just wrong, with nothing erroring.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// One track Tapedeck wants on the far end.
///
/// Names only, because names are all the generating queries have. `album` is a
/// tiebreaker rather than a requirement: a listen's album is whichever release
/// the tagger claimed, which for a compilation track is not the album the
/// server files it under.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    /// The recording's ISRC, where the history has one.
    ///
    /// The only identifier that crosses from a listening history into a
    /// streaming catalogue, so a target that can look one up needs no name
    /// matching at all — which is the whole game, because every miss below is a
    /// name that did not agree.
    #[serde(default)]
    pub isrc: Option<String>,
}

/// A track the far end offered in answer to a search.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The server's own id for it — a Plex `ratingKey`, a Jellyfin item id, a
    /// Subsonic song id. Opaque here; it only ever goes back to the server that
    /// produced it.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

/// Where a playlist can be sent.
///
/// Two calls, because that is genuinely all a push needs. Deliberately *not*
/// modelled on `MusicSource`: reading sessions and writing a playlist share a
/// server and a credential and nothing else, and folding them into one trait
/// would mean every future source had to answer a question about playlists to
/// be allowed to poll.
///
/// Deliberately does **not** carry a `protocol()` name. `server::playlists`
/// already maps a source kind to one, and that mapping is also what decides
/// whether a kind can be a target at all — a second copy on the trait would be
/// free to disagree with it, which is the drift this codebase keeps finding in
/// other forms.
#[async_trait]
pub trait PlaylistTarget: Send + Sync {
    /// Ask the server for tracks that might be this one. Fuzzy by nature; the
    /// caller decides which (if any) is really it.
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String>;

    /// Create the playlist and return the server's id for it.
    async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<String, String>;
}

/// Strip trailing bracketed qualifiers: `Xtal (Remastered 2008)` → `Xtal`.
///
/// Only the *trailing* run, and only when something is left over — `(Reprise)`
/// as an entire title stays as it is, and `Untitled (Alva Noto)` keeps its
/// stem. Leading brackets are left alone because a title that opens with one
/// usually means it.
pub fn without_qualifier(title: &str) -> &str {
    let mut s = title.trim();
    loop {
        let Some(open) = s.rfind(['(', '[']) else { return s };
        // Must run to the end, or it's part of the title rather than a suffix.
        if !s[open..].trim_end().ends_with([')', ']']) {
            return s;
        }
        let stem = s[..open].trim_end();
        if stem.is_empty() {
            return s;
        }
        s = stem;
    }
}

/// How well a candidate answers the request. Higher is better; `None` means it
/// is not the same track at all.
///
/// The artist is a hard gate. Everything else is a ranking, because a library
/// legitimately holds "Xtal", "Xtal (Remastered)" and "Xtal (Live at …)" and the
/// bare one is what was asked for.
fn score(want: &PlaylistTrack, c: &Candidate) -> Option<u32> {
    // `db::artist_matches` rather than a comparison written here — the codebase
    // has exactly one answer to "same performer?" and adding a second is how the
    // sanitiser and the portrait matcher came to disagree about ligatures.
    // The gate, in two tiers. An outright match is what it always was; the
    // second exists because a *credit* is not a different performer.
    //
    // `artist_matches` compares whole folded strings, so a history holding
    // "Low Roar feat. Jófríður Ákadóttir" against a library holding "Low Roar"
    // fails it — and the track is sitting right there. `split_featured` is
    // already the codebase's answer to that shape (the sanitiser uses it), and
    // it is deliberately narrow: only the explicit `feat.`/`ft.`/`featuring`/
    // `with` forms, never `&` or `,`, because "Simon & Garfunkel" is a name.
    //
    // Ranked below an exact match rather than merged with it, so a library that
    // holds both the solo and the collaboration still returns the one asked
    // for. Losing points here is what makes that ordering hold.
    let artist_exact = crate::db::artist_matches(&c.artist, &want.artist);
    if !artist_exact {
        let primary = |a: &str| crate::db::split_featured(a).unwrap_or_else(|| a.to_string());
        if !crate::db::artist_matches(&primary(&c.artist), &primary(&want.artist)) {
            return None;
        }
    }

    // The same normaliser does duty for the title. This is not a second notion
    // of artist identity — it is the same question ("are these two spellings of
    // one label?") asked about a different field, which is what it was written
    // for.
    let norm = crate::db::normalise_artist;
    let (want_full, cand_full) = (norm(&want.title), norm(&c.title));
    if want_full.is_empty() {
        return None;
    }

    let mut points = if want_full == cand_full {
        100
    } else if norm(without_qualifier(&want.title)) == norm(without_qualifier(&c.title)) {
        // Same song, one side carrying "(Remastered)" or "(Live)". Worth taking
        // when nothing exact turned up, and worth losing to an exact hit.
        60
    } else {
        return None;
    };

    // A credit-folded artist is a weaker answer than one that agreed outright,
    // and has to lose to it.
    if !artist_exact {
        points -= 20;
    }

    // Album agreement breaks the tie between three pressings of one track. Never
    // a requirement — see the note on `PlaylistTrack::album`.
    if let (Some(a), Some(b)) = (want.album.as_deref(), c.album.as_deref()) {
        if !a.trim().is_empty() && norm(a) == norm(b) {
            points += 10;
        }
    }
    Some(points)
}

/// Why a track did not land.
///
/// The push used to report only *which* tracks missed, and those are two
/// completely different faults wearing the same face: a server that returned
/// nothing for the query has a search problem, and a server that returned the
/// right song and had it refused has a *matching* problem. Reported as
/// "12 didn't land" they are indistinguishable, and there is no way to tell
/// from outside which one to go and fix.
///
/// `ArtistMismatch` carries what the server actually offered, because that is
/// the fact that settles it in one glance: asked for "Bones" by
/// "Low Roar feat. Jófríður Ákadóttir" and offered "Low Roar" says exactly
/// what is wrong, where a bare miss says nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Miss {
    /// The search itself failed — the server is unwell, not unhelpful.
    SearchFailed,
    /// The server answered, with nothing.
    NoCandidates,
    /// It offered tracks, none by this artist.
    ArtistMismatch { offered: Vec<String> },
    /// The artist agreed and no title did.
    TitleMismatch { offered: Vec<String> },
}

/// Work out which of those it was. Only called when `pick` returned nothing.
pub fn explain(want: &PlaylistTrack, candidates: &[Candidate]) -> Miss {
    if candidates.is_empty() {
        return Miss::NoCandidates;
    }
    let by_artist: Vec<&Candidate> =
        candidates.iter().filter(|c| crate::db::artist_matches(&c.artist, &want.artist)).collect();
    // De-duplicated and capped: this goes in an error message a person reads,
    // not a log. Five names is enough to see the pattern.
    let names = |list: &[&Candidate], field: fn(&Candidate) -> String| {
        let mut seen: Vec<String> = Vec::new();
        for c in list {
            let v = field(c);
            if !v.is_empty() && !seen.iter().any(|s| s.eq_ignore_ascii_case(&v)) {
                seen.push(v);
            }
            if seen.len() == 5 {
                break;
            }
        }
        seen
    };
    if by_artist.is_empty() {
        let all: Vec<&Candidate> = candidates.iter().collect();
        Miss::ArtistMismatch { offered: names(&all, |c| c.artist.clone()) }
    } else {
        Miss::TitleMismatch { offered: names(&by_artist, |c| c.title.clone()) }
    }
}

/// The server's id for `want`, or `None` if nothing offered is actually it.
pub fn pick(want: &PlaylistTrack, candidates: &[Candidate]) -> Option<String> {
    candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| score(want, c).map(|s| (s, i, c)))
        // `max_by_key` keeps the *last* maximum, and the far end already ranked
        // these — so on a genuine tie the server's own first choice must win.
        // Reversing the index inside the key is what preserves that.
        .max_by_key(|(s, i, _)| (*s, std::cmp::Reverse(*i)))
        .map(|(_, _, c)| c.id.clone())
}

/// The search string sent to the far end.
///
/// Artist *and* title, because every one of these engines ranks across fields
/// and a bare title on a large library buries the right hit under covers. The
/// qualifier is dropped: a server holding the plain "Xtal" will not match a
/// query carrying "(Remastered 2008)", while a server holding the remaster
/// still matches the stem.
pub fn search_query(want: &PlaylistTrack) -> String {
    // The **primary** artist, for the same reason the qualifier is dropped from
    // the title: a library filed under "Low Roar" will not answer a query
    // carrying "Low Roar feat. Jófríður Ákadóttir", and the extra words are the
    // half least likely to be spelled the same at both ends. `score` still sees
    // the full credit and prefers an exact match on it.
    let artist = crate::db::split_featured(&want.artist).unwrap_or_else(|| want.artist.clone());
    format!("{} {}", without_qualifier(&want.title), artist).trim().to_string()
}

// ── The three targets ──
//
// The protocol calls live on the source structs, next to `find_artist` and
// `fetch_cover_art`, because that is where the credential and the HTTP client
// already are. These impls are the thin adapter, so `sources/` stays about
// talking to a server and this file stays about deciding what to send.

#[async_trait]
impl PlaylistTarget for crate::sources::PlexSource {
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String> {
        Self::search_tracks(self, &search_query(want)).await.map_err(|e| e.to_string())
    }
    async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<String, String> {
        Self::create_playlist(self, name, ids).await.map_err(|e| e.to_string())
    }
}

#[async_trait]
impl PlaylistTarget for crate::sources::MediaBrowserSource {
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String> {
        Self::search_tracks(self, &search_query(want)).await.map_err(|e| e.to_string())
    }
    async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<String, String> {
        Self::create_playlist(self, name, ids).await.map_err(|e| e.to_string())
    }
}

#[async_trait]
impl PlaylistTarget for crate::sources::NavidromeSource {
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String> {
        Self::search_tracks(self, &search_query(want)).await.map_err(|e| e.to_string())
    }
    async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<String, String> {
        Self::create_playlist(self, name, ids).await.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, title: &str, artist: &str, album: Option<&str>) -> Candidate {
        Candidate {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.map(Into::into),
        }
    }

    fn want(title: &str, artist: &str) -> PlaylistTrack {
        PlaylistTrack { title: title.into(), artist: artist.into(), album: None, isrc: None }
    }

    /// A featured credit is not a different performer, and refusing it loses a
    /// track that is sitting right there.
    ///
    /// `artist_matches` compares whole folded strings, so the history's
    /// "Low Roar feat. Jófríður Ákadóttir" never equalled a library's
    /// "Low Roar" and the hard gate threw it out. This is the real pairing from
    /// a real history.
    #[test]
    fn a_featured_credit_still_finds_the_primary_artist() {
        let w = want("Bones", "Low Roar feat. Jófríður Ákadóttir");
        let c = [cand("77", "Bones", "Low Roar", None)];
        assert_eq!(pick(&w, &c), Some("77".into()), "the credit must not refuse the artist");

        // And the other way round: the library carries the credit, the history
        // does not.
        let w2 = want("Bones", "Low Roar");
        let c2 = [cand("78", "Bones", "Low Roar feat. Jófríður Ákadóttir", None)];
        assert_eq!(pick(&w2, &c2), Some("78".into()));
    }

    /// The relaxation must not outrank an outright match. A library holding
    /// both the solo track and the collaboration has to return the one asked
    /// for.
    #[test]
    fn an_exact_artist_beats_a_credit_folded_one() {
        let w = want("Bones", "Low Roar");
        let c = [
            cand("credit", "Bones", "Low Roar feat. Jófríður Ákadóttir", None),
            cand("exact", "Bones", "Low Roar", None),
        ];
        assert_eq!(pick(&w, &c), Some("exact".into()));
    }

    /// Two different bands are still two different bands. The fold is narrow on
    /// purpose — `&` and `,` are names, not credits.
    #[test]
    fn folding_a_credit_does_not_merge_unrelated_artists() {
        let w = want("Archangel", "Burial");
        let c = [cand("x", "Archangel", "Four Tet", None)];
        assert_eq!(pick(&w, &c), None);

        let w2 = want("The Boxer", "Simon & Garfunkel");
        let c2 = [cand("y", "The Boxer", "Simon", None)];
        assert_eq!(pick(&w2, &c2), None, "an ampersand is a name, never a credit");
    }

    /// A miss has to say which kind it was: nothing came back, the artist was
    /// wrong, or the artist was right and no title matched. Reported as a bare
    /// miss they are indistinguishable, and they need completely different
    /// fixes.
    #[test]
    fn a_miss_says_which_kind_it_was() {
        let w = want("Xtal", "Aphex Twin");
        assert_eq!(explain(&w, &[]), Miss::NoCandidates);

        let wrong = [cand("1", "Xtal", "Arkells", None), cand("2", "Xtal", "Arkells", None)];
        assert_eq!(
            explain(&w, &wrong),
            Miss::ArtistMismatch { offered: vec!["Arkells".into()] },
            "de-duplicated: this goes in a message a person reads"
        );

        let right_artist = [cand("3", "Ageispolis", "Aphex Twin", None)];
        assert_eq!(
            explain(&w, &right_artist),
            Miss::TitleMismatch { offered: vec!["Ageispolis".into()] }
        );
    }

    /// The query drops the credit too. A server whose search engine is given
    /// three extra words it has never seen returns nothing, and then the gate
    /// never gets a chance.
    #[test]
    fn the_search_asks_for_the_primary_artist() {
        assert_eq!(
            search_query(&want("Bones", "Low Roar feat. Jófríður Ákadóttir")),
            "Bones Low Roar"
        );
        // A qualifier on the title goes as well, which is the existing rule.
        assert_eq!(search_query(&want("Xtal (Remastered 2008)", "Aphex Twin")), "Xtal Aphex Twin");
    }

    #[test]
    fn a_search_hit_for_a_different_artist_is_not_a_match() {
        // The exact shape of the Plex trap: asking for one artist and being
        // offered another, ranked first.
        let hits = vec![cand("1", "Comme des enfants", "Arkells", None)];
        assert_eq!(pick(&want("Comme des enfants", "Cœur de pirate"), &hits), None);
    }

    #[test]
    fn accents_and_ligatures_do_not_lose_the_track() {
        let hits = vec![cand("7", "Comme des enfants", "Cœur de pirate", None)];
        assert_eq!(
            pick(&want("Comme des enfants", "Coeur de Pirate"), &hits).as_deref(),
            Some("7")
        );
    }

    #[test]
    fn an_exact_title_beats_a_remaster_however_they_are_ordered() {
        let hits = vec![
            cand("remaster", "Xtal (Remastered 2008)", "Aphex Twin", None),
            cand("plain", "Xtal", "Aphex Twin", None),
        ];
        assert_eq!(pick(&want("Xtal", "Aphex Twin"), &hits).as_deref(), Some("plain"));

        let flipped = vec![
            cand("plain", "Xtal", "Aphex Twin", None),
            cand("remaster", "Xtal (Remastered 2008)", "Aphex Twin", None),
        ];
        assert_eq!(pick(&want("Xtal", "Aphex Twin"), &flipped).as_deref(), Some("plain"));
    }

    #[test]
    fn a_remaster_is_taken_when_it_is_all_the_library_has() {
        let hits = vec![cand("r", "Xtal (Remastered 2008)", "Aphex Twin", None)];
        assert_eq!(pick(&want("Xtal", "Aphex Twin"), &hits).as_deref(), Some("r"));
    }

    #[test]
    fn the_album_only_breaks_a_tie_and_never_gates_a_match() {
        let mut w = want("Xtal", "Aphex Twin");
        w.album = Some("Selected Ambient Works 85-92".into());
        let hits = vec![
            cand("comp", "Xtal", "Aphex Twin", Some("Some Compilation")),
            cand("album", "Xtal", "Aphex Twin", Some("Selected Ambient Works 85–92")),
        ];
        // The en-dash spelling still wins on album — the normaliser drops
        // punctuation, which is the same reason the MusicBrainz cache had to
        // stop keying on canonical strings.
        assert_eq!(pick(&w, &hits).as_deref(), Some("album"));

        // …and with no album on the far end at all, it is still a match.
        let bare = vec![cand("bare", "Xtal", "Aphex Twin", None)];
        assert_eq!(pick(&w, &bare).as_deref(), Some("bare"));
    }

    #[test]
    fn a_different_song_by_the_same_artist_is_not_a_match() {
        let hits = vec![cand("1", "Ageispolis", "Aphex Twin", None)];
        assert_eq!(pick(&want("Xtal", "Aphex Twin"), &hits), None);
    }

    #[test]
    fn a_title_that_is_only_a_bracket_keeps_it() {
        // Stripping here would reduce the title to nothing and match everything.
        assert_eq!(without_qualifier("(Reprise)"), "(Reprise)");
        assert_eq!(without_qualifier("[Untitled]"), "[Untitled]");
        // A bracket that isn't at the end is part of the name.
        assert_eq!(
            without_qualifier("(What's the Story) Morning Glory?"),
            "(What's the Story) Morning Glory?"
        );
        // Several qualifiers come off together.
        assert_eq!(without_qualifier("Xtal (Live) [Remastered]"), "Xtal");
    }

    #[test]
    fn the_query_drops_the_qualifier_but_keeps_the_artist() {
        let w = want("Xtal (Remastered 2008)", "Aphex Twin");
        assert_eq!(search_query(&w), "Xtal Aphex Twin");
    }

    #[test]
    fn an_empty_title_matches_nothing() {
        let hits = vec![cand("1", "", "Aphex Twin", None)];
        assert_eq!(pick(&want("", "Aphex Twin"), &hits), None);
    }
}
