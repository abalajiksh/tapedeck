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
    /// Where the credential-encryption key lives (never inside the database).
    pub secret_key_path: String,
    pub plex: PlexConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub trust_proxy: bool,
    /// The address this instance is reached at from outside, when it differs
    /// from what the origin can see — `https://tapedeck.example.com`.
    ///
    /// Everything else in this file is something the server can determine or
    /// default sensibly. This it genuinely cannot: behind a tunnel or a reverse
    /// proxy the origin is handed plain HTTP on some internal name, and the
    /// only party who knows the public address is the operator.
    ///
    /// It matters far more than it looks. The OAuth metadata documents publish
    /// this as the issuer and as every endpoint URL, and an authorization
    /// server that advertises `http://` is one a conformant client will refuse
    /// outright — which presents as a connector that mysteriously has no tools.
    /// Empty means "work it out", which `server::mcp::public_origin` then does.
    pub public_url: String,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub sqlite_path: String,
    pub scrobble_db_url: String,
    /// The Every Noise genre map. Its own file because it is bulk reference
    /// data acquired from a third party and rebuildable at any time — putting
    /// it in the scrobble database would bloat every backup with data that is
    /// not the user's.
    pub everynoise_path: String,
    /// Where uploaded sleeve and booklet scans are written.
    ///
    /// Files, not blobs, because a booklet is twenty pages and a shelf of a
    /// hundred records would put gigabytes of image data through every
    /// `VACUUM INTO` backup. The consequence is the honest one and is stated in
    /// the UI: `GET /api/v1/backup` does **not** carry them, and the shelf
    /// export is what does.
    pub uploads_dir: String,
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
    pub api_base_url: String,
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
struct TomlListenBrainz {
    url: String,
}
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
    public_url: String,
    secret_key_path: String,
}
impl Default for TomlServer {
    fn default() -> Self {
        Self {
            port: 8080,
            host: "0.0.0.0".into(),
            trust_proxy: false,
            public_url: String::new(),
            secret_key_path: "./tapedeck.key".into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct TomlDatabase {
    sqlite_path: String,
    scrobble_db_url: String,
    everynoise_path: String,
    uploads_dir: String,
}
impl Default for TomlDatabase {
    fn default() -> Self {
        Self {
            sqlite_path: "./tapedeck.db".into(),
            scrobble_db_url: "sqlite:scrobbles.db?mode=rwc".into(),
            everynoise_path: "sqlite:everynoise.db?mode=rwc".into(),
            uploads_dir: "./uploads".into(),
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
    /// Where the web service lives. Configurable so an instance can be pointed
    /// at its own MusicBrainz mirror — and so the breaker can be exercised
    /// against an address that is deliberately not answering.
    api_url: String,
    /// A full override. `None` means "build one from `contact`", which is what
    /// keeps the version from going stale in a config file nobody rereads.
    user_agent: Option<String>,
    contact: String,
    rate_limit_per_second: u32,
    postgres_url: Option<String>,
    postgres_enabled: bool,
}
impl Default for TomlMusicBrainz {
    fn default() -> Self {
        Self {
            api_url: "https://musicbrainz.org/ws/2".into(),
            // `None` means: build it from `CARGO_PKG_*` and `contact`, rather
            // than writing it out. It had been written out four times across
            // the tree — `Tapedeck/1.0`, `/0.5.0`, `/0.10.0`, `/0.96.0` — on an
            // application at 0.101.0, and it is the string MetaBrainz judge us
            // by.
            user_agent: None,
            contact: String::new(),
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
            enabled: false,
            url: "http://localhost:32400".into(),
            token: String::new(),
            users_allow: Vec::new(),
            users_block: Vec::new(),
            devices_allow: Vec::new(),
            devices_block: Vec::new(),
            libraries_allow: Vec::new(),
            libraries_block: Vec::new(),
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
                public_url: env_or("PUBLIC_URL", t.server.public_url)
                    .trim()
                    .trim_end_matches('/')
                    .to_string(),
            },
            database: DatabaseConfig {
                sqlite_path: env_or("SQLITE_DB_PATH", t.database.sqlite_path),
                scrobble_db_url: env_or("DATABASE_URL", t.database.scrobble_db_url),
                everynoise_path: env_or("EVERYNOISE_DB_PATH", t.database.everynoise_path),
                uploads_dir: env_or("TAPEDECK_UPLOADS_DIR", t.database.uploads_dir),
            },
            logging: LoggingConfig {
                level: env_or("RUST_LOG", t.logging.level),
                file_enabled: env_or_bool("ENABLE_FILE_LOGGING", t.logging.file_enabled),
                console_enabled: env_or_bool("ENABLE_CONSOLE_LOGGING", t.logging.console_enabled),
                dir: env_or("LOG_DIR", t.logging.dir),
            },
            listenbrainz_url: env_or("LISTENBRAINZ_URL", t.listenbrainz.url),
            secret_key_path: env_or("TAPEDECK_SECRET_KEY_PATH", t.server.secret_key_path),
            musicbrainz: MusicBrainzConfig {
                api_base_url: env_or("MUSICBRAINZ_API_URL", t.musicbrainz.api_url),
                // Two knobs, and `contact` is the one to reach for. MusicBrainz
                // asks for a product, a version and somebody to contact; only
                // the last of those is the operator's to know, so making them
                // retype the other two is how a template ships unfilled and an
                // instance gets blocked. A full `MUSICBRAINZ_USER_AGENT` still
                // wins outright, for anyone who wants to name themselves.
                user_agent: {
                    let contact = env_or("MUSICBRAINZ_CONTACT", t.musicbrainz.contact);
                    let built = t
                        .musicbrainz
                        .user_agent
                        .unwrap_or_else(|| crate::musicbrainz::user_agent_for(Some(&contact)));
                    env_or("MUSICBRAINZ_USER_AGENT", built)
                },
                rate_limit_per_second: env_or_parse(
                    "MUSICBRAINZ_RATE_LIMIT",
                    t.musicbrainz.rate_limit_per_second,
                ),
                postgres_url: env::var("MUSICBRAINZ_POSTGRES_URL")
                    .ok()
                    .or(t.musicbrainz.postgres_url),
                enable_postgres: env_or_bool(
                    "MUSICBRAINZ_POSTGRES_ENABLED",
                    t.musicbrainz.postgres_enabled,
                ),
            },
            plex: PlexConfig {
                enabled: env_or_bool("PLEX_ENABLED", t.sources.plex.enabled),
                url: env_or("PLEX_URL", t.sources.plex.url),
                token: env_or("PLEX_TOKEN", t.sources.plex.token),
                users_allow: env_or_list("PLEX_USERS_ALLOW", t.sources.plex.users_allow),
                users_block: env_or_list("PLEX_USERS_BLOCK", t.sources.plex.users_block),
                devices_allow: env_or_list("PLEX_DEVICES_ALLOW", t.sources.plex.devices_allow),
                devices_block: env_or_list("PLEX_DEVICES_BLOCK", t.sources.plex.devices_block),
                libraries_allow: env_or_list(
                    "PLEX_LIBRARIES_ALLOW",
                    t.sources.plex.libraries_allow,
                ),
                libraries_block: env_or_list(
                    "PLEX_LIBRARIES_BLOCK",
                    t.sources.plex.libraries_block,
                ),
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
        Ok(val) => {
            let v = val.to_lowercase();
            v == "true" || v == "1" || v == "yes"
        }
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
