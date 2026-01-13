use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::Level;
use tracing_subscriber::{
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
    Layer,
    Registry,
};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use std::path::Path;

/// Shared handle for runtime log level control
#[derive(Clone)]
pub struct LogLevelHandle {
    filter: Arc<RwLock<EnvFilter>>,
}

impl LogLevelHandle {
    /// Update the log level at runtime
    pub async fn set_level(&self, level: &str) -> Result<(), String> {
        let new_filter = match level.to_uppercase().as_str() {
            "TRACE" => EnvFilter::new("trace"),
            "DEBUG" => EnvFilter::new("debug"),
            "INFO" => EnvFilter::new("info"),
            "WARN" => EnvFilter::new("warn"),
            "ERROR" => EnvFilter::new("error"),
            _ => return Err(format!("Invalid log level: {}", level)),
        };

        let mut filter = self.filter.write().await;
        *filter = new_filter;
        Ok(())
    }

    /// Get the current log level
    pub async fn get_level(&self) -> String {
        let filter = self.filter.read().await;
        format!("{:?}", filter)
    }
}

/// Initialize logging with file output and runtime level control
/// 
/// # Arguments
/// * `log_dir` - Directory for log files (default: "./logs")
/// * `enable_file_logging` - Whether to write logs to files
/// * `enable_console` - Whether to write logs to stdout/stderr
/// 
/// # Returns
/// A handle to control log levels at runtime
pub fn init_logging(
    log_dir: Option<&str>,
    enable_file_logging: bool,
    enable_console: bool,
) -> Result<LogLevelHandle, Box<dyn std::error::Error>> {
    // Get initial log level from environment or default to INFO
    let initial_level = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info".to_string());
    
    let filter = Arc::new(RwLock::new(
        EnvFilter::try_new(&initial_level)?
    ));

    let handle = LogLevelHandle {
        filter: filter.clone(),
    };

    // Create the subscriber with layers
    let subscriber = Registry::default();

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
