use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::{AuthUser, ReadAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::models::*;

// ── Chain variants ──
//
// A chain with one step swapped. Two cartridges on one turntable, or the same
// commute with different IEMs, are one chain and a choice — not two chains.
// Modelling them as separate chains is what makes a gear collection produce a
// page of near-duplicates, and it is also what stops per-component wear from
// being answerable: "which chain was it" does not tell you what was fitted.

#[derive(Deserialize)]
pub struct CreateVariantRequest {
    pub chain_id: i64,
    pub name: String,
    #[serde(default)]
    pub overrides: Vec<VariantOverride>,
}

#[derive(Deserialize)]
pub struct UpdateVariantRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub overrides: Option<Vec<VariantOverride>>,
}

/// GET /api/v1/chains/variants — every variant the caller owns.
///
/// Flat rather than nested under each chain: the page needs them keyed by
/// chain, and one list it can group beats N requests.
async fn list_variants(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.list_variants(user.user_id).await {
        Ok(variants) => {
            (StatusCode::OK, Json(serde_json::json!({ "variants": variants }))).into_response()
        }
        Err(e) => internal(e),
    }
}

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
        Ok(id) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => internal(e),
    }
}

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
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Ok(false) => not_found("No such variant"),
        Err(e) => internal(e),
    }
}

/// DELETE /api/v1/chains/variants/:id
///
/// The listens that went through it keep their chain and fall back to its base
/// components — which is what a null variant means everywhere else — rather
/// than being deleted along with it.
async fn delete_variant(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_variant(user.user_id, id).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "status": "deleted" }))).into_response()
        }
        Ok(false) => not_found("No such variant"),
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
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!(ErrorResponse { code: 400, error: msg.into() })),
    )
        .into_response()
}
fn not_found(msg: &str) -> axum::response::Response {
    (StatusCode::NOT_FOUND, Json(serde_json::json!(ErrorResponse { code: 404, error: msg.into() })))
        .into_response()
}
fn internal(e: sqlx::Error) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
    )
        .into_response()
}

// ── Signal Chain endpoints ──

#[derive(Deserialize)]
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
        total_hours: 0.0,
        created_at: 0, // set by db
    };

    match state.db.create_chain(&chain).await {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id, "status": "created" })))
            .into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE") {
                (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!(ErrorResponse {
                        code: 409,
                        error: format!("Chain '{}' already exists", chain.name)
                    })),
                )
                    .into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!(ErrorResponse { code: 500, error: msg })),
                )
                    .into_response()
            }
        }
    }
}

async fn list_chains(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.get_chains(user.user_id).await {
        Ok(chains) => {
            (StatusCode::OK, Json(serde_json::json!({ "chains": chains }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_chain(user.user_id, id).await {
        Ok(Some(chain)) => (StatusCode::OK, Json(serde_json::json!(chain))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Chain not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

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
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "id": id, "status": "updated" })))
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Chain not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_chain(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Chain not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

// ── Device endpoints ──

async fn list_devices(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.get_devices(user.user_id).await {
        Ok(devices) => {
            (StatusCode::OK, Json(serde_json::json!({ "devices": devices }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetDeviceChainRequest {
    pub chain_id: Option<i64>, // null clears it
}

/// Point a submitting device at a chain. The lowest rung of the resolution
/// ladder in `ingest.rs`, and until now the only one with no UI to set it.
async fn set_device_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<SetDeviceChainRequest>,
) -> impl IntoResponse {
    if let Some(cid) = body.chain_id {
        match state.db.get_chain(user.user_id, cid).await {
            Ok(Some(_)) => {}
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!(ErrorResponse {
                        code: 400,
                        error: "Unknown signal chain".into()
                    })),
                )
                    .into_response()
            }
        }
    }
    match state.db.set_device_default_chain(user.user_id, id, body.chain_id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Device not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

// ── Equipment endpoints ──

#[derive(Deserialize)]
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
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id, "status": "created" })))
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn list_equipment(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.get_equipment(user.user_id).await {
        Ok(gear) => {
            (StatusCode::OK, Json(serde_json::json!({ "equipment": gear }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn update_equipment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CreateEquipmentRequest>,
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
        )
        .await
    {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "id": id, "status": "updated" })))
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                code: 404,
                error: "Equipment not found".into()
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_equipment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_equipment(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse {
                code: 404,
                error: "Equipment not found".into()
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

// ── Output-device → chain bindings (Phase B) ──

async fn list_bindings(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.list_bindings(user.user_id).await {
        Ok(bindings) => {
            (StatusCode::OK, Json(serde_json::json!({ "bindings": bindings }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetBindingRequest {
    pub identifier: String,
    pub chain_id: Option<i64>, // null clears the mapping
}

async fn set_binding(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SetBindingRequest>,
) -> impl IntoResponse {
    match state.db.set_binding(user.user_id, &body.identifier, body.chain_id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_binding(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_binding(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Binding not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

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
        .route("/api/v1/devices", get(list_devices))
        .route("/api/v1/devices/:id", patch(set_device_chain))
        .route("/api/v1/equipment", post(create_equipment).get(list_equipment))
        .route("/api/v1/equipment/:id", patch(update_equipment).delete(delete_equipment))
        .route("/api/v1/bindings", get(list_bindings).put(set_binding))
        .route("/api/v1/bindings/:id", axum::routing::delete(delete_binding))
}
