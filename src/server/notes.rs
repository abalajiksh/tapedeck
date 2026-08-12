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

use super::auth::{ReadAuth, WriteAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{Database, EntityRow, NoteRow, NoteTarget};
use crate::notes::{self, RefKind, Resolved};

const VISIBILITIES: [&str; 3] = ["private", "patched", "public"];

fn err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg.into() })))
        .into_response()
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
/// * `period_unit` + `period_key` — a week, a month or a year of listening,
///   named exactly as the Reports page names it.
#[derive(Debug, Deserialize, Default)]
pub struct TargetSpec {
    pub entity_id: Option<i64>,
    pub scrobble_id: Option<i64>,
    /// `week` / `month` / `year`, alongside `period_key`. Both or neither.
    pub period_unit: Option<String>,
    /// One of `reports::period_of`'s keys — `2026`, `2026-08`, `2026-W32`.
    pub period_key: Option<String>,
    pub from_scrobble: Option<i64>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub mbid: Option<String>,
}

/// Accepts the shortcode spelling (`track`/`album`) as well as the stored one
/// (`recording`/`release`), because the UI thinks in the former.
fn entity_kind(raw: &str) -> Option<&'static str> {
    // `gear` parses as a shortcode kind but is not an entity, so it is not a
    // valid note or love target — it falls out here as `None`.
    RefKind::parse(raw).and_then(|k| k.entity_kind())
}

/// Turn a spec into a concrete target, creating the entity if that's what was
/// asked for. Returns a message rather than an id when the spec doesn't name
/// anything resolvable.
async fn resolve_target(
    db: &Database,
    user_id: i64,
    spec: &TargetSpec,
) -> Result<NoteTarget, String> {
    if let Some(id) = spec.entity_id {
        return match db.get_entity(id).await {
            Ok(Some(_)) => Ok(NoteTarget::Entity(id)),
            Ok(None) => Err("No such entity".into()),
            Err(e) => Err(e.to_string()),
        };
    }

    if let Some(id) = spec.scrobble_id {
        return match db.get_scrobble(user_id, id).await {
            Ok(Some(_)) => Ok(NoteTarget::Listen(id)),
            Ok(None) => Err("No such listen".into()),
            Err(e) => Err(e.to_string()),
        };
    }

    // A period is not an entity and never becomes one: there is no row to
    // create, because a report is folded out of the history on every request.
    // Validated rather than trusted — the key *is* the note's identity, so a
    // malformed one files a note the page that wrote it can never find again,
    // which reads to its author as a save that silently did nothing.
    if let (Some(unit), Some(key)) = (spec.period_unit.as_deref(), spec.period_key.as_deref()) {
        return NoteTarget::period(unit, key)
            .ok_or_else(|| format!("Not a period Reports names: {unit} {key}"));
    }

    let kind = spec
        .kind
        .as_deref()
        .and_then(entity_kind)
        .ok_or("kind must be one of track, album or artist")?;

    // Derived from a listen: the names come from the stored row, so a note on
    // "the album this was on" can't disagree with the listen it came from.
    if let Some(sid) = spec.from_scrobble {
        let listen = db
            .get_scrobble(user_id, sid)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("No such listen")?;
        let (name, artist, mbid) = match kind {
            "artist" => (listen.artist.clone(), String::new(), listen.mbid_artist.clone()),
            "release" => (
                listen
                    .album
                    .clone()
                    .filter(|a| !a.trim().is_empty())
                    .ok_or("That listen has no album to write about")?,
                listen.artist.clone(),
                listen.mbid_release.clone(),
            ),
            _ => (listen.title.clone(), listen.artist.clone(), listen.mbid_recording.clone()),
        };
        let id = db
            .upsert_entity(kind, &name, &artist, mbid.as_deref())
            .await
            .map_err(|e| e.to_string())?;
        return Ok(NoteTarget::Entity(id));
    }

    let name =
        spec.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).ok_or("name is required")?;
    // An artist entity's name *is* the artist; carrying it twice would make
    // "Aphex Twin" and "Aphex Twin by Aphex Twin" two different rows.
    let artist = if kind == "artist" { "" } else { spec.artist.as_deref().unwrap_or("").trim() };
    let id = db
        .upsert_entity(kind, name, artist, spec.mbid.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    Ok(NoteTarget::Entity(id))
}

/// Artist and album in scope for a note's shortcodes, so `[[track:3]]` and a
/// bare `[[album:…]]` mean what the writer expects.
async fn note_context(
    db: &Database,
    user_id: i64,
    note: &NoteRow,
) -> (Option<String>, Option<String>) {
    if let Some(sid) = note.scrobble_id {
        if let Ok(Some(s)) = db.get_scrobble(user_id, sid).await {
            return (Some(s.artist), s.album);
        }
        return (None, None);
    }
    let Some(eid) = note.entity_id else {
        return (None, None);
    };
    let Ok(Some(e)) = db.get_entity(eid).await else {
        return (None, None);
    };
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

/// What a note is *about*, as one line.
///
/// The same string names the note in a listing, titles its exported file and is
/// what a wikilink in another exported note points at — so it is computed once,
/// here, rather than three times in three shapes.
fn entity_title(e: &EntityRow) -> String {
    if e.artist_name.trim().is_empty() {
        e.name.clone()
    } else {
        format!("{} — {}", e.name, e.artist_name)
    }
}

/// Markdown → HTML with every shortcode resolved against this user's entities
/// and listening history.
async fn render(
    db: &Database,
    user_id: i64,
    body_md: &str,
    ctx_artist: Option<&str>,
    ctx_album: Option<&str>,
) -> String {
    let mut resolved: HashMap<(RefKind, String), Resolved> = HashMap::new();
    for code in notes::scan(body_md) {
        let key = code.key();
        if resolved.contains_key(&key) {
            continue;
        }
        // Gear is the caller's own equipment, on its own route; the other three
        // are entities. This is the only place the two are told apart, which is
        // why `notes.rs` takes an href rather than building one.
        let Some(entity_kind) = code.kind.entity_kind() else {
            if let Ok(Some((id, label))) = db.find_equipment_by_name(user_id, &code.target).await {
                resolved.insert(key, Resolved { label, href: format!("/chains?gear={}", id) });
            }
            continue;
        };
        let hit = db
            .resolve_reference(
                user_id,
                entity_kind,
                &code.target,
                code.position(),
                ctx_artist,
                ctx_album,
            )
            .await;
        if let Ok(Some((entity_id, label))) = hit {
            resolved.insert(key, Resolved { label, href: format!("/notes?entity={}", entity_id) });
        }
    }
    notes::render(body_md, &resolved)
}

/// One note, with everything a screen needs: the target it hangs off, the
/// rendered body, whether the target is loved, and how many earlier versions
/// exist.
async fn note_json(
    db: &Database,
    user_id: i64,
    note: &NoteRow,
    with_html: bool,
) -> serde_json::Value {
    let (ctx_artist, ctx_album) = note_context(db, user_id, note).await;
    let entity: Option<EntityRow> = match note.entity_id {
        Some(id) => db.get_entity(id).await.ok().flatten(),
        None => None,
    };
    let listen = match note.scrobble_id {
        Some(id) => db.get_scrobble(user_id, id).await.ok().flatten().map(|s| {
            serde_json::json!({
                "id": s.id, "title": s.title, "artist": s.artist,
                "album": s.album, "timestamp": s.timestamp,
            })
        }),
        None => None,
    };
    let loved = match note.entity_id {
        Some(id) => db.is_loved(user_id, id).await.unwrap_or(false),
        None => false,
    };
    let revisions = db.note_revisions(user_id, note.id).await.map(|r| r.len()).unwrap_or(0);

    // Split back into the pair the caller asked with, rather than handed back
    // as the stored `month:2026-08` — the Notes list has to name the period, and
    // formatting "2026-08" as "August 2026" needs to know which of the three it
    // is without parsing the key's shape a second time.
    let period = note.period_key.as_deref().map(|key| {
        let (unit, k) = key.split_once(':').unwrap_or(("", key));
        serde_json::json!({ "unit": unit, "key": k })
    });

    let mut out = serde_json::json!({
        "id": note.id,
        "body_md": note.body_md,
        "excerpt": notes::excerpt(&note.body_md, 140),
        "visibility": note.visibility,
        "created_at": note.created_at,
        "updated_at": note.updated_at,
        "entity": entity,
        "listen": listen,
        "period": period,
        "loved": loved,
        "revisions": revisions,
    });
    if with_html {
        let html =
            render(db, user_id, &note.body_md, ctx_artist.as_deref(), ctx_album.as_deref()).await;
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
    WriteAuth { user }: WriteAuth,
    Json(body): Json<SaveNoteRequest>,
) -> impl IntoResponse {
    let visibility = body.visibility.unwrap_or_else(|| "private".into());
    if !VISIBILITIES.contains(&visibility.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("visibility must be one of {}", VISIBILITIES.join(", ")),
        );
    }
    if body.body_md.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "A note needs something in it");
    }

    let target = match resolve_target(&state.db, user.user_id, &body.target).await {
        Ok(t) => t,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    match state.db.upsert_note(user.user_id, &target, &body.body_md, &visibility).await {
        Ok(id) => match state.db.get_note(user.user_id, id).await {
            Ok(Some(note)) => {
                (StatusCode::OK, Json(note_json(&state.db, user.user_id, &note, true).await))
                    .into_response()
            }
            Ok(None) => err(StatusCode::INTERNAL_SERVER_ERROR, "Note vanished after saving"),
            Err(e) => server_error(e),
        },
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize, Default)]
struct ListQuery {
    /// Free text, matched against the body *and* the subject.
    q: Option<String>,
    /// `recording` / `release` / `artist` / `listen`. Anything else is no
    /// filter — a stale bookmark should show everything, not nothing.
    kind: Option<String>,
    /// `updated` (default), `created` or `title`.
    sort: Option<String>,
}

/// GET /api/v1/notes — everything this user has written, newest edit first.
///
/// Takes a query and a kind because a notes collection is a thing that grows: at
/// twenty notes a flat list is fine and at two hundred it is a wall. Both are
/// applied in SQL rather than here — see `Database::search_notes` for why.
///
/// `total` is the unfiltered count, so the UI can say "12 of 240" and a search
/// returning nothing reads as a search rather than as an empty account.
async fn list_notes(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let query = q.q.unwrap_or_default();
    let kind = q.kind.unwrap_or_default();
    let sort = q.sort.unwrap_or_default();

    match state.db.search_notes(user.user_id, &query, &kind, &sort).await {
        Ok(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            for note in &rows {
                out.push(note_json(&state.db, user.user_id, note, false).await);
            }
            // Only worth a second query when something was actually filtered.
            let total = if query.trim().is_empty() && kind.is_empty() {
                out.len()
            } else {
                state.db.list_notes(user.user_id).await.map(|r| r.len()).unwrap_or(out.len())
            };
            (StatusCode::OK, Json(serde_json::json!({ "notes": out, "total": total })))
                .into_response()
        }
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/notes/:id
async fn get_note(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_note(user.user_id, id).await {
        Ok(Some(note)) => {
            (StatusCode::OK, Json(note_json(&state.db, user.user_id, &note, true).await))
                .into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "No such note"),
        Err(e) => server_error(e),
    }
}

/// DELETE /api/v1/notes/:id — drops the revisions with it.
async fn delete_note(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_note(user.user_id, id).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "status": "deleted" }))).into_response()
        }
        Ok(false) => err(StatusCode::NOT_FOUND, "No such note"),
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/notes/:id/revisions — earlier bodies, newest first.
async fn note_revisions(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.note_revisions(user.user_id, id).await {
        Ok(rows) => {
            let out: Vec<_> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "body_md": r.body_md,
                        "excerpt": notes::excerpt(&r.body_md, 140),
                        "created_at": r.created_at,
                    })
                })
                .collect();
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
    WriteAuth { user }: WriteAuth,
    Json(body): Json<PreviewRequest>,
) -> impl IntoResponse {
    // The target only supplies context here; a preview never creates a note,
    // and an unresolvable target just means no album/artist in scope.
    let (ctx_artist, ctx_album) = match resolve_target(&state.db, user.user_id, &body.target).await
    {
        Ok(NoteTarget::Entity(id)) => {
            let stub = NoteRow {
                id: 0,
                user_id: user.user_id,
                entity_id: Some(id),
                scrobble_id: None,
                period_key: None,
                body_md: String::new(),
                visibility: "private".into(),
                created_at: 0,
                updated_at: 0,
            };
            note_context(&state.db, user.user_id, &stub).await
        }
        Ok(NoteTarget::Listen(id)) => {
            let stub = NoteRow {
                id: 0,
                user_id: user.user_id,
                entity_id: None,
                scrobble_id: Some(id),
                period_key: None,
                body_md: String::new(),
                visibility: "private".into(),
                created_at: 0,
                updated_at: 0,
            };
            note_context(&state.db, user.user_id, &stub).await
        }
        // A period has no record in scope, so a bare `[[album:…]]` in it has
        // nothing to be relative *to* — which is right: a month is not about
        // one record. Named shortcodes still resolve; only the positional
        // `[[track:3]]` loses its meaning, and it renders as plain text.
        Ok(NoteTarget::Period(_)) | Err(_) => (None, None),
    };

    let html =
        render(&state.db, user.user_id, &body.body_md, ctx_artist.as_deref(), ctx_album.as_deref())
            .await;
    (StatusCode::OK, Json(serde_json::json!({ "body_html": html }))).into_response()
}

#[derive(Deserialize)]
struct ForQuery {
    entity_id: Option<i64>,
    scrobble_id: Option<i64>,
    period_unit: Option<String>,
    period_key: Option<String>,
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
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ForQuery>,
) -> impl IntoResponse {
    let spec = TargetSpec {
        entity_id: q.entity_id,
        scrobble_id: q.scrobble_id,
        period_unit: q.period_unit,
        period_key: q.period_key,
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
        NoteTarget::Entity(id) => {
            let entity = state.db.get_entity(id).await.ok().flatten();
            let loved = state.db.is_loved(user.user_id, id).await.unwrap_or(false);
            (
                state.db.note_for_entity(user.user_id, id).await,
                serde_json::json!({ "entity": entity }),
                loved,
            )
        }
        NoteTarget::Listen(id) => {
            let listen = state.db.get_scrobble(user.user_id, id).await.ok().flatten()
                .map(|s| serde_json::json!({ "id": s.id, "title": s.title, "artist": s.artist, "album": s.album, "timestamp": s.timestamp }));
            (
                state.db.note_for_scrobble(user.user_id, id).await,
                serde_json::json!({ "listen": listen }),
                false,
            )
        }
        NoteTarget::Period(ref key) => {
            // The key is echoed back split rather than raw, because that is how
            // the caller asked for it and how Reports names its own periods.
            let (unit, k) = key.split_once(':').unwrap_or(("", key.as_str()));
            (
                state.db.note_for_period(user.user_id, key).await,
                serde_json::json!({ "period": { "unit": unit, "key": k } }),
                false,
            )
        }
    };

    match note {
        Ok(Some(note)) => {
            let body = note_json(&state.db, user.user_id, &note, true).await;
            (
                StatusCode::OK,
                Json(serde_json::json!({ "note": body, "target": target_json, "loved": loved })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::OK,
            Json(serde_json::json!({ "note": null, "target": target_json, "loved": loved })),
        )
            .into_response(),
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
    WriteAuth { user }: WriteAuth,
    Json(body): Json<LoveRequest>,
) -> impl IntoResponse {
    let entity_id = match resolve_target(&state.db, user.user_id, &body.target).await {
        Ok(NoteTarget::Entity(id)) => id,
        // A love is about the record, not about one playing of it — pointing at
        // a listen is a category error rather than something to guess through.
        Ok(NoteTarget::Listen(_)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "Love a track, album or artist — not a single listen",
            )
        }
        // Same category error one step further out: a love points at a thing,
        // and a month is not a thing you own.
        Ok(NoteTarget::Period(_)) => {
            return err(
                StatusCode::BAD_REQUEST,
                "Love a track, album or artist — not a period of listening",
            )
        }
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };

    let loved = body.loved.unwrap_or(true);
    match state.db.set_love(user.user_id, entity_id, loved).await {
        Ok(()) => {
            let entity = state.db.get_entity(entity_id).await.ok().flatten();

            // Mirror it outward, but only for recordings — see `set_loved` on
            // the sink trait. Spawned rather than awaited: the love is already
            // stored, and the UI shouldn't wait on Last.fm to fill a heart.
            if let Some(e) = entity.as_ref().filter(|e| e.kind == "recording") {
                let (db, url) = (state.db.clone(), state.listenbrainz_url.clone());
                let (artist, title, mbid, uid) =
                    (e.artist_name.clone(), e.name.clone(), e.mbid.clone(), user.user_id);
                tokio::spawn(async move {
                    let sinks = crate::sinks::sinks_for_user(&db, uid, &url).await;
                    crate::sinks::submit_love(&sinks, &artist, &title, mbid.as_deref(), loved)
                        .await;
                });
            }

            (StatusCode::OK, Json(serde_json::json!({ "entity": entity, "loved": loved })))
                .into_response()
        }
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/loves/keys — every loved entity as its identity key.
///
/// The history and dashboard put a heart on every row, and asking per row would
/// be one request per listen. The key is deterministic (`kind|artist|name`,
/// lowercased — `Database::entity_key`), so the client fetches this once and
/// matches locally. Entities are only created when something is annotated or
/// loved, so this stays small even on a large library.
async fn love_keys(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.loved_keys(user.user_id).await {
        Ok(keys) => (StatusCode::OK, Json(serde_json::json!({ "keys": keys }))).into_response(),
        Err(e) => server_error(e),
    }
}

/// GET /api/v1/loves
async fn list_loves(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    // `entity` is kept alongside the flattened fields so the shape stays
    // backwards-compatible for anything already reading it — the Loved page
    // wants plays, artwork and the note, which the entity row has never had.
    match state.db.loves_detailed(user.user_id).await {
        Ok(rows) => {
            let out: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "entity": {
                            "id": r.id, "kind": r.kind, "name": r.name,
                            "artist_name": r.artist_name, "mbid": r.mbid,
                        },
                        "loved_at": r.loved_at,
                        "plays": r.plays,
                        "last_played": r.last_played,
                        "album": r.album,
                        "artwork_url": r.artwork_url,
                        "caa_id": r.caa_id,
                        "caa_release_mbid": r.caa_release_mbid,
                        "note_id": r.note_id,
                        "note_excerpt": r.note_excerpt,
                    })
                })
                .collect();
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
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
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
            None => {
                serde_json::json!({ "name": name, "entity_id": null, "note_id": null, "loved": false })
            }
        };
    }
    (StatusCode::OK, Json(out)).into_response()
}

/// GET /api/v1/entities/:id
async fn get_entity(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_entity(id).await {
        Ok(Some(entity)) => {
            let loved = state.db.is_loved(user.user_id, id).await.unwrap_or(false);
            let note = state.db.note_for_entity(user.user_id, id).await.ok().flatten();
            let note_json = match note {
                Some(n) => note_json(&state.db, user.user_id, &n, true).await,
                None => serde_json::Value::Null,
            };
            (
                StatusCode::OK,
                Json(serde_json::json!({ "entity": entity, "loved": loved, "note": note_json })),
            )
                .into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "No such entity"),
        Err(e) => server_error(e),
    }
}

// ── Export ──

/// Resolutions for an *export*, where the label has to be the exported file's
/// title rather than the entity's bare name.
///
/// `render` can use the short label because it emits an `href` alongside it and
/// the browser follows that. A wikilink has no href — it resolves by filename —
/// so "Xtal" would point at a file that does not exist while `Xtal — Aphex
/// Twin.md` sits beside it. Hence the extra `get_entity` per distinct
/// shortcode: cheap in an export, and the difference between a vault that links
/// up and one full of dead ends.
async fn export_resolutions(
    db: &Database,
    user_id: i64,
    body_md: &str,
    ctx_artist: Option<&str>,
    ctx_album: Option<&str>,
) -> HashMap<(RefKind, String), Resolved> {
    let mut resolved: HashMap<(RefKind, String), Resolved> = HashMap::new();
    for code in notes::scan(body_md) {
        let key = code.key();
        if resolved.contains_key(&key) {
            continue;
        }
        let Some(entity_kind) = code.kind.entity_kind() else {
            // Gear has no note file of its own, so the link is to a page named
            // after the equipment. Dangling until the writer makes one, which is
            // ordinary in a wiki and better than dropping the reference.
            if let Ok(Some((_, label))) = db.find_equipment_by_name(user_id, &code.target).await {
                resolved.insert(key, Resolved { label, href: String::new() });
            }
            continue;
        };
        let hit = db
            .resolve_reference(
                user_id,
                entity_kind,
                &code.target,
                code.position(),
                ctx_artist,
                ctx_album,
            )
            .await;
        if let Ok(Some((entity_id, label))) = hit {
            let title = match db.get_entity(entity_id).await {
                Ok(Some(e)) => entity_title(&e),
                _ => label,
            };
            resolved.insert(key, Resolved { label: title, href: String::new() });
        }
    }
    resolved
}

/// YAML frontmatter, which is what Obsidian and Logseq both read.
///
/// Quoted and escaped throughout: a record called `Untitled: "Live"` would
/// otherwise break the block and take the whole file's metadata with it, and
/// exactly that sort of title is what a liner-notes feature attracts.
fn yaml(key: &str, value: &str) -> String {
    format!("{key}: \"{}\"\n", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A period note's human title — "August 2026", "Week 2026-W32", "2026".
///
/// A month is spelled out because "2026-08" reads as a code rather than a
/// title, and this ends up as a filename in somebody's vault. A week keeps its
/// ISO key: turning it into a date range here would be a second implementation
/// of `reports::period_of`'s label, free to disagree with the one the Reports
/// page shows above the very note being exported.
fn period_title(period_key: &str) -> String {
    let (unit, key) = period_key.split_once(':').unwrap_or(("", period_key));
    match unit {
        "month" => match key.split_once('-') {
            Some((y, m)) => {
                let name = match m.parse::<u32>() {
                    Ok(1) => "January",
                    Ok(2) => "February",
                    Ok(3) => "March",
                    Ok(4) => "April",
                    Ok(5) => "May",
                    Ok(6) => "June",
                    Ok(7) => "July",
                    Ok(8) => "August",
                    Ok(9) => "September",
                    Ok(10) => "October",
                    Ok(11) => "November",
                    Ok(12) => "December",
                    _ => return key.to_string(),
                };
                format!("{name} {y}")
            }
            None => key.to_string(),
        },
        "week" => format!("Week {key}"),
        _ => key.to_string(),
    }
}

fn iso(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// One note as a standalone Markdown document: frontmatter, then the body with
/// its shortcodes turned into wikilinks.
async fn note_document(db: &Database, user_id: i64, note: &NoteRow, title: &str) -> String {
    let (ctx_artist, ctx_album) = note_context(db, user_id, note).await;
    let resolutions =
        export_resolutions(db, user_id, &note.body_md, ctx_artist.as_deref(), ctx_album.as_deref())
            .await;

    let entity = match note.entity_id {
        Some(id) => db.get_entity(id).await.ok().flatten(),
        None => None,
    };

    let mut out = String::from("---\n");
    out.push_str(&yaml("title", title));
    match entity.as_ref() {
        Some(e) => {
            // The shortcode spelling, not the stored one: someone reading this
            // in a vault has met `track`/`album`, not `recording`/`release`.
            let kind = match e.kind.as_str() {
                "recording" => "track",
                "release" => "album",
                other => other,
            };
            out.push_str(&yaml("kind", kind));
            out.push_str(&yaml("name", &e.name));
            if !e.artist_name.trim().is_empty() {
                out.push_str(&yaml("artist", &e.artist_name));
            }
            if let Some(mbid) = e.mbid.as_deref().filter(|m| !m.trim().is_empty()) {
                out.push_str(&yaml("mbid", mbid));
            }
            out.push_str(&format!(
                "loved: {}\n",
                db.is_loved(user_id, e.id).await.unwrap_or(false)
            ));
            out.push_str(&format!("tags: [tapedeck, liner-note, {kind}]\n"));
        }
        // A note about a period, which has no entity and no listen. Checked
        // before the listen branch's assumption that `None` means one — that
        // assumption was true when there were only two kinds of target.
        None if note.period_key.is_some() => {
            let key = note.period_key.as_deref().unwrap_or_default();
            let (unit, k) = key.split_once(':').unwrap_or(("period", key));
            out.push_str(&yaml("kind", unit));
            out.push_str(&yaml("period", k));
            out.push_str(&format!("tags: [tapedeck, liner-note, {unit}]\n"));
        }
        None => {
            out.push_str(&yaml("kind", "listen"));
            if let Some(sid) = note.scrobble_id {
                if let Ok(Some(s)) = db.get_scrobble(user_id, sid).await {
                    out.push_str(&yaml("name", &s.title));
                    out.push_str(&yaml("artist", &s.artist));
                    if let Some(album) = s.album.as_deref().filter(|a| !a.trim().is_empty()) {
                        out.push_str(&yaml("album", album));
                    }
                    out.push_str(&yaml("played", &iso(s.timestamp)));
                }
            }
            out.push_str("tags: [tapedeck, liner-note, listen]\n");
        }
    }
    out.push_str(&yaml("visibility", &note.visibility));
    out.push_str(&yaml("created", &iso(note.created_at)));
    out.push_str(&yaml("updated", &iso(note.updated_at)));
    out.push_str("---\n\n");
    out.push_str(&notes::export_markdown(&note.body_md, &resolutions));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// What is true of both export shapes. The sentence describing the *shape* is
/// not in here on purpose — the single-file form claiming "one file per note"
/// would be a lie printed at the top of the thing it describes.
const EXPORT_NOTES: &str = "\
`[[Wikilinks]]` in the text were `[[track:…]]`, `[[album:…]]`, `[[artist:…]]` or
`[[gear:…]]` shortcodes in Tapedeck, and point at the file for whatever they
resolved to. A reference that matched nothing is left exactly as it was written
— it is what you typed, and rewriting it into something else would be a worse
answer than a dangling link.

Nothing here is a live link back to Tapedeck: this is a copy of your writing,
readable on its own in ten years' time with no server running.
";

const ZIP_README_HEAD: &str = "\
# Liner notes

Exported from Tapedeck. One file per note, with YAML frontmatter that Obsidian
and Logseq both read.

";

const SINGLE_HEAD: &str = "\
# Liner notes

Exported from Tapedeck. Every note in one document, a section each. The
per-note metadata — artist, kind, MBID, dates — is in the folder export
instead; a dozen frontmatter blocks down one file is noise, and only the first
would be read as metadata anywhere.

";

#[derive(Deserialize)]
struct ExportQuery {
    /// `zip` (default) — one file per note. `md` — everything in one document.
    format: Option<String>,
}

/// GET /api/v1/notes/export — every note as Markdown.
///
/// Session-only, deliberately. This is the whole of the user's own writing in
/// one request; `read` on a token is meant for an app showing a note, not for
/// draining the lot.
///
/// Held in memory rather than streamed like `/api/v1/export`: notes are a few
/// hundred short documents where listens are hundreds of thousands of rows, and
/// a zip's central directory has to be written after the entries anyway.
async fn export_notes(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    let single = q.format.as_deref() == Some("md");

    let rows = match state.db.list_notes(user.user_id).await {
        Ok(r) => r,
        Err(e) => return server_error(e),
    };

    // Titles first, and all of them, because a filename has to be unique before
    // any body is written — a wikilink pointing at "Untitled" when two files
    // want that name resolves to whichever one the vault saw first.
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut titled: Vec<(&NoteRow, String, String)> = Vec::with_capacity(rows.len());
    for note in &rows {
        let title = match note.entity_id {
            Some(id) => match state.db.get_entity(id).await {
                Ok(Some(e)) => entity_title(&e),
                _ => "Untitled".to_string(),
            },
            None => match (&note.scrobble_id, &note.period_key) {
                (Some(sid), _) => match state.db.get_scrobble(user.user_id, *sid).await {
                    Ok(Some(s)) => format!("{} — {} (listen)", s.title, s.artist),
                    _ => "Untitled listen".to_string(),
                },
                (_, Some(key)) => period_title(key),
                _ => "Untitled".to_string(),
            },
        };
        let base = notes::export_filename(&title);
        let mut file = base.clone();
        let mut n = 2;
        while !used.insert(file.clone()) {
            file = format!("{base} ({n})");
            n += 1;
        }
        titled.push((note, title, file));
    }

    if single {
        let mut doc = String::from(SINGLE_HEAD);
        doc.push_str(EXPORT_NOTES);
        for (note, title, _) in &titled {
            doc.push_str(&format!("\n\n---\n\n## {title}\n\n"));
            let body = note_document(&state.db, user.user_id, note, title).await;
            // The per-file frontmatter would be noise repeated N times inside
            // one document; only the first block in a Markdown file is metadata
            // anywhere that reads it.
            doc.push_str(body.split_once("---\n\n").map(|(_, rest)| rest).unwrap_or(&body));
        }
        return (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    "attachment; filename=\"tapedeck-liner-notes.md\"",
                ),
            ],
            doc,
        )
            .into_response();
    }

    let mut buf = std::io::Cursor::new(Vec::<u8>::new());
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        if zip.start_file("README.md", opts).is_err()
            || zip.write_all(ZIP_README_HEAD.as_bytes()).is_err()
            || zip.write_all(EXPORT_NOTES.as_bytes()).is_err()
        {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "Could not write the export");
        }
        for (note, title, file) in &titled {
            let doc = note_document(&state.db, user.user_id, note, title).await;
            if zip.start_file(format!("{file}.md"), opts).is_err()
                || zip.write_all(doc.as_bytes()).is_err()
            {
                return err(StatusCode::INTERNAL_SERVER_ERROR, "Could not write the export");
            }
        }
        if zip.finish().is_err() {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "Could not close the export");
        }
    }

    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "application/zip"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"tapedeck-liner-notes.zip\"",
            ),
        ],
        buf.into_inner(),
    )
        .into_response()
}

pub fn create_notes_router() -> Router<Arc<AppState>> {
    Router::new()
        // Literal segments first, so none is read as an `:id`.
        .route("/api/v1/notes/preview", post(preview))
        .route("/api/v1/notes/for", get(note_for))
        .route("/api/v1/notes/export", get(export_notes))
        .route("/api/v1/notes", get(list_notes).post(save_note))
        .route("/api/v1/notes/:id", get(get_note).delete(delete_note))
        .route("/api/v1/notes/:id/revisions", get(note_revisions))
        .route("/api/v1/loves", get(list_loves).post(set_love))
        .route("/api/v1/loves/keys", get(love_keys))
        .route("/api/v1/entities/:id", get(get_entity))
        .route("/api/v1/scrobbles/:id/annotations", get(listen_annotations))
}
