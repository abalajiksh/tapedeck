//! JSON-RPC envelope, era detection and header validation.
//!
//! Everything in here is about the shape of a message rather than what it
//! means. `dispatch` decides what to do; this decides whether the request is
//! well-formed, which revision it is speaking, and — for a modern request —
//! whether the headers an intermediary might have routed on actually agree with
//! the body the server is about to act on.

use serde_json::{json, Value};

use super::{LATEST_VERSION, SUPPORTED_VERSIONS};

// ── JSON-RPC error codes ──
//
// The first five are JSON-RPC's own. The last two are allocated by the MCP
// specification from its reserved sub-range, and both are load-bearing for
// interoperability: a client distinguishes a modern server from a legacy one by
// whether a 400 carries one of them.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
// JSON-RPC's own `-32603` (internal error) is deliberately absent: a failure
// inside a tool surfaces as `isError: true` in the *result*, which is what a
// model can act on, and a failure before parsing is a plain HTTP 500.
pub const HEADER_MISMATCH: i64 = -32020;
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// Which revision family a request is speaking.
///
/// This is not decoration. It changes three things: whether header validation
/// applies, whether the result carries `resultType`, and whether `initialize`
/// is a method that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    /// `2026-07-28` and later: stateless, per-request `_meta`, mirrored headers.
    Modern,
    /// `2025-11-25` and earlier: `initialize` handshake, no per-request meta.
    Legacy,
}

impl Era {
    /// ISO dates compare correctly as strings, which is the whole reason the
    /// revisions are named this way — no version table to keep in step.
    pub fn of(version: &str) -> Era {
        if version >= LATEST_VERSION {
            Era::Modern
        } else {
            Era::Legacy
        }
    }
}

/// One parsed request off the wire.
#[derive(Debug, Clone)]
pub struct RpcRequest {
    /// Absent for a notification, which takes `202` and no body.
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
    pub version: String,
    pub era: Era,
}

impl RpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// `params.arguments` for a tool call, or an empty object.
    pub fn arguments(&self) -> Value {
        self.params.get("arguments").cloned().unwrap_or_else(|| json!({}))
    }

    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(Value::as_str)
    }
}

/// A failure that has to become an HTTP response rather than a JSON-RPC result.
#[derive(Debug)]
pub struct RpcError {
    pub http: u16,
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(http: u16, code: i64, message: impl Into<String>) -> Self {
        RpcError { http, code, message: message.into(), data: None }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn body(&self, id: Option<Value>) -> Value {
        let mut err = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            err["data"] = data.clone();
        }
        json!({ "jsonrpc": "2.0", "id": id, "error": err })
    }
}

/// Build a successful JSON-RPC response.
///
/// `resultType: "complete"` is a **modern-only** field. Adding it to a legacy
/// result is not merely redundant — a strict legacy client validating against
/// its own schema can reject the extra key, and the failure would look like the
/// tool returning nothing.
pub fn ok(id: Option<Value>, era: Era, mut result: Value) -> Value {
    if era == Era::Modern {
        if let Some(obj) = result.as_object_mut() {
            obj.entry("resultType").or_insert_with(|| json!("complete"));
        }
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Resolve which protocol version a request is speaking.
///
/// Three sources in priority order, and the fallback is the interesting one:
/// a request with neither `_meta` nor a version header is assumed to be
/// `2025-03-26`, which is what the specification permits for clients predating
/// the header. Rejecting those instead would break every 2025-03-26 client for
/// the sake of strictness about a revision they could not have known about.
fn resolve_version(method: &str, params: &Value, header: Option<&str>) -> String {
    if let Some(v) =
        params.pointer("/_meta/io.modelcontextprotocol~1protocolVersion").and_then(Value::as_str)
    {
        return v.to_string();
    }
    // `initialize` states its version in the body, not in `_meta`.
    if method == "initialize" {
        if let Some(v) = params.get("protocolVersion").and_then(Value::as_str) {
            return v.to_string();
        }
    }
    if let Some(h) = header.map(str::trim).filter(|h| !h.is_empty()) {
        return h.to_string();
    }
    "2025-03-26".to_string()
}

/// Decode the `=?base64?…?=` sentinel the spec defines for header values that
/// cannot be written as plain ASCII — a track title with an accent in it, which
/// on this server's tools is the common case rather than an edge one.
pub fn decode_header_value(raw: &str) -> Option<String> {
    use base64::Engine;
    let Some(inner) = raw.strip_prefix("=?base64?").and_then(|r| r.strip_suffix("?=")) else {
        return Some(raw.to_string());
    };
    base64::engine::general_purpose::STANDARD
        .decode(inner)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}

/// Parse and validate one request body against its headers.
///
/// `headers` is a lookup closure rather than a `HeaderMap` so this stays
/// testable without constructing HTTP types.
pub fn parse(body: &[u8], header: impl Fn(&str) -> Option<String>) -> Result<RpcRequest, RpcError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| RpcError::new(400, PARSE_ERROR, format!("Invalid JSON: {e}")))?;

    // A batch is valid JSON-RPC and is not valid MCP over Streamable HTTP: the
    // body must be a single request or notification.
    if value.is_array() {
        return Err(RpcError::new(
            400,
            INVALID_REQUEST,
            "Batched requests are not supported; send one JSON-RPC message per POST",
        ));
    }

    let Some(method) = value.get("method").and_then(Value::as_str).map(str::to_string) else {
        return Err(RpcError::new(400, INVALID_REQUEST, "Missing 'method'"));
    };
    let id = value.get("id").filter(|v| !v.is_null()).cloned();
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));

    let version_header = header("mcp-protocol-version");
    let version = resolve_version(&method, &params, version_header.as_deref());
    let era = Era::of(&version);

    if !SUPPORTED_VERSIONS.contains(&version.as_str()) {
        return Err(RpcError::new(
            400,
            UNSUPPORTED_PROTOCOL_VERSION,
            "Unsupported protocol version",
        )
        .with_data(json!({ "supported": SUPPORTED_VERSIONS, "requested": version })));
    }

    if era == Era::Modern {
        validate_headers(&method, &params, &version, version_header.as_deref(), &header)?;
    }

    Ok(RpcRequest { id, method, params, version, era })
}

/// The modern-only header/body agreement check.
///
/// The point is not tidiness. These headers exist so an intermediary can route
/// or rate-limit without parsing the body; if the two can disagree then the
/// thing that decided and the thing that acts were looking at different
/// requests. Hence `-32020` rather than quietly preferring one.
fn validate_headers(
    method: &str,
    params: &Value,
    version: &str,
    version_header: Option<&str>,
    header: &impl Fn(&str) -> Option<String>,
) -> Result<(), RpcError> {
    let mismatch = |msg: String| RpcError::new(400, HEADER_MISMATCH, msg);

    match version_header {
        None => {
            return Err(mismatch("Missing required header: MCP-Protocol-Version".into()));
        }
        Some(h) if h.trim() != version => {
            return Err(mismatch(format!(
                "Header mismatch: MCP-Protocol-Version '{}' does not match body value '{}'",
                h.trim(),
                version
            )));
        }
        _ => {}
    }

    match header("mcp-method") {
        None => return Err(mismatch("Missing required header: Mcp-Method".into())),
        Some(h) if h != method => return Err(mismatch(format!(
            "Header mismatch: Mcp-Method header value '{h}' does not match body value '{method}'"
        ))),
        _ => {}
    }

    // `Mcp-Name` is required only where the spec names a source field for it.
    let expected = match method {
        "tools/call" | "prompts/get" => params.get("name").and_then(Value::as_str),
        "resources/read" => params.get("uri").and_then(Value::as_str),
        _ => None,
    };
    if let Some(expected) = expected {
        match header("mcp-name") {
            None => return Err(mismatch("Missing required header: Mcp-Name".into())),
            Some(raw) => {
                let decoded = decode_header_value(&raw).ok_or_else(|| {
                    mismatch("Mcp-Name header is not valid base64 sentinel encoding".into())
                })?;
                if decoded != expected {
                    return Err(mismatch(format!(
                        "Header mismatch: Mcp-Name header value '{decoded}' does not match body value '{expected}'"
                    )));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The headers a well-behaved modern client sends.
    fn modern_headers<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.to_string())
        }
    }

    fn body(v: Value) -> Vec<u8> {
        serde_json::to_vec(&v).unwrap()
    }

    #[test]
    fn a_modern_request_is_recognised_from_its_meta() {
        let req = parse(
            &body(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } }
            })),
            modern_headers(&[("mcp-protocol-version", "2026-07-28"), ("mcp-method", "tools/list")]),
        )
        .unwrap();
        assert_eq!(req.era, Era::Modern);
        assert_eq!(req.version, "2026-07-28");
    }

    #[test]
    fn a_legacy_initialize_is_served_without_header_validation() {
        // The whole point of being dual-era: this client sends no MCP headers
        // at all and must still work.
        let req = parse(
            &body(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {} }
            })),
            modern_headers(&[]),
        )
        .unwrap();
        assert_eq!(req.era, Era::Legacy);
        assert_eq!(req.version, "2025-06-18");
    }

    #[test]
    fn a_request_with_no_version_at_all_is_assumed_to_be_the_oldest() {
        let req = parse(
            &body(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
            modern_headers(&[]),
        )
        .unwrap();
        assert_eq!(req.version, "2025-03-26");
        assert_eq!(req.era, Era::Legacy);
    }

    #[test]
    fn a_modern_request_whose_headers_disagree_with_its_body_is_refused() {
        // The failure this exists to stop: a gateway routes on the header while
        // the server acts on the body.
        let err = parse(
            &body(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {
                    "name": "search_listens",
                    "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
                }
            })),
            modern_headers(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "listening_profile"),
            ]),
        )
        .unwrap_err();
        assert_eq!(err.code, HEADER_MISMATCH);
        assert_eq!(err.http, 400);
    }

    #[test]
    fn a_modern_request_missing_the_mirrored_headers_is_refused() {
        let err = parse(
            &body(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } }
            })),
            modern_headers(&[("mcp-protocol-version", "2026-07-28")]),
        )
        .unwrap_err();
        assert_eq!(err.code, HEADER_MISMATCH);
    }

    #[test]
    fn a_base64_encoded_name_header_matches_its_body_value() {
        // An accented artist is the common case here, not an edge one — the
        // history this was built against has 2,958 plays of Cœur de pirate.
        let err = parse(
            &body(json!({
                "jsonrpc": "2.0", "id": 1, "method": "resources/read",
                "params": {
                    "uri": "tapedeck://artist/Cœur de pirate",
                    "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
                }
            })),
            modern_headers(&[
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "resources/read"),
                ("mcp-name", "=?base64?dGFwZWRlY2s6Ly9hcnRpc3QvQ8WTdXIgZGUgcGlyYXRl?="),
            ]),
        );
        assert!(err.is_ok(), "sentinel-encoded header should match: {:?}", err.err());
    }

    #[test]
    fn an_unknown_version_names_the_ones_that_work() {
        // A client that gets this back retries with a supported version, which
        // it can only do if the list travels with the error.
        let err = parse(
            &body(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "1900-01-01" } }
            })),
            modern_headers(&[]),
        )
        .unwrap_err();
        assert_eq!(err.code, UNSUPPORTED_PROTOCOL_VERSION);
        let data = err.data.unwrap();
        assert_eq!(data["supported"][0], LATEST_VERSION);
        assert_eq!(data["requested"], "1900-01-01");
    }

    #[test]
    fn a_batch_is_refused_rather_than_half_executed() {
        let err =
            parse(&body(json!([{ "jsonrpc": "2.0", "method": "ping" }])), modern_headers(&[]))
                .unwrap_err();
        assert_eq!(err.code, INVALID_REQUEST);
    }

    #[test]
    fn only_a_modern_result_carries_result_type() {
        // A strict legacy client validating its own schema can reject the extra
        // key, and that failure looks like the tool returning nothing.
        let modern = ok(Some(json!(1)), Era::Modern, json!({ "tools": [] }));
        assert_eq!(modern["result"]["resultType"], "complete");
        let legacy = ok(Some(json!(1)), Era::Legacy, json!({ "tools": [] }));
        assert!(legacy["result"].get("resultType").is_none());
    }

    #[test]
    fn a_notification_is_recognised_by_having_no_id() {
        let req = parse(
            &body(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
            modern_headers(&[]),
        )
        .unwrap();
        assert!(req.is_notification());
        // An explicit null id is a notification too, not a request with a null
        // id — serde would otherwise hand back `Some(Value::Null)` and the
        // response would carry an id the client never sent.
        let req = parse(
            &body(json!({ "jsonrpc": "2.0", "id": null, "method": "ping" })),
            modern_headers(&[]),
        )
        .unwrap();
        assert!(req.is_notification());
    }
}
