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
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::auth::{AuthUser, ReadAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{CrateEntry, NewRecommendation};
use crate::openapi::Unauthorized;

/// Everything in the crate, and how much of it landed.
#[derive(Serialize, ToSchema)]
pub struct Crate {
    #[serde(rename = "crate")]
    pub entries: Vec<CrateEntry>,
    /// Entries still `open`.
    pub open: usize,
    /// How many suggestions were actually played.
    pub landed: usize,
}

#[derive(Serialize, ToSchema)]
pub struct CrateFiled {
    pub id: i64,
    /// False when it was already there and its reason was updated.
    pub created: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CrateStatusSet {
    pub ok: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CrateRemoved {
    pub deleted: bool,
}

fn err(code: u16, msg: impl Into<String>) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
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
    fn what(artist: &str, name: Option<&str>) -> String {
        match name {
            Some(n) => format!("{artist} — {n}"),
            None => artist.to_string(),
        }
    }

    /// Said to an **assistant**, which is why it is third-person and names the
    /// tool to reach for instead. This wording is load-bearing on the MCP
    /// surface: the play count is what tells a model its suggestion was
    /// grounded in something the person already has, and the pointer at
    /// rediscovery is what it should have called.
    pub fn message(&self, artist: &str, name: Option<&str>) -> String {
        match self {
            CrateRefusal::Invalid(m) => m.clone(),
            CrateRefusal::AlreadyHeard { plays } => format!(
                "{} is already in this listening history ({plays} plays), so it is not \
something to put in the crate. Suggesting a record they already own is what the rediscovery \
tool is for.",
                Self::what(artist, name)
            ),
        }
    }

    /// Said to the **person whose crate it is**, who is reading it on their own
    /// screen. Same refusal, second person, and it points at a page rather than
    /// at a tool they have never heard of.
    pub fn for_reader(&self, artist: &str, name: Option<&str>) -> String {
        match self {
            CrateRefusal::Invalid(m) => m.clone(),
            CrateRefusal::AlreadyHeard { plays } => format!(
                "You already have {} — {plays} {}. The crate is for records you haven't got; \
Rediscovery is where the ones you have go quiet.",
                Self::what(artist, name),
                if *plays == 1 { "play" } else { "plays" }
            ),
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

/// Things somebody suggested that you do not own.
///
/// Scope: `read` *(since v0.120)*. Each entry carries **`first_played_at`**, derived from the
/// listens rather than stored — whether the suggestion actually landed.
/// That is the thing only a scrobbler can answer, and it is what makes this
/// more than a wantlist: a recommendation is a prediction, and Tapedeck is
/// the only party that can score it.
///
/// `landed` counts how many entries have since turned up in the history.
#[utoipa::path(
    get,
    path = "/api/v1/crate",
    tag = "Crate",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "The crate.", body = Crate),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_crate(State(state): State<Arc<AppState>>, ReadAuth { user }: ReadAuth) -> Response {
    match state.db.list_crate(user.user_id).await {
        Ok(entries) => {
            // The number that makes this more than a wantlist: how many
            // suggestions turned into listening.
            let open = entries.iter().filter(|e| e.rec.status == "open").count();
            let landed = entries.iter().filter(|e| e.first_played_at.is_some()).count();
            (StatusCode::OK, Json(Crate { entries, open, landed })).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// What a hand-filed crate entry says, plus where it came from.
//
// `from` is the *username of a deck the caller is patched into* — the Reel's
// "put this in my crate" button — and it is **resolved and checked**, never
// taken as text. An origin string is displayed on the Crate as "suggested by
// X" and groups everything that person turned you on to, so letting a client
// write one directly would let it assert a provenance it did not have. Same
// reasoning as `fold_artist_credit` deriving its target rather than accepting
// one: what a record *says about where it came from* has to be earned.
#[derive(Deserialize, ToSchema)]
#[schema(as = CrateAdd)]
struct CrateAddBody {
    #[serde(flatten)]
    rec: NewRecommendation,
    /// Optional. The **username of a deck you are patched into** — the Reel's
    /// "into my crate" button. Resolved and checked server-side, and a deck
    /// you cannot read is a **403**: the Crate groups by origin and shows
    /// "suggested by X", so a client writing that string itself would be
    /// asserting a provenance rather than earning one. The patch must point
    /// *from* you *to* them; somebody who follows you has told you nothing.
    /// Omit it for something you found yourself, which files under the
    /// origin `user`.
    from: Option<String>,
}

// POST /api/v1/crate — add something by hand.
/// Add something to the crate.
///
/// Session-only for this endpoint; an assistant uses the MCP `add_to_crate`
/// tool, which shares the same validation.
///
/// Two refusals, both deliberate. **`because` is required** — what in your
/// own listening prompted this — which is the vision doc's pipeline
/// (analytics, candidate selection, explanation) enforced at the door
/// rather than asked for in a prompt. And **anything already in the
/// history is refused with its play count**: that is not a suggestion, it
/// is rediscovery, and letting it through would turn the crate into a
/// worse copy of a screen that already exists.
#[utoipa::path(
    post,
    path = "/api/v1/crate",
    tag = "Crate",
    request_body = CrateAddBody,
    responses(
        (status = 200, description = "Filed. `created` is false when it was already there and the reason was updated.", body = CrateFiled),
        (status = 400, description = "Missing grounding, malformed, or already in the history.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "`from` named a deck you are not patched into.", body = ErrorResponse),
    )
)]
async fn add(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CrateAddBody>,
) -> Response {
    // Absent is "I found this myself", which is what the Crate has always
    // called `user`. Present means the caller is claiming they got it off
    // somebody's deck, and only an accepted patch *into* that deck makes the
    // claim true — a deck you cannot read has told you nothing.
    let origin = match body.from.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => "user".to_string(),
        Some(name) => match state.db.deck_i_am_patched_into(user.user_id, name).await {
            Ok(Some((_, display))) => format!("{display}'s deck"),
            Ok(None) => {
                return err(403, "You are not patched into that deck.");
            }
            Err(e) => return err(500, e.to_string()),
        },
    };

    match file(&state, user.user_id, &body.rec, &origin, None).await {
        Ok((id, created)) => (StatusCode::OK, Json(CrateFiled { id, created })).into_response(),
        Err(refusal) => err(400, refusal.for_reader(&body.rec.artist, body.rec.name.as_deref())),
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(as = CrateStatus)]
struct StatusBody {
    /// `open`, `acquired` or `dismissed`.
    #[schema(example = "acquired")]
    status: String,
}

/// Mark an entry acquired, dismissed, or open again.
#[utoipa::path(
    patch,
    path = "/api/v1/crate/{id}",
    tag = "Crate",
    params(("id" = i64, Path)),
    request_body = StatusBody,
    responses(
        (status = 200, description = "Updated.", body = CrateStatusSet),
        (status = 400, description = "Unknown status.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such entry.", body = ErrorResponse),
    )
)]
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
        Ok(true) => (StatusCode::OK, Json(CrateStatusSet { ok: true })).into_response(),
        Ok(false) => err(404, "No such crate entry"),
        Err(e) => err(500, e.to_string()),
    }
}

/// Remove an entry.
#[utoipa::path(
    delete,
    path = "/api/v1/crate/{id}",
    tag = "Crate",
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "Removed.", body = CrateRemoved),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such entry.", body = ErrorResponse),
    )
)]
async fn remove(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Response {
    match state.db.delete_recommendation(user.user_id, id).await {
        Ok(true) => (StatusCode::OK, Json(CrateRemoved { deleted: true })).into_response(),
        Ok(false) => err(404, "No such crate entry"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(list_crate, add, set_status, remove))]
pub struct Api;

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
