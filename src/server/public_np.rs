//! The one deliberately public read path in Tapedeck.
//!
//! Everything else in this application is self-only by construction: `AuthUser`
//! or `ReadAuth` on every handler, every query scoped by `user_id`, and the
//! `/1/` endpoints authenticated precisely *because* ListenBrainz serves them
//! publicly and a self-hosted instance must not. This module is the exception,
//! and it exists for one reason: a static site — a Hugo build, a README badge —
//! renders long before anyone visits it, so a live "what I'm listening to"
//! widget has to be fetched from the reader's browser, where no credential of
//! the owner's can safely go.
//!
//! Four rules hold it narrow, and none of them is decoration.
//!
//! **It is off until someone turns it on.** `users.public_now_playing` defaults
//! to 0 for new accounts and for every account that existed before the column.
//! Nobody consents to a public page by upgrading a binary.
//!
//! **An unknown username and an unpublished one are the same 404.** This is the
//! only route on the instance that answers without a credential, so anything
//! that tells the two apart turns it into a membership oracle: type names at it
//! and learn who has an account on a private, invite-only server. Same rule
//! `pairing` follows for expired versus unknown device codes.
//!
//! **Every field beyond title/artist/album is opted into individually.** A
//! cover, a playhead, a codec and a signal chain are four different disclosures
//! — a progress bar says when you paused, and a chain says what is in your
//! listening room — so they are four switches rather than one. `PublicNpFields`
//! fails closed: anything unparseable publishes less, never more.
//!
//! **Nothing here reads the history.** The deck is in-memory state about one
//! track. There is no route from this module to a listen, a count, a note or a
//! love, and there must not be: the moment this endpoint can answer a question
//! about the *past*, it stops being a now-playing widget and becomes a public
//! profile, which is Phase 6's decision to make and not this file's.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use std::sync::Arc;

use super::AppState;
use crate::models::PublicNpFields;

/// Live state, so a shared cache must not hold it long — but a widget on a
/// public page can be fetched by a great many browsers at once, and the origin
/// is usually a Raspberry Pi. Five seconds is the engine's own tick: short
/// enough that turning the switch off takes effect about as fast as a person
/// can check, long enough that a burst of readers costs one query rather than
/// hundreds.
const CACHE_CONTROL: &str = "public, max-age=5";

/// Cross-origin by definition — the whole point is a page on another domain
/// reading it. A `GET` carrying no custom headers is a CORS *simple request*,
/// so this needs no preflight and no `OPTIONS` handler.
///
/// Safe to open to `*` only because of what is on the other side: this route
/// reads no cookie and accepts no credential, so a wildcard grants a stranger's
/// page exactly what curl already has. Do not copy this header onto anything
/// that authenticates.
fn public_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE_CONTROL));
    h
}

/// The answer for "no such deck", used for a username that does not exist and
/// for one that has not published. Identical on purpose — see the module note.
fn no_deck() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        public_headers(),
        Json(serde_json::json!({
            "code": 404,
            "error": "No public deck here.",
        })),
    )
        .into_response()
}

/// GET /public/np/:username — what this person is listening to, if they publish.
///
/// Unauthenticated. Returns 404 when there is nothing public to show, 200 with
/// `playing: false` when the person publishes and the deck is empty.
async fn public_now_playing(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
) -> impl IntoResponse {
    let Ok(Some((user_id, display_name, fields))) = state.db.public_np_target(&username).await
    else {
        return no_deck();
    };

    let Some((np, position)) = state.now_playing.get(user_id) else {
        return (
            StatusCode::OK,
            public_headers(),
            Json(serde_json::json!({
                "playing": false,
                "user": username,
                "name": display_name,
            })),
        )
            .into_response();
    };

    // Resolving the cover may need a query, so it is done before the payload is
    // built rather than inside it — `public_payload` stays pure, which is what
    // lets the rule deciding what leaves the instance be tested without a
    // server. Same reasoning as `own_history_tracks` and `consume_sessions`.
    let artwork = if fields.artwork { resolve_artwork(&state, user_id, &np).await } else { None };

    let body = public_payload(&np, position, fields, &username, display_name.as_deref(), artwork);
    (StatusCode::OK, public_headers(), Json(body)).into_response()
}

/// Build the public payload: the whole of what leaves the instance.
///
/// Pure, and separated from the handler for the reason `own_history_tracks` is
/// separated from `fetch_history_tracks` — this is the rule that decides what
/// becomes public, and a rule like that has to be testable without a running
/// server. A field is **omitted** rather than nulled when it is not granted, so
/// there is no shape difference between "you may not see this" and "nobody
/// knows it": a reader learns nothing from the absence either way.
fn public_payload(
    np: &crate::server::now_playing::NowPlaying,
    position: u64,
    fields: PublicNpFields,
    username: &str,
    display_name: Option<&str>,
    artwork: Option<String>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "playing": true,
        "user": username,
        "name": display_name,
        "title": np.title,
        "artist": np.artist,
        "album": np.album,
    });
    let obj = body.as_object_mut().expect("built as an object");

    if fields.position {
        obj.insert("position".into(), serde_json::json!(position));
        obj.insert("duration".into(), serde_json::json!(np.duration));
        // Whether that number is a measurement or our own arithmetic. A widget
        // drawing a confident progress bar over a guess is the same error
        // `Progress` was introduced to stop, exported to somebody's website.
        obj.insert("position_known".into(), serde_json::json!(np.position_known));
        obj.insert("paused".into(), serde_json::json!(np.paused));
    }

    if fields.fidelity {
        obj.insert(
            "quality".into(),
            serde_json::json!({
                "format": np.format_type,
                "codec": np.codec,
                "bit_depth": np.bit_depth,
                "sample_rate": np.sample_rate,
                "dsd_multiplier": np.dsd_multiplier,
                "lossless": np.is_lossless,
            }),
        );
    }

    if fields.gear {
        // Omitted rather than nulled when the source did not know one. A polled
        // listen resolves its chain from the device default — the coarsest rung
        // of the ladder — and naming that as "how you listened" would dress a
        // fallback up as a choice.
        if let Some(chain) = &np.chain_name {
            obj.insert("chain".into(), serde_json::json!(chain));
        }
        if let Some(ctx) = &np.listening_context {
            obj.insert("context".into(), serde_json::json!(ctx));
        }
    }

    if let Some(url) = artwork {
        obj.insert("artwork".into(), serde_json::json!(url));
    }

    body
}

/// A cover for what is on the deck, at most one lookup per track.
///
/// The registry carries no artwork of its own — the deck in the SPA has never
/// drawn one — so this is the first caller that needs it, and it memoises the
/// answer back into the entry. That is the whole reason it is not a plain query
/// here: this route is unauthenticated and cacheable by anyone, so a query per
/// request would put an unbounded history scan behind a URL a stranger can hold
/// down. Resolved once per track it costs what opening the album page costs.
///
/// A resolved `None` is stored too, so a record nobody has a cover for is not
/// re-asked forever — the tombstone discipline `artwork_url = ''` and
/// `caa_id = -1` already follow.
async fn resolve_artwork(
    state: &Arc<AppState>,
    user_id: i64,
    np: &crate::server::now_playing::NowPlaying,
) -> Option<String> {
    if np.artwork_resolved {
        return np.artwork_url.clone();
    }
    let album = np.album.as_deref()?;
    let found = match state.db.album_artwork(user_id, &np.artist, album).await {
        Ok(Some((url, caa_id, _))) => url
            .filter(|u| !u.is_empty())
            // The Cover Art Archive URL is free when enrichment already left an
            // id: it is built rather than fetched. `-1` is the tombstone for
            // "asked, nobody has one", which is why this is `> 0` and not
            // `is_some`.
            .or_else(|| {
                caa_id
                    .filter(|id| *id > 0)
                    .map(|id| format!("https://coverartarchive.org/x/{id}.jpg"))
            }),
        _ => None,
    };
    state.now_playing.set_artwork(user_id, &np.title, &np.artist, found.clone());
    found
}

// ── The owner's side of the switch ──

/// What a caller may ask for. `enabled` is required: this endpoint decides
/// whether a person's listening leaves the instance, and a body that merely
/// misspelled the key must not be read as an answer either way. Same reasoning
/// as `bulk-chain` and `default_chain_id`, in the one place where guessing
/// wrong is published rather than merely wrong.
#[derive(serde::Deserialize)]
struct PublishRequest {
    enabled: Option<bool>,
    /// Names from `PublicNpFields::KNOWN`; anything else is dropped rather than
    /// rejected, so a newer UI cannot make an older server fail — it can only
    /// fail to grant something, which is the safe direction.
    fields: Option<Vec<String>>,
}

fn settings_body(enabled: bool, fields: PublicNpFields, username: &str) -> serde_json::Value {
    serde_json::json!({
        "enabled": enabled,
        "fields": PublicNpFields::KNOWN.iter().filter(|n| fields.has(n)).collect::<Vec<_>>(),
        "known_fields": PublicNpFields::KNOWN,
        // The address to paste into a website, built here so the screen cannot
        // assemble a different one. Relative when the operator has not said what
        // this instance is reached at — a guess would be worse than an obvious
        // blank, since the whole value of the string is being correct.
        "path": format!("/public/np/{username}"),
    })
}

/// GET /api/v1/profile/public-now-playing — the owner's own view of the switch.
async fn get_publish(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
) -> impl IntoResponse {
    match state.db.get_public_np_settings(user.user_id).await {
        Ok((enabled, fields)) => {
            (StatusCode::OK, Json(settings_body(enabled, fields, &user.username))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// PUT /api/v1/profile/public-now-playing — turn publishing on or off.
///
/// **Session-only, and permanently.** A token is held by a scrobble client, and
/// no scrobble client has any business deciding that its owner's listening is
/// public. This sits on the same line as `/api/v1/sources` and the password
/// endpoints: `AuthUser`, so a token gets 401 and the question of scopes never
/// arises.
async fn put_publish(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<PublishRequest>,
) -> impl IntoResponse {
    let Some(enabled) = body.enabled else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "error": "`enabled` is required — say true or false explicitly.",
            })),
        )
            .into_response();
    };
    let fields = PublicNpFields::from_names(body.fields.unwrap_or_default());
    match state.db.set_public_np_settings(user.user_id, enabled, fields).await {
        Ok(()) => {
            (StatusCode::OK, Json(settings_body(enabled, fields, &user.username))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "error": e.to_string() })),
        )
            .into_response(),
    }
}

pub fn create_public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/public/np/:username", get(public_now_playing))
        .route("/api/v1/profile/public-now-playing", get(get_publish).put(put_publish))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Play;
    use crate::server::now_playing::{NowPlaying, Progress};

    fn deck() -> NowPlaying {
        let mut play =
            Play::new("Xtal".into(), "Aphex Twin".into(), 0, "test".into(), "ingest".into());
        play.album = Some("Selected Ambient Works 85-92".into());
        play.duration = Some(297);
        let mut np = NowPlaying::from_play(&play, "ingest", None, Progress::Exact(84));
        np.chain_name = Some("Desk → HD 569".into());
        np.listening_context = Some("Critical".into());
        np.format_type = Some("pcm".into());
        np.codec = Some("flac".into());
        np.is_lossless = Some(true);
        np
    }

    fn keys(v: &serde_json::Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    /// The default is the whole point. A deck carrying a playhead, a codec and a
    /// chain publishes none of them until each is ticked — so an account that
    /// flips the master switch and reads no further has disclosed three fields,
    /// not eight.
    #[test]
    fn nothing_beyond_the_base_payload_is_public_by_default() {
        let np = deck();
        let body = public_payload(&np, 84, PublicNpFields::default(), "ashwin", None, None);
        assert_eq!(keys(&body), ["album", "artist", "name", "playing", "title", "user"]);
    }

    /// Each switch grants its own fields and nobody else's. Written as a loop
    /// over the vocabulary rather than four assertions, so a *fifth* field added
    /// later without a case here fails rather than quietly publishing on every
    /// setting.
    #[test]
    fn each_field_is_its_own_switch() {
        let np = deck();
        let base = keys(&public_payload(&np, 84, PublicNpFields::default(), "a", None, None));

        for name in PublicNpFields::KNOWN {
            let only = PublicNpFields::from_names([name]);
            let art = (name == "artwork").then(|| "https://example.invalid/a.jpg".to_string());
            let got = keys(&public_payload(&np, 84, only, "a", None, art));
            let extra: Vec<&String> = got.iter().filter(|k| !base.contains(k)).collect();

            let expected: &[&str] = match name {
                "artwork" => &["artwork"],
                "position" => &["duration", "paused", "position", "position_known"],
                "fidelity" => &["quality"],
                "gear" => &["chain", "context"],
                _ => panic!("`{name}` is in KNOWN with no case here — it would publish untested"),
            };
            assert_eq!(extra, expected.iter().collect::<Vec<_>>(), "field `{name}`");
        }
    }

    /// A field the source never knew is **absent**, exactly like one that was
    /// not granted. That is what stops a reader learning anything from the
    /// shape of the response: "they don't publish their chain" and "this listen
    /// came off Plex, which has no chain to publish" look identical.
    #[test]
    fn an_unknown_field_is_absent_not_null() {
        let mut np = deck();
        np.chain_name = None;
        np.listening_context = None;
        let all = PublicNpFields::from_names(PublicNpFields::KNOWN);
        let body = public_payload(&np, 84, all, "a", None, None);
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("chain"), "an unknown chain is omitted, never null");
        assert!(!obj.contains_key("context"));
        assert!(!obj.contains_key("artwork"), "same for a cover nobody has");
    }

    /// `position_known` has to travel with the position or a widget draws a
    /// wall-clock guess as a measurement — the error `Progress` exists to stop,
    /// re-exported onto somebody's website where nobody can see the caveat.
    #[test]
    fn a_published_position_says_whether_it_is_a_measurement() {
        let mut np = deck();
        np.position_known = false;
        let body =
            public_payload(&np, 84, PublicNpFields::from_names(["position"]), "a", None, None);
        assert_eq!(body["position"], 84);
        assert_eq!(body["position_known"], false);
    }
}
