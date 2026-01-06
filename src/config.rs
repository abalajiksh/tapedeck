use dotenv::dotenv;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub plex: PlexConfig,
    pub navidrome: NavidromeConfig,
    pub jellyfin: JellyfinConfig,
    pub lastfm: LastFmConfig,
    pub listenbrainz: ListenBrainzConfig,
}

#[derive(Debug, Clone)]
pub struct PlexConfig {
    pub enabled: bool,
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct NavidromeConfig {
    pub enabled: bool,
    pub db_path: String,
}

#[derive(Debug, Clone)]
pub struct JellyfinConfig {
    pub enabled: bool,
    pub url: String,
    pub user_id: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct LastFmConfig {
    pub enabled: bool,
    pub api_key: String,
    pub secret: String,
    pub session_key: String,
}

#[derive(Debug, Clone)]
pub struct ListenBrainzConfig {
    pub enabled: bool,
    pub token: String,
}

impl Config {
    pub fn from_env() -> Self {
        // Load .env file if it exists
        dotenv().ok();

        Config {
            plex: PlexConfig {
                enabled: get_env_bool("PLEX_ENABLED", false),
                url: get_env("PLEX_URL", "http://localhost:32400"),
                token: get_env("PLEX_TOKEN", ""),
            },
            navidrome: NavidromeConfig {
                enabled: get_env_bool("NAVIDROME_ENABLED", false),
                db_path: get_env("NAVIDROME_DB_PATH", "./navidrome.db"),
            },
            jellyfin: JellyfinConfig {
                enabled: get_env_bool("JELLYFIN_ENABLED", false),
                url: get_env("JELLYFIN_URL", "http://localhost:8096"),
                user_id: get_env("JELLYFIN_USER_ID", ""),
                token: get_env("JELLYFIN_TOKEN", ""),
            },
            lastfm: LastFmConfig {
                enabled: get_env_bool("LASTFM_ENABLED", false),
                api_key: get_env("LASTFM_API_KEY", ""),
                secret: get_env("LASTFM_SECRET", ""),
                session_key: get_env("LASTFM_SESSION_KEY", ""),
            },
            listenbrainz: ListenBrainzConfig {
                enabled: get_env_bool("LISTENBRAINZ_ENABLED", false),
                token: get_env("LISTENBRAINZ_TOKEN", ""),
            },
        }
    }
}

/// Helper to get an Env Var with a Default
fn get_env(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Helper to get a Boolean Env Var (e.g., "true" or "1")
fn get_env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(val) => {
            let v = val.to_lowercase();
            v == "true" || v == "1" || v == "yes"
        }
        Err(_) => default,
    }
}
