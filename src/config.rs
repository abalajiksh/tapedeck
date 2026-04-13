use serde::Deserialize;
use std::env;
use std::path::Path;
use tracing::info;

/// Top-level configuration loaded from tapedeck.toml + env overrides.
#[derive(Debug, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub logging: LoggingConfig,
    pub musicbrainz: MusicBrainzConfig,
    pub plex: PlexConfig,
    pub navidrome: NavidromeConfig,
    pub jellyfin: JellyfinConfig,
    pub listenbrainz: ListenBrainzConfig,
    pub lastfm: LastFmConfig,
    pub librefm: LibreFmConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub sqlite_path: String,
    pub scrobble_db_url: String,
}

#[derive(Debug, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub file_enabled: bool,
    pub console_enabled: bool,
    pub dir: String,
}

#[derive(Debug, Clone)]
pub struct MusicBrainzConfig {
    pub user_agent: String,
    pub rate_limit_per_second: u32,
    pub postgres_url: Option<String>,
    pub enable_postgres: bool,
}

#[derive(Debug, Clone)]
pub struct PlexConfig {
    pub enabled: bool,
    pub url: String,
    pub token: String,
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
pub struct ListenBrainzConfig {
    pub enabled: bool,
    pub token: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct LastFmConfig {
    pub enabled: bool,
    pub api_key: String,
    pub secret: String,
    pub session_key: String,
}

#[derive(Debug, Clone)]
pub struct LibreFmConfig {
    pub enabled: bool,
    pub api_key: String,
    pub secret: String,
    pub session_key: String,
    pub base_url: String,
}

// ════════════════════════════════════════════════════════════
//  TOML deserialization structs (with defaults)
// ════════════════════════════════════════════════════════════

#[derive(Deserialize, Default)]
#[serde(default)]
struct TomlConfig {
    server: TomlServer,
    database: TomlDatabase,
    logging: TomlLogging,
    musicbrainz: TomlMusicBrainz,
    sources: TomlSources,
    sinks: TomlSinks,
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlServer {
    port: u16,
    host: String,
}
impl Default for TomlServer {
    fn default() -> Self { Self { port: 8080, host: "0.0.0.0".into() } }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlDatabase {
    sqlite_path: String,
    scrobble_db_url: String,
}
impl Default for TomlDatabase {
    fn default() -> Self {
        Self {
            sqlite_path: "./tapedeck.db".into(),
            scrobble_db_url: "sqlite:scrobbles.db?mode=rwc".into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlLogging {
    level: String,
    file_enabled: bool,
    console_enabled: bool,
    dir: String,
}
impl Default for TomlLogging {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file_enabled: true,
            console_enabled: true,
            dir: "./logs".into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlMusicBrainz {
    user_agent: String,
    rate_limit_per_second: u32,
    postgres_url: Option<String>,
    postgres_enabled: bool,
}
impl Default for TomlMusicBrainz {
    fn default() -> Self {
        Self {
            user_agent: "Tapedeck/0.5.0 ( contact@example.com )".into(),
            rate_limit_per_second: 1,
            postgres_url: None,
            postgres_enabled: false,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TomlSources {
    plex: TomlPlex,
    navidrome: TomlNavidrome,
    jellyfin: TomlJellyfin,
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlPlex {
    enabled: bool,
    url: String,
    token: String,
    users_allow: Vec<String>,
    users_block: Vec<String>,
    devices_allow: Vec<String>,
    devices_block: Vec<String>,
    libraries_allow: Vec<String>,
    libraries_block: Vec<String>,
}
impl Default for TomlPlex {
    fn default() -> Self {
        Self {
            enabled: false, url: "http://localhost:32400".into(), token: String::new(),
            users_allow: Vec::new(), users_block: Vec::new(),
            devices_allow: Vec::new(), devices_block: Vec::new(),
            libraries_allow: Vec::new(), libraries_block: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlNavidrome { enabled: bool, db_path: String }
impl Default for TomlNavidrome {
    fn default() -> Self { Self { enabled: false, db_path: "./navidrome.db".into() } }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlJellyfin { enabled: bool, url: String, user_id: String, token: String }
impl Default for TomlJellyfin {
    fn default() -> Self {
        Self { enabled: false, url: "http://localhost:8096".into(), user_id: String::new(), token: String::new() }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TomlSinks {
    listenbrainz: TomlListenBrainz,
    lastfm: TomlLastFm,
    librefm: TomlLibreFm,
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlListenBrainz { enabled: bool, token: String, base_url: String }
impl Default for TomlListenBrainz {
    fn default() -> Self {
        Self { enabled: false, token: String::new(), base_url: "https://api.listenbrainz.org".into() }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlLastFm { enabled: bool, api_key: String, secret: String, session_key: String }
impl Default for TomlLastFm {
    fn default() -> Self {
        Self { enabled: false, api_key: String::new(), secret: String::new(), session_key: String::new() }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlLibreFm { enabled: bool, api_key: String, secret: String, session_key: String, base_url: String }
impl Default for TomlLibreFm {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(), secret: String::new(), session_key: String::new(),
            base_url: "https://libre.fm/2.0/".into(),
        }
    }
}

// ════════════════════════════════════════════════════════════
//  Loading logic
// ════════════════════════════════════════════════════════════

impl Config {
    /// Load configuration with priority: env vars > .env file > tapedeck.toml > defaults.
    pub fn load() -> Self {
        dotenv::dotenv().ok();

        let toml_path = env::var("TAPEDECK_CONFIG").unwrap_or_else(|_| "tapedeck.toml".into());

        let toml_cfg: TomlConfig = if Path::new(&toml_path).exists() {
            info!("📄 Loading config from {}", toml_path);
            match std::fs::read_to_string(&toml_path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        eprintln!("⚠ Failed to parse {}: {}. Using defaults.", toml_path, e);
                        TomlConfig::default()
                    }
                },
                Err(e) => {
                    eprintln!("⚠ Failed to read {}: {}. Using defaults.", toml_path, e);
                    TomlConfig::default()
                }
            }
        } else {
            TomlConfig::default()
        };

        let t = toml_cfg;

        Config {
            server: ServerConfig {
                port: env_or_parse("PORT", env_or_parse("ADMIN_PORT", t.server.port)),
                host: env_or("HOST", t.server.host),
            },
            database: DatabaseConfig {
                sqlite_path: env_or("SQLITE_DB_PATH", t.database.sqlite_path),
                scrobble_db_url: env_or("DATABASE_URL", t.database.scrobble_db_url),
            },
            logging: LoggingConfig {
                level: env_or("RUST_LOG", t.logging.level),
                file_enabled: env_or_bool("ENABLE_FILE_LOGGING", t.logging.file_enabled),
                console_enabled: env_or_bool("ENABLE_CONSOLE_LOGGING", t.logging.console_enabled),
                dir: env_or("LOG_DIR", t.logging.dir),
            },
            musicbrainz: MusicBrainzConfig {
                user_agent: env_or("MUSICBRAINZ_USER_AGENT", t.musicbrainz.user_agent),
                rate_limit_per_second: env_or_parse("MUSICBRAINZ_RATE_LIMIT", t.musicbrainz.rate_limit_per_second),
                postgres_url: env::var("MUSICBRAINZ_POSTGRES_URL").ok().or(t.musicbrainz.postgres_url),
                enable_postgres: env_or_bool("MUSICBRAINZ_POSTGRES_ENABLED", t.musicbrainz.postgres_enabled),
            },
            plex: PlexConfig {
                enabled: env_or_bool("PLEX_ENABLED", t.sources.plex.enabled),
                url: env_or("PLEX_URL", t.sources.plex.url),
                token: env_or("PLEX_TOKEN", t.sources.plex.token),
                users_allow: env_or_list("PLEX_USERS_ALLOW", t.sources.plex.users_allow),
                users_block: env_or_list("PLEX_USERS_BLOCK", t.sources.plex.users_block),
                devices_allow: env_or_list("PLEX_DEVICES_ALLOW", t.sources.plex.devices_allow),
                devices_block: env_or_list("PLEX_DEVICES_BLOCK", t.sources.plex.devices_block),
                libraries_allow: env_or_list("PLEX_LIBRARIES_ALLOW", t.sources.plex.libraries_allow),
                libraries_block: env_or_list("PLEX_LIBRARIES_BLOCK", t.sources.plex.libraries_block),
            },
            navidrome: NavidromeConfig {
                enabled: env_or_bool("NAVIDROME_ENABLED", t.sources.navidrome.enabled),
                db_path: env_or("NAVIDROME_DB_PATH", t.sources.navidrome.db_path),
            },
            jellyfin: JellyfinConfig {
                enabled: env_or_bool("JELLYFIN_ENABLED", t.sources.jellyfin.enabled),
                url: env_or("JELLYFIN_URL", t.sources.jellyfin.url),
                user_id: env_or("JELLYFIN_USER_ID", t.sources.jellyfin.user_id),
                token: env_or("JELLYFIN_TOKEN", t.sources.jellyfin.token),
            },
            listenbrainz: ListenBrainzConfig {
                enabled: env_or_bool("LISTENBRAINZ_ENABLED", t.sinks.listenbrainz.enabled),
                token: env_or("LISTENBRAINZ_TOKEN", t.sinks.listenbrainz.token),
                base_url: env_or("LISTENBRAINZ_URL",
                    env_or("LISTENBRAINZ_BASE_URL", t.sinks.listenbrainz.base_url)),
            },
            lastfm: LastFmConfig {
                enabled: env_or_bool("LASTFM_ENABLED", t.sinks.lastfm.enabled),
                api_key: env_or("LASTFM_API_KEY", t.sinks.lastfm.api_key),
                secret: env_or("LASTFM_SECRET", t.sinks.lastfm.secret),
                session_key: env_or("LASTFM_SESSION_KEY", t.sinks.lastfm.session_key),
            },
            librefm: LibreFmConfig {
                enabled: env_or_bool("LIBREFM_ENABLED", t.sinks.librefm.enabled),
                api_key: env_or("LIBREFM_API_KEY", t.sinks.librefm.api_key),
                secret: env_or("LIBREFM_SECRET", t.sinks.librefm.secret),
                session_key: env_or("LIBREFM_SESSION_KEY", t.sinks.librefm.session_key),
                base_url: env_or("LIBREFM_BASE_URL", t.sinks.librefm.base_url),
            },
        }
    }

    /// Backward-compatible alias.
    pub fn from_env() -> Self {
        Self::load()
    }
}

// ════════════════════════════════════════════════════════════
//  Env override helpers
// ════════════════════════════════════════════════════════════

fn env_or(key: &str, default: String) -> String {
    env::var(key).unwrap_or(default)
}

fn env_or_parse<T: std::str::FromStr + Copy>(key: &str, default: T) -> T {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_or_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(val) => { let v = val.to_lowercase(); v == "true" || v == "1" || v == "yes" }
        Err(_) => default,
    }
}

fn env_or_list(key: &str, default: Vec<String>) -> Vec<String> {
    match env::var(key) {
        Ok(val) if !val.is_empty() => {
            val.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
        }
        _ => default.into_iter().map(|s| s.to_lowercase()).collect(),
    }
}
