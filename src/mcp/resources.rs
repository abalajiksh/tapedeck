//! Resources — things the *user* attaches to a conversation, as opposed to
//! tools, which the model calls on its own initiative.
//!
//! The distinction is the whole reason this module exists rather than being two
//! more tools. "Here is what I wrote about this record, help me finish it" is a
//! user pulling one document into context deliberately; `liner_notes` searching
//! two hundred of them is the model going looking. Both are worth having and
//! they are not the same gesture.
//!
//! Templates rather than a flat list, because the alternative is enumerating
//! every note and every shelf entry on `resources/list` — which for a real
//! collection is a large payload that is mostly not what anyone wanted.

use std::sync::Arc;

use serde_json::{json, Value};

use super::scopes;
use crate::db::McpAuth;
use crate::server::AppState;

/// URI templates, advertised on `resources/templates/list`.
pub fn templates(auth: &McpAuth) -> Vec<Value> {
    if !auth.has_scope(scopes::LIBRARY_READ) {
        return Vec::new();
    }
    vec![
        json!({
            "uriTemplate": "tapedeck://note/{id}",
            "name": "liner-note",
            "title": "A liner note",
            "description": "One note the user wrote, as the Markdown they typed rather than \
        the rendered HTML. Their own writing.",
            "mimeType": "text/markdown",
        }),
        json!({
            "uriTemplate": "tapedeck://shelf/{id}",
            "name": "shelf-item",
            "title": "A record on the shelf",
            "description": "One pressing they physically own: catalogue number, tracklist by \
        side, and how often each side has been played.",
            "mimeType": "application/json",
        }),
    ]
}

/// The fixed resources. Deliberately short — a resource list is something a
/// person scrolls through in a picker.
pub fn list(auth: &McpAuth) -> Vec<Value> {
    let mut out = Vec::new();
    if auth.has_scope(scopes::LISTENING_READ) {
        out.push(json!({
            "uri": "tapedeck://profile",
            "name": "listening-profile",
            "title": "Listening profile",
            "description": "The shape of this history and how much of it carries each kind of \
        metadata. The same content as the listening_profile tool, for attaching directly.",
            "mimeType": "application/json",
        }));
    }
    if auth.has_scope(scopes::LIBRARY_READ) {
        out.push(json!({
            "uri": "tapedeck://notes",
            "name": "liner-notes",
            "title": "All liner notes",
            "description": "Everything the user has written about their music, one document.",
            "mimeType": "text/markdown",
        }));
    }
    out
}

/// Read one resource.
///
/// Every arm scopes by `auth.user.user_id`, the same as every handler — a URI
/// is a name, not a capability, and an id in it must never be enough to reach
/// another household member's writing.
pub async fn read(state: &Arc<AppState>, auth: &McpAuth, uri: &str) -> Result<Vec<Value>, String> {
    let deny = || Err("This connection is not permitted to read that".to_string());

    if uri == "tapedeck://profile" {
        if !auth.has_scope(scopes::LISTENING_READ) {
            return deny();
        }
        let outcome = super::tools::call(state, auth, "listening_profile", &json!({}))
            .await
            .map_err(|e| format!("{e:?}"))?;
        return Ok(vec![text(uri, "application/json", &to_pretty(&outcome.value))]);
    }

    if !auth.has_scope(scopes::LIBRARY_READ) {
        return deny();
    }

    if uri == "tapedeck://notes" {
        let notes = state.db.list_notes(auth.user.user_id).await.map_err(|e| e.to_string())?;
        let mut doc = String::from("# Liner notes\n\n");
        for n in &notes {
            doc.push_str(&format!("## {}\n\n{}\n\n", note_title(state, n).await, n.body_md));
        }
        return Ok(vec![text(uri, "text/markdown", &doc)]);
    }

    if let Some(id) = uri.strip_prefix("tapedeck://note/").and_then(|s| s.parse::<i64>().ok()) {
        let note = state
            .db
            .get_note(auth.user.user_id, id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("No such note")?;
        let body = format!("# {}\n\n{}\n", note_title(state, &note).await, note.body_md);
        return Ok(vec![text(uri, "text/markdown", &body)]);
    }

    if let Some(id) = uri.strip_prefix("tapedeck://shelf/").and_then(|s| s.parse::<i64>().ok()) {
        let item = state
            .db
            .get_physical(auth.user.user_id, id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("No such shelf item")?;
        let value = serde_json::to_value(&item).map_err(|e| e.to_string())?;
        return Ok(vec![text(uri, "application/json", &to_pretty(&value))]);
    }

    Err(format!("Unknown resource: {uri}"))
}

/// What a note is about, as a heading.
///
/// `NoteRow` carries only the target's *id*, so the name costs a lookup. A note
/// on a listen or a period has no entity at all, which is not a gap — those are
/// notes about an occasion rather than about a record.
async fn note_title(state: &Arc<AppState>, note: &crate::db::NoteRow) -> String {
    if let Some(id) = note.entity_id {
        if let Ok(Some(e)) = state.db.get_entity(id).await {
            // An artist entity carries an empty `artist_name` by design — the
            // name *is* the artist — so this is the usual case, not a gap.
            return if e.artist_name.trim().is_empty() {
                e.name
            } else {
                format!("{} — {}", e.artist_name, e.name)
            };
        }
    }
    if let Some(period) = &note.period_key {
        return period.clone();
    }
    match note.scrobble_id {
        Some(id) => format!("A listen (#{id})"),
        None => format!("Note {}", note.id),
    }
}

fn to_pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn text(uri: &str, mime: &str, body: &str) -> Value {
    json!({ "uri": uri, "mimeType": mime, "text": body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AuthUser;

    fn auth(scopes: &str) -> McpAuth {
        McpAuth {
            connection_id: 1,
            user: AuthUser { user_id: 1, username: "t".into(), is_admin: false },
            scopes: scopes.into(),
        }
    }

    #[test]
    fn the_journal_is_not_offered_without_the_grant_that_covers_it() {
        // Someone who granted only their listening statistics has not granted
        // their writing, and a resource picker showing it would be the same
        // leak whether or not `read` then refused.
        let listening = list(&auth(scopes::LISTENING_READ));
        let uris: Vec<&str> = listening.iter().filter_map(|r| r["uri"].as_str()).collect();
        assert!(uris.contains(&"tapedeck://profile"));
        assert!(!uris.contains(&"tapedeck://notes"));
        assert!(templates(&auth(scopes::LISTENING_READ)).is_empty());
    }

    #[test]
    fn a_connection_with_nothing_granted_sees_nothing() {
        assert!(list(&auth("")).is_empty());
        assert!(templates(&auth("")).is_empty());
    }
}
