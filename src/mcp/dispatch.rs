//! Method dispatch — what each JSON-RPC method does, on either era.
//!
//! The method set is identical across eras. Only three things differ, and they
//! are all handled here or in `protocol`: `initialize` exists only for legacy
//! clients, `server/discover` only for modern ones, and a modern result carries
//! `resultType`.

use std::sync::Arc;

use serde_json::{json, Value};

use super::protocol::{self, CacheScope, RpcError, RpcRequest};
use super::tools::ToolFailure;
use super::{prompts, resources, tools};
use crate::db::McpAuth;
use crate::server::AppState;

/// What the HTTP layer should do with a dispatched request.
pub enum Outcome {
    /// A JSON-RPC response body, ready to send with `200`.
    Reply(Value),
    /// A notification was accepted: `202`, no body.
    Accepted,
    /// Something the transport has to express — a status code and a JSON-RPC
    /// error body together.
    Failed(RpcError),
}

/// One line for the audit log: what ran, and how much came back.
pub struct Trace {
    pub tool: Option<String>,
    pub args: Option<String>,
    pub ok: bool,
    pub rows: Option<i64>,
}

impl Trace {
    fn plain(ok: bool) -> Self {
        Trace { tool: None, args: None, ok, rows: None }
    }

    /// A successful enumeration, recording **how many things it listed**.
    ///
    /// `tools/list` used to record nothing, and that turned out to be the one
    /// gap that mattered: when a real connector reported having no tools, the
    /// audit log could say the call had happened and succeeded but not whether
    /// the answer was twenty-three tools or an empty array. Those are opposite
    /// diagnoses — one is the client's problem and the other is this server's —
    /// and telling them apart needed a reconnect and a code change.
    ///
    /// The row count was already described as the interesting column. It was
    /// only being filled in for tool calls.
    fn listed(n: usize) -> Self {
        Trace { tool: None, args: None, ok: true, rows: Some(n as i64) }
    }
}

/// How long the compiled-in sets may be treated as fresh. They change only when
/// the binary does — but a *grant* can be revoked at any time, and the lists are
/// filtered by grant, so this is minutes rather than hours.
const LIST_TTL_MS: u64 = 300_000;

/// Discovery carries nothing user-specific and changes only on deploy.
const DISCOVER_TTL_MS: u64 = 3_600_000;

pub async fn handle(state: &Arc<AppState>, auth: &McpAuth, req: &RpcRequest) -> (Outcome, Trace) {
    let id = req.id.clone();
    let era = req.era;
    let reply = |v: Value| Outcome::Reply(protocol::ok(id.clone(), era, v));
    // The three list methods vary by the caller's grants, so their results must
    // never be served to anyone else — see `protocol::with_cache`.
    let listing = |v: Value| {
        Outcome::Reply(protocol::ok(
            id.clone(),
            era,
            protocol::with_cache(v, era, LIST_TTL_MS, CacheScope::Private),
        ))
    };

    match req.method.as_str() {
        // ── Lifecycle ──
        //
        // `initialize` is the legacy handshake. No session id is minted: this
        // server is stateless on both eras, and the field is optional in every
        // revision that has it. A legacy client that gets no `Mcp-Session-Id`
        // simply never sends one back.
        "initialize" => (
            reply(json!({
                "protocolVersion": req.version,
                "capabilities": capabilities(),
                "serverInfo": server_info(),
                "instructions": super::instructions(),
            })),
            Trace::plain(true),
        ),

        // Modern discovery. `supportedVersions` is what lets a client that
        // guessed wrong retry with something this server actually speaks.
        "server/discover" => (
            Outcome::Reply(protocol::ok(
                id.clone(),
                era,
                protocol::with_cache(
                    json!({
                        "supportedVersions": super::SUPPORTED_VERSIONS,
                        "capabilities": capabilities(),
                        "instructions": super::instructions(),
                        "_meta": { "io.modelcontextprotocol/serverInfo": server_info() },
                    }),
                    era,
                    DISCOVER_TTL_MS,
                    CacheScope::Public,
                ),
            )),
            Trace::plain(true),
        ),

        "ping" => (reply(json!({})), Trace::plain(true)),

        // Every notification is accepted and none is acted on. `initialized`
        // and `cancelled` are the only ones the core protocol defines from a
        // client, and neither means anything to a stateless server — but a
        // client that gets an error back for one will report the connection as
        // broken.
        //
        // Keyed on the **absent id**, which is what makes something a
        // notification in JSON-RPC, rather than on the method name: a client
        // that sends any method with no id is not waiting for an answer, and
        // sending it one anyway is a response to a request it never made.
        _ if req.is_notification() => (Outcome::Accepted, Trace::plain(true)),

        // Accepted and ignored: log level is a property of this server's own
        // configuration, not something a connected chatbot gets to set.
        "logging/setLevel" => (reply(json!({})), Trace::plain(true)),

        // ── Tools ──
        "tools/list" => {
            let list: Vec<Value> = tools::visible(auth).iter().map(|t| t.to_json()).collect();
            let n = list.len();
            (listing(json!({ "tools": list })), Trace::listed(n))
        }

        "tools/call" => {
            let Some(name) = req.param_str("name") else {
                return (
                    Outcome::Failed(RpcError::new(
                        400,
                        protocol::INVALID_PARAMS,
                        "Missing tool name",
                    )),
                    Trace::plain(false),
                );
            };
            let args = req.arguments();
            let args_summary = Some(args.to_string());

            match tools::call(state, auth, name, &args).await {
                Ok(outcome) => {
                    let rows = outcome.rows;
                    (
                        reply(tool_result(&outcome.value, false)),
                        Trace { tool: Some(name.to_string()), args: args_summary, ok: true, rows },
                    )
                }
                // Unknown tool is a protocol error: the model cannot fix it by
                // rewording, and the spec puts it in the JSON-RPC error channel.
                Err(ToolFailure::Unknown(t)) => (
                    Outcome::Failed(RpcError::new(
                        404,
                        protocol::INVALID_PARAMS,
                        format!("Unknown tool: {t}"),
                    )),
                    Trace {
                        tool: Some(name.to_string()),
                        args: args_summary,
                        ok: false,
                        rows: None,
                    },
                ),
                // Insufficient scope is an HTTP 403 rather than a tool error,
                // which is what lets a client run a step-up authorization and
                // come back with the grant instead of just failing.
                Err(ToolFailure::Denied { scope }) => (
                    Outcome::Failed(
                        RpcError::new(
                            403,
                            protocol::INVALID_REQUEST,
                            format!("This connection has not been granted '{scope}'"),
                        )
                        .with_data(json!({ "requiredScope": scope })),
                    ),
                    Trace {
                        tool: Some(name.to_string()),
                        args: args_summary,
                        ok: false,
                        rows: None,
                    },
                ),
                // Everything else is `isError: true` in the *result*, which the
                // spec reserves for failures a model can correct — a misspelled
                // artist, a playlist with no tracks. A JSON-RPC error here
                // would end the exchange instead of prompting a retry.
                Err(ToolFailure::Execution(msg)) => (
                    reply(tool_result(&json!({ "error": msg }), true)),
                    Trace {
                        tool: Some(name.to_string()),
                        args: args_summary,
                        ok: false,
                        rows: None,
                    },
                ),
            }
        }

        // ── Resources ──
        "resources/list" => {
            let list = resources::list(auth);
            let n = list.len();
            (listing(json!({ "resources": list })), Trace::listed(n))
        }
        "resources/templates/list" => {
            let list = resources::templates(auth);
            let n = list.len();
            (listing(json!({ "resourceTemplates": list })), Trace::listed(n))
        }
        "resources/read" => {
            let Some(uri) = req.param_str("uri") else {
                return (
                    Outcome::Failed(RpcError::new(400, protocol::INVALID_PARAMS, "Missing uri")),
                    Trace::plain(false),
                );
            };
            match resources::read(state, auth, uri).await {
                Ok(contents) => (
                    // A note or a shelf entry is as user-specific as it gets.
                    listing(json!({ "contents": contents })),
                    Trace {
                        tool: Some(uri.to_string()),
                        args: None,
                        ok: true,
                        rows: Some(contents_len(&contents)),
                    },
                ),
                Err(msg) => (
                    Outcome::Failed(RpcError::new(404, protocol::INVALID_PARAMS, msg)),
                    Trace { tool: Some(uri.to_string()), args: None, ok: false, rows: None },
                ),
            }
        }

        // ── Prompts ──
        "prompts/list" => {
            let list = prompts::list(auth);
            let n = list.len();
            (listing(json!({ "prompts": list })), Trace::listed(n))
        }
        "prompts/get" => {
            let Some(name) = req.param_str("name") else {
                return (
                    Outcome::Failed(RpcError::new(400, protocol::INVALID_PARAMS, "Missing name")),
                    Trace::plain(false),
                );
            };
            let args = req.params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match prompts::get(auth, name, &args) {
                Ok(v) => (
                    reply(v),
                    Trace { tool: Some(name.to_string()), args: None, ok: true, rows: None },
                ),
                Err(msg) => (
                    Outcome::Failed(RpcError::new(404, protocol::INVALID_PARAMS, msg)),
                    Trace { tool: Some(name.to_string()), args: None, ok: false, rows: None },
                ),
            }
        }

        // Unknown method is `404` *and* `-32601` together, which is not
        // belt-and-braces: the JSON-RPC body is what distinguishes this from
        // the 404 a legacy HTTP+SSE server returns for a path it does not host,
        // and a dual-era client uses exactly that to decide whether to fall
        // back or give up.
        other => (
            Outcome::Failed(RpcError::new(
                404,
                protocol::METHOD_NOT_FOUND,
                format!("Method not found: {other}"),
            )),
            Trace { tool: Some(other.to_string()), args: None, ok: false, rows: None },
        ),
    }
}

fn contents_len(contents: &[Value]) -> i64 {
    contents.iter().filter_map(|c| c["text"].as_str()).map(|t| t.len() as i64).sum()
}

fn server_info() -> Value {
    json!({ "name": super::SERVER_NAME, "version": super::SERVER_VERSION })
}

/// What this server can do.
///
/// `listChanged` is **false** on all three, and that is a statement rather than
/// a stub: the tool, resource and prompt sets are compiled in and change only
/// when the binary does. Declaring `true` would promise notifications on a
/// `subscriptions/listen` stream this server does not open.
fn capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "resources": { "listChanged": false, "subscribe": false },
        "prompts": { "listChanged": false },
        "logging": {},
    })
}

/// Build a `CallToolResult`.
///
/// The JSON goes back **twice** — once as `structuredContent` and once
/// serialised into a text block. That is the spec's own backwards-compatibility
/// guidance and it is worth following here rather than economising: a client on
/// a 2025 revision has no `structuredContent` field at all, and would show the
/// model an empty result from a tool that worked perfectly.
fn tool_result(value: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": value,
        "isError": is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_result_carries_its_json_in_both_forms() {
        let r = tool_result(&json!({ "plays": 21390 }), false);
        assert_eq!(r["structuredContent"]["plays"], 21390);
        // A client on a 2025 revision reads only this half.
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("21390"));
        assert_eq!(r["isError"], false);
    }

    #[test]
    fn nothing_claims_to_send_list_changed_notifications() {
        // Declaring `listChanged: true` promises a stream this server does not
        // open, and a client that opts in would wait for updates forever.
        let c = capabilities();
        assert_eq!(c["tools"]["listChanged"], false);
        assert_eq!(c["resources"]["listChanged"], false);
        assert_eq!(c["resources"]["subscribe"], false);
        assert_eq!(c["prompts"]["listChanged"], false);
    }
}
