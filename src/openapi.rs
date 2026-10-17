//! The API description, derived from the handlers by utoipa, and served.
//!
//! Every handler carries a `#[utoipa::path]` and every body it reads or writes
//! is a type deriving `ToSchema`, so the schemas in the document are the types
//! that actually go over the wire rather than a description of them kept
//! somewhere else. Each `server::*` module lists its operations in its own
//! `#[derive(OpenApi)] struct Api` beside its router, and `server::api_docs`
//! collects those in the order `build_app` merges the routers. This file adds
//! what belongs to no single module — `info`, the tags, the security schemes,
//! the shared error responses — and keeps the result honest.
//!
//! **What is public and what is not.** utoipa reads *doc comments*: `///` on a
//! handler becomes the operation's summary (first paragraph) and description
//! (the rest), and `///` on a `ToSchema` type or field becomes its schema
//! description. So on anything documented here, `///` is written for a client
//! author, and notes for whoever edits the code are `//` comments. A `///`
//! naming an internal function lands in the published reference.
//!
//! **Why serve it from the binary at all**, when the rendered docs are hosted
//! separately: this is the one thing a static docs site cannot do — describe
//! *that* instance at *that* version. For a self-hosted app a client can ask
//! its own server what it supports, rather than guessing from whatever the
//! docs site last deployed.
//!
//! **`openapi.yaml` at the repo root is a generated snapshot**, not a source.
//! It is committed because the website's spec-sync job fetches it by release
//! tag, and because an API change then shows up as a diff in review. A test
//! fails when it is stale; regenerate it with
//!
//! ```text
//! TAPEDECK_BLESS_OPENAPI=1 cargo test openapi   # or: tapedeck openapi > openapi.yaml
//! ```
//!
//! The tests below are the part that earns its keep. utoipa derives shapes but
//! cannot see the router, so which operations exist, which function answers
//! each, and which credential each accepts are still compared against the
//! `.route(…)` calls and the handlers' extractors — textually, the same trade
//! the drift test has always made.

use std::sync::OnceLock;

use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
};
use utoipa::openapi::{
    info::{ContactBuilder, InfoBuilder, LicenseBuilder},
    response::ResponseBuilder,
    security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme},
    server::ServerBuilder,
    tag::TagBuilder,
    ContentBuilder, OpenApi, Ref, RefOr, Response,
};

const DESCRIPTION: &str = r#"Self-hosted scrobbler and listening journal.

Tapedeck ingests listens, enriches them with MusicBrainz metadata, stores
them in SQLite and forwards them to Last.fm / ListenBrainz / Libre.fm. It
also records *how* you listened — signal chains, gear, audio quality — which
is the part no other scrobbler models.

## Three auth mechanisms, and they are not interchangeable

| | How | Reaches |
|---|---|---|
| **Session cookie** | `td_session`, set by `POST /api/v1/auth/login` | everything |
| **API token** | `Authorization: Token <t>` | `/1/*`, plus whatever its scopes allow |
| **Metrics bearer** | `Authorization: Bearer <t>` | `GET /metrics` only |

### Token scopes

`tokens.scopes` is a space- or comma-separated set. Three grants matter, and
**none of them implies another**:

* `submit` — post listens (`POST /1/submit-listens`). Every scrobble client
  has this; it must not carry the power to delete the history it appends to.
* `read` *(since v0.37)* — read your own data on the endpoints whose security
  lists `apiToken: [read]`.
* `write` *(since v0.39)* — change your own data on the endpoints whose
  security lists `apiToken: [write]`.

`all` grants both `read` and `write`. Matching is exact — a scope of
`rewrite` does not grant `write`.

A valid token that lacks the required scope gets **403, not 401**. It
authenticated fine and re-authenticating will not help; a 401 would send a
client into a token-refresh loop it cannot win.

### What a token can never reach

Everything under `/admin/`, plus your sources, your forwarding connections,
Discogs credentials, data export/backup, the sanitiser and the enrichment
jobs. Those are **session-only, permanently**. A leaked token is a bad
afternoon; a leaked token that can mint more tokens, read another household
member's data, or repoint where your listens are forwarded is a different
category. Requests to them with a token get 401.

## Getting a token

Two ways, and neither asks anyone to type a token by hand:

* **Device-code pairing** — `POST /1/pair/start`, poll `/1/pair/poll` while
  the user approves the short code in Settings. For clients with no browser.
* **Password → token** — post credentials to `POST /api/v1/auth/login`,
  then mint a named token. This is the phone's flow. A named token appears
  in Settings and can be revoked when the device is lost; a session cookie
  expires hard at 7 days with no sliding renewal and cannot be told apart
  from any other browser.

## Conventions

* Timestamps are **Unix seconds, UTC**, and always the *start* of play.
* Errors are `{ "code": <int>, "error": "<message>" }` with a matching HTTP
  status.
* Every user-scoped query is filtered by the caller's own `user_id`. Scopes
  widen *who may ask*, never *whose data is returned*.
* Counting endpoints exclude skipped listens (`skipped = FALSE`).
* Extra keys may appear in any response body; ignore what you do not know.

## Instance-specific

`GET /api/openapi.yaml` serves this document from the running binary, so a
client can ask *its own server* what that version supports rather than
guessing from a hosted copy.

The document is generated from the handlers themselves: every schema in it is
the type the server actually reads or writes, so a field documented here is a
field that exists."#;

/// The tags, in the order a reader meets them. An operation tagged with
/// anything not in this list fails `every_operation_is_tagged_with_a_declared_tag`.
const TAGS: &[(&str, &str)] = &[
    ("Auth", "Login, first-run setup, session state."),
    (
        "ListenBrainz",
        r#"ListenBrainz-compatible surface under `/1/`. Shapes follow the LB Core
API because clients branch on those exact fields. Two deliberate
divergences: these require auth and are scoped to the caller (LB serves
them publicly; a self-hosted instance must not), and an invalid token on
`validate-token` is `200 {valid:false}`, never 401, because clients read
the field."#,
    ),
    ("Pairing", "Device-code pairing for clients with no browser."),
    (
        "Patch",
        r#"Patch — following people, on this instance and others. The ActivityPub
surface (`/users/…`, `/.well-known/webfinger`) is unauthenticated and lets
another server find out who somebody here is and say something to them; the
rest is the session-only local half: the Reel, decks, Dubs and Shared Spools.

Every federated endpoint is off until a user switches federation on, and an
account that has not is indistinguishable from one that does not exist —
the same rule the public now-playing widget and device pairing follow."#,
    ),
    (
        "Account",
        r#"Your own account — password, display name, bio, profile picture. Scoped to
the caller and never takes an id, unlike the admin routes under
`/admin/users`. Session-only: a scrobble client has no business changing
the credential it authenticates with."#,
    ),
    ("Listens", "The scrobble history — read, edit, delete, search."),
    ("Statistics", "Aggregations, charts, personality."),
    ("Library", "Album, artist and entity pages."),
    ("Loves", "Loved recordings, releases and artists."),
    ("Notes", "Liner notes — your own writing about records."),
    ("Sessions", "Contiguous listening, grouped."),
    ("Chains", "Signal chains, gear, devices and bindings."),
    ("Shelf", "Records, tapes and discs — a shelf you catalogue and sides you play."),
    ("Radio", "Stations, presets and the tuner that logs what they play. German public radio, via RadioDNS."),
    ("Genre map", "Every Noise at Once — coverage, colour, trajectory, neighbours."),
    ("Rediscovery", "Five lists, all built from your own history."),
    ("Sources", "Where listens are polled from. Session-only — holds credentials."),
    ("Connections", "Where listens are forwarded. Session-only — holds credentials."),
    ("Import & export", "Bulk in and out. Session-only."),
    ("Maintenance", "Enrichment backfills and the metadata sanitiser. Session-only."),
    ("Settings", "Preferences and artwork."),
    ("Admin", "Users, tokens, genre-map scrape. Session-only, admin role."),
    (
        "MCP",
        r#"The Model Context Protocol surface — one endpoint that lets an AI
assistant read this user's listening and hand playlists back. Runs for
every user, reaches nobody's data until they authorise a connection.

**Dual-era.** Revision `2026-07-28` is stateless with per-request `_meta`
and mirrored headers; `2025-11-25` and earlier open with an `initialize`
handshake. Both are served on the same endpoint.

Credentials live in `mcp_connections`, **not** in `tokens` — an MCP grant
is a standing permission given to somebody else's service, `tokens.scopes`
treats `all` as a wildcard, and an OAuth credential expires and refreshes."#,
    ),
    (
        "OAuth",
        r#"An OAuth 2.1 authorization server, scoped to one job: letting a hosted
assistant obtain an MCP credential. PKCE `S256` only, public clients only,
exact `redirect_uri` matching, single-use codes."#,
    ),
    (
        "Playlists",
        r#"Playlists that live in Tapedeck, reviewed before they reach a media
server, plus the direct push that has always existed."#,
    ),
    (
        "Crate",
        r#"Records and artists somebody suggested that you do not own — the one place
in Tapedeck that names music outside the collection.

**Tapedeck does not recommend; it records recommendations made to you.**
The vision rule governs what Tapedeck *generates* (Rediscovery is built
entirely from your own history and stays that way); this stores what an
assistant said, the way notes store what you wrote. It is its own table,
its own endpoints and its own page, and never appears in a history or
statistics readout."#,
    ),
    ("Ops", "Health, metrics, log level."),
];

// ── Shared error responses ────────────────────────────────────────────────────
//
// Referenced from `#[utoipa::path]` as `(status = 401, response = Unauthorized)`,
// which writes a `$ref` into `components/responses` rather than repeating the
// body on 280 operations. The body is always `ErrorResponse`, published as
// `Error`.

fn error_response(description: &str, example: Option<serde_json::Value>) -> RefOr<Response> {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            ContentBuilder::new()
                .schema(Some(Ref::from_schema_name("Error")))
                .example(example)
                .build(),
        )
        .build()
        .into()
}

macro_rules! shared_response {
    ($name:ident, $description:expr, $example:expr) => {
        pub struct $name;

        impl<'s> utoipa::ToResponse<'s> for $name {
            fn response() -> (&'s str, RefOr<Response>) {
                (stringify!($name), error_response($description, $example))
            }
        }
    };
}

shared_response!(
    Unauthorized,
    "No valid session cookie or token. Also returned when a *token* is\n\
     presented to a session-only endpoint — the endpoint does not accept\n\
     tokens at all, so the scope is irrelevant.",
    Some(serde_json::json!({
        "code": 401,
        "error": "Authentication required. Log in to access this endpoint."
    }))
);
shared_response!(
    Forbidden,
    "Authenticated, but not permitted. Either the token lacks the required\n\
     scope, or the endpoint needs the admin role. Deliberately not a 401 —\n\
     re-authenticating will not help.",
    Some(serde_json::json!({ "code": 403, "error": "This token does not have the 'write' scope" }))
);
shared_response!(NotFound, "No such resource, or it belongs to another user.", None);
shared_response!(BadRequest, "Malformed or rejected input.", None);

/// Raw bytes — an image, an archive, a log file. Used as a response body
/// with the content type of what is actually sent.
pub struct Bytes;

impl utoipa::PartialSchema for Bytes {
    fn schema() -> RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                utoipa::openapi::KnownFormat::Binary,
            )))
            .into()
    }
}

impl utoipa::ToSchema for Bytes {}

/// A `multipart/form-data` upload of one file.
///
/// The server takes the first file part whatever its field name; `file` is the
/// name to use. Its declared `Content-Type` decides how the file is stored — a
/// filename from the client never reaches a path on disk.
#[derive(utoipa::ToSchema)]
#[allow(dead_code)] // describes a multipart body; never constructed
pub struct FileUpload {
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
}

/// Everything that belongs to the document rather than to a module.
#[derive(utoipa::OpenApi)]
#[openapi(
    components(
        schemas(crate::server::models::ErrorResponse, crate::server::models::OkResponse),
        responses(Unauthorized, Forbidden, NotFound, BadRequest)
    ),
    paths(spec),
    security(("sessionCookie" = []))
)]
struct Root;

fn security_schemes(doc: &mut OpenApi) {
    let components = doc.components.get_or_insert_with(Default::default);
    let schemes = [
        (
            "sessionCookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "td_session",
                "Set by `POST /api/v1/auth/login`. `HttpOnly`, `SameSite=Lax`, and\n\
                 `Secure` when the instance can tell the browser used HTTPS (derived\n\
                 from `X-Forwarded-Proto`, and only trusted when `trust_proxy` is set —\n\
                 behind a tunnel the origin sees plain HTTP while the client is on TLS).\n\
                 Expires hard at 7 days with no sliding renewal.",
            ))),
        ),
        (
            "apiToken",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "Authorization",
                "`Authorization: Token <token>`. What the token reaches depends on its\n\
                 scopes — see the description at the top of this document. Lowercase\n\
                 `token ` is also accepted.",
            ))),
        ),
        (
            "metricsBearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "`Authorization: Bearer $TAPEDECK_METRICS_TOKEN`, for `GET /metrics`\n\
                         only. An admin session also works. Never open.",
                    ))
                    .build(),
            ),
        ),
        (
            "mcpBearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "An MCP connection credential, from Settings → AI Connections or from the\n\
                         OAuth flow. Checked against `mcp_connections` — **not** against\n\
                         `tokens`, which is a separate credential store for scrobble clients.\n\
                         \n\
                         Used by `POST /mcp` and nothing else. A 401 from that endpoint carries\n\
                         `WWW-Authenticate` with a `resource_metadata` pointer, which is how a\n\
                         client discovers the authorization server.",
                    ))
                    .build(),
            ),
        ),
    ];
    for (name, scheme) in schemes {
        components.security_schemes.insert(name.to_string(), scheme);
    }
}

/// The document, and any schema name two modules used for different shapes.
///
/// utoipa's own `merge` keeps the first schema of a name and silently drops
/// the second, which would publish one type's shape under another type's
/// operations. Collisions are collected rather than fatal so that serving the
/// document can never take a request down; `no_two_types_publish_one_schema_name`
/// is what makes them a build failure.
fn build() -> (OpenApi, Vec<String>) {
    use utoipa::OpenApi as _;

    let mut doc = Root::openapi();
    doc.info = InfoBuilder::new()
        .title("Tapedeck")
        .version(env!("CARGO_PKG_VERSION"))
        .description(Some(DESCRIPTION))
        .license(Some(
            LicenseBuilder::new()
                .name("AGPL-3.0-or-later")
                .identifier(Some("AGPL-3.0-or-later"))
                .build(),
        ))
        .contact(Some(
            ContactBuilder::new()
                .name(Some("Tapedeck"))
                .url(Some("https://codeberg.org/abksh/tapedeck"))
                .build(),
        ))
        .build();
    doc.servers = Some(vec![ServerBuilder::new()
        .url("/")
        .description(Some("The Tapedeck instance serving this document."))
        .build()]);
    doc.tags = Some(
        TAGS.iter()
            .map(|(name, description)| {
                TagBuilder::new().name(*name).description(Some(*description)).build()
            })
            .collect(),
    );
    security_schemes(&mut doc);

    let mut collisions = Vec::new();
    for module in crate::server::api_docs() {
        if let (Some(ours), Some(theirs)) = (&doc.components, &module.components) {
            for (name, schema) in &theirs.schemas {
                if let Some(existing) = ours.schemas.get(name) {
                    let (a, b) = (serde_json::to_value(existing), serde_json::to_value(schema));
                    if a.ok() != b.ok() {
                        collisions.push(name.clone());
                    }
                }
            }
        }
        // A module's own `Api` carries no tags list and no global security, so
        // nothing else can arrive here that the root has not already set.
        doc.merge(module);
    }
    (doc, collisions)
}

/// This instance's API description.
pub fn document() -> OpenApi {
    build().0
}

/// The YAML served at `/api/openapi.yaml`, rendered once.
pub fn yaml() -> &'static str {
    static YAML: OnceLock<String> = OnceLock::new();
    YAML.get_or_init(|| document().to_yaml().expect("the OpenAPI document serialises"))
}

/// This document.
///
/// Unauthenticated by design — it describes the shape of the API, not any
/// data, and a client needs to read it *before* it has a credential.
///
/// Served from the binary because it is the one thing a separately hosted
/// docs site cannot do: describe **this** instance at **this** version. For
/// a self-hosted app that beats any version dropdown — ask your own server
/// what it supports rather than guessing.
#[utoipa::path(
    get,
    path = "/api/openapi.yaml",
    tag = "Ops",
    security(),
    responses((status = 200, description = "The OpenAPI document.", body = String, content_type = "application/yaml"))
)]
pub async fn spec() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/yaml; charset=utf-8"),
            // Version-specific content at a stable URL; let a client revalidate
            // rather than hold a stale copy across an upgrade.
            (header::CACHE_CONTROL, "public, max-age=0, must-revalidate"),
        ],
        yaml(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use utoipa::openapi::{path::Operation, HttpMethod, OpenApi};

    /// Axum 0.7 spells a path parameter `:id`; OpenAPI spells it `{id}`. Fold
    /// both to `{}` so the comparison is about *structure*, not naming — a
    /// route renamed from `:id` to `:job_id` is not a drift.
    fn normalise(path: &str) -> String {
        path.split('/')
            .map(|seg| {
                if seg.starts_with(':') || (seg.starts_with('{') && seg.ends_with('}')) {
                    "{}"
                } else if seg.starts_with('*') {
                    "*"
                } else {
                    seg
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    fn sources() -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read src/").flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        files
            .into_iter()
            .map(|f| {
                let src = std::fs::read_to_string(&f).expect("read source file");
                (f, src)
            })
            .collect()
    }

    /// One method on one route, as the router registers it.
    #[derive(Debug, Clone)]
    struct Routed {
        file: std::path::PathBuf,
        path: String,
        method: String,
        /// The handler as written — `list_sessions`, or `ingest::submit_listens`.
        handler: String,
    }

    /// Skip a Rust string literal starting at `s[0] == '"'`; returns the index
    /// just past its closing quote.
    fn skip_string(s: &[u8]) -> usize {
        let mut i = 1;
        while i < s.len() {
            match s[i] {
                b'\\' => i += 2,
                b'"' => return i + 1,
                _ => i += 1,
            }
        }
        s.len()
    }

    /// The text between `.route(` and its matching `)`, with nested calls,
    /// string literals and line comments accounted for.
    fn call_body(s: &str) -> &str {
        let b = s.as_bytes();
        let (mut depth, mut i) = (1, 0);
        while i < b.len() {
            match b[i] {
                b'"' => {
                    i += skip_string(&b[i..]);
                    continue;
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &s[..i];
                    }
                }
                _ => {}
            }
            i += 1;
        }
        s
    }

    /// Every `.route("…", method(handler)…)` under `src/`, one entry per method.
    fn router_routes() -> Vec<Routed> {
        const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];
        let mut out = Vec::new();
        for (file, src) in sources() {
            let mut rest = src.as_str();
            while let Some(i) = rest.find(".route(") {
                rest = &rest[i + ".route(".len()..];
                // The literal is the next quoted string, allowing for
                // whitespace and newlines between `(` and it.
                let Some(open) = rest.find('"') else { break };
                if rest[..open].contains(')') {
                    continue;
                }
                let after = &rest[open + 1..];
                let Some(close) = after.find('"') else { break };
                let literal = &after[..close];
                // Every axum route starts with a slash. The filter matters
                // because this scanner also reads *its own* source, where
                // `.route(` appears inside string literals in these tests.
                if !literal.starts_with('/') {
                    continue;
                }
                let body = call_body(&after[close + 1..]);
                // Each `get(handler)`, `.post(handler)`, `axum::routing::put(handler)`.
                let bytes = body.as_bytes();
                for method in METHODS {
                    let pat = format!("{method}(");
                    let mut from = 0;
                    while let Some(j) = body[from..].find(&pat) {
                        let at = from + j;
                        from = at + pat.len();
                        let before = at.checked_sub(1).map(|k| bytes[k]);
                        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
                            continue; // `forget(`, `set_put(` and the like
                        }
                        let handler: String = body[from..]
                            .trim_start()
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                            .collect();
                        if !handler.is_empty() {
                            out.push(Routed {
                                file: file.clone(),
                                path: literal.to_string(),
                                method: method.to_string(),
                                handler,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    fn method_name(m: &HttpMethod) -> &'static str {
        match m {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
            HttpMethod::Head => "head",
            HttpMethod::Options => "options",
            HttpMethod::Trace => "trace",
        }
    }

    /// Every operation in the generated document, keyed by (shape, method).
    fn documented(doc: &OpenApi) -> BTreeMap<(String, String), (String, Operation)> {
        let mut out = BTreeMap::new();
        for (path, item) in &doc.paths.paths {
            let ops = [
                (HttpMethod::Get, &item.get),
                (HttpMethod::Post, &item.post),
                (HttpMethod::Put, &item.put),
                (HttpMethod::Patch, &item.patch),
                (HttpMethod::Delete, &item.delete),
                (HttpMethod::Head, &item.head),
                (HttpMethod::Options, &item.options),
                (HttpMethod::Trace, &item.trace),
            ];
            for (m, op) in ops {
                if let Some(op) = op {
                    out.insert(
                        (normalise(path), method_name(&m).to_string()),
                        (path.clone(), op.clone()),
                    );
                }
            }
        }
        out
    }

    /// The document and the router must describe the same surface, in both
    /// directions, **per method**.
    ///
    /// Undocumented endpoints are the obvious failure. The reverse — a spec
    /// operation that no longer exists — is the one that actually bites: it
    /// sends a client author to an endpoint that 404s or 405s, and nothing
    /// else in the build would ever notice. Per method, because comparing
    /// paths alone missed `GET /api/v1/patch/deck/:username` for as long as
    /// its `DELETE` was documented.
    #[test]
    fn the_spec_and_the_router_describe_the_same_api() {
        let routed: BTreeSet<_> =
            router_routes().into_iter().map(|r| (normalise(&r.path), r.method)).collect();
        let documented: BTreeSet<_> = documented(&super::document()).into_keys().collect();

        assert!(
            routed.len() > 250,
            "route scan found only {} operations — the scanner is broken, not the spec",
            routed.len()
        );

        let undocumented: Vec<_> = routed.difference(&documented).collect();
        let stale: Vec<_> = documented.difference(&routed).collect();

        assert!(
            undocumented.is_empty() && stale.is_empty(),
            "the OpenAPI document has drifted from the router.\n\
             \n  Routed but not documented ({}): {:#?}\
             \n  Documented but not routed ({}): {:#?}\n\
             \nA new handler needs a #[utoipa::path] and an entry in its module's \
             `Api` paths(...) list.",
            undocumented.len(),
            undocumented,
            stale.len(),
            stale,
        );
    }

    /// The operation documented for a route is the function the route calls.
    ///
    /// utoipa names an operation after the function its `#[utoipa::path]` sits
    /// on, so this catches an annotation that describes one handler while the
    /// router sends the request to another — the path and method would match,
    /// and the body schemas would be somebody else's. Where two modules have a
    /// handler of the same name, the operation id is `<module>_<fn>`; that is
    /// the only renaming allowed.
    #[test]
    fn every_documented_operation_names_the_handler_that_serves_it() {
        let docs = documented(&super::document());
        let mut wrong = Vec::new();
        for r in router_routes() {
            let Some((_, op)) = docs.get(&(normalise(&r.path), r.method.clone())) else {
                continue; // the drift test reports these
            };
            let handler = r.handler.rsplit("::").next().unwrap_or_default();
            let module = match r.handler.rsplit("::").nth(1) {
                Some(m) => m.to_string(),
                None => r.file.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            };
            let id = op.operation_id.clone().unwrap_or_default();
            if id != handler && id != format!("{module}_{handler}") {
                wrong.push(format!(
                    "{} {}: routed to {handler}, documented as {id}",
                    r.method, r.path
                ));
            }
        }
        assert!(wrong.is_empty(), "annotations on the wrong handler:\n  {}", wrong.join("\n  "));
    }

    /// Duplicate operation ids make the document invalid, and a client
    /// generator names its methods after them.
    #[test]
    fn operation_ids_are_unique() {
        let mut seen = BTreeMap::new();
        for ((path, method), (_, op)) in documented(&super::document()) {
            let id = op.operation_id.unwrap_or_default();
            if let Some(prev) = seen.insert(id.clone(), format!("{method} {path}")) {
                panic!("operation id `{id}` is used by both {prev} and {method} {path}");
            }
        }
    }

    /// The scanner is the load-bearing half of the tests above; if it silently
    /// stopped matching, the drift check would pass by finding nothing.
    #[test]
    fn the_route_scanner_actually_finds_known_routes() {
        let routed: BTreeSet<_> = router_routes()
            .into_iter()
            .map(|r| (normalise(&r.path), r.method, r.handler))
            .collect();
        for (path, method, handler) in [
            ("/health", "get", "health_check"),
            ("/1/submit-listens", "post", "ingest::submit_listens"),
            ("/api/v1/scrobbles", "get", "list_scrobbles"),
            ("/api/v1/scrobbles/{}", "delete", "delete_scrobble"),
            ("/admin/tokens/{}/rotate", "post", "rotate_token"),
            ("/api/v1/sessions/{}", "patch", "update_session"),
        ] {
            assert!(
                routed.contains(&(path.into(), method.into(), handler.into())),
                "scanner missed {method} {path} → {handler}"
            );
        }
    }

    #[test]
    fn path_parameters_compare_by_shape_not_by_name() {
        assert_eq!(normalise("/api/v1/scrobbles/:id"), normalise("/api/v1/scrobbles/{id}"));
        assert_eq!(normalise("/api/v1/import/jobs/:job_id"), normalise("/api/v1/import/jobs/{id}"));
        assert_ne!(normalise("/api/v1/loves"), normalise("/api/v1/loves/keys"));
    }

    /// The signature of `async fn <name>` in `src`, between its parentheses.
    fn signature<'a>(src: &'a str, name: &str) -> Option<&'a str> {
        let pat = format!("async fn {name}(");
        let at = src.find(&pat)? + pat.len();
        Some(call_body(&src[at..]))
    }

    /// Which credentials the handler's extractor accepts, in the document's
    /// terms, or `None` when it takes no auth extractor at all.
    fn extracted_security(sig: &str) -> Option<&'static str> {
        // Checked most specific first: `WriteAuth` contains no `AuthUser`, but
        // a destructured `ReadAuth { user }` does mention `user`.
        if sig.contains("IngestAuth") {
            Some("apiToken | sessionCookie")
        } else if sig.contains("WriteAuth") {
            Some("apiToken[write] | sessionCookie")
        } else if sig.contains("ReadAuth") {
            Some("apiToken[read] | sessionCookie")
        } else if sig.contains("AuthUser") {
            Some("sessionCookie")
        } else {
            None
        }
    }

    fn documented_security(
        security: &Option<Vec<utoipa::openapi::security::SecurityRequirement>>,
        global: &str,
    ) -> String {
        match security {
            None => global.to_string(),
            Some(reqs) => {
                let v = serde_json::to_value(reqs).unwrap_or_default();
                let mut alts: Vec<String> = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|req| req.as_object())
                    .flat_map(|o| {
                        o.iter().map(|(scheme, scopes)| {
                            let scopes: Vec<_> = scopes
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|s| s.as_str())
                                .collect();
                            if scopes.is_empty() {
                                scheme.clone()
                            } else {
                                format!("{scheme}[{}]", scopes.join(","))
                            }
                        })
                    })
                    .collect();
                alts.sort();
                if alts.is_empty() {
                    "none".into()
                } else {
                    alts.join(" | ")
                }
            }
        }
    }

    /// Handlers that authenticate by hand rather than through an extractor,
    /// and what the document says for them. Anything else with no extractor
    /// must be documented as open (`security: []`).
    const MANUAL_AUTH: &[(&str, &str)] = &[
        // An admin session or the metrics bearer, checked in the handler.
        ("metrics", "metricsBearer | sessionCookie"),
        // The MCP credential store, not `tokens`.
        ("endpoint", "mcpBearer"),
        // Reads whatever token it is given; an invalid one is `valid: false`.
        ("validate_token", "apiToken"),
        // Clears whatever cookie arrived; with none it is a no-op.
        ("logout", "sessionCookie"),
    ];

    /// The credential the document names for an operation is the one its
    /// handler's extractor actually accepts.
    ///
    /// `security` is the field a client author reads to decide whether a token
    /// will do, and it was hand-written: `/api/v1/lyrics/library`,
    /// `/api/v1/lyrics/matches`, `/api/v1/art/library` and
    /// `/api/v1/chains/variants` all accept a scoped token and were documented
    /// as session-only, while `/api/v1/tidal/callback` is reached with no
    /// credential at all. The extractor is the truth, so it is read from the
    /// signature.
    #[test]
    fn the_documented_security_is_what_the_handler_extracts() {
        let doc = super::document();
        let global = documented_security(&doc.security, "none");
        let docs = documented(&doc);
        let srcs: BTreeMap<_, _> = sources().into_iter().collect();
        let server = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server");

        let mut wrong = Vec::new();
        for r in router_routes() {
            let Some((_, op)) = docs.get(&(normalise(&r.path), r.method.clone())) else {
                continue;
            };
            let mut parts = r.handler.rsplit("::");
            let handler = parts.next().unwrap_or_default();
            let file = match parts.next() {
                Some("openapi") => {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/openapi.rs")
                }
                Some(module) => server.join(format!("{module}.rs")),
                None => r.file.clone(),
            };
            let Some(sig) = srcs.get(&file).and_then(|s| signature(s, handler)) else {
                wrong.push(format!("{} {}: cannot find `async fn {handler}`", r.method, r.path));
                continue;
            };
            let want = match extracted_security(sig) {
                Some(s) => s.to_string(),
                None => MANUAL_AUTH
                    .iter()
                    .find(|(h, _)| *h == handler)
                    .map(|(_, s)| s.to_string())
                    .unwrap_or_else(|| "none".into()),
            };
            let got = documented_security(&op.security, &global);
            if got != want {
                wrong.push(format!(
                    "{} {} ({handler}): handler accepts `{want}`, document says `{got}`",
                    r.method, r.path
                ));
            }
        }
        assert!(wrong.is_empty(), "security drift:\n  {}", wrong.join("\n  "));
    }

    /// Every `{name}` in a path is declared as a path parameter of that name.
    ///
    /// The hand-written document had upload routes whose `{id}` was declared
    /// nowhere, which a generator either refuses or turns into a method that
    /// cannot be called. utoipa takes the path as a string and checks nothing.
    #[test]
    fn every_path_parameter_is_declared() {
        let mut missing = Vec::new();
        for ((_, method), (path, op)) in documented(&super::document()) {
            let declared: BTreeSet<String> = op
                .parameters
                .iter()
                .flatten()
                .filter(|p| matches!(p.parameter_in, utoipa::openapi::path::ParameterIn::Path))
                .map(|p| p.name.clone())
                .collect();
            let wanted: BTreeSet<String> = path
                .split('/')
                .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
                .map(str::to_string)
                .collect();
            if declared != wanted {
                missing
                    .push(format!("{method} {path}: path has {wanted:?}, declared {declared:?}"));
            }
        }
        assert!(missing.is_empty(), "path parameters out of step:\n  {}", missing.join("\n  "));
    }

    /// The name every `ToSchema` type under `src/` publishes as — its
    /// `#[schema(as = …)]` when it has one, its identifier otherwise — with
    /// where it is defined.
    ///
    /// Textual, like the route scanner, and for a reason the document cannot
    /// supply: two types of one name **in one module's `Api`** are collapsed by
    /// utoipa's derive before `build()` ever sees two of them, so the document
    /// alone cannot tell that one shape was dropped. Only the source can.
    fn published_schema_names() -> Vec<(String, String)> {
        fn item_name(line: &str) -> Option<&str> {
            let mut words = line.split_whitespace();
            let mut w = words.next()?;
            if w.starts_with("pub") {
                w = words.next()?;
            }
            if w != "struct" && w != "enum" {
                return None;
            }
            let name = words.next()?;
            Some(name.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or(name))
        }
        let mut out = Vec::new();
        for (file, src) in sources() {
            let lines: Vec<&str> = src.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let t = line.trim();
                if !(t.starts_with("#[derive(") && t.contains("ToSchema")) {
                    continue;
                }
                // The attributes and comments around the derive, down to the item.
                let mut start = i;
                while start > 0 && {
                    let above = lines[start - 1].trim();
                    above.starts_with("#[") || above.starts_with("//")
                } {
                    start -= 1;
                }
                let Some(end) = (i + 1..lines.len()).find(|&k| item_name(lines[k]).is_some())
                else {
                    continue;
                };
                let ident = item_name(lines[end]).unwrap_or_default().to_string();
                let renamed = lines[start..end].iter().find_map(|l| {
                    let l = l.trim();
                    let rest = l.strip_prefix("#[schema(")?;
                    let at = rest.find("as =").or_else(|| rest.find("as="))?;
                    let value = rest[at..].split('=').nth(1)?.trim();
                    let path: String = value
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                        .collect();
                    path.rsplit("::").next().map(str::to_string)
                });
                let name = renamed.unwrap_or_else(|| ident.clone());
                let at = format!("{}:{} ({ident})", file.display(), end + 1);
                out.push((name, at));
            }
        }
        out
    }

    /// Two different types published under one schema name would have one of
    /// them silently replaced by the other — utoipa keeps the first.
    ///
    /// Checked twice, because the two halves fail differently. Across modules,
    /// `build()` sees both shapes and reports the name. Within one module the
    /// derive has already collapsed them, so the source is scanned for every
    /// `ToSchema` type and the name each publishes.
    #[test]
    fn no_two_types_publish_one_schema_name() {
        let (_, collisions) = super::build();
        assert!(
            collisions.is_empty(),
            "schema names claimed by two different shapes: {collisions:?} — rename one with \
             #[schema(as = …)]"
        );

        let named = published_schema_names();
        assert!(
            named.len() > 400,
            "only {} ToSchema types found — the scan is broken",
            named.len()
        );
        let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (name, at) in &named {
            by_name.entry(name.as_str()).or_default().push(at.as_str());
        }
        let clashes: Vec<String> = by_name
            .into_iter()
            .filter(|(_, at)| at.len() > 1)
            .map(|(name, at)| format!("{name}: {}", at.join(", ")))
            .collect();
        assert!(
            clashes.is_empty(),
            "one schema name, several types — rename all but one with #[schema(as = …)]:\n  {}",
            clashes.join("\n  ")
        );
    }

    /// Every `$ref` points at something the document defines.
    ///
    /// A `#[schema(value_type = …)]` or `as = …` can name a component that
    /// nothing registers, and the document then serialises fine and fails in
    /// every client generator at the first dereference.
    #[test]
    fn every_reference_resolves() {
        let doc = serde_json::to_value(super::document()).expect("document serialises");
        fn refs(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(o) => {
                    if let Some(serde_json::Value::String(r)) = o.get("$ref") {
                        out.push(r.clone());
                    }
                    o.values().for_each(|v| refs(v, out));
                }
                serde_json::Value::Array(a) => a.iter().for_each(|v| refs(v, out)),
                _ => {}
            }
        }
        let mut all = Vec::new();
        refs(&doc, &mut all);
        assert!(!all.is_empty(), "no $refs at all — the walk is broken");
        let dangling: BTreeSet<_> = all
            .into_iter()
            .filter(|r| {
                let Some(pointer) = r.strip_prefix('#') else { return true };
                doc.pointer(pointer).is_none()
            })
            .collect();
        assert!(dangling.is_empty(), "references to nothing: {dangling:#?}");
    }

    /// Every operation has a summary and exactly one declared tag, so the
    /// reference renders as a grouped list rather than an "untagged" heap.
    #[test]
    fn every_operation_is_tagged_with_a_declared_tag() {
        let declared: BTreeSet<_> = super::TAGS.iter().map(|(n, _)| n.to_string()).collect();
        let mut bad = Vec::new();
        for ((path, method), (_, op)) in documented(&super::document()) {
            let tags = op.tags.clone().unwrap_or_default();
            if tags.len() != 1 || !declared.contains(&tags[0]) {
                bad.push(format!("{method} {path}: tags {tags:?}"));
            }
            if op.summary.as_deref().unwrap_or("").trim().is_empty() {
                bad.push(format!("{method} {path}: no summary"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    fn committed() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.yaml");
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// `openapi.yaml` at the repo root is exactly what the binary serves.
    ///
    /// It is a snapshot — the website's spec-sync job fetches it by tag, and a
    /// reviewer sees an API change as a diff — so a stale one publishes a
    /// reference for some other version. Regenerate it with
    /// `TAPEDECK_BLESS_OPENAPI=1 cargo test openapi`. This also covers what
    /// `the_spec_states_the_version_the_binary_actually_is` used to: bumping
    /// `Cargo.toml` changes `info.version`, so the snapshot has to move too.
    #[test]
    fn the_committed_spec_is_the_generated_one() {
        let generated = super::yaml();
        if std::env::var("TAPEDECK_BLESS_OPENAPI").is_ok_and(|v| v != "0") {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.yaml");
            std::fs::write(path, generated).expect("write openapi.yaml");
            return;
        }
        let committed = committed();
        if committed != generated {
            let line = committed
                .lines()
                .zip(generated.lines())
                .position(|(a, b)| a != b)
                .map(|i| i + 1)
                .unwrap_or_else(|| committed.lines().count().min(generated.lines().count()) + 1);
            panic!(
                "openapi.yaml is stale (first difference at line {line}).\n\
                 Regenerate it: TAPEDECK_BLESS_OPENAPI=1 cargo test openapi"
            );
        }
    }

    /// The committed document has to *parse*, and for a long time it did not.
    ///
    /// Found 2026-09-13: two over-indented keys made the hand-written document
    /// unloadable by any YAML parser while reading it by eye worked perfectly.
    /// A generated document cannot be mis-indented, but the check is cheap, it
    /// reads with a second, independent parser, and it is what stands between
    /// a truncated or hand-edited snapshot and the website.
    #[test]
    fn the_spec_is_a_document_a_client_can_actually_load() {
        let text = committed();
        let docs = yaml_rust2::YamlLoader::load_from_str(&text)
            .unwrap_or_else(|e| panic!("openapi.yaml does not parse as YAML: {e}"));
        let doc = docs.first().expect("openapi.yaml is empty");
        let paths = doc["paths"].as_hash().expect("openapi.yaml has no paths mapping");
        assert!(paths.len() > 200, "only {} paths parsed — a block has moved", paths.len());
        assert_eq!(
            doc["info"]["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION")),
            "openapi.yaml states another version than Cargo.toml"
        );
    }
}
