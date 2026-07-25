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
    pub listenbrainz_url: String,
    pub plex: PlexConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub trust_proxy: bool,
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
    listenbrainz: TomlListenBrainz,
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlListenBrainz { url: String }
impl Default for TomlListenBrainz {
    fn default() -> Self {
        Self { url: crate::sinks::LISTENBRAINZ_DEFAULT_URL.into() }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlServer {
    port: u16,
    host: String,
    trust_proxy: bool,
}
impl Default for TomlServer {
    fn default() -> Self { Self { port: 8080, host: "0.0.0.0".into(), trust_proxy: false } }
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
                trust_proxy: env_or_bool("TRUST_PROXY", t.server.trust_proxy),
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
            listenbrainz_url: env_or("LISTENBRAINZ_URL", t.listenbrainz.url),
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
