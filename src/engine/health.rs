//! What the engine tick is actually doing, for Settings → Operations.
//!
//! **One number would have made 2026-08-24 obvious in a glance**: the tick is
//! configured at 5 seconds and was taking 45–92 minutes. Every symptom that day
//! — Plex "not syncing", the deck permanently empty, three force restarts —
//! was that one fact wearing different clothes, and there was no way to see it
//! from inside the running application. It took reading 3 MB of log over SSH.
//!
//! So the tick times itself. A tick that runs longer than its own interval is
//! the single number that says the engine is in trouble, whatever the cause,
//! and it is meaningful without knowing what the cause is.
//!
//! Process-global rather than threaded through `AppState`: the engine never
//! sees `AppState` (the `NowPlayingRegistry` is the one piece of state they
//! share, and it is shared by being cloned into both), and this is written by
//! exactly one loop and read by one handler. Same shape as [`crate::sinks::health`].

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, Default)]
pub struct TickHealth {
    /// The interval the engine is configured for. The denominator for
    /// everything else here — a 40-second tick is fine at a 30-second interval
    /// and a catastrophe at five.
    pub interval_secs: u64,
    pub ticks: u64,
    pub last_started: Option<SystemTime>,
    pub last_duration_ms: u64,
    /// Worst tick since the process started. A single bad one is a blip; a
    /// worst of 5,400,000 ms is the incident.
    pub longest_ms: u64,
    /// Ticks that ran longer than the configured interval — i.e. the engine
    /// could not keep its own schedule.
    pub overruns: u64,
    /// Listens the flush had to leave for the next tick because it ran out of
    /// budget. Nonzero here means forwarding is behind, and is the difference
    /// between "the deck is fine, a sink is slow" and "everything is stuck".
    pub deferred_listens: u64,
}

#[derive(Default)]
struct Inner(TickHealth);

fn state() -> &'static Mutex<Inner> {
    static S: OnceLock<Mutex<Inner>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Inner::default()))
}

/// Recover from poisoning rather than propagate it — this is a readout, and a
/// panicked reader must not take the engine down with it.
fn lock() -> std::sync::MutexGuard<'static, Inner> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Record the engine's configured interval, once at startup.
pub fn set_interval(d: Duration) {
    lock().0.interval_secs = d.as_secs();
}

/// Mark the start of a tick.
pub fn tick_started() {
    let mut g = lock();
    g.0.last_started = Some(SystemTime::now());
}

/// Mark the end of one, with how long it took.
pub fn tick_finished(took: Duration) {
    let mut g = lock();
    let ms = took.as_millis().min(u64::MAX as u128) as u64;
    g.0.ticks = g.0.ticks.saturating_add(1);
    g.0.last_duration_ms = ms;
    g.0.longest_ms = g.0.longest_ms.max(ms);
    if g.0.interval_secs > 0 && took.as_secs() > g.0.interval_secs {
        g.0.overruns = g.0.overruns.saturating_add(1);
    }
}

/// Listens the flush deliberately left for next time.
pub fn deferred(n: usize) {
    let mut g = lock();
    g.0.deferred_listens = g.0.deferred_listens.saturating_add(n as u64);
}

pub fn snapshot() -> TickHealth {
    lock().0.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number the whole panel exists for: a tick that cannot keep its own
    /// schedule is counted as such, whatever made it slow.
    #[test]
    fn a_tick_longer_than_its_interval_is_an_overrun() {
        set_interval(Duration::from_secs(5));
        let before = snapshot().overruns;
        tick_finished(Duration::from_millis(900));
        assert_eq!(snapshot().overruns, before, "a fast tick is not an overrun");
        tick_finished(Duration::from_secs(90));
        assert_eq!(snapshot().overruns, before + 1);
        assert!(snapshot().longest_ms >= 90_000, "the worst tick is kept, not just the last");
    }
}
