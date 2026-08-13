//! Album and artist pages (UX 4).
//!
//! Both are read-only aggregations over the caller's own `scrobbles`, addressed
//! by **entity id** rather than MBID. That is a deliberate departure from the
//! design guide in `temp/artist album design files/`: most of a library is
//! un-enriched and has no MBID at all, and those are exactly the records worth a
//! page. Entities already carry an MBID as one alias among others (Phase 7), so
//! nothing is lost by keying on the internal id.
//!
//! `resolve` is what makes a listen row clickable: the browser knows an artist
//! and an album by name, not an id, so it asks here and gets redirected. That
//! *creates* the entity if it doesn't exist yet, which is the same write-on-read
//! trade `resolve_reference` already makes — without it every link from the
//! history would be dead until you happened to annotate the thing first.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

fn err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg.into() })))
        .into_response()
}

fn not_found(what: &str) -> axum::response::Response {
    err(StatusCode::NOT_FOUND, format!("{} not found", what))
}

/// Stat-card strings are built here, not in the browser, so every surface that
/// shows "412 plays" phrases it identically.
fn stat(k: &str, v: String, sub: String, dark: bool) -> serde_json::Value {
    serde_json::json!({ "k": k, "v": v, "sub": sub, "tone": if dark { "dark" } else { "plain" } })
}

fn hours(seconds: i64) -> String {
    let h = seconds as f64 / 3600.0;
    if h >= 10.0 {
        format!("{}", h.round() as i64)
    } else if h >= 1.0 {
        format!("{:.1}", h)
    } else {
        format!("{}m", (seconds as f64 / 60.0).round() as i64)
    }
}

fn pct(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        0
    } else {
        (part as f64 / whole as f64 * 100.0).round() as i64
    }
}

/// "2019" / "14 Aug" from a unix timestamp, in UTC. Local-time formatting is the
/// browser's job everywhere else, but these two strings sit inside a stat card
/// the server composes, so they're built here and kept coarse enough that the
/// zone doesn't matter.
fn year_and_day(ts: Option<i64>) -> (String, String) {
    let Some(ts) = ts else {
        return ("—".into(), String::new());
    };
    use chrono::{TimeZone, Utc};
    match Utc.timestamp_opt(ts, 0).single() {
        Some(dt) => (dt.format("%Y").to_string(), dt.format("%-d %b").to_string()),
        None => ("—".into(), String::new()),
    }
}

/// How long a cached description is trusted before it is re-asked. Descriptions
/// change on the order of years; a tombstone is worth re-checking eventually
/// because coverage grows, but not often.
const BIO_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// The cached description for an entity, fetching it if we've never asked (or
/// haven't asked in a month).
///
/// Providers are only ever called from here — never from the browser. The
/// attribution rides with the text because Last.fm's is CC BY-SA and showing
/// one without the other breaks the licence.
async fn bio_for(
    state: &Arc<AppState>,
    entity_id: i64,
    kind: crate::metadata::bio::BioKind,
    name: &str,
    artist: &str,
    mbid: Option<&str>,
) -> serde_json::Value {
    use crate::metadata::bio::{fetch_bio, BioQuery};

    let now = chrono::Utc::now().timestamp();
    if let Ok(Some(row)) = state.db.get_bio(entity_id).await {
        if now - row.fetched_at < BIO_TTL_SECS {
            return serde_json::json!({
                "text": row.bio,
                "attribution": row.attribution,
                "provider": row.provider,
                "url": row.url,
            });
        }
    }

    let q = BioQuery {
        kind,
        name: name.to_string(),
        artist: artist.to_string(),
        mbid: mbid.map(str::to_string),
    };
    let found = fetch_bio(&state.bio_providers, &q).await;

    // Written either way: `None` is the tombstone that stops us asking again.
    let _ = state
        .db
        .set_bio(
            entity_id,
            found.as_ref().map(|b| b.text.as_str()),
            found.as_ref().and_then(|b| b.attribution.as_deref()),
            found.as_ref().map(|b| b.provider),
            found.as_ref().and_then(|b| b.url.as_deref()),
        )
        .await;

    match found {
        Some(b) => serde_json::json!({
            "text": b.text, "attribution": b.attribution,
            "provider": b.provider, "url": b.url,
        }),
        None => serde_json::json!({
            "text": null, "attribution": null, "provider": null, "url": null,
        }),
    }
}

// ── GET /api/v1/albums/:id ──

pub async fn album_detail(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let entity = match state.db.get_entity(id).await {
        Ok(Some(e)) if e.kind == "release" => e,
        Ok(Some(_)) => return err(StatusCode::BAD_REQUEST, "That entity is not an album"),
        Ok(None) => return not_found("Album"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let (album, artist) = (entity.name.clone(), entity.artist_name.clone());
    let summary = match state.db.album_summary(user.user_id, &artist, &album).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let tracks = state.db.album_tracks(user.user_id, &artist, &album).await.unwrap_or_default();
    let more =
        state.db.artist_albums(user.user_id, &artist, Some(&album), 4).await.unwrap_or_default();
    // A hand-set cover beats the derived one outright, including its CAA id —
    // leaving those in place would have `CoverArt` fall back to the release the
    // listens claim the moment the chosen URL fails to load, which is exactly
    // the picture the operator said no to.
    let override_url = state.db.entity_image(id).await.ok().flatten();
    let artwork = match &override_url {
        Some(_) => None,
        None => state.db.album_artwork(user.user_id, &artist, &album).await.ok().flatten(),
    };

    // The artist gets an entity too, so the header link has somewhere to go.
    let artist_entity_id = state.db.upsert_entity("artist", &artist, "", None).await.ok();

    let loved = state.db.is_loved(user.user_id, id).await.unwrap_or(false);
    let bio = bio_for(
        &state,
        id,
        crate::metadata::bio::BioKind::Album,
        &album,
        &artist,
        entity.mbid.as_deref(),
    )
    .await;

    // A record with no plays is still a real page — it just has zeroes on it.
    // Falling through to a 404 would be wrong for something you annotated once
    // and then removed the listens for.
    // The fourth card used to be a lossless share of this record's plays, which
    // on an imported history is a percentage of nothing: 97% of a real export
    // carries no format at all, so a record with 25 imported plays read "0%
    // lossless" — an accusation about a rip Tapedeck never saw. Fidelity is
    // reported on the dashboard and Statistics now, against the population it
    // is actually known for (`FIDELITY_KNOWN`), and this slot takes a number
    // that always has an answer instead.
    //
    // Share of the *artist*, mirroring the artist page's share of all
    // listening: the dark card on both entity pages is now "how much of the
    // thing one level up is this?".
    let artist_plays = state.db.artist_plays(user.user_id, &artist).await.unwrap_or(0);

    let stats = match &summary {
        Some(s) => {
            let (year, day) = year_and_day(s.first_heard);
            vec![
                stat(
                    "Your Plays",
                    s.plays.to_string(),
                    format!(
                        "across {} track{}",
                        s.track_count,
                        if s.track_count == 1 { "" } else { "s" }
                    ),
                    false,
                ),
                stat("Hours", hours(s.seconds), "on the deck".into(), false),
                stat("First Heard", year, day, false),
                stat(
                    "Share",
                    format!("{}%", pct(s.plays, artist_plays)),
                    format!("of your {artist}"),
                    true,
                ),
            ]
        }
        None => vec![
            stat("Your Plays", "0".into(), "never played".into(), false),
            stat("Hours", "0m".into(), "on the deck".into(), false),
            stat("First Heard", "—".into(), String::new(), false),
            stat("Share", "—".into(), format!("of your {artist}"), true),
        ],
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": entity.id,
            "kind": "album",
            "title": album,
            "artist": artist,
            "artist_entity_id": artist_entity_id,
            "mbid": entity.mbid,
            "year": summary.as_ref().and_then(|s| s.year),
            "track_count": summary.as_ref().map(|s| s.track_count).unwrap_or(0),
            // Time *you* spent on this record, not the record's length — the
            // album's own runtime is the sum of the distinct track durations
            // below, which the client already has.
            "listening_seconds": summary.as_ref().map(|s| s.seconds).unwrap_or(0),
            "plays": summary.as_ref().map(|s| s.plays).unwrap_or(0),
            "artwork_url": override_url.clone().or_else(|| artwork.as_ref().and_then(|a| a.0.clone())),
            "caa_id": artwork.as_ref().and_then(|a| a.1),
            "caa_release_mbid": artwork.as_ref().and_then(|a| a.2.clone()),
            // So the page can offer "reset to the cover from your listens"
            // rather than only "clear", which would read as "no cover at all".
            "cover_override": override_url.is_some(),
            "loved": loved,
            "bio": bio,
            "stats": stats,
            "tracks": tracks,
            "more": more,
        })),
    )
        .into_response()
}

// ── GET /api/v1/artists/:id ──

pub async fn artist_detail(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let entity = match state.db.get_entity(id).await {
        Ok(Some(e)) if e.kind == "artist" => e,
        Ok(Some(_)) => return err(StatusCode::BAD_REQUEST, "That entity is not an artist"),
        Ok(None) => return not_found("Artist"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let name = entity.name.clone();
    let summary = match state.db.artist_summary(user.user_id, &name).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let albums = state.db.artist_albums(user.user_id, &name, None, 6).await.unwrap_or_default();
    let top_tracks = state.db.artist_top_tracks(user.user_id, &name, 10).await.unwrap_or_default();
    let trend = state.db.artist_trend(user.user_id, &name).await.unwrap_or_default();
    let (rank, artist_total) = state.db.artist_rank(user.user_id, &name).await.unwrap_or((0, 0));
    let all_plays = state.db.total_plays(user.user_id).await.unwrap_or(0);
    let loved = state.db.is_loved(user.user_id, id).await.unwrap_or(false);
    // Same portrait the top-artist charts use — the enrichment chain already
    // stores it on the artist row, it just had nowhere to be shown here.
    let image_url = state
        .db
        .artist_images(std::slice::from_ref(&name))
        .await
        .unwrap_or_default()
        .into_values()
        .next();
    // The wide one, for the Reports chapter tile. Nothing supplies it
    // automatically, so it is present only where the user chose one.
    let landscape_url = state
        .db
        .artist_landscapes(std::slice::from_ref(&name))
        .await
        .unwrap_or_default()
        .into_values()
        .next();
    // The `artists` row is a different thing from this entity — it is keyed by
    // name and shared across users — and it is what the portrait proxy and the
    // manual override address. Created on demand so the override is always
    // available, even for an artist the backfill hasn't reached.
    let artist_row_id = state.db.artist_row_id(&name).await.unwrap_or(None);
    let bio = bio_for(
        &state,
        id,
        crate::metadata::bio::BioKind::Artist,
        &name,
        "",
        entity.mbid.as_deref(),
    )
    .await;

    let stats = match &summary {
        Some(s) => {
            let (year, day) = year_and_day(s.first_heard);
            vec![
                stat(
                    "Your Plays",
                    s.plays.to_string(),
                    format!("{} track{}", s.track_count, if s.track_count == 1 { "" } else { "s" }),
                    false,
                ),
                stat("Hours", hours(s.seconds), "on the deck".into(), false),
                stat("First Heard", year, day, false),
                stat(
                    "Share",
                    format!("{}%", pct(s.plays, all_plays)),
                    "of your listening".into(),
                    true,
                ),
            ]
        }
        None => vec![
            stat("Your Plays", "0".into(), "never played".into(), false),
            stat("Hours", "0m".into(), "on the deck".into(), false),
            stat("First Heard", "—".into(), String::new(), false),
            stat("Share", "—".into(), "of your listening".into(), true),
        ],
    };

    let trend: Vec<_> = trend
        .into_iter()
        .map(|(label, value)| serde_json::json!({ "label": label, "value": value }))
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": entity.id,
            "kind": "artist",
            "name": name,
            "mbid": entity.mbid,
            "image_url": image_url,
            "landscape_url": landscape_url,
            "artist_row_id": artist_row_id,
            "rank": rank,
            "artist_total": artist_total,
            "plays": summary.as_ref().map(|s| s.plays).unwrap_or(0),
            "album_count": summary.as_ref().map(|s| s.album_count).unwrap_or(0),
            "track_count": summary.as_ref().map(|s| s.track_count).unwrap_or(0),
            "loved": loved,
            "bio": bio,
            "stats": stats,
            "albums": albums,
            "top_tracks": top_tracks,
            "trend": trend,
        })),
    )
        .into_response()
}

// ── GET /api/v1/charts ──

#[derive(Debug, Deserialize)]
pub struct ChartQuery {
    /// `artists` (default), `albums` or `tracks`.
    pub kind: Option<String>,
    pub range: Option<String>,
    pub limit: Option<i64>,
    /// `plays` (default) or `hours` — the *ranking*, not just the number shown.
    /// Statistics hands off here carrying its own metric, and the two lists
    /// disagreeing would defeat the point of the handoff.
    pub metric: Option<String>,
}

/// Ranked chart over one window. The Statistics page shows the head of these
/// three lists; this serves the whole thing for the dedicated charts screen.
pub async fn charts(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ChartQuery>,
) -> impl IntoResponse {
    let kind = match q.kind.as_deref().unwrap_or("artists") {
        k @ ("artists" | "albums" | "tracks") => k,
        _ => return err(StatusCode::BAD_REQUEST, "kind must be artists, albums or tracks"),
    };
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let after = crate::server::scrobbles::range_after(q.range.as_deref());

    let by_hours = q.metric.as_deref() == Some("hours");
    match state.db.chart(user.user_id, kind, after, limit, by_hours).await {
        Ok(rows) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "kind": kind,
                "range": q.range.unwrap_or_else(|| "all".into()),
                "rows": rows,
            })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ── GET /api/v1/resolve ──

#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    /// `album` / `release`, `artist`, or `track` / `recording`.
    pub kind: String,
    pub name: String,
    /// Required for an album or a track: the same title by two artists is two
    /// different records.
    pub artist: Option<String>,
}

/// Name → entity id, creating the entity if this is the first time anyone has
/// pointed at it. Lets a history row link to `/album/<id>` without the browser
/// knowing any ids.
pub async fn resolve(
    State(state): State<Arc<AppState>>,
    // Unused, and required anyway: `entities` is a shared table with no
    // `user_id`, so there is nothing here to scope — but this endpoint *writes*,
    // and an unauthenticated caller must not be able to fill it with rows.
    _user: AuthUser,
    Query(q): Query<ResolveQuery>,
) -> impl IntoResponse {
    let kind = match crate::notes::RefKind::parse(&q.kind).and_then(|k| k.entity_kind()) {
        Some(k) => k,
        None => return err(StatusCode::BAD_REQUEST, "kind must be track, album or artist"),
    };
    let name = q.name.trim();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name is required");
    }
    // An artist entity's name *is* the artist — carrying it twice would make a
    // second row for the same person. Mirrors `resolve_target` in notes.rs.
    let artist = if kind == "artist" { "" } else { q.artist.as_deref().unwrap_or("").trim() };

    match state.db.upsert_entity(kind, name, artist, None).await {
        Ok(id) => {
            (StatusCode::OK, Json(serde_json::json!({ "id": id, "kind": kind }))).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// ── GET /api/v1/albums/:id/tracklist ──

/// The record's *own* tracklist, from MusicBrainz — including the tracks you
/// have never played.
///
/// The album page is otherwise built entirely from listens, which means a record
/// you have played twice shows two tracks and looks like a two-track record.
/// That is honest about the listening and useless as a picture of the object.
///
/// Deliberately **not** merged into `album_detail`: this costs a rate-limited
/// call to somebody else's server, and the page has to render instantly and
/// without one. It is behind a button for the same reason the Cover Art Archive
/// pull is — an explicit ask, not a page load.
///
/// Nothing is stored. The response is the record as MusicBrainz has it, and the
/// browser matches it against the plays it already holds; writing a tracklist
/// into `scrobbles` would be inventing listens that never happened.
async fn album_tracklist(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let entity = match state.db.get_entity(id).await {
        Ok(Some(e)) if e.kind == "release" => e,
        Ok(Some(_)) => return err(StatusCode::BAD_REQUEST, "That entity is not an album"),
        Ok(None) => return not_found("Album"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let (album, artist) = (entity.name.clone(), entity.artist_name.clone());

    // The entity's own MBID first — it is the one somebody may have set by
    // hand. Then whichever release the listens mostly claim, which is what a
    // tagged library gives for free. Only then a search, which is a guess.
    let mbid = match entity.mbid.clone() {
        Some(m) if !m.trim().is_empty() => Some(m),
        _ => state.db.album_release_mbid(user.user_id, &artist, &album).await.unwrap_or(None),
    };

    let candidate = match mbid {
        Some(m) => match crate::metadata::release_lookup::musicbrainz_release(&m).await {
            Ok(found) => found,
            Err(e) => return err(StatusCode::BAD_GATEWAY, format!("MusicBrainz: {e}")),
        },
        None => {
            // `artist:"…" AND release:"…"` rather than a bare string: the
            // unquoted form matches the words anywhere and cheerfully returns a
            // different record by a different artist.
            let query = format!("artist:\"{}\" AND release:\"{}\"", artist, album);
            match crate::metadata::release_lookup::musicbrainz_search(&query, 1).await {
                Ok(mut hits) => hits.drain(..).next(),
                Err(e) => return err(StatusCode::BAD_GATEWAY, format!("MusicBrainz: {e}")),
            }
        }
    };

    let Some(candidate) = candidate else {
        // Not an error: plenty of a real library is not in MusicBrainz, and
        // those are exactly the records this app exists to keep. 404 with a
        // sentence rather than a 500 with a stack of nothing.
        return err(StatusCode::NOT_FOUND, "MusicBrainz has no release matching this record");
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "source": candidate.source,
            "mbid_release": candidate.mbid_release,
            "title": candidate.title,
            "artist": candidate.artist,
            "year": candidate.year,
            "format": candidate.format,
            // `side` and `position` come straight from the lookup. A CD is one
            // side, a double LP four — the shelf's own splitter decided that,
            // and the page shows whatever it says rather than re-deriving it.
            "tracks": candidate.tracks,
        })),
    )
        .into_response()
}

pub fn create_library_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/resolve", get(resolve))
        .route("/api/v1/charts", get(charts))
        .route("/api/v1/albums/:id", get(album_detail))
        .route("/api/v1/albums/:id/tracklist", get(album_tracklist))
        .route("/api/v1/artists/:id", get(artist_detail))
}
