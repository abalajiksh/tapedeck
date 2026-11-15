use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    layer::SubscriberExt, reload, util::SubscriberInitExt, EnvFilter, Registry,
};

/// Shared handle for runtime log level control.
///
/// Wraps the live `reload::Handle` for the subscriber's `EnvFilter`, so
/// swapping the level here actually takes effect on the running subscriber.
#[derive(Clone)]
pub struct LogLevelHandle {
    filter: reload::Handle<EnvFilter, Registry>,
}

impl LogLevelHandle {
    /// Update the log level at runtime
    pub fn set_level(&self, level: &str) -> Result<(), String> {
        let directive = match level.to_uppercase().as_str() {
            "TRACE" => "trace",
            "DEBUG" => "debug",
            "INFO" => "info",
            "WARN" => "warn",
            "ERROR" => "error",
            _ => return Err(format!("Invalid log level: {}", level)),
        };

        self.filter
            .reload(EnvFilter::new(directive))
            .map_err(|e| format!("Failed to apply log level: {}", e))
    }

    /// Get the current log level
    pub fn get_level(&self) -> String {
        self.filter.with_current(|f| f.to_string()).unwrap_or_else(|_| "unknown".to_string())
    }
}

/// Initialize logging with file output and runtime level control
///
/// # Arguments
/// * `log_dir` - Directory for log files (default: "./logs")
/// * `enable_file_logging` - Whether to write logs to files
/// * `enable_console` - Whether to write logs to stdout/stderr
/// * `level` - Initial filter directive (e.g. "info", "tapedeck=debug")
///
/// # Returns
/// A handle to control log levels at runtime
pub fn init_logging(
    log_dir: Option<&str>,
    enable_file_logging: bool,
    enable_console: bool,
    level: &str,
) -> Result<LogLevelHandle, Box<dyn std::error::Error>> {
    let (filter_layer, reload_handle) =
        reload::Layer::<EnvFilter, Registry>::new(EnvFilter::try_new(level)?);

    let handle = LogLevelHandle { filter: reload_handle };

    // The filter layer must be part of the subscriber, otherwise every event
    // is emitted regardless of level and runtime reloads do nothing.
    let subscriber = Registry::default().with(filter_layer);

    // Console layer
    if enable_console {
        let console_layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false)
            .compact();

        let subscriber = subscriber.with(console_layer);

        // File layer (if enabled)
        if enable_file_logging {
            let log_path = log_dir.unwrap_or("./logs");
            std::fs::create_dir_all(log_path)?;

            let file_appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix("tapedeck")
                .filename_suffix("log")
                .max_log_files(7) // Keep 7 days of logs
                .build(log_path)?;

            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(file_appender)
                .with_ansi(false) // No color codes in files
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true);

            subscriber.with(file_layer).init();
        } else {
            subscriber.init();
        }
    } else if enable_file_logging {
        // File only, no console
        let log_path = log_dir.unwrap_or("./logs");
        std::fs::create_dir_all(log_path)?;

        let file_appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix("tapedeck")
            .filename_suffix("log")
            .max_log_files(7)
            .build(log_path)?;

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file_appender)
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true);

        subscriber.with(file_layer).init();
    }

    Ok(handle)
}

/// Render an error together with everything that caused it.
///
/// **This function exists because of 2026-08-27.** Two days of logs said, 11,819
/// times:
///
/// ```text
/// error sending request for url (https://musicbrainz.org/ws/2/recording?...)
/// ```
///
/// which is the bare `Display` of a `reqwest::Error`, and is the *same string*
/// for a DNS failure, a refused connection, a connect timeout and a TLS
/// handshake the peer cut off. It was the last of those, and finding that out
/// took four rounds of network archaeology on a live Raspberry Pi. The cause
/// chain — `unexpected eof while reading` — was hanging off the error the whole
/// time and was never printed, because every call site formatted with `{}`.
///
/// So: when logging an error that crossed a network, use this. The rule is not
/// "log more", it is that **the top-level message of a transport error names
/// the operation and never the fault**.
pub fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    // Bounded: a cause chain is a handful of links, and a cycle would otherwise
    // hang the logger — which is a worse outage than the one being logged.
    for _ in 0..8 {
        let Some(cause) = source else { break };
        let text = cause.to_string();
        // Chains commonly restate the layer above them verbatim; repeating it
        // makes the line longer without making it say more.
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = cause.source();
    }
    out
}

/// A read that is one optional part of a larger answer, such as the trend on
/// an artist page or the loved flag beside a note.
///
/// `Ok(v)` becomes `Some(v)`. An error is logged at WARN, with its cause chain
/// and `what` naming the part that failed, and becomes `None`, so the caller
/// still decides what the missing part looks like (`.unwrap_or_default()`,
/// `.flatten()`, …) while the failure is no longer silent.
//
// Since 2026-11-15. 147 handler reads turned an error into an empty section
// with nothing logged (`.await.unwrap_or_default()` and friends), which is how
// artist_trend's SQLite-only `strftime` showed every Postgres user an artist
// page with no trend and no trace of why. A read the response is *about*
// should answer 500 instead (`server::read_failed`); this is only for the
// parts a page can honestly do without. Opengrep's
// `db-read-error-becomes-empty` rule keeps the bare forms out.
pub trait OrWarn<T> {
    fn or_warn(self, what: &str) -> Option<T>;
}

impl<T, E: std::error::Error + 'static> OrWarn<T> for Result<T, E> {
    fn or_warn(self, what: &str) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("{what}: {}", error_chain(&e));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);
    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref().map(|l| l as &(dyn std::error::Error + 'static))
        }
    }

    /// The whole point. `reqwest` says "error sending request for url (…)" for
    /// every transport fault there is; the answer to *which* one is one link
    /// down, and on 2026-08-27 it was the difference between chasing DNS for an
    /// afternoon and reading "unexpected eof" once.
    #[test]
    fn the_cause_is_printed_and_not_only_the_symptom() {
        let e = Layer(
            "error sending request for url (https://musicbrainz.org/ws/2/recording)",
            Some(Box::new(Layer("unexpected eof while reading", None))),
        );
        let s = error_chain(&e);
        assert!(s.contains("error sending request"), "keeps the operation");
        assert!(s.contains("unexpected eof while reading"), "and names the fault: {s}");
    }

    /// A chain that restates its parent should not print it twice — the point
    /// is a line somebody reads, not every link for its own sake.
    #[test]
    fn a_repeated_cause_is_not_restated() {
        let e = Layer("connect error", Some(Box::new(Layer("connect error", None))));
        assert_eq!(error_chain(&e), "connect error");
    }

    /// `or_warn` keeps the caller's own absence (`Ok(None)` stays `Some(None)`)
    /// apart from a failure, which is the whole point of not using `.ok()`.
    #[test]
    fn or_warn_keeps_an_absence_apart_from_a_failure() {
        let found: Result<Option<i64>, Layer> = Ok(Some(3));
        let absent: Result<Option<i64>, Layer> = Ok(None);
        let failed: Result<Option<i64>, Layer> = Err(Layer("pool timed out", None));
        assert_eq!(found.or_warn("t"), Some(Some(3)));
        assert_eq!(absent.or_warn("t"), Some(None));
        assert_eq!(failed.or_warn("t"), None);
    }
}
