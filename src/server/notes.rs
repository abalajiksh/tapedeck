//! Liner notes and loves (Phase 7).
//!
//! A note targets either an **entity** (a recording, release or artist) or one
//! **listen**. Both are addressed through the same `TargetSpec` so the UI can
//! say "note about this album" and "note about this playing" without two
//! different request shapes.
//!
//! Three rules this module keeps:
//!
//! * **Everything is scoped to the caller.** `visibility` is stored from day one
//!   because Phase 6 needs it, but nothing here serves another user's notes yet
//!   — a `public` note is public *later*, not now. Do not add a read path that
//!   forgets this before patches exist.
//! * **Markdown never reaches a client unrendered-and-unsanitised.** `body_html`
//!   is produced here, by `crate::notes`, and that is the only place it happens.
//! * **Static paths register before `/:id`.** `preview` and `for` are literal
//!   segments; matchit prefers static siblings, but the order is kept explicit
//!   the same way `sources.rs` does it.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::{Database, EntityRow, NoteRow};
use crate::notes::{self, RefKind, Resolved};
use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

const VISIBILITIES: [&str; 3] = ["private", "patched", "public"];

fn err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg.into() }))).into_response()
}

fn server_error(e: sqlx::Error) -> axum::response::Response {
    err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// What a note or a love points at. Exactly one of these forms is used:
///
/// * `entity_id` — an entity that already exists.
/// * `scrobble_id` — the listen itself.
/// * `from_scrobble` + `kind` — the track / album / artist *of* that listen,
///   created on demand. This is how the UI turns a row in the history into
///   something annotatable without the browser having to know entity ids.
/// * `kind` + `name` (+ `artist`) — spelled out.
#[derive(Debug, Deserialize, Default)]
pub struct TargetSpec {
    pub entity_id: Option<i64>,
    pub scrobble_id: Option<i64>,
    pub from_scrobble: Option<i64>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub mbid: Option<String>,
}

/// A note target after resolution: an entity id, or a listen id.
enum Target {
    Entity(i64),
    Listen(i64),
}

/// Accepts the shortcode spelling (`track`/`album`) as well as the stored one
/// (`recording`/`release`), because the UI thinks in the former.
fn entity_kind(raw: &str) -> Option<&'static str> {
    RefKind::parse(raw).map(|k| k.entity_kind())
}

/// Turn a spec into a concrete target, creating the entity if that's what was
/// asked for. Returns a message rather than an id when the spec doesn't name
/// anything resolvable.
async fn resolve_target(db: &Database, user_id: i64, spec: &TargetSpec) -> Result<Target, String> {
    if let Some(id) = spec.entity_id {
        return match db.get_entity(id).await {
            Ok(Some(_)) => Ok(Target::Entity(id)),
            Ok(None) => Err("No such entity".into()),
            Err(e) => Err(e.to_string()),
        };
    }

    if let Some(id) = spec.scrobble_id {
        return match db.get_scrobble(user_id, id).await {
            Ok(Some(_)) => Ok(Target::Listen(id)),
            Ok(None) => Err("No such listen".into()),
            Err(e) => Err(e.to_string()),
        };
    }

    let kind = spec.kind.as_deref().and_then(entity_kind)
        .ok_or("kind must be one of track, album or artist")?;

    // Derived from a listen: the names come from the stored row, so a note on
    // "the album this was on" can't disagree with the listen it came from.
    if let Some(sid) = spec.from_scrobble {
        let listen = db.get_scrobble(user_id, sid).await
            .map_err(|e| e.to_string())?
            .ok_or("No such listen")?;
        let (name, artist, mbid) = match kind {
            "artist" => (listen.artist.clone(), String::new(), listen.mbid_artist.clone()),
            "release" => (
                listen.album.clone().filter(|a| !a.trim().is_empty())
                    .ok_or("That listen has no album to write about")?,
                listen.artist.clone(),
                listen.mbid_release.clone(),
            ),
            _ => (listen.title.clone(), listen.artist.clone(), listen.mbid_recording.clone()),
        };
        let id = db.upsert_entity(kind, &name, &artist, mbid.as_deref()).await
            .map_err(|e| e.to_string())?;
        return Ok(Target::Entity(id));
    }

    let name = spec.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
        .ok_or("name is required")?;
    // An artist entity's name *is* the artist; carrying it twice would make
    // "Aphex Twin" and "Aphex Twin by Aphex Twin" two different rows.
    let artist = if kind == "artist" { "" } else { spec.artist.as_deref().unwrap_or("").trim() };
    let id = db.upsert_entity(kind, name, artist, spec.mbid.as_deref()).await
        .map_err(|e| e.to_string())?;
    Ok(Target::Entity(id))
}

/// Artist and album in scope for a note's shortcodes, so `[[track:3]]` and a
/// bare `[[album:…]]` mean what the writer expects.
async fn note_context(db: &Database, user_id: i64, note: &NoteRow)
    -> (Option<String>, Option<String>)
{
    if let Some(sid) = note.scrobble_id {
        if let Ok(Some(s)) = db.get_scrobble(user_id, sid).await {
            return (Some(s.artist), s.album);
        }
        return (None, None);
    }
    let Some(eid) = note.entity_id else { return (None, None) };
    let Ok(Some(e)) = db.get_entity(eid).await else { return (None, None) };
    match e.kind.as_str() {
        "artist" => (Some(e.name), None),
        "release" => (Some(e.artist_name), Some(e.name)),
        // For a track, the album comes from where this user actually heard it.
        _ => {
            let album = db.album_for_track(user_id, &e.name, &e.artist_name).await.ok().flatten();
            (Some(e.artist_name), album)
        }
    }
}

/// Markdown → HTML with every shortcode resolved against this user's entities
/// and listening history.
async fn render(
    db: &Database, user_id: i64, body_md: &str,
    ctx_artist: Option<&str>, ctx_album: Option<&str>,
) -> String {
    let mut resolved: HashMap<(RefKind, String), Resolved> = HashMap::new();
    for code in notes::scan(body_md) {
        let key = code.key();
        if resolved.contains_key(&key) { continue; }
        let hit = db.resolve_reference(
            user_id, code.kind.entity_kind(), &code.target,
            code.position(), ctx_artist, ctx_album,
        ).await;
        if let Ok(Some((entity_id, label))) = hit {
            resolved.insert(key, Resolved { entity_id, label });
        }
    }
    notes::render(body_md, &resolved)
}

/// One note, with everything a screen needs: the target it hangs off, the
/// rendered body, whether the target is loved, and how many earlier versions
/// exist.
async fn note_json(db: &Database, user_id: i64, note: &NoteRow, with_html: bool) -> serde_json::Value {
    let (ctx_artist, ctx_album) = note_context(db, user_id, note).await;
    let entity: Option<EntityRow> = match note.entity_id {
        Some(id) => db.get_entity(id).await.ok().flatten(),
        None => None,
    };
    let listen = match note.scrobble_id {
        Some(id) => db.get_scrobble(user_id, id).await.ok().flatten().map(|s| serde_json::json!({
            "id": s.id, "title": s.title, "artist": s.artist,
            "album": s.album, "timestamp": s.timestamp,
        })),
        None => None,
    };
    let loved = match note.entity_id {
        Some(id) => db.is_loved(user_id, id).await.unwrap_or(false),
        None => false,
    };
    let revisions = db.note_revisions(user_id, note.id).await.map(|r| r.len()).unwrap_or(0);

    let mut out = serde_json::json!({
        "id": note.id,
        "body_md": note.body_md,
        "excerpt": notes::excerpt(&note.body_md, 140),
        "visibility": note.visibility,
        "created_at": note.created_at,
        "updated_at": note.updated_at,
        "entity": entity,
        "listen": listen,
        "loved": loved,
        "revisions": revisions,
    });
    if with_html {
        let html = render(db, user_id, &note.body_md, ctx_artist.as_deref(), ctx_album.as_deref()).await;
        out["body_html"] = serde_json::Value::String(html);
    }
    out
}

// ── Handlers ──

#[derive(Deserialize)]
struct SaveNoteRequest {
    #[serde(flatten)]
    target: TargetSpec,
    body_md: String,
    visibility: Option<String>,
}

/// POST /api/v1/notes — write the note for a target, creating it or editing the
/// one already there. The previous body is archived by the database layer.
async fn save_note(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SaveNoteRequest>,
) -> impl IntoResponse {
    let visibility = body.visibility.unwrap_or_else(|| "private".into());
    if !VISIBILITIES.contains(&visibility.as_str()) {
        return err(StatusCode::BAD_REQUEST, format!("visibility must be one of {}", VISIBILITIES.join(", ")));
    }
    if body.body_md.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "A note needs something in it");
    }

    let target = match resolve_target(&state.db, user.user_id, &body.target).await {
        Ok(t) => t,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    let (entity_id, scrobble_id) = match target {
        Target::Entity(id) => (Some(id), None),
        Target::Listen(id) => (None, Some(id)),
    };

    match state.db.upsert_note(user.user_id, entity_id, scrobble_id, &body.body_md, &visibility).await {
        Ok(id) => match state.db.get_note(user.user_id, id).await {
            Ok(Some(note)) => (StatusCode::OK, Json(note_json(&state.db, user.user_id, &note, true).await)).into_response(),
            Ok(None) => err(StatusCode::INTERNAL_SERVER_ERROR, "Note vanished after saving"),
            Err(e) => server_error(e),
        },
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/notes — everything this user has written, newest edit first.
async fn list_notes(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_notes(user.user_id).await {
        Ok(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            for note in &rows {
                out.push(note_json(&state.db, user.user_id, note, false).await);
            }
            (StatusCode::OK, Json(serde_json::json!({ "notes": out }))).into_response()
        }
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/notes/:id
async fn get_note(
    State(state): State<Arc<AppState>>, user: AuthUser, Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_note(user.user_id, id).await {
        Ok(Some(note)) => (StatusCode::OK, Json(note_json(&state.db, user.user_id, &note, true).await)).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "No such note"),
        Err(e) => server_error(e),
    }
}

/// DELETE /api/v1/notes/:id — drops the revisions with it.
async fn delete_note(
    State(state): State<Arc<AppState>>, user: AuthUser, Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_note(user.user_id, id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "deleted" }))).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "No such note"),
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/notes/:id/revisions — earlier bodies, newest first.
async fn note_revisions(
    State(state): State<Arc<AppState>>, user: AuthUser, Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.note_revisions(user.user_id, id).await {
        Ok(rows) => {
            let out: Vec<_> = rows.iter().map(|r| serde_json::json!({
                "id": r.id,
                "body_md": r.body_md,
                "excerpt": notes::excerpt(&r.body_md, 140),
                "created_at": r.created_at,
            })).collect();
            (StatusCode::OK, Json(serde_json::json!({ "revisions": out }))).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
struct PreviewRequest {
    #[serde(flatten)]
    target: TargetSpec,
    body_md: String,
}

/// POST /api/v1/notes/preview — render without saving, so the editor can show
/// what a shortcode will actually resolve to before you commit to it.
async fn preview(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<PreviewRequest>,
) -> impl IntoResponse {
    // The target only supplies context here; a preview never creates a note,
    // and an unresolvable target just means no album/artist in scope.
    let (ctx_artist, ctx_album) = match resolve_target(&state.db, user.user_id, &body.target).await {
        Ok(Target::Entity(id)) => {
            let stub = NoteRow {
                id: 0, user_id: user.user_id, entity_id: Some(id), scrobble_id: None,
                body_md: String::new(), visibility: "private".into(), created_at: 0, updated_at: 0,
            };
            note_context(&state.db, user.user_id, &stub).await
        }
        Ok(Target::Listen(id)) => {
            let stub = NoteRow {
                id: 0, user_id: user.user_id, entity_id: None, scrobble_id: Some(id),
                body_md: String::new(), visibility: "private".into(), created_at: 0, updated_at: 0,
            };
            note_context(&state.db, user.user_id, &stub).await
        }
        Err(_) => (None, None),
    };

    let html = render(&state.db, user.user_id, &body.body_md, ctx_artist.as_deref(), ctx_album.as_deref()).await;
    (StatusCode::OK, Json(serde_json::json!({ "body_html": html }))).into_response()
}

#[derive(Deserialize)]
struct ForQuery {
    entity_id: Option<i64>,
    scrobble_id: Option<i64>,
    from_scrobble: Option<i64>,
    kind: Option<String>,
    name: Option<String>,
    artist: Option<String>,
}

/// GET /api/v1/notes/for?… — the note for a target, if there is one.
///
/// Answers `{ note: null, target: {…} }` rather than 404 when nothing has been
/// written yet: "no note here" is the normal case, and the caller still needs
/// the resolved target to open an editor on it.
async fn note_for(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ForQuery>,
) -> impl IntoResponse {
    let spec = TargetSpec {
        entity_id: q.entity_id,
        scrobble_id: q.scrobble_id,
        from_scrobble: q.from_scrobble,
        kind: q.kind,
        name: q.name,
        artist: q.artist,
        mbid: None,
    };
    let target = match resolve_target(&state.db, user.user_id, &spec).await {
        Ok(t) => t,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };

    let (note, target_json, loved) = match target {
        Target::Entity(id) => {
            let entity = state.db.get_entity(id).await.ok().flatten();
            let loved = state.db.is_loved(user.user_id, id).await.unwrap_or(false);
            (state.db.note_for_entity(user.user_id, id).await, serde_json::json!({ "entity": entity }), loved)
        }
        Target::Listen(id) => {
            let listen = state.db.get_scrobble(user.user_id, id).await.ok().flatten()
                .map(|s| serde_json::json!({ "id": s.id, "title": s.title, "artist": s.artist, "album": s.album, "timestamp": s.timestamp }));
            (state.db.note_for_scrobble(user.user_id, id).await, serde_json::json!({ "listen": listen }), false)
        }
    };

    match note {
        Ok(Some(note)) => {
            let body = note_json(&state.db, user.user_id, &note, true).await;
            (StatusCode::OK, Json(serde_json::json!({ "note": body, "target": target_json, "loved": loved }))).into_response()
        }
        Ok(None) => (StatusCode::OK, Json(serde_json::json!({ "note": null, "target": target_json, "loved": loved }))).into_response(),
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
struct LoveRequest {
    #[serde(flatten)]
    target: TargetSpec,
    /// Omitted means "love it"; pass false to un-love.
    loved: Option<bool>,
}

/// POST /api/v1/loves — love or un-love an entity.
async fn set_love(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LoveRequest>,
) -> impl IntoResponse {
    let entity_id = match resolve_target(&state.db, user.user_id, &body.target).await {
        Ok(Target::Entity(id)) => id,
        // A love is about the record, not about one playing of it — pointing at
        // a listen is a category error rather than something to guess through.
        Ok(Target::Listen(_)) => return err(StatusCode::BAD_REQUEST, "Love a track, album or artist — not a single listen"),
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };

    let loved = body.loved.unwrap_or(true);
    match state.db.set_love(user.user_id, entity_id, loved).await {
        Ok(()) => {
            let entity = state.db.get_entity(entity_id).await.ok().flatten();
            (StatusCode::OK, Json(serde_json::json!({ "entity": entity, "loved": loved }))).into_response()
        }
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/loves
async fn list_loves(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_loves(user.user_id).await {
        Ok(rows) => {
            let out: Vec<_> = rows.into_iter().map(|(e, loved_at)| serde_json::json!({
                "entity": e, "loved_at": loved_at,
            })).collect();
            (StatusCode::OK, Json(serde_json::json!({ "loves": out }))).into_response()
        }
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/scrobbles/:id/annotations
///
/// What's already been written or loved around one listen: the note on the
/// playing itself, and — for its track, album and artist — the entity **only if
/// it already exists**. Deliberately read-only: inspecting a listen shouldn't
/// mint three entity rows, so the ids come back null until someone actually
/// writes or loves something.
async fn listen_annotations(
    State(state): State<Arc<AppState>>, user: AuthUser, Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(listen)) = state.db.get_scrobble(user.user_id, id).await else {
        return err(StatusCode::NOT_FOUND, "No such listen");
    };

    let mut out = serde_json::json!({
        "listen_note_id": state.db.note_for_scrobble(user.user_id, id).await
            .ok().flatten().map(|n| n.id),
    });

    let album = listen.album.clone().unwrap_or_default();
    let targets: [(&str, &str, &str); 3] = [
        ("track", "recording", &listen.title),
        ("album", "release", &album),
        ("artist", "artist", &listen.artist),
    ];
    for (label, kind, name) in targets {
        if name.trim().is_empty() {
            out[label] = serde_json::Value::Null;
            continue;
        }
        let artist = if kind == "artist" { None } else { Some(listen.artist.as_str()) };
        let entity = state.db.find_entity(kind, name, artist).await.ok().flatten();
        out[label] = match entity {
            Some(e) => serde_json::json!({
                "name": name,
                "entity_id": e.id,
                "note_id": state.db.note_for_entity(user.user_id, e.id).await.ok().flatten().map(|n| n.id),
                "loved": state.db.is_loved(user.user_id, e.id).await.unwrap_or(false),
            }),
            None => serde_json::json!({ "name": name, "entity_id": null, "note_id": null, "loved": false }),
        };
    }
    (StatusCode::OK, Json(out)).into_response()
}

/// GET /api/v1/entities/:id
async fn get_entity(
    State(state): State<Arc<AppState>>, user: AuthUser, Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_entity(id).await {
        Ok(Some(entity)) => {
            let loved = state.db.is_loved(user.user_id, id).await.unwrap_or(false);
            let note = state.db.note_for_entity(user.user_id, id).await.ok().flatten();
            let note_json = match note {
                Some(n) => note_json(&state.db, user.user_id, &n, true).await,
                None => serde_json::Value::Null,
            };
            (StatusCode::OK, Json(serde_json::json!({ "entity": entity, "loved": loved, "note": note_json }))).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "No such entity"),
        Err(e) => server_error(e),
    }
}

pub fn create_notes_router() -> Router<Arc<AppState>> {
    Router::new()
        // Literal segments first, so neither is read as an `:id`.
        .route("/api/v1/notes/preview", post(preview))
        .route("/api/v1/notes/for", get(note_for))
        .route("/api/v1/notes", get(list_notes).post(save_note))
        .route("/api/v1/notes/:id", get(get_note).delete(delete_note))
        .route("/api/v1/notes/:id/revisions", get(note_revisions))
        .route("/api/v1/loves", get(list_loves).post(set_love))
        .route("/api/v1/entities/:id", get(get_entity))
        .route("/api/v1/scrobbles/:id/annotations", get(listen_annotations))
}
