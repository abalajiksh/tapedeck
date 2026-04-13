// src/web_assets.rs — Serve the embedded SvelteKit SPA
//
// The `web/` SvelteKit project builds into `static/` via adapter-static.
// At compile time, rust-embed bundles everything in `static/` into the binary.
// At runtime, Axum serves these files and falls back to index.html for SPA routing.

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "static/"]
struct Assets;

/// Serve an embedded file by path, with correct content type.
async fn serve_asset(Path(path): Path<String>) -> Response {
    serve_file(&path)
}

/// Serve the SPA index (fallback for client-side routing).
async fn serve_index() -> Response {
    serve_file("index.html")
}

fn serve_file(path: &str) -> Response {
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();

            let mut response = (StatusCode::OK, content.data.to_vec()).into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, mime.parse().unwrap());

            // Cache immutable hashed assets aggressively
            if path.contains("/_app/") {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".parse().unwrap(),
                );
            }

            response
        }
        None => {
            // SPA fallback: serve index.html for unrecognized paths
            // (lets SvelteKit handle client-side routing)
            if let Some(index) = Assets::get("index.html") {
                let mut response = (StatusCode::OK, index.data.to_vec()).into_response();
                response
                    .headers_mut()
                    .insert(header::CONTENT_TYPE, "text/html".parse().unwrap());
                response
            } else {
                (StatusCode::NOT_FOUND, "Not Found").into_response()
            }
        }
    }
}

/// Create a router that serves the embedded frontend.
///
/// Mount this AFTER your API routes so `/api/*`, `/1/*`, `/admin/*`, and `/health`
/// are handled first, and everything else falls through to the SPA.
pub fn create_frontend_router() -> Router {
    Router::new()
        // Catch-all for static assets (JS, CSS, images, fonts)
        .route("/*path", get(serve_asset))
        // Root serves index.html
        .route("/", get(serve_index))
}
