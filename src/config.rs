use dotenv::dotenv;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub plex: PlexConfig,
    pub navidrome: NavidromeConfig,
    pub jellyfin: JellyfinConfig,
    pub lastfm: LastFmConfig,
    pub listenbrainz: ListenBrainzConfig,
    pub musicbrainz: MusicBrainzConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone)]
pub struct PlexConfig {
    pub enabled: bool,
    pub url: String,
    pub token: String,
    // Filtering options
    pub users_allow: Vec<String>,
    pub users_block: Vec<String>,
    pub devices_allow: Vec<String>,
    pub devices_block: Vec<String>,
    pub libraries_allow: Vec<String>,
    pub libraries_block: Vec<String>,
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
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct MusicBrainzConfig {
    pub user_agent: String,
    pub rate_limit_per_second: u32,
    pub postgres_url: Option<String>,
    pub enable_postgres: bool,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub sqlite_path: String,
}

impl Config {
    pub fn from_env() -> Self {
        // Load .env file if it exists
        dotenv().ok();

        // Determine ListenBrainz URL logic
        // Default is PRODUCTION (true) unless explicitly set to "false"
        let is_prod = get_env_bool("IS_PRODUCTION", true);
        let lb_url = if is_prod {
            "https://api.listenbrainz.org".to_string()
        } else {
            // In DEV mode, use custom URL or fallback to localhost mock
            get_env("LISTENBRAINZ_URL", "http://localhost:8080")
        };

        // MusicBrainz PostgreSQL URL (optional)
        let mb_pg_url = env::var("MUSICBRAINZ_POSTGRES_URL").ok();
        let mb_pg_enabled = mb_pg_url.is_some() && get_env_bool("MUSICBRAINZ_POSTGRES_ENABLED", false);

        Config {
            plex: PlexConfig {
                enabled: get_env_bool("PLEX_ENABLED", false),
                url: get_env("PLEX_URL", "http://localhost:32400"),
                token: get_env("PLEX_TOKEN", ""),
                users_allow: parse_list(&get_env("PLEX_USERS_ALLOW", "")),
                users_block: parse_list(&get_env("PLEX_USERS_BLOCK", "")),
                devices_allow: parse_list(&get_env("PLEX_DEVICES_ALLOW", "")),
                devices_block: parse_list(&get_env("PLEX_DEVICES_BLOCK", "")),
                libraries_allow: parse_list(&get_env("PLEX_LIBRARIES_ALLOW", "")),
                libraries_block: parse_list(&get_env("PLEX_LIBRARIES_BLOCK", "")),
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
                base_url: lb_url,
            },
            musicbrainz: MusicBrainzConfig {
                user_agent: get_env(
                    "MUSICBRAINZ_USER_AGENT",
                    &format!("Tapedeck/0.3.7 ( {} )", get_env("CONTACT_EMAIL", "contact@example.com")),
                ),
                rate_limit_per_second: get_env("MUSICBRAINZ_RATE_LIMIT", "1")
                    .parse()
                    .unwrap_or(1),
                postgres_url: mb_pg_url,
                enable_postgres: mb_pg_enabled,
            },
            database: DatabaseConfig {
                sqlite_path: get_env("SQLITE_DB_PATH", "./tapedeck.db"),
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

/// Parse comma-separated list from env var
fn parse_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}
