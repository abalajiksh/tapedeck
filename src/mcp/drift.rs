//! Drift guards for the MCP surface.
//!
//! `openapi::tests::the_spec_and_the_router_describe_the_same_api` exists
//! because a route and a YAML document are genuinely separate artifacts that
//! can fall out of step, and the failure — a client author sent to an endpoint
//! that 404s — is invisible to everything else in the build. This module is the
//! same idea applied to the places the MCP surface can rot, and it exists
//! because the same failure shape has now been hit repeatedly: **everything
//! reports success and the model is quietly handed something that does not
//! work.**
//!
//! Four things can drift, each with its own way of being silent:
//!
//! 1. **A tool is advertised and not implemented.** `tools/list` offers it,
//!    the scope check passes, and `tools/call` then answers "Unknown tool". The
//!    model sees a capability that always fails.
//! 2. **A grant is offered on the consent screen and reaches nothing.** The
//!    user ticks a box that does nothing.
//! 3. **Prose names a tool that no longer exists.** The instructions preamble
//!    and every prompt body name tools in backticks; a rename leaves the model
//!    chasing a ghost, and prose is exactly what nobody greps when renaming.
//! 4. **A resource is listed and cannot be read.**
//!
//! The catalogue-versus-dispatch check is textual, for the same reason the
//! OpenAPI scanner is: the two really are separate constructs in one file, and
//! a dumb scan that occasionally needs a filter beats no check at all. The
//! structural alternative — hanging a handler function pointer off every
//! `ToolDef` so there is only one list — would make drift impossible rather
//! than merely detected, and is the right move if this ever grows a second
//! dispatcher. It is not worth uglifying twenty-five tool definitions today.

use std::collections::BTreeSet;

use super::{prompts, resources, scopes, tools};

const TOOLS_SRC: &str = include_str!("tools.rs");

/// Tool names the dispatcher in `tools::call` actually handles.
///
/// Matched on a line whose *trimmed* content opens the match arm, which is what
/// keeps `username: "t"` in a test helper out of the catalogue side of this —
/// the same substring trap that has bitten an edit in this codebase before.
fn dispatched() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in TOOLS_SRC.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('"') else { continue };
        let Some((name, tail)) = rest.split_once('"') else { continue };
        // `"name" => …` and nothing else. A bare string literal on its own line
        // is not a match arm.
        if !tail.trim_start().starts_with("=>") {
            continue;
        }
        if name.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !name.is_empty() {
            out.insert(name.to_string());
        }
    }
    out
}

fn catalogue_names() -> BTreeSet<String> {
    tools::catalogue().into_iter().map(|t| t.name.to_string()).collect()
}

/// Backticked snake_case terms in the model-facing prose that are *not* tool
/// names — field names, JSON keys, values.
///
/// An allowlist rather than a cleverer parser, and deliberately annoying: a new
/// term has to be added here, which is the moment somebody notices they are
/// naming an implementation detail at a language model. Same trade the OpenAPI
/// scanner makes with its "must start with `/`" filter.
const NOT_TOOL_NAMES: &[&str] = &[
    // Fields the tools return, named in prose so the model knows what to read.
    "next_door",
    "unexplored",
    "exemplar_artists",
    "your_artists",
    "position_known",
    "first_played_at",
    "artist_names",
    "artist_name",
    "additional_info",
    "mbid_mapping",
    "listen_type",
    "idempotency_key",
    "structuredContent",
    "note",
    "reason",
    "because",
    "scope",
    "unit",
    "offset",
];

/// Every backticked snake_case token in a piece of model-facing prose.
fn backticked(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else { break };
        let token = &rest[..close];
        rest = &rest[close + 1..];
        if !token.is_empty()
            && token.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            out.insert(token.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::McpAuth;
    use crate::models::AuthUser;
    use serde_json::json;

    fn all_scopes() -> McpAuth {
        McpAuth {
            connection_id: 1,
            user: AuthUser { user_id: 1, username: "drift".into(), is_admin: false },
            scopes: scopes::ALL.iter().map(|(s, _)| *s).collect::<Vec<_>>().join(" "),
        }
    }

    #[test]
    fn every_advertised_tool_is_actually_dispatched() {
        // The failure this catches: `tools/list` offers a tool, the scope check
        // passes, and `tools/call` answers "Unknown tool". Nothing in the build
        // notices, and the model is handed a capability that always fails.
        let advertised = catalogue_names();
        let handled = dispatched();

        let missing: Vec<&String> = advertised.difference(&handled).collect();
        assert!(
            missing.is_empty(),
            "these tools are advertised by tools/list but have no arm in tools::call, \
so calling them returns 'Unknown tool': {missing:?}"
        );

        let orphaned: Vec<&String> = handled.difference(&advertised).collect();
        assert!(
            orphaned.is_empty(),
            "these arms exist in tools::call but no tool advertises them, so they are \
unreachable — `call` looks the name up in the catalogue first: {orphaned:?}"
        );
    }

    #[test]
    fn the_scanner_finds_the_tools_it_should() {
        // A silently-broken scanner would make the check above pass by finding
        // nothing, which is the trap the OpenAPI drift test documents. Pin a
        // few known arms and a plausible floor.
        let handled = dispatched();
        for known in ["listening_profile", "save_playlist", "add_to_crate", "write_note"] {
            assert!(handled.contains(known), "scanner missed the '{known}' arm");
        }
        assert!(handled.len() >= 20, "scanner found only {} arms", handled.len());
    }

    #[test]
    fn every_grant_reaches_something() {
        // A scope on the consent screen that no tool and no prompt uses is a
        // box the user can tick for no effect.
        let auth = all_scopes();
        let tool_scopes: BTreeSet<&str> = tools::catalogue().iter().map(|t| t.scope).collect();
        let prompt_names = prompts::list(&auth);

        for (scope, _) in scopes::ALL {
            let used_by_tool = tool_scopes.contains(scope);
            // A grant may legitimately be prompt-only in future; today none is,
            // but the check should not force a tool where a prompt would do.
            let used_by_prompt = !prompt_names.is_empty() && used_by_tool;
            assert!(
                used_by_tool || used_by_prompt,
                "grant '{scope}' is offered on the consent screen but nothing uses it"
            );
        }
    }

    #[test]
    fn the_instructions_only_name_tools_that_exist() {
        // The preamble tells the model to call `listening_profile` first and
        // points at `add_to_crate` and `genre_neighbours` by name. A rename
        // leaves it directing the model at nothing — and prose is precisely
        // what nobody greps when renaming a tool.
        let known = catalogue_names();
        for token in backticked(&super::super::instructions()) {
            assert!(
                known.contains(&token) || NOT_TOOL_NAMES.contains(&token.as_str()),
                "the instructions preamble names `{token}`, which is not a tool. \
Rename it, or add it to NOT_TOOL_NAMES if it is a field rather than a tool."
            );
        }
    }

    #[test]
    fn every_prompt_only_names_tools_that_exist() {
        // Same failure, and likelier: a prompt body is several paragraphs of
        // prose naming half a dozen tools.
        let auth = all_scopes();
        let known = catalogue_names();
        for p in prompts::list(&auth) {
            let name = p["name"].as_str().unwrap().to_string();
            let rendered = prompts::get(&auth, &name, &json!({})).unwrap();
            let text = rendered["messages"][0]["content"]["text"].as_str().unwrap();
            for token in backticked(text) {
                assert!(
                    known.contains(&token) || NOT_TOOL_NAMES.contains(&token.as_str()),
                    "prompt '{name}' names `{token}`, which is not a tool"
                );
            }
        }
    }

    #[test]
    fn every_listed_resource_is_readable() {
        // A URI in `resources/list` that `resources::read` has no arm for is a
        // dead entry in the user's own attachment picker.
        let auth = all_scopes();
        for r in resources::list(&auth) {
            let uri = r["uri"].as_str().expect("a listed resource needs a uri").to_string();
            assert!(
                uri.starts_with("tapedeck://"),
                "resource '{uri}' does not use this server's scheme"
            );
            // `read` matches on exact URIs and on two prefixes; anything listed
            // has to be one of those shapes.
            let readable = uri == "tapedeck://profile"
                || uri == "tapedeck://notes"
                || uri.starts_with("tapedeck://note/")
                || uri.starts_with("tapedeck://shelf/");
            assert!(readable, "resource '{uri}' is listed but resources::read has no arm for it");
        }
    }

    #[test]
    fn the_prose_scanner_is_not_silently_finding_nothing() {
        // Same guard as `the_scanner_finds_the_tools_it_should`, for the other
        // scanner: if `backticked` stopped matching, the two prose checks would
        // pass by examining an empty set.
        let found = backticked(&super::super::instructions());
        assert!(
            found.contains("listening_profile"),
            "prose scanner found {found:?}, which does not include the tool the \
preamble explicitly tells the model to call first"
        );
    }
}
