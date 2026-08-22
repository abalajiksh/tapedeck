// src/patch/signatures.rs — draft-cavage HTTP Signatures, both directions
//
// This is the whole of the trust boundary with the rest of the fediverse. An
// inbox is an unauthenticated POST endpoint that anyone on the internet can
// reach; the only thing separating "a Follow from ash@deck.example" from "a
// Follow from somebody claiming to be ash@deck.example" is that these bytes
// verified against a key fetched from the actor they name.
//
// The scheme is `draft-cavage-http-signatures`, which is expired as an
// internet-draft and universally deployed anyway — RFC 9421 is the successor
// and almost nothing speaks it yet. Same trade as RSA over Ed25519 in `keys`.
//
// Everything here is pure. Deciding whether to believe a request is exactly
// the kind of rule that must be testable without a server, for the reason
// `own_history_tracks` and `public_payload` are split out from their handlers.

use std::error::Error;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};

/// A parsed `Signature:` header.
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureHeader {
    /// The actor's key, as a URI with a fragment: `https://host/users/ash#main-key`.
    /// This is *the claim of identity* — everything else only establishes that
    /// whoever holds that key made the request.
    pub key_id: String,
    pub algorithm: Option<String>,
    /// Which headers the signature covers, in the order they are signed in.
    pub headers: Vec<String>,
    pub signature: String,
}

/// How far out of step a remote clock may be before its request is refused.
///
/// **This bounds a replay window; it does not close one.** Closing it needs a
/// seen-nonce store, and nothing here has one — a request captured off the wire
/// can be resent for this long and will verify, because it is byte-for-byte the
/// request that was legitimately signed. Five minutes is the cost of not having
/// that store.
///
/// Deliberately tighter than Mastodon, which tolerates twelve hours. If real
/// instances start failing delivery here, this is the number to loosen, and the
/// symptom will be signature refusals that correlate with one peer rather than
/// with one activity.
pub const MAX_SKEW_SECS: i64 = 300;

/// The headers an inbound signature must actually cover.
///
/// **A signature over a subset it chooses is not a signature over the request.**
/// A sender that signs only `date` has authenticated a timestamp: the body,
/// the path and the host are all unprotected, so the same signature moves any
/// payload to any endpoint on this server for as long as the clock allows. So
/// the covered set is checked before the cryptography, and a request that omits
/// one of these is refused whether or not the maths works out.
///
/// `digest` is required only when there is a body, which for an inbox is
/// always — but the rule is written against the body rather than the method so
/// a future signed GET is not forced to invent one.
pub const REQUIRED_COVERAGE: [&str; 3] = ["(request-target)", "host", "date"];

/// Parse `keyId="…",algorithm="…",headers="…",signature="…"`.
///
/// Tolerant about the things senders vary on — the optional `Signature ` scheme
/// prefix, spacing after commas, unquoted values — and strict about the two
/// fields that mean anything: without `keyId` there is nobody to check against,
/// and without `signature` there is nothing to check.
pub fn parse(header: &str) -> Option<SignatureHeader> {
    let body = header.trim().strip_prefix("Signature ").unwrap_or(header.trim());

    let mut key_id = None;
    let mut algorithm = None;
    let mut headers = None;
    let mut signature = None;

    for part in split_params(body) {
        let (name, value) = part.split_once('=')?;
        let value = value.trim().trim_matches('"').to_string();
        match name.trim() {
            "keyId" => key_id = Some(value),
            "algorithm" => algorithm = Some(value),
            "headers" => headers = Some(value),
            "signature" => signature = Some(value),
            _ => {}
        }
    }

    Some(SignatureHeader {
        key_id: key_id.filter(|s| !s.is_empty())?,
        algorithm,
        // An absent `headers` means `date` alone, per the draft. Kept rather
        // than defaulted to something safer, because `REQUIRED_COVERAGE` is
        // what refuses it — one rule about coverage, in one place.
        headers: headers
            .unwrap_or_else(|| "date".into())
            .split_whitespace()
            .map(|h| h.to_ascii_lowercase())
            .collect(),
        signature: signature.filter(|s| !s.is_empty())?,
    })
}

/// Split on commas that are not inside a quoted value. A base64 signature can
/// contain no comma, but `keyId` is a URI and a naive `split(',')` is the kind
/// of thing that works until somebody's key id has a query string in it.
fn split_params(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                current.push(c);
            }
            ',' if !in_quotes => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

/// Rebuild the exact bytes that were signed.
///
/// `lookup` is given a lowercased header name and returns its value. Returning
/// `None` for a header the signature claims to cover is fatal and must stay
/// fatal: silently signing over an empty string for a missing header would let
/// a sender list `digest` in `headers`, omit the header, and have the body go
/// unprotected while the signature still verified.
pub fn signing_string(
    method: &str,
    path_and_query: &str,
    covered: &[String],
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let mut lines = Vec::with_capacity(covered.len());
    for name in covered {
        let line = if name == "(request-target)" {
            format!("(request-target): {} {}", method.to_ascii_lowercase(), path_and_query)
        } else {
            format!("{}: {}", name, lookup(name)?.trim())
        };
        lines.push(line);
    }
    Some(lines.join("\n"))
}

/// `SHA-256=<base64>`, the form the `Digest` header takes.
#[allow(dead_code)] // outbound half; used by `sign_post` and the round-trip test.
pub fn digest_header(body: &[u8]) -> String {
    format!("SHA-256={}", B64.encode(Sha256::digest(body)))
}

/// Does the `Digest` header actually describe this body?
///
/// The algorithm name is compared case-insensitively (`SHA-256` and `sha-256`
/// are both on the wire) and the encoded digest is **not** — base64 is
/// case-significant, and folding it would accept a different digest.
pub fn digest_matches(header: &str, body: &[u8]) -> bool {
    header.split(',').any(|entry| {
        match entry.trim().split_once('=') {
            // `split_once` stops at the first `=`, which is right: base64
            // padding puts `=` inside the value.
            Some((alg, want)) if alg.trim().eq_ignore_ascii_case("sha-256") => {
                want.trim() == B64.encode(Sha256::digest(body))
            }
            _ => false,
        }
    })
}

/// Is the request's `Date` close enough to now to be believed?
///
/// Both directions are bounded. Too old is replay; too far in the future is a
/// broken clock, and accepting it would let a request stay valid for as long as
/// that clock is wrong.
pub fn date_is_fresh(date_header: &str, now: i64) -> bool {
    let Some(sent) = parse_http_date(date_header) else {
        return false;
    };
    (sent - now).abs() <= MAX_SKEW_SECS
}

/// HTTP-date, as unix seconds.
///
/// RFC 7231 names three formats and only IMF-fixdate is allowed to be
/// generated, which is what `parse_from_rfc2822` reads — including the `GMT`
/// zone spelling that a strict RFC 2822 parser would refuse.
pub fn parse_http_date(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value.trim()).ok().map(|d| d.timestamp())
}

/// Format `now` the way the `Date` header must be sent.
#[allow(dead_code)] // outbound half; used by `sign_post` and the round-trip test.
pub fn http_date(now: i64) -> String {
    chrono::DateTime::from_timestamp(now, 0)
        .unwrap_or_default()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

/// The headers to attach to an outbound signed POST, in the order they are
/// signed. Returned rather than applied so the caller owns the request builder
/// and this stays pure.
///
/// **Nothing calls this yet — delivery is Step 3 — and it is here rather than
/// waiting for one.** Not an oversight: `a_request_we_signed_is_a_request_we
/// _believe` is the sharpest test the *verifier* has, because it exercises the
/// signing string, the digest and the date against a real key end to end
/// instead of against a fixture somebody wrote to match the implementation. A
/// verifier tested only on inputs its own author invented is a verifier that
/// agrees with itself.
///
/// `key_id` is the *actor's* key URI, not the actor URI: a receiver
/// dereferences it to find the key, and pointing it at the bare actor works
/// only by accident on servers that happen to look inside the document anyway.
#[allow(dead_code)] // see the note above: Step 3 calls this.
pub fn sign_post(
    key_id: &str,
    private_pem: &str,
    url: &str,
    body: &[u8],
    now: i64,
) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let (host, path) = split_url(url).ok_or("not an absolute http(s) URL")?;
    let date = http_date(now);
    let digest = digest_header(body);

    let covered: Vec<String> =
        ["(request-target)", "host", "date", "digest"].iter().map(|s| s.to_string()).collect();

    let message = signing_string("post", &path, &covered, |name| match name {
        "host" => Some(host.clone()),
        "date" => Some(date.clone()),
        "digest" => Some(digest.clone()),
        _ => None,
    })
    .ok_or("could not build the signing string")?;

    let signature = super::keys::sign(private_pem, message.as_bytes())?;

    Ok(vec![
        ("host".into(), host),
        ("date".into(), date),
        ("digest".into(), digest),
        (
            "signature".into(),
            format!(
                "keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"{}\",signature=\"{signature}\"",
                covered.join(" ")
            ),
        ),
    ])
}

/// `https://host/path?q` → `("host", "/path?q")`.
#[allow(dead_code)] // outbound half.
fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    (!host.is_empty()).then(|| (host.to_string(), path.to_string()))
}

/// Why an inbound request was not believed. Separate variants because they need
/// completely different things from whoever is looking: a `Stale` is a clock, a
/// `Coverage` is a sender's implementation, and a `BadSignature` is either a
/// key that has rotated or somebody trying it on.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// No `Signature:` header, or one that does not parse.
    NoSignature,
    /// Signed, but does not cover enough of the request to mean anything.
    Coverage(&'static str),
    /// `Date` missing, unparseable, or outside `MAX_SKEW_SECS`.
    Stale,
    /// `Digest` missing or not describing this body.
    Digest,
    /// A header the signature claims to cover was not sent.
    MissingHeader(String),
    /// The maths did not work out.
    BadSignature,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoSignature => write!(f, "no parseable Signature header"),
            Refusal::Coverage(what) => write!(f, "signature does not cover {what}"),
            Refusal::Stale => write!(f, "Date is missing or outside the accepted skew"),
            Refusal::Digest => write!(f, "Digest is missing or does not match the body"),
            Refusal::MissingHeader(h) => write!(f, "signature covers {h}, which was not sent"),
            Refusal::BadSignature => write!(f, "signature did not verify against the actor's key"),
        }
    }
}

/// Everything about believing an inbound request *except* fetching the key.
///
/// Split that way on purpose: fetching an actor document is a network call with
/// a cache behind it, and the rules below are the part that has to be provable
/// without one. The caller resolves `parsed.key_id` and hands the PEM back in.
pub fn verify_inbound(
    method: &str,
    path_and_query: &str,
    body: &[u8],
    now: i64,
    lookup: impl Fn(&str) -> Option<String>,
    public_pem: &str,
    parsed: &SignatureHeader,
) -> Result<(), Refusal> {
    // Coverage first, and before any cryptography: a signature that verifies
    // perfectly over the wrong set of headers is worse than no signature,
    // because it looks like it proved something.
    for required in REQUIRED_COVERAGE {
        if !parsed.headers.iter().any(|h| h == required) {
            return Err(Refusal::Coverage(required));
        }
    }
    if !body.is_empty() && !parsed.headers.iter().any(|h| h == "digest") {
        return Err(Refusal::Coverage("digest"));
    }

    match lookup("date") {
        Some(date) if date_is_fresh(&date, now) => {}
        _ => return Err(Refusal::Stale),
    }

    if !body.is_empty() {
        match lookup("digest") {
            Some(digest) if digest_matches(&digest, body) => {}
            _ => return Err(Refusal::Digest),
        }
    }

    // Named before the string is built rather than discovered while building
    // it, so the refusal can say *which* header was claimed and not sent — a
    // bare "signature invalid" here sends whoever debugs it to the wrong half.
    if let Some(missing) =
        parsed.headers.iter().find(|h| h.as_str() != "(request-target)" && lookup(h).is_none())
    {
        return Err(Refusal::MissingHeader(missing.clone()));
    }

    let message = signing_string(method, path_and_query, &parsed.headers, lookup)
        .ok_or(Refusal::BadSignature)?;

    if super::keys::verify(public_pem, message.as_bytes(), &parsed.signature) {
        Ok(())
    } else {
        Err(Refusal::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> &'static crate::patch::keys::Keypair {
        use std::sync::OnceLock;
        static KEYS: OnceLock<crate::patch::keys::Keypair> = OnceLock::new();
        KEYS.get_or_init(|| crate::patch::keys::generate().expect("keygen"))
    }

    /// The header as Mastodon actually sends it: no scheme prefix, quoted
    /// values, a key id with a fragment.
    #[test]
    fn a_real_signature_header_parses() {
        let raw = "keyId=\"https://deck.example/users/ash#main-key\",\
                   algorithm=\"rsa-sha256\",\
                   headers=\"(request-target) host date digest\",\
                   signature=\"AbCd+/12==\"";
        let p = parse(raw).expect("parses");
        assert_eq!(p.key_id, "https://deck.example/users/ash#main-key");
        assert_eq!(p.algorithm.as_deref(), Some("rsa-sha256"));
        assert_eq!(p.headers, ["(request-target)", "host", "date", "digest"]);
        assert_eq!(p.signature, "AbCd+/12==");
    }

    /// A key id is a URI and may carry a query string. A naive `split(',')`
    /// survives that; a naive `split('=')` on the value does not, and neither
    /// does anything that assumes values are unquoted.
    #[test]
    fn a_key_id_with_punctuation_in_it_survives_parsing() {
        let raw = "keyId=\"https://h.example/k?a=1,b=2#main\",headers=\"date\",signature=\"zz==\"";
        let p = parse(raw).expect("parses");
        assert_eq!(p.key_id, "https://h.example/k?a=1,b=2#main");
        assert_eq!(p.signature, "zz==");
    }

    #[test]
    fn a_header_with_nothing_to_check_is_not_a_signature() {
        assert!(parse("algorithm=\"rsa-sha256\"").is_none(), "no keyId: nobody to check against");
        assert!(parse("keyId=\"https://h/k\"").is_none(), "no signature: nothing to check");
        assert!(parse("").is_none());
    }

    #[test]
    fn the_signing_string_is_the_draft_s_own_shape() {
        let covered: Vec<String> =
            ["(request-target)", "host", "date"].iter().map(|s| s.to_string()).collect();
        let s = signing_string("POST", "/users/ash/inbox", &covered, |n| match n {
            "host" => Some("deck.example".into()),
            "date" => Some("Sun, 06 Nov 1994 08:49:37 GMT".into()),
            _ => None,
        })
        .expect("builds");

        assert_eq!(
            s,
            "(request-target): post /users/ash/inbox\n\
             host: deck.example\n\
             date: Sun, 06 Nov 1994 08:49:37 GMT"
        );
    }

    /// The failure this prevents is subtle and total: a sender lists `digest`
    /// among the covered headers and does not send one. If a missing header
    /// signed as an empty string, the signature would verify and the body would
    /// be entirely unprotected.
    #[test]
    fn a_covered_header_that_was_not_sent_fails_the_string_rather_than_defaulting() {
        let covered: Vec<String> = ["host", "digest"].iter().map(|s| s.to_string()).collect();
        assert!(signing_string("post", "/inbox", &covered, |n| (n == "host")
            .then(|| "deck.example".into()))
        .is_none());
    }

    #[test]
    fn a_digest_describes_its_own_body_and_no_other() {
        let d = digest_header(b"{\"type\":\"Follow\"}");
        assert!(d.starts_with("SHA-256="));
        assert!(digest_matches(&d, b"{\"type\":\"Follow\"}"));
        assert!(!digest_matches(&d, b"{\"type\":\"Undo\"}"), "a changed body must not match");
        assert!(!digest_matches("SHA-256=nonsense", b"{\"type\":\"Follow\"}"));
        assert!(!digest_matches("", b""));
    }

    /// The algorithm name folds and the payload does not. Folding the base64
    /// would accept a digest of something else entirely.
    #[test]
    fn the_algorithm_name_folds_but_the_digest_does_not() {
        let d = digest_header(b"body");
        let (_, want) = d.split_once('=').unwrap();
        assert!(digest_matches(&format!("sha-256={want}"), b"body"));
        assert!(!digest_matches(&format!("SHA-256={}", want.to_ascii_lowercase()), b"body"));
    }

    #[test]
    fn a_clock_is_believed_in_both_directions_and_only_so_far() {
        let now = 1_760_000_000;
        assert!(date_is_fresh(&http_date(now), now));
        assert!(date_is_fresh(&http_date(now - MAX_SKEW_SECS + 1), now), "just inside is fine");
        assert!(!date_is_fresh(&http_date(now - MAX_SKEW_SECS - 1), now), "too old is replay");
        assert!(
            !date_is_fresh(&http_date(now + MAX_SKEW_SECS + 1), now),
            "too new is a broken clock, and would stay valid for as long as it is wrong"
        );
        assert!(!date_is_fresh("not a date", now));
    }

    #[test]
    fn the_date_we_send_is_the_date_we_can_read() {
        let now = 1_760_000_000;
        assert_eq!(parse_http_date(&http_date(now)), Some(now));
        assert!(http_date(now).ends_with(" GMT"));
    }

    /// The round trip: sign a request the way we send one, then believe it the
    /// way we receive one.
    #[test]
    fn a_request_we_signed_is_a_request_we_believe() {
        let k = keypair();
        let now = 1_760_000_000;
        let body = br#"{"type":"Follow"}"#;
        let sent = sign_post(
            "https://us.example/users/ash#main-key",
            &k.private_pem,
            "https://them.example/users/kim/inbox",
            body,
            now,
        )
        .expect("sign");

        let get = |name: &str| sent.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        let parsed = parse(&get("signature").unwrap()).expect("parses");

        assert_eq!(get("host").as_deref(), Some("them.example"));
        assert_eq!(
            verify_inbound("POST", "/users/kim/inbox", body, now, get, &k.public_pem, &parsed),
            Ok(())
        );
    }

    /// Each of these is a separate refusal because each needs a different fix,
    /// and reporting them as one "invalid signature" is what makes federation
    /// undebuggable.
    #[test]
    fn every_way_a_request_can_fail_to_be_believed() {
        let k = keypair();
        let now = 1_760_000_000;
        let body = br#"{"type":"Follow"}"#;
        let url = "https://them.example/users/kim/inbox";
        let sent =
            sign_post("https://us.example/users/ash#main-key", &k.private_pem, url, body, now)
                .expect("sign");
        let base = |name: &str| sent.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        let parsed = parse(&base("signature").unwrap()).expect("parses");

        // Stale: the same bytes, five minutes and one second later.
        assert_eq!(
            verify_inbound(
                "POST",
                "/users/kim/inbox",
                body,
                now + MAX_SKEW_SECS + 1,
                base,
                &k.public_pem,
                &parsed
            ),
            Err(Refusal::Stale)
        );

        // A different body under the same digest.
        assert_eq!(
            verify_inbound(
                "POST",
                "/users/kim/inbox",
                b"{\"type\":\"Undo\"}",
                now,
                base,
                &k.public_pem,
                &parsed
            ),
            Err(Refusal::Digest)
        );

        // The same signature aimed at somebody else's inbox. This is the one
        // that `(request-target)` coverage exists to stop.
        assert_eq!(
            verify_inbound("POST", "/users/eve/inbox", body, now, base, &k.public_pem, &parsed),
            Err(Refusal::BadSignature)
        );

        // Somebody else's key.
        let other = crate::patch::keys::generate().expect("keygen");
        assert_eq!(
            verify_inbound("POST", "/users/kim/inbox", body, now, base, &other.public_pem, &parsed),
            Err(Refusal::BadSignature)
        );

        // Signed, but covering only `date` — which authenticates a timestamp
        // and nothing else.
        let thin = SignatureHeader {
            key_id: parsed.key_id.clone(),
            algorithm: None,
            headers: vec!["date".into()],
            signature: parsed.signature.clone(),
        };
        assert_eq!(
            verify_inbound("POST", "/users/kim/inbox", body, now, base, &k.public_pem, &thin),
            Err(Refusal::Coverage("(request-target)"))
        );

        // Claims to cover a header it did not send.
        let overclaimed = SignatureHeader {
            headers: vec![
                "(request-target)".into(),
                "host".into(),
                "date".into(),
                "digest".into(),
                "content-type".into(),
            ],
            ..parsed.clone()
        };
        assert_eq!(
            verify_inbound(
                "POST",
                "/users/kim/inbox",
                body,
                now,
                base,
                &k.public_pem,
                &overclaimed
            ),
            Err(Refusal::MissingHeader("content-type".into()))
        );
    }

    /// A body with no `Digest` is refused on coverage rather than reaching the
    /// cryptography — the body is the whole of what an inbox acts on.
    #[test]
    fn a_body_that_nothing_covers_is_refused() {
        let k = keypair();
        let now = 1_760_000_000;
        let parsed = SignatureHeader {
            key_id: "https://us.example/users/ash#main-key".into(),
            algorithm: None,
            headers: vec!["(request-target)".into(), "host".into(), "date".into()],
            signature: "zz==".into(),
        };
        assert_eq!(
            verify_inbound(
                "POST",
                "/users/kim/inbox",
                b"{}",
                now,
                |n| match n {
                    "host" => Some("them.example".into()),
                    "date" => Some(http_date(now)),
                    _ => None,
                },
                &k.public_pem,
                &parsed
            ),
            Err(Refusal::Coverage("digest"))
        );
    }
}
