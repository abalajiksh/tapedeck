//! Tiny in-memory failure-rate limiter for the login endpoint — throttles
//! online password guessing. Keyed by client IP; a burst of failures within the
//! window blocks further attempts until the window rolls over. A success resets
//! the key. State is process-local (fine for a single-binary self-host).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Attempt {
    failures: u32,
    window_start: Instant,
}

pub struct RateLimiter {
    inner: Mutex<HashMap<String, Attempt>>,
    max_failures: u32,
    window: Duration,
}

/// Entries above which a write sweeps expired keys.
///
/// The map only ever forgot a key when that same key was asked about again, so
/// an attacker rotating source addresses grew it without bound — every failed
/// guess from a fresh address was a permanent allocation. Sweeping on write is
/// enough: entries are only created there, and the sweep is O(n) over a map
/// that is a handful of rows in every legitimate deployment.
const SWEEP_ABOVE: usize = 1_024;

impl RateLimiter {
    pub fn new(max_failures: u32, window: Duration) -> Self {
        Self { inner: Mutex::new(HashMap::new()), max_failures, window }
    }

    /// If the key is currently blocked, returns `Some(retry_after_seconds)`.
    pub fn blocked_for(&self, key: &str) -> Option<u64> {
        let mut map = self.lock();
        if let Some(a) = map.get(key) {
            let elapsed = a.window_start.elapsed();
            if elapsed >= self.window {
                map.remove(key); // window rolled over — forget it
            } else if a.failures >= self.max_failures {
                return Some((self.window - elapsed).as_secs() + 1);
            }
        }
        None
    }

    pub fn record_failure(&self, key: &str) {
        let mut map = self.lock();
        if map.len() > SWEEP_ABOVE {
            let window = self.window;
            map.retain(|_, a| a.window_start.elapsed() < window);
        }
        let a = map
            .entry(key.to_string())
            .or_insert(Attempt { failures: 0, window_start: Instant::now() });
        if a.window_start.elapsed() >= self.window {
            a.failures = 0;
            a.window_start = Instant::now();
        }
        a.failures += 1;
    }

    pub fn reset(&self, key: &str) {
        self.lock().remove(key);
    }

    /// Lock, recovering from poisoning rather than propagating it.
    ///
    /// `.lock().unwrap()` meant that one panic anywhere in a critical section
    /// would poison this mutex permanently and every subsequent login attempt
    /// on the instance would panic with it. Nothing in these sections can
    /// realistically panic, so this is cheap insurance rather than a fix — but
    /// the failure it guards against is "nobody can log in until the process is
    /// restarted", which is not a proportionate answer to a throttling table.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Attempt>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_failures_then_success_resets() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.blocked_for("ip").is_none());
        rl.record_failure("ip");
        rl.record_failure("ip");
        assert!(rl.blocked_for("ip").is_none(), "2 failures under the limit");
        rl.record_failure("ip"); // 3rd → at limit
        assert!(rl.blocked_for("ip").is_some(), "blocked at max failures");
        rl.reset("ip");
        assert!(rl.blocked_for("ip").is_none(), "success clears the block");
    }

    #[test]
    fn window_rollover_forgets_failures() {
        let rl = RateLimiter::new(1, Duration::from_millis(40));
        rl.record_failure("ip");
        assert!(rl.blocked_for("ip").is_some());
        std::thread::sleep(Duration::from_millis(55));
        assert!(rl.blocked_for("ip").is_none(), "window expired, no longer blocked");
    }

    #[test]
    fn keys_are_independent() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        rl.record_failure("a");
        assert!(rl.blocked_for("a").is_some());
        assert!(rl.blocked_for("b").is_none());
    }
}
