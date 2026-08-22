// src/patch/actor.rs — the documents a remote server reads about us
//
// Everything here is a pure function from stored facts to JSON-LD. The
// documents are small and the temptation is to build them inline in the
// handler; they are here because their *shape* is the interoperability
// surface, and a shape is exactly the thing that fails silently — a missing
// field is not an error at either end, it is a remote server quietly deciding
// this actor is not usable and never saying so. That is the MCP envelope bug
// (v0.63.5) waiting to happen again in a different protocol.

use serde_json::{json, Value};

/// The content type an actor document, an outbox and an activity must be
/// served as.
///
/// A server that gets `application/json` here is entitled to treat the document
/// as un-typed JSON rather than as JSON-LD, and several do — with the result
/// that the actor "does not exist" while returning 200.
pub const ACTIVITY_JSON: &str = "application/activity+json";

/// WebFinger's own type, from RFC 7033. Not `application/json`.
pub const JRD_JSON: &str = "application/jrd+json";

/// The namespace for the terms this application adds to ActivityStreams — the
/// signal chain and the audio format, which is the half that makes a Tapedeck
/// listen a Tapedeck listen rather than a sentence.
///
/// **PROVISIONAL, and it has a deadline.** Deferred by Ashwin on 2026-08-22:
/// the value below is the project's own repository, on the grounds that a
/// namespace IRI should resolve to something that explains it, but a
/// forge-hosted URL is not obviously the right permanent home.
///
/// It is safe to change **only while nothing has ever been delivered
/// outbound**, which is true today and stops being true the moment Step 3
/// ships delivery. After that the IRI is a fact about documents other servers
/// hold: renaming it migrates nothing, it just leaves every stored activity
/// pointing at a vocabulary that no longer exists.
///
/// So: settle this before the first `Create` leaves the building. Nothing in
/// Step 1 emits a `td:` term — the namespace rides on the actor document to
/// declare what this server speaks, and an actor document is refetched rather
/// than stored as history.
pub const TD_NS: &str = "https://codeberg.org/abksh/tapedeck/ns#";

/// The `@context` every document this server emits carries.
///
/// `security/v1` is what makes `publicKey` mean something — without it a
/// conforming JSON-LD processor drops the key as an undefined term, and the
/// actor becomes one whose signatures can never be checked.
pub fn context() -> Value {
    json!([
        "https://www.w3.org/ns/activitystreams",
        "https://w3id.org/security/v1",
        { "td": TD_NS }
    ])
}

/// The key's own id. A fragment on the actor, which is what a `keyId` in a
/// signature header points at.
pub fn key_id(actor_uri: &str) -> String {
    format!("{actor_uri}#main-key")
}

pub fn inbox_uri(actor_uri: &str) -> String {
    format!("{actor_uri}/inbox")
}

pub fn outbox_uri(actor_uri: &str) -> String {
    format!("{actor_uri}/outbox")
}

/// What a deck looks like to a server that has never heard of Tapedeck.
///
/// Three deliberate omissions, each because advertising an endpoint this
/// server does not serve is worse than not having it — the same call
/// `listChanged: false` makes on the MCP surface:
///
/// * **No `endpoints.sharedInbox`.** There is no `POST /inbox`. A sender that
///   believed the advertisement would batch every recipient on this host into
///   one delivery and have all of them 404.
/// * **No `followers` / `following` collections.** Both are optional in
///   ActivityPub, and neither is served yet. They arrive with Step 2, where
///   `patches` rows exist to fill them.
/// * **No `published`.** The account's creation date is a fact about the
///   person, and this document is fetched by strangers.
pub fn document(
    actor_uri: &str,
    username: &str,
    display_name: Option<&str>,
    summary: Option<&str>,
    icon_url: Option<&str>,
    public_pem: &str,
) -> Value {
    let mut doc = json!({
        "@context": context(),
        "id": actor_uri,
        "type": "Person",
        "preferredUsername": username,
        // The human-facing page for this actor. The same URI today, because
        // `GET /users/:name` negotiates: a browser gets the app, a server gets
        // this document.
        "url": actor_uri,
        "inbox": inbox_uri(actor_uri),
        "outbox": outbox_uri(actor_uri),
        // Private by default is the instance's whole posture, and this is where
        // a remote server learns it: a follow is a request, not a subscription.
        "manuallyApprovesFollowers": true,
        "publicKey": {
            "id": key_id(actor_uri),
            "owner": actor_uri,
            "publicKeyPem": public_pem,
        }
    });

    let map = doc.as_object_mut().expect("built as an object");

    // `name` is the display name and falls back to the username: an actor with
    // no `name` renders as a blank line in several clients rather than as the
    // handle.
    map.insert(
        "name".into(),
        json!(display_name.filter(|s| !s.trim().is_empty()).unwrap_or(username)),
    );

    if let Some(summary) = summary.filter(|s| !s.trim().is_empty()) {
        map.insert("summary".into(), json!(summary));
    }
    if let Some(icon) = icon_url.filter(|s| !s.trim().is_empty()) {
        map.insert("icon".into(), json!({ "type": "Image", "url": icon }));
    }

    doc
}

/// An `OrderedCollection`, served whether or not there is anything in it.
///
/// **The shape has to be right from the start even while it is empty**, because
/// remote servers probe an outbox to decide whether an actor is worth
/// following, and a 404 or a bare `[]` reads as a broken actor rather than as a
/// quiet one. Paging (`first`/`next`) arrives with delivery — an empty
/// collection has no page to point at, and inventing one would advertise a
/// third endpoint that does not exist.
pub fn outbox(actor_uri: &str, total: i64, items: Vec<Value>) -> Value {
    json!({
        "@context": context(),
        "id": outbox_uri(actor_uri),
        "type": "OrderedCollection",
        "totalItems": total,
        "orderedItems": items,
    })
}

/// The WebFinger response: `acct:ash@deck.example` → the actor URI.
///
/// This is the entry point for the whole protocol — it is what somebody typing
/// a handle into another server's search box hits first — and it is the one
/// document where the `rel` values matter more than the content.
pub fn webfinger(handle: &str, actor_uri: &str) -> Value {
    json!({
        "subject": format!("acct:{handle}"),
        "aliases": [actor_uri],
        "links": [
            {
                "rel": "self",
                "type": ACTIVITY_JSON,
                "href": actor_uri,
            },
            {
                "rel": "http://webfinger.net/rel/profile-page",
                "type": "text/html",
                "href": actor_uri,
            }
        ]
    })
}

/// Read a WebFinger `resource` parameter into `(username, host)`.
///
/// Accepts `acct:ash@host`, a bare `ash@host`, and the actor URI itself —
/// clients send all three, and refusing the ones that are merely unfashionable
/// makes this server undiscoverable from software that is otherwise fine.
///
/// The host is **not** matched against ours here. That is the caller's job,
/// because the answer to a resource on another host is a 404 and the answer to
/// a malformed one is a 400, and only the caller knows which host it is.
pub fn parse_resource(resource: &str) -> Option<(String, String)> {
    let r = resource.trim();

    // The actor URI form: https://host/users/ash
    if let Some(rest) = r.strip_prefix("https://").or_else(|| r.strip_prefix("http://")) {
        let (host, path) = rest.split_once('/')?;
        let user = path.strip_prefix("users/")?;
        // Nothing beneath the actor is an actor: `/users/ash/inbox` names an
        // endpoint, not a person.
        return (!user.is_empty() && !user.contains('/'))
            .then(|| (user.to_string(), host.to_string()));
    }

    let acct = r.strip_prefix("acct:").unwrap_or(r);
    let acct = acct.strip_prefix('@').unwrap_or(acct);
    let (user, host) = acct.split_once('@')?;
    (!user.is_empty() && !host.is_empty() && !user.contains('@'))
        .then(|| (user.to_string(), host.to_string()))
}

/// The actor URI for a username on this origin. **Written once, at creation,
/// and read from the column ever after** — see `actors.uri`. This function
/// exists to mint it, not to re-derive it.
pub fn uri_for(origin: &str, username: &str) -> String {
    format!("{}/users/{}", origin.trim_end_matches('/'), username)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEM: &str = "-----BEGIN PUBLIC KEY-----\nMII...\n-----END PUBLIC KEY-----\n";

    /// These field names are the interoperability surface. A rename here is a
    /// remote server silently deciding the actor is unusable, so they are
    /// pinned by name rather than by shape.
    #[test]
    fn an_actor_document_carries_what_a_remote_server_looks_for() {
        let doc = document(
            "https://deck.example/users/ash",
            "ash",
            Some("Ashwin"),
            Some("plays records"),
            Some("https://deck.example/avatar.png"),
            PEM,
        );

        assert_eq!(doc["type"], "Person");
        assert_eq!(doc["id"], "https://deck.example/users/ash");
        assert_eq!(doc["preferredUsername"], "ash");
        assert_eq!(doc["name"], "Ashwin");
        assert_eq!(doc["summary"], "plays records");
        assert_eq!(doc["inbox"], "https://deck.example/users/ash/inbox");
        assert_eq!(doc["outbox"], "https://deck.example/users/ash/outbox");
        assert_eq!(doc["icon"]["type"], "Image");
        assert_eq!(doc["publicKey"]["id"], "https://deck.example/users/ash#main-key");
        assert_eq!(doc["publicKey"]["owner"], "https://deck.example/users/ash");
        assert_eq!(doc["publicKey"]["publicKeyPem"], PEM);

        // Private by default, stated to the network rather than only enforced
        // at home.
        assert_eq!(doc["manuallyApprovesFollowers"], true);

        // `security/v1` is what makes `publicKey` a defined term. Without it a
        // conforming processor drops the key.
        let ctx = doc["@context"].as_array().expect("context is a list");
        assert!(ctx.iter().any(|c| c == "https://www.w3.org/ns/activitystreams"));
        assert!(ctx.iter().any(|c| c == "https://w3id.org/security/v1"));
        assert!(ctx.iter().any(|c| c["td"] == TD_NS), "the frozen namespace travels from day one");
    }

    /// Advertising an endpoint that 404s is worse than not having it: a sender
    /// that believes a `sharedInbox` batches every recipient here into one
    /// delivery and loses all of them.
    #[test]
    fn nothing_unserved_is_advertised() {
        let doc = document("https://deck.example/users/ash", "ash", None, None, None, PEM);
        assert!(doc.get("endpoints").is_none(), "no sharedInbox until POST /inbox exists");
        assert!(doc.get("followers").is_none(), "no followers collection until Step 2 serves one");
        assert!(doc.get("following").is_none());
    }

    #[test]
    fn an_actor_with_no_profile_still_has_a_name() {
        let doc = document("https://deck.example/users/ash", "ash", None, None, None, PEM);
        assert_eq!(doc["name"], "ash", "falls back to the handle rather than rendering blank");
        assert!(doc.get("summary").is_none(), "an absent bio is absent, not an empty string");
        assert!(doc.get("icon").is_none());

        // A profile filled in with spaces is not a profile.
        let blank =
            document("https://deck.example/users/ash", "ash", Some("  "), Some(""), Some(""), PEM);
        assert_eq!(blank["name"], "ash");
        assert!(blank.get("summary").is_none());
        assert!(blank.get("icon").is_none());
    }

    #[test]
    fn an_empty_outbox_is_a_collection_and_not_a_404() {
        let ob = outbox("https://deck.example/users/ash", 0, vec![]);
        assert_eq!(ob["type"], "OrderedCollection");
        assert_eq!(ob["id"], "https://deck.example/users/ash/outbox");
        assert_eq!(ob["totalItems"], 0);
        assert!(ob["orderedItems"].as_array().expect("a list, not null").is_empty());
    }

    #[test]
    fn webfinger_points_at_the_actor() {
        let wf = webfinger("ash@deck.example", "https://deck.example/users/ash");
        assert_eq!(wf["subject"], "acct:ash@deck.example");

        let links = wf["links"].as_array().expect("links");
        let self_link = links.iter().find(|l| l["rel"] == "self").expect("a self link");
        assert_eq!(self_link["type"], ACTIVITY_JSON);
        assert_eq!(self_link["href"], "https://deck.example/users/ash");
    }

    /// All three spellings are on the wire from real clients. Refusing the
    /// unfashionable ones makes this server undiscoverable from software that
    /// is otherwise fine.
    #[test]
    fn every_spelling_of_a_handle_resolves() {
        let want = Some(("ash".to_string(), "deck.example".to_string()));
        assert_eq!(parse_resource("acct:ash@deck.example"), want);
        assert_eq!(parse_resource("ash@deck.example"), want);
        assert_eq!(parse_resource("@ash@deck.example"), want);
        assert_eq!(parse_resource("  acct:ash@deck.example  "), want);
        assert_eq!(parse_resource("https://deck.example/users/ash"), want);
    }

    #[test]
    fn a_resource_that_names_no_actor_is_refused() {
        assert!(parse_resource("").is_none());
        assert!(parse_resource("acct:").is_none());
        assert!(parse_resource("acct:ash").is_none(), "no host");
        assert!(parse_resource("acct:@deck.example").is_none(), "no user");
        assert!(parse_resource("https://deck.example/").is_none());
        assert!(
            parse_resource("https://deck.example/users/ash/inbox").is_none(),
            "an endpoint beneath an actor is not an actor"
        );
    }

    #[test]
    fn a_uri_is_minted_from_the_origin_and_the_name() {
        assert_eq!(uri_for("https://deck.example", "ash"), "https://deck.example/users/ash");
        assert_eq!(uri_for("https://deck.example/", "ash"), "https://deck.example/users/ash");
    }
}
