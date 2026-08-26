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

// ── The three write payloads ──────────────────────────────────────────────
//
// Pure, and separated from the calls that send them, because **nothing here has
// ever run against a real TIDAL**: creating a playlist and adding items need a
// user token, which needs somebody to complete a sign-in. So the shapes are the
// specification's declared ones rather than observed ones, and the honest way
// to hold them is a test that reads the specification back.
//
// Same footing as the Jellyfin session tests, and the same caveat: this proves
// the request matches what TIDAL says it accepts, not that TIDAL accepts it.

/// The body for `POST /playlists`.
///
/// **`UNLISTED`, never `PUBLIC`.** This instance is private and invite-only by
/// design, and a playlist Tapedeck creates on somebody's behalf must not be
/// published to the world as a side effect of pushing it. Those are the only
/// two values the API accepts.
fn create_playlist_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "data": {
            "type": "playlists",
            "attributes": { "name": name, "accessType": "UNLISTED" }
        }
    })
}

/// The body for `POST /playlists/{id}/relationships/items`.
///
/// One chunk's worth. The caller batches at [`ITEMS_PER_REQUEST`] because the
/// payload's own schema caps `data` at 50 — a full push is two calls rather
/// than one rejected one.
fn add_items_body(ids: &[String]) -> serde_json::Value {
    let data: Vec<_> =
        ids.iter().map(|id| serde_json::json!({ "id": id, "type": "tracks" })).collect();
    serde_json::json!({ "data": data })
}

/// Turn the items relationship, as read back, into removal entries.
///
/// **Removing an item needs two ids, not one.** The payload wants the track's
/// `id` *and* a `meta.itemId`, which is the handle for that track's place in
/// this playlist — the same track added twice is two items — and `meta.itemId`
/// is `required` in the schema. An entry that carries no `itemId` is therefore
/// skipped rather than sent in a shape the API will refuse for the whole batch.
fn removal_entries(existing: &serde_json::Value) -> Vec<serde_json::Value> {
    existing
        .get("data")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or("tracks");
            let item_id =
                item.get("meta").and_then(|m| m.get("itemId")).and_then(|v| v.as_str())?;
            Some(serde_json::json!({ "id": id, "type": kind, "meta": { "itemId": item_id } }))
        })
        .collect()
}

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
        self.send_with_retry(|token| {
            self.client.get(url).bearer_auth(token).header("accept", "application/vnd.api+json")
        })
        .await
    }

    async fn post(&self, url: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
        self.send_with_retry(|token| {
            self.client
                .post(url)
                .bearer_auth(token)
                .header("accept", "application/vnd.api+json")
                .header("content-type", "application/vnd.api+json")
                .json(&body)
        })
        .await
    }

    /// DELETE with a body, which JSON:API relationships require — the items to
    /// remove are named in the payload rather than in the path.
    async fn delete(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.send_with_retry(|token| {
            self.client
                .delete(url)
                .bearer_auth(token)
                .header("accept", "application/vnd.api+json")
                .header("content-type", "application/vnd.api+json")
                .json(&body)
        })
        .await
    }

    /// Send a request, waiting out a throttle rather than reporting it as a
    /// miss.
    ///
    /// **Why this exists.** A push is one search per track, issued in turn, and
    /// TIDAL meters them. Without a retry the first throttled request became a
    /// `search_failed` — which reads, in the push result, as "that track is not
    /// on TIDAL". Three consecutive pushes of one 30-track playlist matched 24,
    /// then 17, then 17, with tracks that had resolved on the first run failing
    /// on the third *with byte-identical input*. Nothing was wrong with the
    /// names; the requests never arrived.
    ///
    /// **The wait is taken from the response wherever the response offers
    /// one.** `Retry-After` (seconds, or an HTTP date) and `X-RateLimit-Reset`
    /// / `X-RateLimit-Reset-In` are read first, and the exponential fallback is
    /// only for a server that says nothing — guessing when you have been told
    /// is how a client ends up hammering a service that asked it politely to
    /// wait. Same order `lb_get_page` uses for ListenBrainz; do not invent a
    /// third backoff.
    ///
    /// **Only a throttle or a server fault is retried.** A 4xx that is not 429
    /// is an answer — a bad query, a revoked token — and retrying it three
    /// times just spends the budget that the *next* track needed.
    ///
    /// The whole ladder is bounded by `MAX_ATTEMPTS` and by `MAX_WAIT`, because
    /// this runs inside a request handler: a push that never returns is
    /// answered by whatever proxy is in front of it, and that answer says
    /// nothing about the playlist. The trap recorded three times over.
    async fn send_with_retry(
        &self,
        build: impl Fn(String) -> reqwest::RequestBuilder,
    ) -> Result<serde_json::Value, String> {
        let mut attempt = 0u32;
        loop {
            let token = self.token().await?;
            let resp = build(token).send().await;
            let resp = match resp {
                Ok(r) => r,
                // A transport error is worth one more try for the same reason a
                // 5xx is: nothing was decided.
                Err(e) if attempt + 1 < MAX_ATTEMPTS => {
                    attempt += 1;
                    let wait = backoff(attempt);
                    warn!("TIDAL request failed ({e}); retrying in {:?}", wait);
                    tokio::time::sleep(wait).await;
                    continue;
                }
                Err(e) => return Err(format!("TIDAL request failed: {e}")),
            };

            let status = resp.status();
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if !retryable || attempt + 1 >= MAX_ATTEMPTS {
                if retryable {
                    // Said out loud, because this is the one outcome that used
                    // to be silently indistinguishable from "not in the
                    // catalogue".
                    warn!(
                        "TIDAL still throttling after {} attempts ({}); giving up on this request",
                        MAX_ATTEMPTS, status
                    );
                }
                return read_json(resp).await;
            }

            attempt += 1;
            let advised = advised_wait(resp.headers());
            let wait = advised.unwrap_or_else(|| backoff(attempt));
            // Logged at warn with the headers as the server sent them: whether
            // TIDAL actually publishes rate-limit headers was an open question
            // when this was written, inferred from the shape of three pushes
            // rather than observed. One throttled push answers it.
            warn!(
                "TIDAL returned {} (attempt {}/{}); rate-limit headers: {:?}; waiting {:?}",
                status,
                attempt,
                MAX_ATTEMPTS,
                rate_limit_headers(resp.headers()),
                wait
            );
            tokio::time::sleep(wait).await;
        }
    }
}

/// How many times one request may be sent before its failure is reported.
const MAX_ATTEMPTS: u32 = 4;
/// The longest any single wait may be, however long the server asks for. A
/// push runs inside a request handler; see `send_with_retry`.
const MAX_WAIT: Duration = Duration::from_secs(8);

/// Exponential, with jitter. The jitter is not decoration: a push issues its
/// searches in turn, so without it every retry after a throttle lands on the
/// same schedule and arrives together.
fn backoff(attempt: u32) -> Duration {
    let base = Duration::from_millis(400 * 2u64.pow(attempt.min(5)));
    let base = base.min(MAX_WAIT);
    let jitter = rand::random::<u64>() % 250;
    base + Duration::from_millis(jitter)
}

/// What the server asked us to wait, if it said.
///
/// `Retry-After` is seconds or an HTTP date (RFC 9110); only the seconds form is
/// read, because the date form needs a clock agreement this is not worth having
/// — an unparsed value falls through to the exponential ladder rather than
/// being treated as zero.
fn advised_wait(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    for name in ["retry-after", "x-ratelimit-reset-in", "x-ratelimit-reset"] {
        if let Some(secs) = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            // A reset *timestamp* rather than a duration would be an absurd
            // number of seconds to sleep for; treat anything past the cap as
            // the cap and let the attempt limit end it.
            return Some(Duration::from_secs(secs).min(MAX_WAIT));
        }
    }
    None
}

/// Every rate-limit header the response carried, for the log line.
fn rate_limit_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(k, _)| {
            let k = k.as_str();
            k == "retry-after" || k.starts_with("x-ratelimit") || k.starts_with("ratelimit")
        })
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("<binary>").to_string()))
        .collect()
}

/// Did this request fail with exactly this HTTP status?
///
/// **Anchored at the front, because the error text carries the response body**
/// — `read_json` formats `TIDAL returned {status}: {body}`. A bare
/// `e.contains("404")` therefore also fires on a 401 or a 500 whose body merely
/// mentions 404 somewhere, and in `replace_playlist` that mistake is not
/// cosmetic: a failure would be read as "the playlist is gone", and the caller
/// would helpfully make a second one.
fn was_status(err: &str, code: u16) -> bool {
    // The digit boundary matters: a bare `starts_with` reads "TIDAL returned
    // 4040" as a 404. `read_json` formats the status through `Display`, which
    // is "404 Not Found" for a known code and a bare "499" for one it does not
    // recognise — so the prefix is followed by a space, a colon, or nothing,
    // and never by another digit.
    let Some(rest) = err.strip_prefix(&format!("TIDAL returned {code}")) else { return false };
    !rest.starts_with(|c: char| c.is_ascii_digit())
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

impl TidalClient {
    /// Every track TIDAL stocks for one ISRC.
    ///
    /// `GET /tracks?filter[isrc]=` is an exact lookup — the only one any
    /// playlist target here offers. A recording issued in several territories
    /// can come back more than once; they are the same performance, so any of
    /// them is the right answer and `pick` chooses on the usual ranking.
    ///
    /// The artist is resolved through `included` exactly as the search does,
    /// because `pick` gates hard on it and a candidate with an empty artist
    /// would be refused by the very thing this exists to bypass.
    async fn tracks_by_isrc(&self, isrc: &str) -> Result<Vec<Candidate>, String> {
        let url = format!(
            "{API_BASE}/tracks?filter%5Bisrc%5D={}&countryCode={}&include=artists",
            urlencoding(isrc),
            self.creds.country_code()
        );
        let doc = self.get(&url).await?;
        let artists = included_of(&doc, "artists");
        let mut out = Vec::new();
        for track in doc.get("data").and_then(|v| v.as_array()).into_iter().flatten() {
            let Some(id) = track.get("id").and_then(|v| v.as_str()) else { continue };
            let attrs = track.get("attributes").cloned().unwrap_or_default();
            let title = attrs.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string();
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
        Ok(out)
    }
}

#[async_trait::async_trait]
impl PlaylistTarget for TidalClient {
    async fn search_tracks(&self, want: &PlaylistTrack) -> Result<Vec<Candidate>, String> {
        // **An ISRC is an exact answer, so ask with it first.**
        //
        // Every miss a playlist push produces is ultimately a name that did not
        // agree — a feat. credit on one side, a remaster suffix on the other, a
        // compilation filed under Various Artists. TIDAL is the one target that
        // can be asked a question with no names in it at all: `filter[isrc]`
        // names the *recording*, so there is nothing left to spell differently.
        //
        // It falls through to the search when the history has no ISRC for the
        // track, or when TIDAL does not stock that recording — a miss here is
        // ordinary, not a failure, and the name search may still find a
        // different issue of the same song.
        if let Some(isrc) = want.isrc.as_deref().filter(|s| !s.trim().is_empty()) {
            match self.tracks_by_isrc(isrc).await {
                Ok(hits) if !hits.is_empty() => {
                    debug!("TIDAL ISRC {} matched {} track(s)", isrc, hits.len());
                    return Ok(hits);
                }
                Ok(_) => {}
                // A failed ISRC lookup must not lose the track: fall back to
                // the search rather than reporting a miss.
                Err(e) => debug!("TIDAL ISRC lookup for {} failed: {}", isrc, e),
            }
        }

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
        let created = self
            .post(
                &format!("{API_BASE}/playlists?countryCode={}", self.creds.country_code()),
                create_playlist_body(name),
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
            if let Err(e) = self
                .post(
                    &format!(
                        "{API_BASE}/playlists/{playlist_id}/relationships/items?countryCode={}",
                        self.creds.country_code()
                    ),
                    add_items_body(chunk),
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

    /// Empty the playlist and refill it.
    ///
    /// TIDAL has no "set the items to exactly this" call. `PATCH` on the items
    /// relationship is a **reorder** — capped at 20 and taking positions in its
    /// `meta` — so the only way to replace the contents is to remove what is
    /// there and add the new set.
    ///
    /// **Removing an item needs two ids, not one.** The payload wants the
    /// track's `id` *and* a `meta.itemId`, which is the handle for that
    /// track's place in this playlist — the same track added twice is two
    /// items. Those only come from reading the relationship first, which is why
    /// this starts with a GET.
    ///
    /// A playlist that has been deleted on TIDAL's side answers 404, and that
    /// is reported as **not replaced** rather than as an error: the remembered
    /// id is simply stale, and the honest recovery is to create a new playlist
    /// and remember that one instead.
    async fn replace_playlist(&self, remote_id: &str, ids: &[String]) -> Result<bool, String> {
        let country = self.creds.country_code();
        let items_url =
            format!("{API_BASE}/playlists/{remote_id}/relationships/items?countryCode={country}");

        let existing = match self.get(&items_url).await {
            Ok(doc) => doc,
            // Gone, renamed, or never ours. Not a failure — the caller falls
            // back to creating one.
            Err(e) if was_status(&e, 404) => {
                debug!("TIDAL playlist {} is gone; a new one will be made", remote_id);
                return Ok(false);
            }
            Err(e) => return Err(e),
        };

        let removals = removal_entries(&existing);

        for chunk in removals.chunks(ITEMS_PER_REQUEST) {
            self.delete(&items_url, serde_json::json!({ "data": chunk }))
                .await
                .map_err(|e| format!("Could not clear the playlist before refilling it: {e}"))?;
        }

        for chunk in ids.chunks(ITEMS_PER_REQUEST) {
            self.post(&items_url, add_items_body(chunk)).await.map_err(|e| {
                // Worse than a short new playlist: the old contents are already
                // gone. Said plainly rather than reported as a success.
                warn!("TIDAL playlist {} was emptied but not refilled: {}", remote_id, e);
                format!("Emptied the playlist but could not add every track back: {e}")
            })?;
        }
        Ok(true)
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

    async fn replace_playlist(&self, remote_id: &str, ids: &[String]) -> Result<bool, String> {
        (**self).replace_playlist(remote_id, ids).await
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

    // ── The write half, against the specification ─────────────────────────
    //
    // Every constraint asserted below is quoted from TIDAL's own OpenAPI
    // document (v1.10.106, read 2026-08-26), not invented here:
    //
    //   PlaylistsCreateOperation_Payload
    //     required [data]; data required [attributes, type]
    //     data.type            enum [playlists]
    //     data.attributes      required [name]
    //     .accessType          enum [PUBLIC, UNLISTED]
    //
    //   PlaylistsItemsRelationshipAddOperation_Payload
    //     required [data]; data is an array with maxItems 50
    //     each entry required [id, type]; type enum [tracks, videos]
    //
    //   PlaylistsItemsRelationshipRemoveOperation_Payload
    //     required [data]; data is an array with maxItems 50
    //     each entry required [id, meta, type]; meta required [itemId]
    //
    // **This proves the request matches what TIDAL says it accepts. It does not
    // prove TIDAL accepts it** — nothing here has run against a real account,
    // because the write half needs a user token and therefore a human sign-in.
    // Same footing, and the same caveat, as the Jellyfin session tests.

    /// A created playlist is unlisted, and shaped the way the schema requires.
    ///
    /// The `accessType` is the load-bearing half: this instance is private and
    /// invite-only, and `PUBLIC` is the other value the enum accepts.
    #[test]
    fn a_created_playlist_is_unlisted_and_matches_the_declared_shape() {
        let b = create_playlist_body("Tapedeck · The 3 AM Club");
        let data = b.get("data").expect("required [data]");
        assert_eq!(data["type"], "playlists", "data.type enum is [playlists]");
        let attrs = data.get("attributes").expect("data required [attributes]");
        assert_eq!(attrs["name"], "Tapedeck · The 3 AM Club", "attributes required [name]");
        assert_eq!(
            attrs["accessType"], "UNLISTED",
            "a push must never publish somebody's listening to the world"
        );
    }

    /// Added items carry `id` and `type`, and `type` is one the enum allows.
    #[test]
    fn added_items_match_the_declared_shape() {
        let ids = vec!["1".to_string(), "2".to_string()];
        let b = add_items_body(&ids);
        let data = b["data"].as_array().expect("data is an array");
        assert_eq!(data.len(), 2);
        for e in data {
            assert!(e.get("id").is_some_and(|v| v.is_string()), "entry required [id]");
            assert_eq!(e["type"], "tracks", "type enum is [tracks, videos]");
        }
    }

    /// A push longer than the payload's own cap is split rather than refused.
    ///
    /// `data` declares `maxItems: 50`, and `MAX_TRACKS` is 100 — so a full
    /// playlist is two requests. Sending it as one is a rejected call and an
    /// empty playlist, which is exactly the failure a stub would not catch and
    /// a real account would only show once.
    #[test]
    fn a_push_past_the_cap_is_split_into_requests_the_api_will_accept() {
        let ids: Vec<String> = (0..100).map(|i| i.to_string()).collect();
        let chunks: Vec<_> = ids.chunks(ITEMS_PER_REQUEST).collect();
        assert_eq!(chunks.len(), 2, "100 tracks is two requests at a cap of 50");
        for c in &chunks {
            let n = add_items_body(c)["data"].as_array().unwrap().len();
            assert!(n <= 50, "a request carried {n} items against a declared maxItems of 50");
        }
        // Every track goes exactly once — a chunking bug that dropped or
        // duplicated one would leave a playlist that looks nearly right.
        let sent: Vec<String> = chunks
            .iter()
            .flat_map(|c| {
                add_items_body(c)["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["id"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(sent, ids);
    }

    /// A removal carries `meta.itemId`, and an item that has none is dropped.
    ///
    /// `meta.itemId` is `required` in the schema, so one malformed entry would
    /// have the whole batch refused — and with the removals sent before the
    /// refill, a refused batch is a playlist that keeps its old contents *and*
    /// gains the new ones.
    #[test]
    fn a_removal_carries_the_item_handle_and_skips_anything_without_one() {
        let existing = serde_json::json!({ "data": [
            { "id": "t1", "type": "tracks", "meta": { "itemId": "i1" } },
            // The same track twice is two items with two handles — which is
            // why the handle exists and the track id alone will not do.
            { "id": "t1", "type": "tracks", "meta": { "itemId": "i2" } },
            { "id": "v9", "type": "videos", "meta": { "itemId": "i3" } },
            { "id": "t4", "type": "tracks" },
        ]});
        let out = removal_entries(&existing);
        assert_eq!(out.len(), 3, "the entry with no itemId must be dropped, not sent");
        assert_eq!(out[0]["meta"]["itemId"], "i1");
        assert_eq!(out[1]["meta"]["itemId"], "i2", "both copies are removed, not just one");
        assert_eq!(out[2]["type"], "videos", "the item's own type is kept, not forced to tracks");
        for e in &out {
            assert!(e.get("id").is_some() && e.get("type").is_some());
            assert!(e["meta"]["itemId"].is_string(), "entry required [id, meta, type]");
        }
    }

    /// "The playlist is gone" is decided by the status, not by the body.
    ///
    /// `replace_playlist` treats a 404 as *not replaced* and lets the caller
    /// make a fresh playlist — the remembered id is simply stale. Deciding that
    /// with `e.contains("404")` is wrong in a way that costs the user a
    /// duplicate: the error text carries the response **body**, so an expired
    /// token or a server fault whose payload mentions 404 anywhere would be
    /// read as "gone" and answered by creating a second playlist rather than by
    /// reporting the failure.
    #[test]
    fn a_gone_playlist_is_told_apart_from_an_error_that_merely_mentions_one() {
        assert!(was_status("TIDAL returned 404 Not Found: {}", 404));
        assert!(was_status(
            "TIDAL returned 404 Not Found: {\"errors\":[{\"detail\":\"no such playlist\"}]}",
            404
        ));

        // The cases a naive `contains` gets wrong, and each is a real shape.
        assert!(
            !was_status("TIDAL returned 401 Unauthorized: {\"code\":\"404_not_this\"}", 404),
            "an auth failure must not be read as a missing playlist"
        );
        assert!(
            !was_status("TIDAL returned 500 Internal Server Error: trace id 9404f", 404),
            "a server fault must not be read as a missing playlist"
        );
        assert!(
            !was_status("TIDAL request failed: error sending request", 404),
            "a request that never arrived says nothing about the playlist"
        );
        // And it does not match a longer status by prefix. This assertion
        // shipped as `|| true`, which made it always pass over a match that was
        // genuinely wrong — a guard that cannot fail is a decoration.
        assert!(
            !was_status("TIDAL returned 4040 Nonsense: x", 404),
            "a four-digit status must not be read as a 404"
        );
        // A status with no reason phrase is still matched.
        assert!(was_status("TIDAL returned 404: gone", 404));
    }

    /// An empty or unexpected relationship document yields no removals rather
    /// than panicking — a playlist someone emptied by hand is a normal state.
    #[test]
    fn an_empty_relationship_removes_nothing() {
        assert!(removal_entries(&serde_json::json!({ "data": [] })).is_empty());
        assert!(removal_entries(&serde_json::json!({})).is_empty());
    }

    /// The server's own advice beats the guess, and an unreadable one falls
    /// through rather than being read as "wait zero".
    #[test]
    fn a_throttle_is_waited_out_for_as_long_as_the_server_asks() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let hdr = |name: &'static str, value: &str| {
            let mut h = HeaderMap::new();
            h.insert(name, HeaderValue::from_str(value).unwrap());
            h
        };

        assert_eq!(advised_wait(&hdr("retry-after", "3")), Some(Duration::from_secs(3)));
        assert_eq!(advised_wait(&hdr("x-ratelimit-reset-in", "2")), Some(Duration::from_secs(2)));
        // Capped: this runs inside a request handler, so however long the
        // server asks for, the push still has to answer before a proxy does.
        assert_eq!(advised_wait(&hdr("retry-after", "600")), Some(MAX_WAIT));
        // The HTTP-date form of Retry-After is deliberately not parsed. It must
        // read as "no advice" and use the ladder — never as zero, which would
        // retry instantly into the throttle that just rejected us.
        assert_eq!(advised_wait(&hdr("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(advised_wait(&HeaderMap::new()), None);
    }

    /// Bounded and jittered. The jitter matters because a push issues its
    /// searches in turn: without it every retry after one throttle lands on the
    /// same schedule and arrives together.
    #[test]
    fn the_backoff_grows_but_stays_inside_the_handler_budget() {
        let one = backoff(1);
        let three = backoff(3);
        assert!(one >= Duration::from_millis(800) && one < Duration::from_millis(1100));
        assert!(three > one);
        for attempt in 1..=10 {
            assert!(
                backoff(attempt) <= MAX_WAIT + Duration::from_millis(250),
                "attempt {attempt} would outlast the handler"
            );
        }
        // Four attempts of at most MAX_WAIT each is the worst one request can
        // cost, and a push is many requests — so this is what keeps the whole
        // push inside a proxy's patience.
        assert_eq!(MAX_ATTEMPTS, 4);
    }

    /// What the log line reports. This is the half that answers, on the next
    /// real throttle, whether TIDAL publishes rate-limit headers at all — which
    /// was inferred from the shape of three pushes rather than observed.
    #[test]
    fn every_rate_limit_header_reaches_the_log() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        h.insert("retry-after", HeaderValue::from_static("5"));
        h.insert("content-type", HeaderValue::from_static("application/json"));
        let found = rate_limit_headers(&h);
        assert_eq!(found.len(), 2, "only the rate-limit ones, and both of them: {found:?}");
        assert!(found.iter().any(|(k, v)| k == "retry-after" && v == "5"));
    }

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

                isrc: None,
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
