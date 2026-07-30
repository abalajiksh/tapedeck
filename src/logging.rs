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
        self.filter
            .with_current(|f| f.to_string())
            .unwrap_or_else(|_| "unknown".to_string())
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

    let handle = LogLevelHandle {
        filter: reload_handle,
    };

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
