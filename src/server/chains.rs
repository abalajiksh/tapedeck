use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::auth::{AuthUser, ReadAuth};
use super::models::{ErrorResponse, OkResponse};
use super::AppState;
use crate::db::{
    BindingRow, ChainVariant, DeviceRow, EquipmentRow, FoldOutcome, FoldSuggestion, FoldTarget,
};
use crate::models::*;
use crate::openapi::{BadRequest, Forbidden, NotFound, Unauthorized};

/// A row that was just created.
#[derive(Serialize, ToSchema)]
#[schema(as = RowCreated)]
pub struct Created {
    pub id: i64,
    /// `created`.
    pub status: &'static str,
}

/// A row that was just updated.
#[derive(Serialize, ToSchema)]
#[schema(as = RowUpdated)]
pub struct Updated {
    pub id: i64,
    /// `updated`.
    pub status: &'static str,
}

/// A new variant's id.
#[derive(Serialize, ToSchema)]
pub struct VariantId {
    pub id: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ChainList {
    pub chains: Vec<SignalChain>,
}

#[derive(Serialize, ToSchema)]
pub struct VariantList {
    pub variants: Vec<ChainVariant>,
}

#[derive(Serialize, ToSchema)]
pub struct DeviceList {
    pub devices: Vec<DeviceRow>,
}

#[derive(Serialize, ToSchema)]
pub struct EquipmentList {
    pub equipment: Vec<EquipmentRow>,
}

#[derive(Serialize, ToSchema)]
pub struct BindingList {
    pub bindings: Vec<BindingRow>,
}

/// Pairs of chains that look like the same rig. Nothing has been changed.
#[derive(Serialize, ToSchema)]
pub struct Duplicates {
    pub duplicates: Vec<FoldSuggestion>,
}

/// A setup promoted to be the chain.
#[derive(Serialize, ToSchema)]
pub struct Promoted {
    /// The setup that is now the chain.
    pub promoted: i64,
    /// The variant the old chain became.
    pub old_base_variant_id: i64,
}

// ── Chain variants ──
//
// A chain with one step swapped. Two cartridges on one turntable, or the same
// commute with different IEMs, are one chain and a choice — not two chains.
// Modelling them as separate chains is what makes a gear collection produce a
// page of near-duplicates, and it is also what stops per-component wear from
// being answerable: "which chain was it" does not tell you what was fitted.

#[derive(Deserialize, ToSchema)]
pub struct CreateVariantRequest {
    pub chain_id: i64,
    pub name: String,
    #[serde(default)]
    pub overrides: Vec<VariantOverride>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateVariantRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overrides: Option<Vec<VariantOverride>>,
}

/// GET /api/v1/chains/variants — every variant the caller owns.
///
/// Flat rather than nested under each chain: the page needs them keyed by
#[derive(Deserialize, ToSchema)]
struct PromoteVariant {
    /// What to call the setup the chain is being demoted to. The old base has
    /// no name of its own — it *was* the chain — so one has to be supplied.
    name: Option<String>,
}

// POST /api/v1/chains/variants/:id/promote — make this setup the chain, and
// the chain a setup.
//
// **A relabelling, not a change.** What was fitted for any given listen is
// identical afterwards and so is every piece of gear's hours; the work is in
// re-expressing the *other* variants, whose overrides are relative to a base
// that is about to move under them. See `Database::promote_variant`.
/// Make this setup the chain, and the chain a setup.
///
/// For the common case where the variant is the one you actually use — the
/// other cartridge is in the headshell most of the time, so it should be
/// what the chain *is*.
///
/// **A relabelling, not a change.** What was fitted for any given listen is
/// identical afterwards, and so is every piece of gear's hours.
///
/// That is harder than it looks, and it is the whole of this endpoint. An
/// override is `{position, equipment_id}` **relative to the base**, so
/// another variant that swaps position 2 means "the base, but with X at 2"
/// — change the base and that sentence names different gear at position 1,
/// silently, with hours landing on a box that was never fitted. So every
/// variant is resolved to an absolute list of steps first and re-expressed
/// against the new base afterwards. Listens follow: those on the chain move
/// to the new setup that restores it, and those on the promoted setup move
/// to the chain.
///
/// `name` is what to call the setup the chain is demoted to — the old base
/// has no name of its own, because it *was* the chain. Defaults to
/// "Previous setup".
///
/// **Refuses a setup that removes a step** (400): removal shifts every
/// later position, so promoting one would change what every other setup's
/// overrides point at. Nothing in the UI creates such a setup.
#[utoipa::path(
    post,
    path = "/api/v1/chains/variants/{id}/promote",
    tag = "Chains",
    security(("sessionCookie" = [])),
    params(("id" = i64, Path, description = "The variant to promote.")),
    request_body = PromoteVariant,
    responses(
        (status = 200, description = "The promoted setup, and the id of the one the chain became.", body = Promoted),
        (status = 400, description = "That setup removes a step, or has none.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such setup.", body = ErrorResponse),
    )
)]
async fn promote_variant(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<PromoteVariant>,
) -> impl IntoResponse {
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Previous setup")
        .to_string();

    match state.db.promote_variant(user.user_id, id, &name).await {
        Ok(old_id) => {
            (StatusCode::OK, Json(Promoted { promoted: id, old_base_variant_id: old_id }))
                .into_response()
        }
        Err(sqlx::Error::RowNotFound) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "No such setup".into() }),
        )
            .into_response(),
        // The one refusal worth its own message: a variant that *removes* a
        // step cannot become the chain, because every later position would
        // shift under every other variant.
        Err(sqlx::Error::Protocol(msg)) => {
            (StatusCode::BAD_REQUEST, Json(ErrorResponse { code: 400, error: msg })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// chain, and one list it can group beats N requests.
/// Every chain variant the caller owns.
///
/// Session or a token carrying `read`. A variant is a chain with one step
/// swapped — two cartridges on one turntable, or the same commute on
/// different IEMs. They are one chain and a choice, not two chains.
///
/// Flat rather than nested under each chain, because the page groups them
/// itself and one list beats a request per chain.
///
/// `total_hours` here is the listening that went through *this variant*.
/// A chain's own `total_hours` includes every variant of it, since they all
/// ran through that chain.
#[utoipa::path(
    get,
    path = "/api/v1/chains/variants",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    tag = "Chains",
    responses(
        (status = 200, description = "OK.", body = VariantList),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
async fn list_variants(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.list_variants(user.user_id).await {
        Ok(variants) => (StatusCode::OK, Json(VariantList { variants })).into_response(),
        Err(e) => internal(e),
    }
}

/// Create a variant.
///
/// Session-only. Only the steps that *differ* are stored, which is what
/// makes this an override rather than a copy: editing the shared part of a
/// chain updates every variant of it.
///
/// The chain and every piece of gear named must be the caller's.
#[utoipa::path(
    post,
    path = "/api/v1/chains/variants",
    tag = "Chains",
    request_body = CreateVariantRequest,
    responses(
        (status = 200, description = "Created.", body = VariantId),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn create_variant(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateVariantRequest>,
) -> impl IntoResponse {
    if body.name.trim().is_empty() {
        return bad("A variant needs a name");
    }
    // The chain must be the caller's — same rule ingest follows for a token's
    // default chain, and the only id in this body that names something owned.
    match state.db.get_chain(user.user_id, body.chain_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return bad("No such signal chain"),
        Err(e) => return internal(e),
    }
    if let Err(e) = check_gear(&state, user.user_id, &body.overrides).await {
        return e;
    }

    match state
        .db
        .create_variant(user.user_id, body.chain_id, body.name.trim(), &body.overrides)
        .await
    {
        Ok(id) => (StatusCode::OK, Json(VariantId { id })).into_response(),
        Err(e) => internal(e),
    }
}

/// Rename a variant or change what it swaps.
///
/// Session-only. An omitted field is left alone.
#[utoipa::path(
    patch,
    path = "/api/v1/chains/variants/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The variant.")),
    request_body = UpdateVariantRequest,
    responses(
        (status = 200, description = "Saved. `status` is `ok`.", body = OkResponse),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_variant(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateVariantRequest>,
) -> impl IntoResponse {
    if let Some(ref o) = body.overrides {
        if let Err(e) = check_gear(&state, user.user_id, o).await {
            return e;
        }
    }
    match state
        .db
        .update_variant(user.user_id, id, body.name.as_deref(), body.overrides.as_deref())
        .await
    {
        Ok(true) => (StatusCode::OK, OkResponse::new("ok")).into_response(),
        Ok(false) => not_found("No such variant"),
        Err(e) => internal(e),
    }
}

// The listens that went through it keep their chain and fall back to its base
// components — which is what a null variant means everywhere else — rather
// than being deleted along with it.
/// Delete a variant.
///
/// Session-only. The listens that went through it **keep their chain** and
/// fall back to its base components, which is what a null variant means
/// everywhere else — they are not deleted along with it.
#[utoipa::path(
    delete,
    path = "/api/v1/chains/variants/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The variant.")),
    responses(
        (status = 200, description = "Deleted. `status` is `deleted`.", body = OkResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_variant(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_variant(user.user_id, id).await {
        Ok(true) => (StatusCode::OK, OkResponse::new("deleted")).into_response(),
        Ok(false) => not_found("No such variant"),
        Err(e) => internal(e),
    }
}

/// What a fold request has to say. Deliberately not one nullable field.
///
/// `variant_id` (land on one that exists) and `variant_name` (derive a new one
/// from the two chains) are different intents, and a single nullable field
/// would make "use the variant I built" and "make me another one" the same
/// request with a typo between them — on an endpoint that is not reversible.
#[derive(Deserialize, ToSchema)]
pub struct FoldRequest {
    pub into_chain_id: i64,
    #[serde(default)]
    pub variant_id: Option<i64>,
    #[serde(default)]
    pub variant_name: Option<String>,
}

// GET /api/v1/chains/duplicates — chains that look like the same rig.
//
// The discovery half of the fold and the same shape the metadata sanitiser
// uses: it reports what it would do and changes nothing. A fold is not
// reversible, so the operator sees the pair, the distance and what the variant
// would override before agreeing to any of it.
/// Chains that look like the same rig, and what folding one would do.
///
/// Session-only. The discovery half of the fold below, and the same
/// preview-then-apply shape the metadata sanitiser uses: this reports, it
/// never acts.
///
/// A pair is reported when the two chains disagree on at most two steps.
/// `distance` is how many — `0` is the same wiring under two names, `1` is
/// the swapped-box case (the same commute on different IEMs) that variants
/// exist for. Past two they are simply different rigs, and a variant
/// overriding half a chain is a copy wearing an override's clothes.
///
/// The chain with **more listening** is reported as the base, since it
/// keeps its id and fewer rows move — but the fold takes an explicit
/// direction, so this is a suggestion rather than the decision.
#[utoipa::path(
    get,
    path = "/api/v1/chains/duplicates",
    tag = "Chains",
    responses(
        (status = 200, description = "OK.", body = Duplicates),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_duplicates(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.near_duplicate_chains(user.user_id).await {
        Ok(pairs) => (StatusCode::OK, Json(Duplicates { duplicates: pairs })).into_response(),
        Err(e) => internal(e),
    }
}

// POST /api/v1/chains/:id/fold — fold this chain into another as a variant.
//
// The listening moves with it and this chain is deleted, so the hours that
// were split across two near-duplicate rows add up on one — and, because
// `equipment_seconds` resolves overrides per `(chain, variant)` pair, they land
// on the gear that was actually fitted rather than on the base chain's.
//
// Session-only, like every other write here. Not reversible; the UI says so.
/// Fold this chain into another as a variant of it.
///
/// Session-only. **Not reversible.**
///
/// Every listen, side, session, binding, device default and token default
/// of this chain is repointed at `into_chain_id` carrying the variant, and
/// this chain is then deleted. Because gear hours resolve per
/// `(chain, variant)` pair, the moved listening lands on the gear that was
/// actually fitted — which is what makes two near-duplicate chains' hours
/// add up instead of sitting in two rows that cannot be compared.
///
/// Send **either** `variant_id` (a variant of the target chain you already
/// built) **or** `variant_name` (derive a new one from the difference
/// between the two chains). Sending both, or neither, is a 400: on an
/// endpoint that cannot be undone, "use the one I built" and "make me
/// another" must not be the same request with a typo between them.
///
/// Refused with a 400 naming the reason when: the chain is folded into
/// itself, the variant belongs to a different chain (it would resolve to
/// components never fitted together), the name is already taken on the
/// target, or **this chain carries variants of its own** — their positions
/// index into components the fold replaces, and re-deriving them would be a
/// guess about which box was fitted.
#[utoipa::path(
    post,
    path = "/api/v1/chains/{id}/fold",
    tag = "Chains",
    params(("id" = i64, Path, description = "The chain to fold away.")),
    request_body = FoldRequest,
    responses(
        (status = 200, description = "Folded. The counts are what actually moved.", body = FoldOutcome),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn fold_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<FoldRequest>,
) -> impl IntoResponse {
    let target = match (body.variant_id, body.variant_name.as_deref()) {
        (Some(_), Some(_)) => {
            return bad("Send variant_id or variant_name, not both — one picks a variant you already built, the other derives a new one")
        }
        (Some(v), None) => FoldTarget::Existing(v),
        (None, Some(n)) => FoldTarget::New(n),
        (None, None) => {
            return bad("Which variant should this chain become? Send variant_id to use one you already built, or variant_name to derive a new one")
        }
    };

    match state.db.fold_chain_into_variant(user.user_id, id, body.into_chain_id, target).await {
        // The refusals are all things the operator can act on — a variant of
        // the wrong chain, a name already taken, a chain carrying variants of
        // its own — so they are 400s naming the reason rather than a 500.
        Ok(Err(why)) => bad(&why),
        Ok(Ok(out)) => (StatusCode::OK, Json::<FoldOutcome>(out)).into_response(),
        Err(e) => internal(e),
    }
}

/// Every piece of gear named in an override has to be the caller's.
async fn check_gear(
    state: &Arc<AppState>,
    user_id: i64,
    overrides: &[VariantOverride],
) -> Result<(), axum::response::Response> {
    let wanted: Vec<i64> = overrides.iter().filter_map(|o| o.equipment_id).collect();
    if wanted.is_empty() {
        return Ok(());
    }
    match state.db.get_equipment(user_id).await {
        Ok(gear) => {
            if wanted.iter().all(|w| gear.iter().any(|g| g.id == *w)) {
                Ok(())
            } else {
                Err(bad("A variant names equipment that isn't yours"))
            }
        }
        Err(e) => Err(internal(e)),
    }
}

fn bad(msg: &str) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { code: 400, error: msg.into() })).into_response()
}
fn not_found(msg: &str) -> axum::response::Response {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { code: 404, error: msg.into() })).into_response()
}
fn internal(e: sqlx::Error) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { code: 500, error: e.to_string() }))
        .into_response()
}

// ── Signal Chain endpoints ──

#[derive(Deserialize, ToSchema)]
pub struct CreateChainRequest {
    pub name: String,
    pub description: Option<String>,
    pub components: Vec<ChainComponent>,
    #[serde(default)]
    pub listening_context: ListeningContext,
    /// A glyph name from the UI's set. Not validated here — an unknown value
    /// falls back to a generic mark on render, which is a better failure than
    /// rejecting a save because the front end learned a new icon first.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Create a chain.
///
/// Session-only.
///
/// **There is no `is_default`**, and until v0.115.1 this said there was —
/// sending it got a 201 and no effect, because unknown fields are ignored.
/// A chain is made the default *by* a device binding or an API token; see
/// the `Chain` schema.
///
/// A new chain is never `shared`. Sharing is switched on deliberately, from
/// the chain drawer or Patch → Decks, and never as a side effect of
/// creating something.
#[utoipa::path(
    post,
    path = "/api/v1/chains",
    tag = "Chains",
    request_body = CreateChainRequest,
    responses(
        (status = 201, description = "Created.", body = Created),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 409, description = "A chain of that name already exists for this user.", body = ErrorResponse),
    )
)]
async fn create_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateChainRequest>,
) -> impl IntoResponse {
    let chain = SignalChain {
        id: None,
        user_id: user.user_id,
        name: body.name,
        description: body.description,
        components: body.components,
        listening_context: body.listening_context,
        icon: body.icon.clone(),
        is_active: true,
        // A new chain publishes nothing. Sharing is switched on deliberately,
        // from the chain drawer or Patch → Decks, and never as a side effect of
        // creating something.
        shared: false,
        total_hours: 0.0,
        created_at: 0, // set by db
    };

    match state.db.create_chain(&chain).await {
        Ok(id) => (StatusCode::CREATED, Json(Created { id, status: "created" })).into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE") {
                (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        code: 409,
                        error: format!("Chain '{}' already exists", chain.name),
                    }),
                )
                    .into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { code: 500, error: msg }))
                    .into_response()
            }
        }
    }
}

/// Signal chains.
///
/// Scope: `read` *(since v0.37)*. This is what lets a client offer a chain
/// **picker** instead of taking unvalidatable free text for a field that
/// has to match a name exactly.
#[utoipa::path(
    get,
    path = "/api/v1/chains",
    tag = "Chains",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    responses(
        (status = 200, description = "OK.", body = ChainList),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
pub async fn list_chains(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.get_chains(user.user_id).await {
        Ok(chains) => (StatusCode::OK, Json(ChainList { chains })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// `read`, like the list it is one row of. It was session-only while the list
// took a token and the published document said both did — so a client that
// had the list could not ask for one chain out of it.
/// One chain.
///
/// Scope: `read` *(since v0.37)*.
#[utoipa::path(
    get,
    path = "/api/v1/chains/{id}",
    tag = "Chains",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(("id" = i64, Path, description = "The chain.")),
    responses(
        (status = 200, description = "OK.", body = SignalChain),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
async fn get_chain(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_chain(user.user_id, id).await {
        Ok(Some(chain)) => (StatusCode::OK, Json(chain)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Chain not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

/// Update a chain.
///
/// Session-only, and a **whole-record replace** despite the verb — it takes
/// the same body as the create. Every field the editor owns is written, so
/// an omitted `icon`, `description` or `listening_context` is *cleared*.
/// Send the full record back.
///
/// Two fields it deliberately does **not** touch: `shared`, which is
/// switched from the chain drawer or Patch → Decks — writing it here would
/// un-publish a chain as a side effect of renaming it — and `total_hours`,
/// which is derived from the listens.
///
/// A component naming gear the chain did not name before is stamped with
/// `since` at the moment of the save. That is what stops a box added to a
/// chain with history being credited with all of it; send your own `since`
/// to correct the date, which is usually a few days before anybody got
/// round to recording it.
#[utoipa::path(
    patch,
    path = "/api/v1/chains/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The chain.")),
    request_body = CreateChainRequest,
    responses(
        (status = 200, description = "Updated.", body = Updated),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CreateChainRequest>,
) -> impl IntoResponse {
    let ctx: &str = body.listening_context.as_str();
    match state
        .db
        .update_chain(
            user.user_id,
            id,
            &body.name,
            body.description.as_deref(),
            &body.components,
            ctx,
            body.icon.as_deref(),
        )
        .await
    {
        Ok(true) => (StatusCode::OK, Json(Updated { id, status: "updated" })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Chain not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

/// Delete a chain.
///
/// Session-only. Clears referencing rows first — foreign keys are on.
#[utoipa::path(
    delete,
    path = "/api/v1/chains/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The chain.")),
    responses(
        (status = 204, description = "Deleted."),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_chain(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Chain not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// ── Device endpoints ──

/// Devices that have reported listens.
///
/// Scope: `read` *(since v0.37)*.
#[utoipa::path(
    get,
    path = "/api/v1/devices",
    tag = "Chains",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    responses(
        (status = 200, description = "OK.", body = DeviceList),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
async fn list_devices(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.get_devices(user.user_id).await {
        Ok(devices) => (StatusCode::OK, Json(DeviceList { devices })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

#[derive(Deserialize, ToSchema)]
pub struct SetDeviceChainRequest {
    /// The chain this device's listens default to — rung 4 of the ladder.
    /// **Required: `null` clears it; absent is a 400.** Must be one of your
    /// chains.
    // Doubly wrapped for the reason `bulk-chain`'s `signal_chain_id` is: a
    // plain `Option` reads a misspelled key as "clear", and this overwrites.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<i64>, required = true)]
    pub chain_id: Option<Option<i64>>,
}

/// Distinguish an absent field from an explicit `null` — see `bulk-chain`.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

// Point a submitting device at a chain. The lowest rung of the resolution
// ladder in `ingest.rs`, and until now the only one with no UI to set it.
/// Set a device's default chain.
///
/// Session-only. Rung 4 of the chain ladder — the coarsest fallback.
#[utoipa::path(
    patch,
    path = "/api/v1/devices/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The device.")),
    request_body = SetDeviceChainRequest,
    responses(
        (status = 200, description = "Updated. `status` is `ok`.", body = OkResponse),
        (status = 400, description = "`chain_id` absent, or not one of your chains.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn set_device_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<SetDeviceChainRequest>,
) -> impl IntoResponse {
    let Some(chain_id) = body.chain_id else {
        return bad("chain_id is required — send a chain id, or an explicit null to clear it");
    };
    if let Some(cid) = chain_id {
        match state.db.get_chain(user.user_id, cid).await {
            Ok(Some(_)) => {}
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse { code: 400, error: "Unknown signal chain".into() }),
                )
                    .into_response()
            }
        }
    }
    match state.db.set_device_default_chain(user.user_id, id, chain_id).await {
        Ok(true) => (StatusCode::OK, OkResponse::new("ok")).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Device not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// ── Equipment endpoints ──

#[derive(Deserialize, ToSchema)]
pub struct CreateEquipmentRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub equipment_type: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub notes: Option<String>,
    /// When it was bought, unix seconds. Distinct from `first_used`, which is
    /// when Tapedeck first saw it — gear is usually owned for a while before
    /// anyone gets round to cataloguing it, and burn-in hours are counted from
    /// the purchase.
    pub purchased_at: Option<i64>,
}

/// Add gear.
///
/// Session-only. The type field is **`type`** here and comes back as
/// `equipment_type` — see the `Equipment` schema.
#[utoipa::path(
    post,
    path = "/api/v1/equipment",
    tag = "Chains",
    request_body = CreateEquipmentRequest,
    responses(
        (status = 201, description = "Created — **or matched**. Gear is keyed on `(user, name)`: posting a name you already own returns that row's id and stamps `last_used`, and every other field you sent is **ignored** rather than applied. Use `PATCH /api/v1/equipment/{id}` to change one. Always 201, even when nothing was created.", body = Created),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn create_equipment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateEquipmentRequest>,
) -> impl IntoResponse {
    match state
        .db
        .upsert_equipment(
            user.user_id,
            &body.name,
            &body.equipment_type,
            body.brand.as_deref(),
            body.model.as_deref(),
            0.0,
            body.purchased_at,
        )
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(Created { id, status: "created" })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

/// Your gear.
///
/// Scope: `read` *(since v0.37)*. Hours are derived from the chains that use each piece.
#[utoipa::path(
    get,
    path = "/api/v1/equipment",
    tag = "Chains",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    responses(
        (status = 200, description = "OK.", body = EquipmentList),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
pub async fn list_equipment(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.get_equipment(user.user_id).await {
        Ok(gear) => (StatusCode::OK, Json(EquipmentList { equipment: gear })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

/// The update body is deliberately its own type rather than a reuse of
/// `CreateEquipmentRequest`: `status` is settable here and not at creation
/// (gear is tagged from the "Not in a chain yet" table, once it exists), and a
/// shared struct would mean POST accepting a field it silently threw away.
///
/// Every field is written unconditionally — this endpoint is a whole-record
/// replace, not a partial patch, which is why an omitted `brand` has always
/// cleared the brand. Clients must send the full record back.
#[derive(Deserialize, ToSchema)]
pub struct UpdateEquipmentRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub equipment_type: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub notes: Option<String>,
    pub purchased_at: Option<i64>,
    /// Why it is not in a chain: `new`, `spare`, `lent`. Absent clears it.
    pub status: Option<String>,
}

/// Update gear.
///
/// Session-only. A **whole-record replace**, not a partial patch — every
/// field is written, so an omitted `brand` clears the brand. Send the full
/// record back.
#[utoipa::path(
    patch,
    path = "/api/v1/equipment/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The gear.")),
    request_body = UpdateEquipmentRequest,
    responses(
        (status = 200, description = "Updated.", body = Updated),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_equipment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateEquipmentRequest>,
) -> impl IntoResponse {
    match state
        .db
        .update_equipment(
            user.user_id,
            id,
            &body.name,
            &body.equipment_type,
            body.brand.as_deref(),
            body.model.as_deref(),
            body.notes.as_deref(),
            body.purchased_at,
            body.status.as_deref(),
        )
        .await
    {
        Ok(true) => (StatusCode::OK, Json(Updated { id, status: "updated" })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Equipment not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

/// Delete gear.
///
/// Session-only. Chains referencing it stay readable — every component
/// keeps a name snapshot alongside its optional `equipment_id`.
#[utoipa::path(
    delete,
    path = "/api/v1/equipment/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The gear.")),
    responses(
        (status = 204, description = "Deleted."),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_equipment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_equipment(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Equipment not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// ── Output-device → chain bindings (Phase B) ──

/// Output-device bindings.
///
/// Scope: `read` *(since v0.37)*. Rung 2 of the chain ladder.
#[utoipa::path(
    get,
    path = "/api/v1/bindings",
    tag = "Chains",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    responses(
        (status = 200, description = "OK.", body = BindingList),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
async fn list_bindings(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.list_bindings(user.user_id).await {
        Ok(bindings) => (StatusCode::OK, Json(BindingList { bindings })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

#[derive(Deserialize, ToSchema)]
pub struct SetBindingRequest {
    /// The output device, as a client reports it in
    /// `tapedeck_device.output_device`.
    pub identifier: String,
    /// The chain it maps to. **Required: `null` clears the mapping; absent is
    /// a 400.** Must be one of your chains.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<i64>, required = true)]
    pub chain_id: Option<Option<i64>>,
}

/// Bind an output device name to a chain.
///
/// Session-only.
#[utoipa::path(
    put,
    path = "/api/v1/bindings",
    tag = "Chains",
    request_body = SetBindingRequest,
    responses(
        (status = 200, description = "Saved. `status` is `ok`.", body = OkResponse),
        (status = 400, description = "`chain_id` absent, or not one of your chains.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn set_binding(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SetBindingRequest>,
) -> impl IntoResponse {
    let Some(chain_id) = body.chain_id else {
        return bad("chain_id is required — send a chain id, or an explicit null to clear it");
    };
    // A chain id from a request body is user input — check it is the caller's,
    // as the device setter does. Without this a binding could point at another
    // household member's chain, and every listen through that output would be
    // attributed to gear that is not the listener's.
    if let Some(cid) = chain_id {
        match state.db.get_chain(user.user_id, cid).await {
            Ok(Some(_)) => {}
            Ok(None) => return bad("Unknown signal chain"),
            Err(e) => return internal(e),
        }
    }
    match state.db.set_binding(user.user_id, &body.identifier, chain_id).await {
        Ok(()) => (StatusCode::OK, OkResponse::new("ok")).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

/// Remove a binding.
///
/// Session-only.
#[utoipa::path(
    delete,
    path = "/api/v1/bindings/{id}",
    tag = "Chains",
    params(("id" = i64, Path, description = "The binding.")),
    responses(
        (status = 204, description = "Deleted."),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_binding(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_binding(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse { code: 404, error: "Binding not found".into() }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    create_chain,
    list_chains,
    get_chain,
    update_chain,
    delete_chain,
    list_variants,
    create_variant,
    update_variant,
    delete_variant,
    promote_variant,
    list_duplicates,
    fold_chain,
    list_devices,
    set_device_chain,
    create_equipment,
    list_equipment,
    update_equipment,
    delete_equipment,
    list_bindings,
    set_binding,
    delete_binding,
))]
pub struct Api;

// ── Router ──

pub fn create_gear_router() -> Router<Arc<AppState>> {
    use axum::routing::patch;
    Router::new()
        .route("/api/v1/chains", post(create_chain).get(list_chains))
        .route("/api/v1/chains/:id", get(get_chain).patch(update_chain).delete(delete_chain))
        // Before "/:id" would be wrong here — "variants" is a distinct literal
        // segment under /chains, not a chain id, and matchit prefers literals.
        .route("/api/v1/chains/variants", get(list_variants).post(create_variant))
        .route(
            "/api/v1/chains/variants/:id",
            axum::routing::patch(update_variant).delete(delete_variant),
        )
        .route("/api/v1/chains/variants/:id/promote", post(promote_variant))
        // Same reasoning as "variants" above: a literal segment under /chains,
        // which matchit prefers over "/:id".
        .route("/api/v1/chains/duplicates", get(list_duplicates))
        .route("/api/v1/chains/:id/fold", post(fold_chain))
        .route("/api/v1/devices", get(list_devices))
        .route("/api/v1/devices/:id", patch(set_device_chain))
        .route("/api/v1/equipment", post(create_equipment).get(list_equipment))
        .route("/api/v1/equipment/:id", patch(update_equipment).delete(delete_equipment))
        .route("/api/v1/bindings", get(list_bindings).put(set_binding))
        .route("/api/v1/bindings/:id", axum::routing::delete(delete_binding))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both setters overwrite, so a misspelled key must not read as "clear".
    #[test]
    fn a_chain_setter_tells_absent_from_null_from_a_value() {
        let dev = |s: &str| serde_json::from_str::<SetDeviceChainRequest>(s).unwrap().chain_id;
        assert_eq!(dev(r#"{"chain_id":7}"#), Some(Some(7)));
        assert_eq!(dev(r#"{"chain_id":null}"#), Some(None));
        assert_eq!(dev(r#"{"chainId":7}"#), None);

        let bind = |s: &str| serde_json::from_str::<SetBindingRequest>(s).unwrap().chain_id;
        assert_eq!(bind(r#"{"identifier":"DAC","chain_id":7}"#), Some(Some(7)));
        assert_eq!(bind(r#"{"identifier":"DAC","chain_id":null}"#), Some(None));
        assert_eq!(bind(r#"{"identifier":"DAC"}"#), None);
    }
}
