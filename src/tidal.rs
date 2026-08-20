//! TIDAL — a playlist target you reach over OAuth rather than over your LAN.
//!
//! Every other target in this application is a server the user already
//! configured as a *source*: a URL they typed and a credential that does not
//! expire. TIDAL breaks all three assumptions, which is why it lives in
//! `user_connections` (per-user, encrypted, keyed by service) rather than in
//! `user_sources`, and why the credential needs refreshing.
//!
//! **Authorization Code + PKCE, and the secret is optional.** PKCE exists so a
//! client can prove it started the flow without holding a secret, which is what
//! makes a self-hosted instance a legitimate public client. TIDAL issues a
//! secret anyway, so the token calls use HTTP Basic when one is configured and
//! fall back to a public-client body when it is not. The `code_verifier` is
//! sent either way — it is the half that is actually load-bearing.
//!
//! **What the access tier allows, checked against the spec rather than
//! assumed** (`tidal-api-oas.json`, 1.10.104): `playlists.write`,
//! `playlists.read`, `search.read` and `user.read` all require `THIRD_PARTY`,
//! which is the tier a self-registered app gets. `r_usr`/`w_usr` appear beside
//! `playlists.write` in the raw spec's security blocks and are `INTERNAL` — they
//! are legacy annotations, not a second gate, and the reference filtered to
//! Third-party lists both write endpoints.
//!
//! Two shapes worth not rediscovering, both from the spec:
//!
//! * **Search is `GET /searchResults?filter[query]=`**, not `/searchResults/{q}`
//!   — the `{id}` there is an opaque handle and a query in that position is
//!   rejected with `INVALID_RESOURCE_ID`.
//! * **Adding items is capped at 50 per request** (`maxItems` on the payload),
//!   so a full push is batched rather than sent in one call.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

use crate::playlists::{Candidate, PlaylistTarget, PlaylistTrack};

pub const AUTHORIZE_URL: &str = "https://login.tidal.com/authorize";
pub const TOKEN_URL: &str = "https://auth.tidal.com/v1/oauth2/token";
pub const API_BASE: &str = "https://openapi.tidal.com/v2";

/// The service key in `user_connections`.
///
/// `sinks::sinks_for_user` matches service names with an explicit `match`, so a
/// row under this key creates no scrobble sink — TIDAL is a playlist target and
/// nothing else.
pub const SERVICE: &str = "tidal";

/// What a push may ask for. Deliberately the minimum: `playlists.write` to
/// create one and `search.read` to find the tracks, with no `collection.*` and
/// no `playback`. A consent screen naming more than the feature needs is a
/// worse consent screen.
pub const SCOPES: &str = "playlists.write playlists.read search.read user.read";

/// Adding items is capped at 50 per request by the API's own payload schema.
const ITEMS_PER_REQUEST: usize = 50;

/// Refresh this long before the token actually expires, so a push that takes a
/// minute does not have it die halfway through.
const REFRESH_SKEW_SECS: i64 = 60;

/// Stored in `user_connections.credentials` as JSON, encrypted at rest by
/// `Database::seal` like every other replayed secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TidalCredentials {
    pub access_token: String,
    /// TIDAL may or may not rotate this. When a refresh comes back without one,
    /// the old value is kept rather than blanked — losing it means the user has
    /// to reconnect by hand.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds. Absent from very old rows, which are simply refreshed on
    /// first use.
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// The catalogue is licensed per country and the API asks for one on every
    /// read. Captured at connect time from the user's own account rather than
    /// guessed, because a wrong country silently returns an empty catalogue.
    #[serde(default)]
    pub country: Option<String>,
}

impl TidalCredentials {
    fn stale(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|e| now >= e - REFRESH_SKEW_SECS)
    }

    pub fn country_code(&self) -> &str {
        self.country.as_deref().unwrap_or("US")
    }
}

/// The operator's app registration. One per instance — each self-hoster
/// registers their own at developer.tidal.com, the same way Discogs and Apple
/// Music are configured.
#[derive(Debug, Clone)]
pub struct TidalApp {
    pub client_id: String,
    pub client_secret: Option<String>,
}

/// Where the stored registration lives. Encrypted at rest by `set_app_setting`
/// like every other credential this application replays, and server-wide rather
/// than per-user: it is the *operator's* app registration, and every user of the
/// instance connects their own account through it.
pub const CLIENT_ID_KEY: &str = "tidal_client_id";
pub const CLIENT_SECRET_KEY: &str = "tidal_client_secret";

impl TidalApp {
    /// The registration as configured in Settings, falling back to the
    /// environment.
    ///
    /// **Stored wins over environment**, which is the order Discogs already
    /// uses: what somebody typed into this instance's own settings screen is a
    /// more deliberate statement than a variable inherited from a shell.
    pub async fn resolve(db: &crate::db::Database) -> Option<Self> {
        let stored_id =
            db.get_app_setting(CLIENT_ID_KEY).await.ok().flatten().filter(|v| !v.trim().is_empty());
        if let Some(id) = stored_id {
            let secret = db
                .get_app_setting(CLIENT_SECRET_KEY)
                .await
                .ok()
                .flatten()
                .filter(|v| !v.trim().is_empty());
            return Some(Self { client_id: id.trim().to_string(), client_secret: secret });
        }
        Self::from_env()
    }

    /// Resolution order mirrors Discogs: the house-style name first, then the
    /// bare names TIDAL's own portal uses, which is what people already have
    /// exported when they come to configure this.
    pub fn from_env() -> Option<Self> {
        let id = first_env(&["TAPEDECK_TIDAL_CLIENT_ID", "TIDAL_CLIENTID", "TIDAL_CLIENT_ID"])?;
        let secret = first_env(&[
            "TAPEDECK_TIDAL_CLIENT_SECRET",
            "TIDAL_CLIENTSECRET",
            "TIDAL_CLIENT_SECRET",
        ]);
        Some(Self { client_id: id, client_secret: secret })
    }
}

fn first_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| std::env::var(n).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))
}

fn http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("tapedeck/", env!("CARGO_PKG_VERSION")))
        .build()
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(
    app: &TidalApp,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TidalCredentials, String> {
    let form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
        ("client_id", app.client_id.as_str()),
    ];
    post_token(app, form).await
}

/// Trade a refresh token for a new access token.
pub async fn refresh(app: &TidalApp, refresh_token: &str) -> Result<TidalCredentials, String> {
    let form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", app.client_id.as_str()),
    ];
    post_token(app, form).await
}

async fn post_token(app: &TidalApp, form: Vec<(&str, &str)>) -> Result<TidalCredentials, String> {
    let client = http().map_err(|e| e.to_string())?;
    let mut req = client.post(TOKEN_URL).form(&form);
    // A confidential client authenticates with Basic; a public one identifies
    // itself with `client_id` in the body, which is already there either way.
    if let Some(secret) = &app.client_secret {
        req = req.basic_auth(&app.client_id, Some(secret));
    }
    let resp = req.send().await.map_err(|e| format!("TIDAL token request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("TIDAL rejected the token request ({status}): {}", body.trim()));
    }
    let parsed: TokenResponse =
        serde_json::from_str(&body).map_err(|e| format!("TIDAL token response unreadable: {e}"))?;
    Ok(TidalCredentials {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at: parsed.expires_in.map(|s| now_secs() + s),
        country: None,
    })
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A connected TIDAL account, ready to be pushed to.
pub struct TidalClient {
    app: TidalApp,
    creds: TidalCredentials,
    client: reqwest::Client,
    /// Set when a refresh produced new tokens, so the caller can persist them.
    /// The client cannot write to the database itself — `Database` owns the
    /// encryption and ciphertext never leaves `db.rs`.
    refreshed: std::sync::Mutex<Option<TidalCredentials>>,
}

impl TidalClient {
    pub fn new(app: TidalApp, creds: TidalCredentials) -> Result<Self, String> {
        Ok(Self {
            app,
            creds,
            client: http().map_err(|e| e.to_string())?,
            refreshed: std::sync::Mutex::new(None),
        })
    }

    /// Tokens to persist, if the client had to refresh mid-push.
    pub fn take_refreshed(&self) -> Option<TidalCredentials> {
        self.refreshed.lock().ok().and_then(|mut g| g.take())
    }

    /// The token to send, refreshing first when it is about to expire.
    ///
    /// Refreshing *before* the call rather than retrying on a 401 is the
    /// cheaper half of the same idea: a push makes one request per track, and
    /// discovering expiry through a failure would cost a wasted round trip and
    /// a retry on whichever track happened to straddle the boundary.
    async fn token(&self) -> Result<String, String> {
        if !self.creds.stale(now_secs()) {
            return Ok(self.creds.access_token.clone());
        }
        let Some(rt) = self.creds.refresh_token.clone() else {
            return Err("TIDAL access token has expired and there is no refresh token — \
                        reconnect the account"
                .into());
        };
        let mut fresh = refresh(&self.app, &rt).await?;
        // TIDAL may or may not rotate the refresh token. Keeping the old one
        // when none comes back is what stops a silent disconnection.
        if fresh.refresh_token.is_none() {
            fresh.refresh_token = Some(rt);
        }
        fresh.country = self.creds.country.clone();
        let token = fresh.access_token.clone();
        if let Ok(mut g) = self.refreshed.lock() {
            *g = Some(fresh);
        }
        Ok(token)
    }

    async fn get(&self, url: &str) -> Result<serde_json::Value, String> {
        let token = self.token().await?;
        let resp = self
            .client
            .get(url)
            .bearer_auth(token)
            .header("accept", "application/vnd.api+json")
            .send()
            .await
            .map_err(|e| format!("TIDAL request failed: {e}"))?;
        read_json(resp).await
    }

    async fn post(&self, url: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
        let token = self.token().await?;
        let resp = self
            .client
            .post(url)
            .bearer_auth(token)
            .header("accept", "application/vnd.api+json")
            .header("content-type", "application/vnd.api+json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("TIDAL request failed: {e}"))?;
        read_json(resp).await
    }
}

async fn read_json(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "TIDAL returned {status}: {}",
            body.trim().chars().take(300).collect::<String>()
        ));
    }
    if body.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&body).map_err(|e| format!("TIDAL response unreadable: {e}"))
}

/// Pull `included` resources of one type into a map of id → resource.
fn included_of<'a>(
    doc: &'a serde_json::Value,
    kind: &str,
) -> std::collections::HashMap<&'a str, &'a serde_json::Value> {
    doc.get("included")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|r| r.get("type").and_then(|t| t.as_str()) == Some(kind))
                .filter_map(|r| r.get("id").and_then(|i| i.as_str()).map(|id| (id, r)))
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl PlaylistTarget for TidalClient {
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String> {
        // Artist *and* title, because a bare title buries the right hit on a
        // catalogue this size — the same reasoning the media-server targets use.
        let query = format!("{} {}", want.artist, want.title);
        let url = format!(
            "{API_BASE}/searchResults?filter%5Bquery%5D={}&countryCode={}&include=tracks.artists",
            urlencoding(&query),
            self.creds.country_code()
        );
        let doc = self.get(&url).await?;

        let artists = included_of(&doc, "artists");
        let mut out = Vec::new();
        for track in doc
            .get("included")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter(|r| r.get("type").and_then(|t| t.as_str()) == Some("tracks"))
        {
            let Some(id) = track.get("id").and_then(|v| v.as_str()) else { continue };
            let attrs = track.get("attributes").cloned().unwrap_or_default();
            let title = attrs.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            // The artist comes back as a relationship, resolved through
            // `included` — which is why the search asks for `tracks.artists` in
            // one round trip rather than a lookup per hit.
            let artist = track
                .get("relationships")
                .and_then(|r| r.get("artists"))
                .and_then(|a| a.get("data"))
                .and_then(|d| d.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("id"))
                .and_then(|i| i.as_str())
                .and_then(|id| artists.get(id))
                .and_then(|a| a.get("attributes"))
                .and_then(|a| a.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string();
            out.push(Candidate { id: id.to_string(), title, artist, album: None });
        }
        debug!("TIDAL search for '{}' returned {} candidates", query, out.len());
        Ok(out)
    }

    async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<String, String> {
        // **`UNLISTED`, never `PUBLIC`.** This instance is private and
        // invite-only by design, and a playlist Tapedeck creates on somebody's
        // behalf must not be published to the world as a side effect of pushing
        // it. The two are the only values the API accepts.
        let created = self
            .post(
                &format!("{API_BASE}/playlists?countryCode={}", self.creds.country_code()),
                serde_json::json!({
                    "data": {
                        "type": "playlists",
                        "attributes": { "name": name, "accessType": "UNLISTED" }
                    }
                }),
            )
            .await?;

        let playlist_id = created
            .get("data")
            .and_then(|d| d.get("id"))
            .and_then(|i| i.as_str())
            .ok_or("TIDAL created a playlist but did not say which")?
            .to_string();

        // Capped at 50 per request by the API's own payload schema, so a full
        // push is two calls rather than one rejected one.
        for chunk in ids.chunks(ITEMS_PER_REQUEST) {
            let data: Vec<_> =
                chunk.iter().map(|id| serde_json::json!({ "id": id, "type": "tracks" })).collect();
            if let Err(e) = self
                .post(
                    &format!(
                        "{API_BASE}/playlists/{playlist_id}/relationships/items?countryCode={}",
                        self.creds.country_code()
                    ),
                    serde_json::json!({ "data": data }),
                )
                .await
            {
                // The playlist exists and holds whatever landed before this.
                // Reporting the failure beats leaving the user to discover a
                // short playlist on their own.
                warn!("TIDAL playlist {} is short: {}", playlist_id, e);
                return Err(format!("Created the playlist but could not add every track: {e}"));
            }
        }
        Ok(playlist_id)
    }
}

/// So a push can hold a handle for persisting refreshed tokens while also
/// handing the trait object to the generic push loop.
#[async_trait::async_trait]
impl PlaylistTarget for std::sync::Arc<TidalClient> {
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String> {
        (**self).search_tracks(want).await
    }
    async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<String, String> {
        (**self).create_playlist(name, ids).await
    }
}

/// Percent-encode a query value. Small enough not to earn a dependency, and
/// deliberately encodes `&`, `=` and `+` — an artist like "Simon & Garfunkel"
/// would otherwise truncate the query at the ampersand.
pub fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_encoded_so_an_ampersand_cannot_truncate_it() {
        assert_eq!(urlencoding("Simon & Garfunkel"), "Simon%20%26%20Garfunkel");
        assert_eq!(urlencoding("Cœur de pirate"), "C%C5%93ur%20de%20pirate");
        assert_eq!(urlencoding("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn a_token_about_to_expire_is_stale_before_it_actually_is() {
        let now = now_secs();
        let fresh = TidalCredentials {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: Some(now + 3600),
            country: None,
        };
        assert!(!fresh.stale(now));

        // Inside the skew: a push that takes a minute must not have the token
        // die halfway through it.
        let nearly = TidalCredentials { expires_at: Some(now + 30), ..fresh.clone() };
        assert!(nearly.stale(now));

        // No expiry recorded is not an assertion that it never expires — it is
        // an old row, refreshed on first use rather than assumed good.
        let unknown = TidalCredentials { expires_at: None, ..fresh.clone() };
        assert!(!unknown.stale(now), "nothing to go on, so let the request decide");
    }

    /// The `included` walk, against the live catalogue.
    ///
    /// Ignored by default because it needs credentials and the network, but it
    /// is the one thing unit tests cannot cover: JSON:API returns the artist as
    /// a *relationship* resolved through `included`, and getting that wrong
    /// yields candidates with an empty artist — which `playlists::pick` then
    /// rejects on its hard gate, so a push silently matches nothing while every
    /// request returns 200.
    ///
    /// `cargo test tidal_live_search -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs TIDAL_CLIENTID/TIDAL_CLIENTSECRET and the network"]
    async fn tidal_live_search() {
        let app = TidalApp::from_env().expect("set TIDAL_CLIENTID and TIDAL_CLIENTSECRET");
        // Client credentials are enough for the catalogue half; only the
        // playlist half needs a user token.
        let client = http().unwrap();
        let resp = client
            .post(TOKEN_URL)
            .basic_auth(&app.client_id, app.client_secret.as_ref())
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .unwrap();
        let tok: TokenResponse = resp.json().await.unwrap();

        let creds = TidalCredentials {
            access_token: tok.access_token,
            refresh_token: None,
            expires_at: None,
            country: Some("US".into()),
        };
        let tidal = TidalClient::new(app, creds).unwrap();
        let hits = tidal
            .search_tracks(&PlaylistTrack {
                title: "Windowlicker".into(),
                artist: "Aphex Twin".into(),
                album: None,
            })
            .await
            .expect("search should succeed");

        assert!(!hits.is_empty(), "the catalogue should know Aphex Twin");
        for h in hits.iter().take(5) {
            println!("  {} | {} — {}", h.id, h.artist, h.title);
        }
        assert!(
            hits.iter().any(|h| !h.artist.is_empty()),
            "the artist must be resolved out of `included`, or the hard gate rejects everything"
        );
        assert!(
            hits.iter().any(|h| h.title.to_lowercase().contains("windowlicker")),
            "the track we asked for should be in there somewhere"
        );
    }

    #[test]
    fn the_country_falls_back_rather_than_failing() {
        let c = TidalCredentials {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: None,
            country: None,
        };
        assert_eq!(c.country_code(), "US");
        let c = TidalCredentials { country: Some("DE".into()), ..c };
        assert_eq!(c.country_code(), "DE");
    }
}
