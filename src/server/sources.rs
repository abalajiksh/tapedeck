//! Per-user polling sources (`user_sources`).
//!
//! Every user configures their own — there is no admin-managed server-wide
//! source any more. For Plex that means a user supplies *their own* token, which
//! we resolve to a Plex account and pin the source to; other people's sessions on
//! the same server never become their listens.
//!
//! A user may configure **several servers of the same kind** — two Plex boxes, a
//! Navidrome and a Jellyfin — so sources are addressed by id rather than by kind,
//! and each carries a `label` to tell them apart. Every listen records which
//! source produced it (`scrobbles.source_ref`), which is what lets the artwork
//! proxy know which of two Plex servers to ask.
//!
//! Stored credentials are never returned to the browser (`has_token` instead),
//! matching how API tokens and scrobble connections already behave. An omitted
//! secret on update leaves the stored one alone, so a user can change the URL or
//! poll interval without re-pasting it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;
use utoipa::ToSchema;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::UserSource;
use crate::openapi::{BadRequest, NotFound, Unauthorized};
use crate::sources::mediabrowser::MediaBrowserAccount;
use crate::sources::plex::PlexAccount;
use crate::sources::{Flavour, MediaBrowserSource, NavidromeSource, PlexFilters, PlexSource};

/// A configured source, without its credentials.
#[derive(Serialize, ToSchema)]
pub struct Source {
    pub id: i64,
    /// `plex`, `navidrome`, `jellyfin`, `emby` or `roon`.
    pub kind: String,
    /// Your name for it, so two servers of the same kind are tellable apart.
    pub label: String,
    pub enabled: bool,
    pub url: String,
    /// A credential is stored. It is never returned.
    pub has_token: bool,
    /// The account on the server whose plays are scrobbled, resolved on save.
    pub remote_account: Option<String>,
    /// Navidrome's login name.
    pub username: Option<String>,
    /// Null means the engine's default.
    pub poll_interval_secs: Option<i64>,
    pub libraries_allow: Vec<String>,
    pub libraries_block: Vec<String>,
    pub devices_allow: Vec<String>,
    pub devices_block: Vec<String>,
    pub last_polled_at: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SourceList {
    pub sources: Vec<Source>,
    /// The kinds a source can be created as.
    pub kinds: Vec<&'static str>,
}

/// Every account on the server, for the "scrobble as" picker. Plex numbers
/// its accounts; Jellyfin and Emby use string ids.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum SourceAccounts {
    Plex(Vec<PlexAccount>),
    MediaBrowser(Vec<MediaBrowserAccount>),
}

/// Whether the credentials work. A failed check is a 200 with `ok: false`.
#[derive(Serialize, ToSchema)]
pub struct SourceCheck {
    pub ok: bool,
    /// The account the credential belongs to, where the server can say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The server was reached but could not name the account — recoverable
    /// by picking one from `accounts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_error: Option<String>,
    /// Empty or absent when the credential may not list them; the picker is
    /// then not offered. Jellyfin needs this more than Plex: a dashboard API
    /// key belongs to no user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<SourceAccounts>,
    /// Plex's music libraries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraries: Option<Vec<String>>,
    /// Why it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SourceCheck {
    fn failed(e: impl ToString) -> Json<Self> {
        Json(Self {
            ok: false,
            account: None,
            account_error: None,
            accounts: None,
            libraries: None,
            error: Some(e.to_string()),
        })
    }
}

/// Jellyfin and Emby are one protocol with a prefix on it, so the two kinds
/// share every code path here and differ only by this.
fn media_browser_flavour(kind: &str) -> Option<Flavour> {
    match kind {
        "jellyfin" => Some(Flavour::Jellyfin),
        "emby" => Some(Flavour::Emby),
        _ => None,
    }
}

/// Kinds we can actually build. Kept explicit so a typo'd path can't create a
/// row the engine will never poll.
const KINDS: [&str; 5] = ["plex", "navidrome", "jellyfin", "emby", "roon"];

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

fn public_view(s: &UserSource) -> Source {
    Source {
        id: s.id,
        kind: s.kind.clone(),
        label: s.label.clone(),
        enabled: s.enabled,
        url: s.url.clone(),
        has_token: !s.credentials.is_empty(),
        remote_account: s.remote_account.clone(),
        username: s.credential("username"),
        poll_interval_secs: s.poll_interval_secs,
        libraries_allow: s.option_list("libraries_allow"),
        libraries_block: s.option_list("libraries_block"),
        devices_allow: s.option_list("devices_allow"),
        devices_block: s.option_list("devices_block"),
        last_polled_at: s.last_polled_at,
        last_error: s.last_error.clone(),
        updated_at: s.updated_at,
    }
}

// GET /api/v1/sources — the caller's own sources.
/// Your configured sources.
///
/// **Session-only, permanently** — these rows hold credentials. The stored
/// token or password is never returned; `has_token` says whether one is
/// set.
#[utoipa::path(
    get,
    path = "/api/v1/sources",
    tag = "Sources",
    responses(
        (status = 200, description = "OK.", body = SourceList),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_sources(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_sources_for_user(user.user_id).await {
        Ok(rows) => {
            let sources = rows.iter().map(public_view).collect();
            (StatusCode::OK, Json(SourceList { sources, kinds: KINDS.to_vec() })).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// A source to save. On update every field is optional, and an omitted or
/// empty credential **keeps the stored one** — it is never sent back, so a
/// client could not echo it anyway.
#[derive(Deserialize, ToSchema)]
#[schema(as = SourceSettings)]
struct SourceUpdate {
    /// `plex`, `navidrome`, `jellyfin`, `emby` or `roon`. Required on create
    /// and ignored afterwards; a source cannot change kind.
    kind: Option<String>,
    label: Option<String>,
    /// Enabling needs a complete set of credentials for the kind — a 400
    /// otherwise. Defaults to off on create.
    enabled: Option<bool>,
    url: Option<String>,
    /// Plex.
    token: Option<String>,
    /// Navidrome logs in as a user.
    //
    // Subsonic hashes password+salt per request, which is why the password
    // itself is stored (encrypted) rather than a token.
    username: Option<String>,
    password: Option<String>,
    /// Jellyfin and Emby.
    api_key: Option<String>,
    poll_interval_secs: Option<i64>,
    libraries_allow: Option<Vec<String>>,
    libraries_block: Option<Vec<String>>,
    devices_allow: Option<Vec<String>>,
    devices_block: Option<Vec<String>>,
    /// Which Plex account's sessions this source should scrobble. Omitted
    /// means whoever the token belongs to. Validated against the server's
    /// accounts on save.
    //
    // It matters on a household server, where one admin holds the token:
    // without this, every source resolved to the server owner and a second
    // user could not be set up at all. A name that isn't on the server would
    // match no session and scrobble nothing, silently.
    plex_account: Option<String>,
    /// Which Jellyfin or Emby account's sessions to scrobble. Needed whenever
    /// the key is a dashboard API key, which belongs to no user.
    //
    // `/Users/Me` answers 400 for such a key and there is nothing to infer
    // from. Only a token minted by a user login resolves on its own.
    jellyfin_account: Option<String>,
}

/// Everything a save needs, resolved from the body and whatever is stored.
struct Resolved {
    url: String,
    credentials: String,
    remote_account: Option<String>,
    options: String,
    complete: bool,
}

/// Work out the final credential set and confirm which remote account it
/// belongs to. Shared by create and update so both validate identically.
async fn resolve_save(
    kind: &str,
    body: &mut SourceUpdate,
    current: Option<&UserSource>,
    enabled: bool,
) -> Result<Resolved, axum::response::Response> {
    let url = body
        .url
        .take()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .or_else(|| current.map(|c| c.url.clone()))
        .unwrap_or_default();

    let keep = |incoming: Option<String>, key: &str| -> String {
        incoming
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().to_string())
            .or_else(|| current.and_then(|c| c.credential(key)))
            .unwrap_or_default()
    };
    let token = keep(body.token.take(), "token");
    let nd_user = keep(body.username.take(), "username");
    let nd_pass = keep(body.password.take(), "password");
    let jf_key = keep(body.api_key.take(), "api_key");

    let complete = match kind {
        "plex" => !url.is_empty() && !token.is_empty(),
        "navidrome" => !url.is_empty() && !nd_user.is_empty() && !nd_pass.is_empty(),
        "jellyfin" | "emby" => !url.is_empty() && !jf_key.is_empty(),
        "roon" => !url.is_empty(),
        _ => false,
    };
    if enabled && !complete {
        return Err(err(
            400,
            match kind {
                "navidrome" => {
                    "A server URL, username and password are required to enable Navidrome"
                }
                "jellyfin" => "A server URL and API key are required to enable Jellyfin",
                "emby" => "A server URL and API key are required to enable Emby",
                "roon" => "The address of your Roon Core is required to enable Roon",
                _ => "A server URL and token are required to enable Plex",
            },
        ));
    }

    // Establishing *which account* the credentials belong to is what keeps one
    // user's listens out of another's. Done on save so a bad credential fails
    // here rather than as an unexplained empty history later.
    let mut remote_account = current.and_then(|c| c.remote_account.clone());
    let credentials = match kind {
        "plex" => {
            let changed = current.map(|c| c.url != url).unwrap_or(true)
                || current.and_then(|c| c.credential("token")).as_deref() != Some(token.as_str())
                || remote_account.is_none();
            // An explicitly chosen account wins, but only after checking it is
            // real. Accepting it unverified would produce a source that looks
            // saved and never scrobbles, which is the least debuggable outcome:
            // no error anywhere, just an empty history.
            let chosen = body.plex_account.take().filter(|a| !a.trim().is_empty());
            if let Some(chosen) = chosen {
                if complete {
                    match PlexSource::list_accounts(&url, &token).await {
                        Ok(accounts) => {
                            match accounts
                                .iter()
                                .find(|a| a.name.eq_ignore_ascii_case(chosen.trim()))
                            {
                                // Store the server's spelling, not the caller's:
                                // session matching is on this string.
                                Some(found) => remote_account = Some(found.name.clone()),
                                None => {
                                    return Err(err(
                                        400,
                                        format!(
                                            "No Plex account called \"{}\" on that server. \
                                             Tapedeck would scrobble nothing.",
                                            chosen.trim()
                                        ),
                                    ))
                                }
                            }
                        }
                        Err(e) => {
                            return Err(err(400, format!("Could not list Plex accounts: {e}")))
                        }
                    }
                }
            } else if complete && changed {
                match PlexSource::resolve_account(&url, &token).await {
                    Ok(name) => remote_account = Some(name),
                    Err(e) => {
                        return Err(err(
                            400,
                            format!(
                                "Could not confirm which Plex account that token belongs to: {e}. \
                         Tapedeck needs this to keep other people's sessions out of your history."
                            ),
                        ))
                    }
                }
            }
            if token.is_empty() {
                String::new()
            } else {
                serde_json::json!({ "token": token }).to_string()
            }
        }
        k @ ("jellyfin" | "emby") => {
            // Unwrap is safe: the arm pattern is the same set the helper knows.
            let flavour = media_browser_flavour(k).expect("arm matches the flavour map");
            let label = flavour.label();
            let changed = current.map(|c| c.url != url).unwrap_or(true)
                || current.and_then(|c| c.credential("api_key")).as_deref()
                    != Some(jf_key.as_str())
                || remote_account.is_none();
            let chosen = body.jellyfin_account.take().filter(|a| !a.trim().is_empty());
            if let Some(chosen) = chosen {
                if complete {
                    match MediaBrowserSource::list_accounts(flavour, &url, &jf_key).await {
                        Ok(accounts) => match accounts
                            .iter()
                            .find(|a| a.name.eq_ignore_ascii_case(chosen.trim()))
                        {
                            // Store the server's spelling — sessions match on it.
                            Some(found) => remote_account = Some(found.name.clone()),
                            None => {
                                return Err(err(
                                    400,
                                    format!(
                                        "No {label} account called \"{}\" on that server. \
                                         Tapedeck would scrobble nothing.",
                                        chosen.trim()
                                    ),
                                ))
                            }
                        },
                        Err(e) => {
                            return Err(err(400, format!("Could not list {label} accounts: {e}")))
                        }
                    }
                }
            } else if complete && changed {
                match MediaBrowserSource::resolve_account(flavour, &url, &jf_key).await {
                    Ok(name) => remote_account = Some(name),
                    Err(e) => {
                        return Err(err(
                            400,
                            format!(
                                "Could not confirm which {label} account that key belongs to: {e}"
                            ),
                        ))
                    }
                }
            }
            if jf_key.is_empty() {
                String::new()
            } else {
                serde_json::json!({ "api_key": jf_key }).to_string()
            }
        }
        // Roon issues its own credential, and only after the user approves
        // the extension in Roon Settings → Extensions. So there is nothing to
        // validate here and nothing to store yet: the engine writes the pairing
        // token back itself once the handshake succeeds. Whatever is already
        // stored is preserved — clearing it would un-pair a working source.
        "roon" => current.map(|c| c.credentials.clone()).unwrap_or_default(),
        _ => {
            if complete && enabled {
                let mut probe = NavidromeSource::new(url.clone(), nd_user.clone(), nd_pass.clone());
                if let Err(e) = probe.initialize().await {
                    return Err(err(400, format!("Could not sign in to Navidrome: {e}")));
                }
            }
            remote_account = (!nd_user.is_empty()).then(|| nd_user.clone());
            if nd_user.is_empty() {
                String::new()
            } else {
                serde_json::json!({ "username": nd_user, "password": nd_pass }).to_string()
            }
        }
    };

    // A list the body omits keeps its stored value, so saving a URL doesn't wipe
    // someone's podcast exclusions.
    let keep_list = |incoming: Option<Vec<String>>, key: &str| -> Vec<String> {
        incoming
            .map(|v| {
                v.into_iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
            })
            .unwrap_or_else(|| current.map(|c| c.option_list(key)).unwrap_or_default())
    };
    let options = serde_json::json!({
        "libraries_allow": keep_list(body.libraries_allow.take(), "libraries_allow"),
        "libraries_block": keep_list(body.libraries_block.take(), "libraries_block"),
        "devices_allow": keep_list(body.devices_allow.take(), "devices_allow"),
        "devices_block": keep_list(body.devices_block.take(), "devices_block"),
    })
    .to_string();

    Ok(Resolved { url, credentials, remote_account, options, complete })
}

// POST /api/v1/sources — add another server.
/// Add a source.
///
/// Session-only. Sources are per user, and one user can have several of a
/// kind, told apart by `label`. For Plex
/// the token is resolved to a Plex account via plex.tv and the source is
/// filtered to that account, so there is no name-matching — which means
/// saving needs outbound internet even for a LAN Plex.
#[utoipa::path(
    post,
    path = "/api/v1/sources",
    tag = "Sources",
    request_body = SourceUpdate,
    responses(
        (status = 201, description = "Created.", body = Source),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn create_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(mut body): Json<SourceUpdate>,
) -> impl IntoResponse {
    let kind = body.kind.clone().unwrap_or_default();
    if !KINDS.contains(&kind.as_str()) {
        return err(400, format!("Unknown source kind: {kind}"));
    }
    let enabled = body.enabled.unwrap_or(false);
    let label = body.label.clone().unwrap_or_default().trim().to_string();
    let interval = body.poll_interval_secs;

    let resolved = match resolve_save(&kind, &mut body, None, enabled).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state
        .db
        .create_source(
            user.user_id,
            &kind,
            &label,
            enabled && resolved.complete,
            &resolved.url,
            &resolved.credentials,
            resolved.remote_account.as_deref(),
            interval,
            &resolved.options,
        )
        .await
    {
        Ok(id) => {
            info!("⚙️ {} source added by {}", kind, user.username);
            match state.db.get_source(user.user_id, id).await {
                Ok(Some(s)) => (StatusCode::CREATED, Json(public_view(&s))).into_response(),
                _ => err(500, "source vanished after write"),
            }
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// Update a source.
///
/// Session-only. **An omitted token keeps the stored one** — it is not cleared.
#[utoipa::path(
    put,
    path = "/api/v1/sources/{id}",
    tag = "Sources",
    params(("id" = i64, Path)),
    request_body = SourceUpdate,
    responses(
        (status = 200, description = "Updated.", body = Source),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(mut body): Json<SourceUpdate>,
) -> impl IntoResponse {
    let current = match state.db.get_source(user.user_id, id).await {
        Ok(Some(c)) => c,
        Ok(None) => return err(404, "No such source"),
        Err(e) => return err(500, e.to_string()),
    };

    let enabled = body.enabled.unwrap_or(current.enabled);
    let label = body.label.clone().unwrap_or_else(|| current.label.clone()).trim().to_string();
    let interval = body.poll_interval_secs.or(current.poll_interval_secs);

    let resolved = match resolve_save(&current.kind, &mut body, Some(&current), enabled).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state
        .db
        .update_source(
            user.user_id,
            id,
            &label,
            enabled,
            &resolved.url,
            &resolved.credentials,
            resolved.remote_account.as_deref(),
            interval,
            &resolved.options,
        )
        .await
    {
        Ok(true) => {
            info!("⚙️ {} source updated by {} (enabled: {})", current.kind, user.username, enabled);
            match state.db.get_source(user.user_id, id).await {
                Ok(Some(s)) => (StatusCode::OK, Json(public_view(&s))).into_response(),
                _ => err(500, "source vanished after write"),
            }
        }
        Ok(false) => err(404, "No such source"),
        Err(e) => err(500, e.to_string()),
    }
}

/// Delete a source.
#[utoipa::path(
    delete,
    path = "/api/v1/sources/{id}",
    tag = "Sources",
    params(("id" = i64, Path)),
    responses(
        (status = 204, description = "Deleted."),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_source(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(404, "No such source"),
        Err(e) => err(500, e.to_string()),
    }
}

/// Credentials to try. Anything omitted falls back to the stored source named
/// by `id`.
#[derive(Deserialize, ToSchema)]
#[schema(as = SourceCredentials)]
struct SourceTest {
    /// Required unless `id` is given.
    kind: Option<String>,
    /// Test an existing source, reusing its stored secret.
    id: Option<i64>,
    url: Option<String>,
    token: Option<String>,
    username: Option<String>,
    password: Option<String>,
    api_key: Option<String>,
}

// POST /api/v1/sources/test — verify credentials before or after saving.
//
// Takes `kind` in the body rather than the path so it can be called while
// adding a server that doesn't exist yet; pass `id` instead to re-test one
// that does, reusing its stored secret.
/// Test a source's credentials without saving.
///
/// Session-only. `kind` rides in the body rather than the path so a server
/// can be tested before it is saved; pass `id` instead to re-test one that
/// is, reusing its stored secret.
#[utoipa::path(
    post,
    path = "/api/v1/sources/test",
    tag = "Sources",
    request_body = SourceTest,
    responses(
        (status = 200, description = "Result. A failed check is still a 200, with `ok: false`.", body = SourceCheck),
        (status = 400, description = "Unknown kind, or not enough to test with.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn test_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SourceTest>,
) -> impl IntoResponse {
    let stored = match body.id {
        Some(id) => match state.db.get_source(user.user_id, id).await {
            Ok(Some(s)) => Some(s),
            Ok(None) => return err(404, "No such source"),
            Err(e) => return err(500, e.to_string()),
        },
        None => None,
    };

    let kind =
        body.kind.clone().or_else(|| stored.as_ref().map(|s| s.kind.clone())).unwrap_or_default();
    if !KINDS.contains(&kind.as_str()) {
        return err(400, format!("Unknown source kind: {kind}"));
    }

    let url = body
        .url
        .clone()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| stored.as_ref().map(|s| s.url.clone()))
        .unwrap_or_default();
    let cred = |incoming: Option<String>, key: &str| -> String {
        incoming
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().to_string())
            .or_else(|| stored.as_ref().and_then(|s| s.credential(key)))
            .unwrap_or_default()
    };

    match kind.as_str() {
        k @ ("jellyfin" | "emby") => {
            let flavour = media_browser_flavour(k).expect("arm matches the flavour map");
            let key = cred(body.api_key.clone(), "api_key");
            if url.is_empty() || key.is_empty() {
                return err(400, "Enter a server URL and API key first");
            }
            // Listed first: with a dashboard API key `resolve_account` cannot
            // answer on its own, and the list is what the picker needs anyway.
            let accounts =
                MediaBrowserSource::list_accounts(flavour, &url, &key).await.unwrap_or_default();
            match MediaBrowserSource::resolve_account(flavour, &url, &key).await {
                Ok(name) => (
                    StatusCode::OK,
                    Json(SourceCheck {
                        ok: true,
                        account: Some(name),
                        account_error: None,
                        accounts: Some(SourceAccounts::MediaBrowser(accounts)),
                        libraries: None,
                        error: None,
                    }),
                )
                    .into_response(),
                // Reaching the server but not being able to name the account is
                // a different thing from an unreachable server, and with a list
                // in hand it is recoverable — the caller picks. Reported as a
                // successful connection with an account error, the same shape
                // Plex uses.
                Err(e) if !accounts.is_empty() => (
                    StatusCode::OK,
                    Json(SourceCheck {
                        ok: true,
                        account: None,
                        account_error: Some(e.to_string()),
                        accounts: Some(SourceAccounts::MediaBrowser(accounts)),
                        libraries: None,
                        error: None,
                    }),
                )
                    .into_response(),
                Err(e) => (StatusCode::OK, SourceCheck::failed(e)).into_response(),
            }
        }
        "navidrome" => {
            let (u, p) =
                (cred(body.username.clone(), "username"), cred(body.password.clone(), "password"));
            if url.is_empty() || u.is_empty() || p.is_empty() {
                return err(400, "Enter a server URL, username and password first");
            }
            let mut probe = NavidromeSource::new(url, u.clone(), p);
            match probe.initialize().await {
                Ok(()) => (
                    StatusCode::OK,
                    Json(SourceCheck {
                        ok: true,
                        account: Some(u),
                        account_error: None,
                        accounts: None,
                        libraries: None,
                        error: None,
                    }),
                )
                    .into_response(),
                Err(e) => (StatusCode::OK, SourceCheck::failed(e)).into_response(),
            }
        }
        _ => {
            let token = cred(body.token.clone(), "token");
            if url.is_empty() || token.is_empty() {
                return err(400, "Enter a server URL and token first");
            }
            // Reaching the server but failing to identify the account is a
            // different problem from an unreachable server, so report both.
            // Flattened immediately — `Box<dyn Error>` isn't `Send`, so holding
            // it across the next await would make this un-handler-able.
            let (account, account_error) = match PlexSource::resolve_account(&url, &token).await {
                Ok(name) => (Some(name), None),
                Err(e) => (None, Some(e.to_string())),
            };
            // Every account on the server, so the UI can offer a picker instead
            // of silently assuming the token holder. Best-effort: a shared token
            // may not be allowed to read /accounts, and that is not a reason to
            // fail the connection test — the caller just gets no picker and the
            // token's own account as before.
            let accounts = PlexSource::list_accounts(&url, &token).await.unwrap_or_default();
            let mut probe = PlexSource::with_filters(url, token, PlexFilters::default());
            match probe.initialize().await {
                Ok(()) => (
                    StatusCode::OK,
                    Json(SourceCheck {
                        ok: true,
                        libraries: Some(probe.library_names()),
                        account,
                        account_error,
                        accounts: Some(SourceAccounts::Plex(accounts)),
                        error: None,
                    }),
                )
                    .into_response(),
                Err(e) => (StatusCode::OK, SourceCheck::failed(e)).into_response(),
            }
        }
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(test_source, list_sources, create_source, update_source, delete_source))]
pub struct Api;

pub fn create_sources_router() -> Router<Arc<AppState>> {
    Router::new()
        // `/test` is registered before `/:id` so it isn't swallowed by the
        // parameterised route.
        .route("/api/v1/sources/test", post(test_source))
        .route("/api/v1/sources", get(list_sources).post(create_source))
        .route("/api/v1/sources/:id", axum::routing::put(update_source).delete(delete_source))
}
