//! What the tuner believes is on air, and what each new line does to it.
//!
//! **Pure**: no clock, no database, no network. [`Machine::step`] takes a line
//! the station sent and returns the [`Effect`]s to carry out, in order; the
//! async half in `radio::mod` carries them out (writes the air log, asks
//! MusicBrainz, settles listens, updates now-playing) and hands back what
//! those produced through [`Machine::attach`] and [`Machine::attach_break`].
//! Every rule about when a track starts, pauses, resumes and ends lives here,
//! where it can be tested line by line.
//!
//! The rules, as a station's text runs past:
//!
//! * **A track line** that names what is already on resumes it — WDR 2
//!   alternates "Songtitel gesucht? Die Playlist auf wdr2.de" with the title
//!   all the way through a song. A different track ends the current one and
//!   starts its own.
//! * **Station text** (a slogan, a web address) over a track *pauses* it: the
//!   station may be talking over the song. Over nothing, it opens one air-log
//!   entry for the whole run of text.
//! * **A break** (news, traffic, weather, ads) ends the track — the song is
//!   over — and opens or continues the run's entry.
//! * A paused track ends **at the moment the talk began**, whether a different
//!   track, a break, [`SUSPEND_MAX_SECS`] of talk, or Off is what ends it.
//! * The first line of a connection was already playing when we arrived, so a
//!   track that starts with it was not seen from its start.

use super::icy::Line;
use super::TunerState;

/// A track paused by station text for this long has ended, whatever follows.
pub const SUSPEND_MAX_SECS: i64 = 600;

/// The track on air, with whatever the async half has attached to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Track<P> {
    /// Ties a [`Effect::StartTrack`] to its later [`Machine::attach`].
    pub id: u64,
    pub artist: String,
    pub title: String,
    pub started_at: i64,
    /// It began while we were listening, rather than being on already.
    pub seen_start: bool,
    /// When station text took the title's place, if it has.
    pub interrupted_at: Option<i64>,
    /// The air-log row and the looked-up listen, once they exist.
    pub payload: Option<P>,
}

impl<P> Track<P> {
    fn is(&self, artist: &str, title: &str) -> bool {
        self.artist.eq_ignore_ascii_case(artist) && self.title.eq_ignore_ascii_case(title)
    }

    /// When it ended, if it ended now: at the start of any talk over it.
    fn ended(&self, now: i64) -> i64 {
        self.interrupted_at.unwrap_or(now)
    }
}

/// An open air-log entry for a run of non-track lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenBreak {
    pub id: u64,
    /// The air-log row, once it has been written.
    pub log: Option<i64>,
}

/// What a run of non-track lines is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    /// News, traffic, weather, ads.
    Break,
    /// Anything else the station said.
    Other,
}

/// Something the async half has to do, in the order given.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect<P> {
    /// The same line again: keep the current track fresh in now-playing.
    Refresh,
    /// A track is over. Settle it as a listen or not.
    EndTrack { track: Track<P>, ended_at: i64, natural: bool },
    /// A track began: log it, look it up, and [`Machine::attach`] the result.
    StartTrack { id: u64, artist: String, title: String, started_at: i64 },
    /// A run of non-track lines began: log it and [`Machine::attach_break`].
    OpenBreak { id: u64, kind: BreakKind, at: i64 },
    /// That run is over. `log` is its row, if it was written in time.
    CloseBreak { log: Option<i64>, at: i64 },
    /// Nothing is playing any more.
    ClearNowPlaying,
}

/// The tuner's view of the broadcast.
#[derive(Debug, Clone, PartialEq)]
pub struct Machine<P> {
    pub current: Option<Track<P>>,
    pub break_open: Option<OpenBreak>,
    /// The last line, verbatim.
    pub line: Option<String>,
    /// `None` until the station has said anything.
    pub state: Option<TunerState>,
    next_id: u64,
}

impl<P> Default for Machine<P> {
    fn default() -> Self {
        Machine { current: None, break_open: None, line: None, state: None, next_id: 0 }
    }
}

impl<P: Clone> Machine<P> {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn close_break(&mut self, at: i64, out: &mut Vec<Effect<P>>) {
        if let Some(b) = self.break_open.take() {
            out.push(Effect::CloseBreak { log: b.log, at });
        }
    }

    fn end_track(&mut self, now: i64, natural: bool, out: &mut Vec<Effect<P>>) {
        if let Some(track) = self.current.take() {
            let ended_at = track.ended(now);
            // A paused track ended when the talk began, which is a natural
            // end however it is noticed.
            let natural = natural || track.interrupted_at.is_some();
            out.push(Effect::EndTrack { track, ended_at, natural });
        }
    }

    fn open_break(&mut self, kind: BreakKind, now: i64, out: &mut Vec<Effect<P>>) {
        if self.break_open.is_none() {
            let id = self.id();
            self.break_open = Some(OpenBreak { id, log: None });
            out.push(Effect::OpenBreak { id, kind, at: now });
        }
    }

    /// A line arrived. `class` is what [`super::icy::classify`] made of it;
    /// `first_of_connection` is whether it is the first line since the
    /// stream (re)connected.
    pub fn step(
        &mut self,
        now: i64,
        raw: &str,
        class: Line,
        first_of_connection: bool,
    ) -> Vec<Effect<P>> {
        let mut out = Vec::new();
        if self.line.as_deref() == Some(raw) {
            if self.current.is_some() {
                out.push(Effect::Refresh);
            }
            return out;
        }
        self.line = Some(raw.to_string());

        match class {
            Line::Track { artist, title } => {
                self.close_break(now, &mut out);
                if let Some(t) = self.current.as_mut().filter(|t| t.is(&artist, &title)) {
                    t.interrupted_at = None;
                    self.state = Some(TunerState::Live);
                    return out;
                }
                self.end_track(now, true, &mut out);
                let id = self.id();
                self.current = Some(Track {
                    id,
                    artist: artist.clone(),
                    title: title.clone(),
                    started_at: now,
                    seen_start: !first_of_connection,
                    interrupted_at: None,
                    payload: None,
                });
                self.state = Some(TunerState::Live);
                out.push(Effect::StartTrack { id, artist, title, started_at: now });
            }
            Line::Break => {
                self.end_track(now, true, &mut out);
                self.open_break(BreakKind::Break, now, &mut out);
                self.state = Some(TunerState::Break);
                out.push(Effect::ClearNowPlaying);
            }
            Line::Other => {
                if let Some(t) = self.current.as_mut() {
                    // Talk over a track pauses it, and is not logged: the
                    // track's own line already says what was on.
                    t.interrupted_at.get_or_insert(now);
                    self.state = Some(TunerState::Live);
                } else {
                    self.open_break(BreakKind::Other, now, &mut out);
                    // Text after a break is still that break.
                    if self.state != Some(TunerState::Break) {
                        self.state = Some(TunerState::Unknown);
                    }
                    out.push(Effect::ClearNowPlaying);
                }
            }
        }
        out
    }

    /// Time passed with no new line. Ends a track paused for too long.
    pub fn tick(&mut self, now: i64) -> Vec<Effect<P>> {
        let mut out = Vec::new();
        let stale = self
            .current
            .as_ref()
            .and_then(|t| t.interrupted_at)
            .is_some_and(|i| now - i > SUSPEND_MAX_SECS);
        if stale {
            self.end_track(now, true, &mut out);
            self.state = Some(TunerState::Unknown);
            out.push(Effect::ClearNowPlaying);
        }
        out
    }

    /// The tuner is turning off: a track still named was cut short; a paused
    /// one ended when the talk began.
    pub fn off(&mut self, now: i64) -> Vec<Effect<P>> {
        let mut out = Vec::new();
        self.end_track(now, false, &mut out);
        self.close_break(now, &mut out);
        out.push(Effect::ClearNowPlaying);
        out
    }

    /// Give the track started by `id` what its lookup produced. `Err` hands
    /// the payload back when that track has already ended — something newer
    /// arrived while MusicBrainz was being asked — so the caller can mark its
    /// row as cut short.
    pub fn attach(&mut self, id: u64, payload: P) -> Result<&Track<P>, P> {
        match self.current.as_mut() {
            Some(t) if t.id == id && t.payload.is_none() => {
                t.payload = Some(payload);
                Ok(t)
            }
            _ => Err(payload),
        }
    }

    /// Record the air-log row of the run opened by `id`. `false` when that run
    /// has already closed, so the caller closes the row itself.
    pub fn attach_break(&mut self, id: u64, log: i64) -> bool {
        match self.break_open.as_mut() {
            Some(b) if b.id == id => {
                b.log = Some(log);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type M = Machine<&'static str>;

    fn track(a: &str, t: &str) -> Line {
        Line::Track { artist: a.into(), title: t.into() }
    }

    /// Feed a line and attach whatever it started, the way the async half
    /// does once its lookups are done.
    fn feed(m: &mut M, now: i64, raw: &str, class: Line, first: bool) -> Vec<Effect<&'static str>> {
        let out = m.step(now, raw, class, first);
        for e in &out {
            match e {
                Effect::StartTrack { id, .. } => {
                    m.attach(*id, "heard").unwrap();
                }
                Effect::OpenBreak { id, .. } => {
                    assert!(m.attach_break(*id, 900 + *id as i64));
                }
                _ => {}
            }
        }
        out
    }

    fn ended(out: &[Effect<&'static str>]) -> Vec<(String, i64, bool)> {
        out.iter()
            .filter_map(|e| match e {
                Effect::EndTrack { track, ended_at, natural } => {
                    Some((track.title.clone(), *ended_at, *natural))
                }
                _ => None,
            })
            .collect()
    }

    fn started(out: &[Effect<&'static str>]) -> Vec<String> {
        out.iter()
            .filter_map(|e| match e {
                Effect::StartTrack { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect()
    }

    fn opened(out: &[Effect<&'static str>]) -> usize {
        out.iter().filter(|e| matches!(e, Effect::OpenBreak { .. })).count()
    }

    const PROMO: &str = "Songtitel gesucht? Die Playlist auf wdr2.de";

    #[test]
    fn a_track_interrupted_by_promo_text_and_named_again_is_one_track() {
        let mut m = M::default();
        let out = feed(
            &mut m,
            0,
            "Teddy Swims - Mr. Know it all",
            track("Teddy Swims", "Mr. Know it all"),
            true,
        );
        assert_eq!(started(&out), ["Mr. Know it all"]);

        let out = feed(&mut m, 60, PROMO, Line::Other, false);
        assert!(ended(&out).is_empty(), "talk over a song does not end it: {out:?}");
        assert_eq!(opened(&out), 0, "and is not logged");
        assert!(!out.contains(&Effect::ClearNowPlaying), "the song is still on");
        assert_eq!(m.current.as_ref().unwrap().interrupted_at, Some(60));
        assert_eq!(m.state, Some(TunerState::Live));

        // Matched without regard to case: stations re-send titles freely.
        let out = feed(
            &mut m,
            90,
            "TEDDY SWIMS - Mr. Know It All",
            track("TEDDY SWIMS", "Mr. Know It All"),
            false,
        );
        assert!(out.is_empty(), "resuming is not a new track: {out:?}");
        let t = m.current.as_ref().unwrap();
        assert_eq!((t.started_at, t.interrupted_at, t.payload), (0, None, Some("heard")));
    }

    #[test]
    fn a_different_track_after_promo_ends_the_first_when_the_talk_began() {
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        feed(&mut m, 170, PROMO, Line::Other, false);
        feed(&mut m, 175, "Another promo", Line::Other, false);
        let out = feed(&mut m, 200, "B - Two", track("B", "Two"), false);
        assert_eq!(ended(&out), [("One".to_string(), 170, true)]);
        assert_eq!(started(&out), ["Two"]);
        assert!(m.current.as_ref().unwrap().seen_start, "watched it start");
    }

    #[test]
    fn a_break_ends_the_track_at_once() {
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        let out = feed(&mut m, 200, "Nachrichten", Line::Break, false);
        assert_eq!(ended(&out), [("One".to_string(), 200, true)]);
        assert_eq!(opened(&out), 1);
        assert!(out.contains(&Effect::ClearNowPlaying));
        assert_eq!(m.state, Some(TunerState::Break));
        assert!(m.current.is_none());
    }

    #[test]
    fn a_run_of_station_text_is_one_entry_closed_by_the_next_track() {
        let mut m = M::default();
        let mut opens = 0;
        opens += opened(&feed(&mut m, 0, "Nachrichten", Line::Break, true));
        opens += opened(&feed(&mut m, 30, "Verkehr", Line::Break, false));
        opens += opened(&feed(&mut m, 60, "Wir sind der Westen", Line::Other, false));
        assert_eq!(opens, 1, "a bulletin rotating three texts is one break");
        assert_eq!(m.state, Some(TunerState::Break), "text after a break is still the break");

        let out = feed(&mut m, 120, "A - One", track("A", "One"), false);
        assert_eq!(
            out[0],
            Effect::CloseBreak { log: Some(901), at: 120 },
            "closed before the track starts"
        );
        assert_eq!(started(&out), ["One"]);
        assert!(m.break_open.is_none());
    }

    #[test]
    fn text_over_a_paused_track_is_never_logged() {
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        for (i, raw) in ["Promo 1", "Promo 2", "Promo 3"].iter().enumerate() {
            let out = feed(&mut m, 10 + i as i64, raw, Line::Other, false);
            assert_eq!(opened(&out), 0, "{raw}");
        }
        assert_eq!(
            m.current.as_ref().unwrap().interrupted_at,
            Some(10),
            "paused at the first, not the last"
        );
    }

    #[test]
    fn a_track_paused_for_ten_minutes_ends_when_the_talk_began() {
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        feed(&mut m, 100, PROMO, Line::Other, false);
        assert!(m.tick(100 + SUSPEND_MAX_SECS).is_empty(), "not yet");
        let out = m.tick(101 + SUSPEND_MAX_SECS);
        assert_eq!(ended(&out), [("One".to_string(), 100, true)]);
        assert!(out.contains(&Effect::ClearNowPlaying));
        assert_eq!(m.state, Some(TunerState::Unknown));
        assert!(m.tick(5000).is_empty(), "nothing left to end");

        // A track that was never paused never expires.
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        assert!(m.tick(100_000).is_empty());
    }

    #[test]
    fn off_while_paused_ends_at_the_pause_and_off_while_named_is_cut_short() {
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        feed(&mut m, 150, PROMO, Line::Other, false);
        assert_eq!(ended(&m.off(400)), [("One".to_string(), 150, true)]);

        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), false);
        assert_eq!(ended(&m.off(400)), [("One".to_string(), 400, false)]);

        let mut m = M::default();
        feed(&mut m, 0, "Nachrichten", Line::Break, false);
        let out = m.off(50);
        assert_eq!(out[0], Effect::CloseBreak { log: Some(901), at: 50 });
    }

    #[test]
    fn the_first_line_of_a_connection_was_already_playing() {
        let mut m = M::default();
        feed(&mut m, 0, "A - One", track("A", "One"), true);
        assert!(!m.current.as_ref().unwrap().seen_start);
        feed(&mut m, 200, "B - Two", track("B", "Two"), false);
        assert!(m.current.as_ref().unwrap().seen_start);
        // After a reconnect, a new title arriving first was not seen to start.
        feed(&mut m, 400, "C - Three", track("C", "Three"), true);
        assert!(!m.current.as_ref().unwrap().seen_start);
    }

    #[test]
    fn the_same_line_again_only_refreshes_a_track() {
        let mut m = M::default();
        assert!(feed(&mut m, 0, "1LIVE", Line::Other, true).len() == 2);
        assert!(m.step(5, "1LIVE", Line::Other, false).is_empty(), "nothing to refresh");
        feed(&mut m, 10, "A - One", track("A", "One"), false);
        assert_eq!(m.step(20, "A - One", track("A", "One"), false), [Effect::Refresh]);
        // A reconnect re-sending the line it left on changes nothing either.
        assert_eq!(m.step(80, "A - One", track("A", "One"), true), [Effect::Refresh]);
        assert!(m.current.as_ref().unwrap().seen_start, "still the track we watched start");
    }

    #[test]
    fn a_lookup_that_returns_after_its_track_ended_is_handed_back() {
        let mut m = M::default();
        let out = m.step(0, "A - One", track("A", "One"), false);
        let Effect::StartTrack { id: first, .. } = out[0].clone() else { panic!("{out:?}") };
        // B arrives while A's lookup is still out.
        let out = m.step(5, "B - Two", track("B", "Two"), false);
        let ended_without_payload = ended(&out);
        assert_eq!(ended_without_payload, [("One".to_string(), 5, true)]);
        assert_eq!(
            m.attach(first, "late"),
            Err("late"),
            "A is over; its row is cut short by the caller"
        );
        let Effect::StartTrack { id: second, .. } = out[1].clone() else { panic!("{out:?}") };
        assert!(m.attach(second, "heard").is_ok());
        assert_eq!(m.attach(second, "twice"), Err("twice"), "one payload per track");
    }

    #[test]
    fn a_break_whose_row_was_written_after_it_closed_is_handed_back() {
        let mut m = M::default();
        let out = m.step(0, "Nachrichten", Line::Break, false);
        let Effect::OpenBreak { id, .. } = out[0] else { panic!("{out:?}") };
        let out = m.step(30, "A - One", track("A", "One"), false);
        assert_eq!(out[0], Effect::CloseBreak { log: None, at: 30 });
        assert!(!m.attach_break(id, 7), "already closed: the caller closes the row itself");
    }
}
