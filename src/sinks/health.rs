//! Per-sink health, and the breaker that keeps one dead service out of the
//! engine tick.
//!
//! **This exists because of 2026-08-24.** Two MetaBrainz hosts went unreachable
//! for a day while Last.fm stayed perfectly healthy. Every pending listen then
//! independently rediscovered that ListenBrainz was down, at ~66s of retry
//! backoff each, and since `flush_pending` is awaited inline at the tail of
//! `run_tick` the engine's 5-second tick became 45–92 minutes: Plex polling
//! collapsed from ~720 reads an hour to three, and the deck — which expires an
//! entry after at most ten minutes — was empty all day. 978 listens burned
//! about eighteen hours of sleeping between them to learn one fact.
//!
//! So the fact is learned **once** and remembered. After
//! [`FAILURES_TO_OPEN`] consecutive failures a sink's breaker opens and every
//! call to it is skipped until the cooldown lapses, at which point one request
//! is let through to see whether it is back. Nothing is lost by skipping: a
//! scrobble that is not delivered stays `pending`, and `delivered_sinks` means
//! a later retry cannot double-send it.
//!
//! State is **process-global and keyed by sink name**, which it has to be:
//! `flush_pending` builds a fresh set of sinks every tick, so anything held on
//! the sink itself is forgotten before it can be useful. It is also what backs
//! Settings → Operations, where "ListenBrainz — failing for 6h, 978 attempts"
//! is the one line that would have made that day obvious in a glance.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Consecutive failures before a sink is presumed down. Two is too jumpy for a
/// service that drops the occasional request; five is most of a minute of a
/// tick spent proving something already known.
pub const FAILURES_TO_OPEN: u32 = 3;

/// How long to leave it alone, growing with each failed probe so a long outage
/// costs a handful of requests rather than one per tick forever.
const COOLDOWN_BASE: Duration = Duration::from_secs(60);
const COOLDOWN_MAX: Duration = Duration::from_secs(15 * 60);

/// What the breaker knows about one service.
#[derive(Clone, Debug)]
pub struct SinkStatus {
    pub name: String,
    /// Consecutive failures since the last success.
    pub failures: u32,
    /// Total failures ever recorded this process. The count that says whether a
    /// blip was a blip.
    pub total_failures: u64,
    pub total_successes: u64,
    pub last_ok: Option<SystemTime>,
    pub last_failure: Option<SystemTime>,
    /// The most recent error, verbatim. One copy, not 6,096 log lines of it.
    pub last_error: Option<String>,
    /// `Some(seconds)` while the breaker is open and calls are being skipped.
    pub open_for: Option<u64>,
}

#[derive(Default)]
struct Entry {
    failures: u32,
    total_failures: u64,
    total_successes: u64,
    last_ok: Option<SystemTime>,
    last_failure: Option<SystemTime>,
    last_error: Option<String>,
    open_until: Option<Instant>,
    /// How many cooldowns in a row have elapsed without a success, so the wait
    /// can back off rather than probing a week-long outage every minute.
    cooldowns: u32,
}

#[derive(Default)]
pub struct SinkHealth {
    inner: Mutex<HashMap<String, Entry>>,
}

/// The process-wide registry.
pub fn health() -> &'static SinkHealth {
    static H: OnceLock<SinkHealth> = OnceLock::new();
    H.get_or_init(SinkHealth::default)
}

impl SinkHealth {
    /// Lock, recovering from a poisoned mutex rather than propagating the
    /// panic. A breaker is advisory: the worst a stale entry can do is let one
    /// request through, where an `unwrap` here would take down every forward in
    /// the process for the rest of its life.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Should this sink be skipped right now? Returns how much longer the
    /// breaker stays open.
    ///
    /// Consumes the cooldown when it lapses, so exactly **one** caller gets the
    /// probe and the rest keep being skipped until it reports back.
    pub fn skip_for(&self, name: &str) -> Option<Duration> {
        let mut map = self.lock();
        let e = map.get_mut(name)?;
        let until = e.open_until?;
        let now = Instant::now();
        if now >= until {
            e.open_until = None; // one probe goes through
            return None;
        }
        Some(until - now)
    }

    pub fn record_ok(&self, name: &str) {
        let mut map = self.lock();
        let e = map.entry(name.to_string()).or_default();
        if e.failures >= FAILURES_TO_OPEN {
            tracing::info!("✅ {} is answering again after {} failures", name, e.failures);
        }
        e.failures = 0;
        e.cooldowns = 0;
        e.open_until = None;
        e.last_error = None;
        e.total_successes = e.total_successes.saturating_add(1);
        e.last_ok = Some(SystemTime::now());
    }

    /// Record a failure and open the breaker once the run is long enough.
    ///
    /// Logs at most once per state change rather than once per call — the
    /// 2026-08-24 log is 3 MB of which 6,096 lines are the same sentence, which
    /// is its own kind of outage: nothing else in it could be found.
    pub fn record_failure(&self, name: &str, err: &str) {
        let mut map = self.lock();
        let e = map.entry(name.to_string()).or_default();
        e.failures = e.failures.saturating_add(1);
        e.total_failures = e.total_failures.saturating_add(1);
        e.last_failure = Some(SystemTime::now());
        e.last_error = Some(err.chars().take(300).collect());

        if e.failures >= FAILURES_TO_OPEN {
            let backoff =
                COOLDOWN_BASE.saturating_mul(1u32 << e.cooldowns.min(4)).min(COOLDOWN_MAX);
            e.cooldowns = e.cooldowns.saturating_add(1);
            e.open_until = Some(Instant::now() + backoff);
            tracing::warn!(
                "⛔ {} has failed {} times in a row — not asking again for {}s. Last error: {}",
                name,
                e.failures,
                backoff.as_secs(),
                err.chars().take(200).collect::<String>()
            );
        }
    }

    /// Every sink this process has spoken to, for Settings → Operations.
    pub fn snapshot(&self) -> Vec<SinkStatus> {
        let map = self.lock();
        let now = Instant::now();
        let mut out: Vec<SinkStatus> = map
            .iter()
            .map(|(name, e)| SinkStatus {
                name: name.clone(),
                failures: e.failures,
                total_failures: e.total_failures,
                total_successes: e.total_successes,
                last_ok: e.last_ok,
                last_failure: e.last_failure,
                last_error: e.last_error.clone(),
                open_for: e.open_until.filter(|u| *u > now).map(|u| (u - now).as_secs()),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    #[cfg(test)]
    fn reset(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh registry per test — the real one is global, and these would
    /// otherwise be reading each other's state depending on thread scheduling.
    fn fresh() -> SinkHealth {
        let h = SinkHealth::default();
        h.reset();
        h
    }

    /// The whole point: one dead service is learned about once, not once per
    /// listen. Before this, 978 pending scrobbles each spent ~66s discovering
    /// the same outage while the engine tick waited on all of it.
    #[test]
    fn a_dead_sink_is_skipped_rather_than_asked_once_per_listen() {
        let h = fresh();
        for _ in 0..FAILURES_TO_OPEN {
            assert!(h.skip_for("ListenBrainz").is_none(), "must ask before it knows");
            h.record_failure("ListenBrainz", "error sending request");
        }
        assert!(h.skip_for("ListenBrainz").is_some(), "breaker should be open now");
    }

    /// The breaker is per service. On 2026-08-24 Last.fm answered 244 of 244
    /// while ListenBrainz answered 194 of 1,176 — so a registry that tripped
    /// globally would have stopped the half that was working.
    #[test]
    fn one_dead_sink_does_not_silence_a_healthy_one() {
        let h = fresh();
        for _ in 0..FAILURES_TO_OPEN {
            h.record_failure("ListenBrainz", "error sending request");
        }
        assert!(h.skip_for("ListenBrainz").is_some());
        assert!(h.skip_for("Last.fm").is_none(), "Last.fm was never the problem");
    }

    /// A success clears the run outright. An outage that ends must not leave a
    /// sink one blip away from being presumed dead again.
    #[test]
    fn a_success_closes_the_breaker() {
        let h = fresh();
        for _ in 0..FAILURES_TO_OPEN {
            h.record_failure("ListenBrainz", "boom");
        }
        h.record_ok("ListenBrainz");
        assert!(h.skip_for("ListenBrainz").is_none());
        let s = &h.snapshot()[0];
        assert_eq!(s.failures, 0);
        assert!(s.last_error.is_none());
        assert_eq!(s.total_failures, FAILURES_TO_OPEN as u64);
    }

    /// Exactly one caller gets the probe when the cooldown lapses. Letting the
    /// whole queue through at once would reproduce the original stall in
    /// bursts, once a minute, forever.
    #[test]
    fn only_one_call_probes_a_lapsed_cooldown() {
        let h = fresh();
        for _ in 0..FAILURES_TO_OPEN {
            h.record_failure("ListenBrainz", "boom");
        }
        // Force the cooldown to have elapsed.
        h.lock().get_mut("ListenBrainz").unwrap().open_until =
            Some(Instant::now() - Duration::from_secs(1));

        assert!(h.skip_for("ListenBrainz").is_none(), "first caller probes");
        // The probe has not reported back, so the run still stands and the
        // next failure re-opens it.
        h.record_failure("ListenBrainz", "still down");
        assert!(h.skip_for("ListenBrainz").is_some(), "and the rest keep being skipped");
    }

    /// Reported verbatim and only once, rather than 6,096 times into a log
    /// nobody can then read.
    #[test]
    fn the_snapshot_carries_the_last_error_for_the_ops_panel() {
        let h = fresh();
        h.record_failure("ListenBrainz", "error sending request for url (…/submit-listens)");
        let s = h.snapshot();
        assert_eq!(s.len(), 1);
        assert!(s[0].last_error.as_deref().unwrap().contains("submit-listens"));
        assert!(s[0].last_failure.is_some());
        assert!(s[0].last_ok.is_none());
    }
}
