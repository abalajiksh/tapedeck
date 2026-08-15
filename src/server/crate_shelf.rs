//! The Crate — things somebody told you about that you do not own yet.
//!
//! **Tapedeck does not recommend. It records recommendations made to you.**
//!
//! That sentence is the whole justification for this module existing beside a
//! vision rule which says nothing here points at music the user does not have.
//! The rule governs what Tapedeck *generates* — Rediscovery is built entirely
//! from the user's own history, and stays that way. This stores what an
//! assistant said, the way `notes` stores what the user wrote: Tapedeck is the
//! notebook, not the author.
//!
//! Three properties keep that honest, and none of them is a matter of
//! remembering:
//!
//! * **The crate never appears in a history or statistics readout.** It is its
//!   own table, its own endpoints and its own page. The same containment
//!   `genre_neighbours` uses, where an Every Noise exemplar and an artist the
//!   user owns are never populated on the same card.
//! * **A recommendation of something already in the history is refused**, with
//!   the play count. That is not a tip, it is Rediscovery under another name,
//!   and letting it through would quietly turn the crate into a second and
//!   worse version of a screen that already exists.
//! * **`because` is required.** What in the user's own listening prompted this
//!   — the vision doc's pipeline (analytics, candidate selection, explanation)
//!   held to at the door rather than asked for in a prompt.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::NewRecommendation;

fn err(code: u16, msg: impl Into<String>) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

/// Why an entry could not be filed.
pub enum CrateRefusal {
    /// Malformed — a missing field, an unknown kind.
    Invalid(String),
    /// Already in the history. Carries the play count, because "you have 43
    /// plays of this" is the useful half of the answer.
    AlreadyHeard { plays: i64 },
}

impl CrateRefusal {
    pub fn message(&self, artist: &str, name: Option<&str>) -> String {
        match self {
            CrateRefusal::Invalid(m) => m.clone(),
            CrateRefusal::AlreadyHeard { plays } => {
                let what = match name {
                    Some(n) => format!("{artist} — {n}"),
                    None => artist.to_string(),
                };
                format!(
                    "{what} is already in this listening history ({plays} plays), so it is not \
something to put in the crate. Suggesting a record they already own is what the rediscovery \
tool is for."
                )
            }
        }
    }
}

/// Validate and file one entry. Shared by the HTTP endpoint and the MCP tool so
/// the guards cannot apply on one path and not the other.
pub async fn file(
    state: &AppState,
    user_id: i64,
    rec: &NewRecommendation,
    origin: &str,
    connection_id: Option<i64>,
) -> Result<(i64, bool), CrateRefusal> {
    let kind = rec.kind.trim().to_lowercase();
    if !matches!(kind.as_str(), "artist" | "album" | "track") {
        return Err(CrateRefusal::Invalid("kind must be artist, album or track".into()));
    }
    if rec.artist.trim().is_empty() {
        return Err(CrateRefusal::Invalid("artist is required".into()));
    }
    // An artist's name lives in `artist`; anything else needs a title of its
    // own. Mirrors the rule `entities` follows.
    if kind != "artist" && rec.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(CrateRefusal::Invalid(format!("name is required for a {kind}")));
    }
    if rec.because.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(CrateRefusal::Invalid(
            "because is required — say what in this person's own listening prompted the \
suggestion. A recommendation with no grounding in their history is not one."
                .into(),
        ));
    }

    let plays = state
        .db
        .already_heard(user_id, &kind, &rec.artist, rec.name.as_deref())
        .await
        .map_err(|e| CrateRefusal::Invalid(e.to_string()))?;
    if plays > 0 {
        return Err(CrateRefusal::AlreadyHeard { plays });
    }

    let normalised = NewRecommendation { kind, ..rec.clone() };
    state
        .db
        .add_recommendation(user_id, &normalised, origin, connection_id)
        .await
        .map_err(|e| CrateRefusal::Invalid(e.to_string()))
}

/// GET /api/v1/crate
async fn list_crate(State(state): State<Arc<AppState>>, user: AuthUser) -> Response {
    match state.db.list_crate(user.user_id).await {
        Ok(entries) => {
            // The number that makes this more than a wantlist: how many
            // suggestions turned into listening.
            let open = entries.iter().filter(|e| e.rec.status == "open").count();
            let landed = entries.iter().filter(|e| e.first_played_at.is_some()).count();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "crate": entries,
                    "open": open,
                    "landed": landed,
                })),
            )
                .into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// POST /api/v1/crate — add something by hand.
async fn add(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<NewRecommendation>,
) -> Response {
    match file(&state, user.user_id, &body, "user", None).await {
        Ok((id, created)) => {
            (StatusCode::OK, Json(serde_json::json!({ "id": id, "created": created })))
                .into_response()
        }
        Err(refusal) => err(400, refusal.message(&body.artist, body.name.as_deref())),
    }
}

#[derive(Deserialize)]
struct StatusBody {
    /// `open`, `acquired` or `dismissed`.
    status: String,
}

/// PATCH /api/v1/crate/:id
async fn set_status(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<StatusBody>,
) -> Response {
    if !matches!(body.status.as_str(), "open" | "acquired" | "dismissed") {
        return err(400, "status must be open, acquired or dismissed");
    }
    match state.db.set_recommendation_status(user.user_id, id, &body.status).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(false) => err(404, "No such crate entry"),
        Err(e) => err(500, e.to_string()),
    }
}

/// DELETE /api/v1/crate/:id
async fn remove(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Response {
    match state.db.delete_recommendation(user.user_id, id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": true }))).into_response(),
        Ok(false) => err(404, "No such crate entry"),
        Err(e) => err(500, e.to_string()),
    }
}

pub fn create_crate_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/crate", get(list_crate).post(add))
        .route("/api/v1/crate/:id", axum::routing::patch(set_status).delete(remove))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        kind: &str,
        artist: &str,
        name: Option<&str>,
        because: Option<&str>,
    ) -> NewRecommendation {
        NewRecommendation {
            kind: kind.into(),
            artist: artist.into(),
            name: name.map(str::to_string),
            mbid: None,
            reason: Some("you'd like it".into()),
            because: because.map(str::to_string),
        }
    }

    /// The validation half of `file`, without a database. Kept in step with the
    /// real thing by being the same order of checks.
    fn validate(r: &NewRecommendation) -> Result<(), String> {
        let kind = r.kind.trim().to_lowercase();
        if !matches!(kind.as_str(), "artist" | "album" | "track") {
            return Err("kind".into());
        }
        if r.artist.trim().is_empty() {
            return Err("artist".into());
        }
        if kind != "artist" && r.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err("name".into());
        }
        if r.because.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err("because".into());
        }
        Ok(())
    }

    #[test]
    fn a_suggestion_with_no_grounding_is_refused() {
        // `because` is the vision doc's pipeline held to at the door: analytics,
        // then candidate selection, then explanation. Without it the model is
        // free-associating and the crate fills with noise.
        assert_eq!(validate(&rec("album", "Burial", Some("Untrue"), None)), Err("because".into()));
        assert_eq!(
            validate(&rec("album", "Burial", Some("Untrue"), Some("  "))),
            Err("because".into())
        );
        assert!(validate(&rec(
            "album",
            "Burial",
            Some("Untrue"),
            Some("you play a lot of 2-step")
        ))
        .is_ok());
    }

    #[test]
    fn an_artist_needs_no_title_and_a_record_does() {
        assert!(validate(&rec("artist", "Burial", None, Some("x"))).is_ok());
        assert_eq!(validate(&rec("album", "Burial", None, Some("x"))), Err("name".into()));
        assert_eq!(validate(&rec("track", "Burial", None, Some("x"))), Err("name".into()));
    }

    #[test]
    fn the_already_heard_refusal_says_how_much() {
        // The useful half of the answer. "You have 43 plays of this" tells the
        // model what to do differently; "invalid" does not.
        let msg = CrateRefusal::AlreadyHeard { plays: 43 }.message("Burial", Some("Untrue"));
        assert!(msg.contains("Burial — Untrue"));
        assert!(msg.contains("43 plays"));
        assert!(msg.contains("rediscovery"));
    }
}
