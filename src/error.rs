use thiserror::Error;

#[derive(Debug, Error)]
pub enum TapedeckError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("MusicBrainz lookup failed: {0}")]
    MusicBrainz(String),

    #[error("Source error ({origin}): {message}")]
    Source { origin: String, message: String },

    #[error("Sink error ({sink}): {message}")]
    Sink { sink: String, message: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for TapedeckError {
    fn from(e: anyhow::Error) -> Self {
        TapedeckError::Other(e.to_string())
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, TapedeckError>;
