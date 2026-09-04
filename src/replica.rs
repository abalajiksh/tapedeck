//! Whether this instance holds the data but none of the outward-facing roles.
//!
//! A **replica** is a full copy of somebody's Tapedeck that must not act on the
//! world on their behalf. Two things want to be one: a staging instance running
//! `main` against a restored production snapshot, and — later — a desktop app
//! that holds the same history offline. They differ in how they stay current
//! and not at all in what they are forbidden to do, so there is one switch.
//!
//! ```text
//! TAPEDECK_REPLICA=1
//! ```
//!
//! # Why a mode rather than a sanitised snapshot
//!
//! The restore script can delete `user_connections` and disable every source,
//! and that is worth doing as well. But a script is a thing somebody forgets on
//! the one night they restore by hand, and the failure is not a broken staging
//! instance — it is a listen forwarded a second time onto somebody's permanent
//! public Last.fm record, which is the failure this codebase treats as cardinal
//! and which cannot be taken back. A mode is enforced by the binary on every
//! path, including the ones added next year.
//!
//! # What it forbids, and where
//!
//! Each gate sits at the one chokepoint the codebase already has for that role,
//! rather than at the call sites:
//!
//! | role | gate |
//! |---|---|
//! | forwarding listens to sinks | [`crate::engine`]'s `flush_pending` |
//! | building a user's sinks at all | `sinks::sinks_for_user` |
//! | now-playing to Last.fm / ListenBrainz | `sinks::submit_now_playing` |
//! | asking MetaBrainz anything | `musicbrainz::api_get` |
//! | being discoverable as an actor | `Database::discoverable_user_id` |
//!
//! **Forwarding is gated twice on purpose.** An empty sink list alone would be
//! worse than useless there: `flush_pending` marks a listen `synced` when every
//! enabled sink has taken it, and over an empty list that test is *vacuously
//! true* — so the row would be recorded as delivered having been sent nowhere.
//! That is a real defect this application has already had (v0.91.0, where a
//! transient read error produced the same empty list). The flush gate is what
//! makes the sink gate safe.
//!
//! # The MetaBrainz gate is not politeness
//!
//! A replica and its production instance sit behind one public address, so
//! enrichment from both doubles the request rate against a service that blocks
//! by IP at the connection level with no status code — which is exactly what
//! happened on 2026-08-27 and took two days of network archaeology to
//! diagnose. Staging must read the cache and ask nobody.
//!
//! # What it does *not* forbid
//!
//! Everything else. A replica serves every screen, runs every report, and may
//! poll sources if its snapshot left them enabled — reads are harmless and the
//! dedup key is deterministic. The list above is the list; a replica that could
//! not be used would not be a useful test of anything.

use std::sync::OnceLock;

/// Is this build running as a replica?
///
/// Read once. A mode that could change under a running process would mean
/// every gate has to reason about which half of a tick it is in.
pub fn is_replica() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("TAPEDECK_REPLICA").is_ok_and(|v| v != "0" && !v.is_empty()))
}

/// The line to log at startup, if there is one.
///
/// Worth saying out loud: the whole point of a replica is that it looks
/// identical to the instance it copied, so an operator with two tabs open has
/// nothing else to tell them apart.
pub fn banner() -> Option<&'static str> {
    is_replica().then_some(
        "🪞 Replica mode — this instance forwards nothing, publishes no actor, \
         and asks MusicBrainz nothing. Set TAPEDECK_REPLICA=0 to make it authoritative.",
    )
}

#[cfg(test)]
mod tests {
    /// The env spelling is the one the deploy writes, and `0` has to mean off.
    ///
    /// Asserted against a local parse rather than `is_replica`, which caches in
    /// a `OnceLock` and would make one test decide the answer for every other
    /// test in the process.
    #[test]
    fn only_a_deliberate_value_turns_it_on() {
        let on = |v: &str| v != "0" && !v.is_empty();
        assert!(on("1"));
        assert!(on("true"));
        assert!(!on("0"), "an explicit 0 is off, so a deploy can set it either way");
        assert!(!on(""), "an empty value is not a decision");
    }
}
