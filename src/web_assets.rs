// src/web_assets.rs — Serve the embedded SvelteKit SPA
//
// The `web/` SvelteKit project builds into `static/` via adapter-static.
// At compile time, rust-embed bundles everything in `static/` into the binary.
// At runtime this handler is installed as the router *fallback*: anything the
// API routers didn't claim lands here.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "static/"]
struct Assets;

/// Path prefixes owned by the API. These must never fall through to the SPA —
/// returning `index.html` for a mistyped API path hands the browser a 200 with
/// an HTML body that `res.json()` then chokes on.
///
/// `oauth/` and `.well-known/` are here for a sharper reason than a typo: an
/// MCP client discovering this server *expects* a 404 when it is not an OAuth
/// authorization server, and hands it the SPA shell instead would have it try
/// to `JSON.parse` an HTML document and report the instance as broken rather
/// than as unconfigured.
const API_PREFIXES: [&str; 5] = ["api/", "1/", "admin/", "oauth/", ".well-known/"];
/// `mcp` is exact, not a prefix — `/mcp` is the endpoint and there is nothing
/// beneath it.
const API_EXACT: [&str; 4] = ["health", "log-level", "metrics", "mcp"];

fn is_api_path(path: &str) -> bool {
    API_PREFIXES.iter().any(|p| path.starts_with(p)) || API_EXACT.contains(&path)
}

/// A path that names a concrete file (hashed bundle, image, favicon) rather
/// than a client-side route. Missing ones are a real 404, not an SPA route.
fn looks_like_file(path: &str) -> bool {
    path.starts_with("_app/") || path.rsplit('/').next().is_some_and(|seg| seg.contains('.'))
}

/// Router fallback: serve an embedded asset, else the SPA shell.
pub async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if is_api_path(path) {
        return not_found_json(uri.path());
    }

    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream().to_string();

        let mut response = (StatusCode::OK, content.data.to_vec()).into_response();
        let headers = response.headers_mut();
        if let Ok(value) = mime.parse() {
            headers.insert(header::CONTENT_TYPE, value);
        }

        // Vite emits content-hashed filenames under _app/immutable, so these
        // can be cached forever.
        if path.starts_with("_app/") {
            headers.insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        }

        return response;
    }

    if looks_like_file(path) {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    serve_index()
}

/// The SPA shell — SvelteKit takes over routing from here.
fn serve_index() -> Response {
    match Assets::get("index.html") {
        Some(index) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            index.data.to_vec(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Frontend not built — run ./build-web.sh").into_response(),
    }
}

fn not_found_json(path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(crate::server::models::ErrorResponse {
            code: 404,
            error: format!("No such endpoint: {}", path),
        }),
    )
        .into_response()
}
