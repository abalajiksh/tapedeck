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
///
/// `public/` is here for a third reason again: it is the one namespace a
/// *stranger* types, from a widget on somebody else's website. A mistyped
/// username has to come back as the module's own JSON 404 — which is the same
/// answer an unpublished deck gets, deliberately — rather than as 200 and an
/// HTML shell that the fetch on that page would try to parse.
const API_PREFIXES: [&str; 6] = ["api/", "1/", "admin/", "oauth/", ".well-known/", "public/"];
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
pub(crate) fn serve_index() -> Response {
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

#[cfg(test)]
mod tests {
    /// **The UI's version is the crate's, and there is no second copy to
    /// drift.**
    ///
    /// It used to come out of `web/package.json`, which sat **eighteen releases
    /// behind** the server (ui 0.84.0 against 0.102.0, v0.102.1) — so the one
    /// readout anybody would use to check whether their frontend was stale was
    /// itself the stalest thing on the screen. Bumping it into line fixed the
    /// number and kept the shape: two versions, always equal, one maintained by
    /// remembering. This is the rule v0.102.0 earned on the MusicBrainz
    /// User-Agent, applied to the next hand-written version string in the tree:
    /// *a version a human has to remember to bump is one that is always
    /// wrong.*
    ///
    /// Textual, the same technique as the OpenAPI route-drift test, because the
    /// failure it guards is somebody "tidying" `vite.config.ts` back to
    /// `pkg.version` — which reintroduces the drift **silently**, since the two
    /// numbers agree on the day of the change and only part company later.
    #[test]
    fn the_ui_version_is_the_crate_version_and_not_a_second_copy() {
        let raw =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/web/vite.config.ts"))
                .expect("web/vite.config.ts is where __UI_VERSION__ is injected");
        // **Comments out first, and this is not fussiness.** The first version
        // of this test asserted the file *mentioned* `../Cargo.toml` — which
        // the doc comment above `crateVersion` does, at length. So pointing the
        // `readFileSync` back at `./package.json` left the guard **passing**,
        // and only breaking it on purpose said so. A guard that reads prose is
        // reading the explanation rather than the code.
        let vite = strip_comments(&raw);

        // The companion assertion the OpenAPI note insists on: a scanner that
        // found nothing would pass by examining an empty set.
        assert!(
            vite.contains("__UI_VERSION__"),
            "vite.config.ts no longer injects __UI_VERSION__ — this guard is \
             looking at the wrong file"
        );
        assert!(
            vite.contains("readFileSync('../Cargo.toml'"),
            "__UI_VERSION__ must be derived from the crate's own version, not \
             kept as a second copy someone has to remember to bump"
        );
        assert!(
            !vite.contains("readFileSync('./package.json'"),
            "vite.config.ts is reading a version out of web/package.json \
             again — that is the copy which drifted eighteen releases"
        );

        let pkg = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/web/package.json"))
            .expect("web/package.json");
        assert!(
            !pkg.contains("\"version\""),
            "web/package.json has a version field again. It is a `private` \
             package, so the field has no packaging meaning — it only ever fed \
             __UI_VERSION__, and a copy that exists is a copy that drifts"
        );
    }

    /// Block and line comments out, so a scan sees code rather than the
    /// explanation of it. Crude on purpose — it reads one small file whose
    /// shape is known, and a real TypeScript parse here would be a dependency
    /// bought to check three lines.
    fn strip_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        while let Some(ch) = rest.chars().next() {
            if rest.starts_with("/*") {
                match rest.find("*/") {
                    Some(end) => rest = &rest[end + 2..],
                    None => break,
                }
            } else if rest.starts_with("//") {
                match rest.find('\n') {
                    Some(end) => rest = &rest[end..],
                    None => break,
                }
            } else {
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
        out
    }
}
