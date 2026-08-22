// src/patch/keys.rs — the keypair that *is* an identity on the fediverse
//
// A remote server does not know what a Tapedeck user is. What it knows is a
// public key it fetched from an actor document, and whether the bytes of a
// request verify against it. So this key is not a credential protecting an
// account — it is the account, as far as anyone else is concerned, and losing
// it means every server holding the actor stops believing anything it says.
//
// **RSA-2048, not Ed25519.** Ed25519 is smaller, faster and better, and it is
// not yet safe to assume: the deployed fediverse verifies `rsa-sha256` and a
// large part of it understands nothing else. This is a case where the right
// cryptographic choice is the wrong interoperability one.

use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs1v15::{Signature, SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use std::error::Error;

use base64::{engine::general_purpose::STANDARD as B64, Engine};

/// What the fediverse expects to find in an actor document, and what a
/// signature is made with. Both halves are PEM because that is the form the
/// `publicKeyPem` field carries — there is no binary encoding to choose here.
pub struct Keypair {
    /// SPKI (`BEGIN PUBLIC KEY`). Published.
    pub public_pem: String,
    /// PKCS#8 (`BEGIN PRIVATE KEY`). Sealed before it reaches a column.
    pub private_pem: String,
}

/// Bits, stated once. 2048 is the floor the fediverse actually accepts; 4096
/// verifies slower on every delivery for a margin nobody is asking for.
const BITS: usize = 2048;

/// Generate a fresh identity.
///
/// **This blocks for a noticeable fraction of a second** (longer in a debug
/// build — prime search is the whole cost), so callers run it on a blocking
/// thread rather than holding an async worker. It is called once per user, ever.
pub fn generate() -> Result<Keypair, Box<dyn Error>> {
    // `rsa::rand_core`, not `rand::rngs` — this tree is on rand 0.9 and rsa 0.9
    // is built against rand_core 0.6, so the obvious import does not satisfy
    // the trait bound. Reaching through the re-export is what keeps the two
    // versions from having to agree.
    let mut rng = rsa::rand_core::OsRng;
    let private = RsaPrivateKey::new(&mut rng, BITS)?;
    let public = RsaPublicKey::from(&private);

    Ok(Keypair {
        public_pem: public.to_public_key_pem(LineEnding::LF)?,
        private_pem: private.to_pkcs8_pem(LineEnding::LF)?.to_string(),
    })
}

/// Sign the signing string of an outbound request. Base64, standard alphabet
/// with padding — the `signature=` parameter of an HTTP Signature is specified
/// that way, and the URL-safe alphabet used elsewhere in this codebase for PKCE
/// would be quietly rejected by every verifier.
#[allow(dead_code)] // the signing half: outbound delivery is Step 3. See `signatures::sign_post`.
pub fn sign(private_pem: &str, message: &[u8]) -> Result<String, Box<dyn Error>> {
    let key = RsaPrivateKey::from_pkcs8_pem(private_pem)?;
    let signing_key = SigningKey::<Sha256>::new(key);
    let signature = signing_key.try_sign(message)?;
    Ok(B64.encode(signature.to_bytes()))
}

/// Check a signature against a published key.
///
/// Returns a plain `bool` rather than a `Result` at the call site's insistence:
/// a malformed key, a malformed signature and a wrong signature are all *"do
/// not believe this request"*, and giving them separate outcomes invites a
/// caller to handle one of them as something other than a refusal.
pub fn verify(public_pem: &str, message: &[u8], signature_b64: &str) -> bool {
    let Some(key) = parse_public_key(public_pem) else {
        return false;
    };
    let Ok(raw) = B64.decode(signature_b64.trim()) else {
        return false;
    };
    let Ok(signature) = Signature::try_from(raw.as_slice()) else {
        return false;
    };
    VerifyingKey::<Sha256>::new(key).verify(message, &signature).is_ok()
}

/// **Both PEM spellings are accepted, and that is not politeness.** SPKI
/// (`BEGIN PUBLIC KEY`) is what Mastodon and this implementation publish, but
/// PKCS#1 (`BEGIN RSA PUBLIC KEY`) is on the wire from other software — and
/// reading only one of them fails as *"signature did not verify"*, which is
/// indistinguishable from a forgery and sends whoever debugs it looking at the
/// signing string instead of at the key.
///
/// The `trim` is there for the same reason and was earned the same way. The
/// PKCS#8 parser is strict about what follows the final `-----`, so a key
/// carrying one stray blank line — an ordinary thing to find in a value that
/// has been through a config file, a database export or a copy-paste — does
/// not parse, and *that* failure is reported as `BadSignature` as well. A whole
/// peer would become permanently unbelievable over a whitespace character,
/// with the logs at both ends saying forgery.
fn parse_public_key(pem: &str) -> Option<RsaPublicKey> {
    use rsa::pkcs8::DecodePublicKey;
    let pem = pem.trim();
    RsaPublicKey::from_public_key_pem(pem).ok().or_else(|| RsaPublicKey::from_pkcs1_pem(pem).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real key, published by a real Mastodon instance — the exact bytes
    /// `mastodon.social` served for `/users/Mastodon` on 2026-08-22, captured
    /// by the inbox's own fetch path rather than typed in.
    ///
    /// Pinned because `verify` returns a bare `false` both for a key it could
    /// not parse *and* for a signature that did not match. That is right at the
    /// call site — both mean "do not believe this" — and useless here: without
    /// this test, "we cannot read Mastodon's keys at all" and "that particular
    /// signature was forged" are the same observation, and the first would
    /// present as federation quietly never working.
    #[test]
    fn a_real_mastodon_key_parses() {
        const REAL: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAtpNfuGPl/WTnSq3dTurF\nMRelAIdvGVkO/VKYZJvIleYA27/YTnpmlY2g+0az4xEhOBtVNA1cTpS63CdXRyNz\ncH/GZtzxkdxN91vZSw0JVy+wG34dzwcq1KWFDz9D/5Tqf16KUJH+TDTlxdOBds91\nIZg+TTkiT+xfnSiC5SLMnn1dTzCW9P0yNJxpn37z7p6pEs63X1wstEEX1qGDUQTO\n1JICpKDjuQZMlioAAA5KG25tg2f+zKlv5M/NI33DblquyJ7TYvIpDN8hsFCRjuvA\nmjtKz/1XIRvQkeKND3UkqX8s6qTGyNOjcT86qt9BqYHYGuppjpRG/QNGoKYalio1\nwwIDAQAB\n-----END PUBLIC KEY-----\n";
        assert!(
            parse_public_key(REAL).is_some(),
            "SPKI in the spelling the fediverse actually publishes"
        );
        // Read well enough to refuse a forgery against, rather than silently
        // unreadable.
        assert!(!verify(REAL, b"anything", "AAAA"));

        // A stray blank line is an ordinary thing to find in a key that has
        // been through a config file or an export, and the strict parser
        // refuses it — reporting a whitespace character as a forged signature.
        assert!(
            parse_public_key(&format!("{REAL}\n")).is_some(),
            "a trailing blank line must not make a peer permanently unbelievable"
        );
        assert!(parse_public_key(&format!("  \n{REAL}  \n\n")).is_some());
    }

    /// Generating is slow enough that every test here shares one keypair.
    fn keys() -> &'static Keypair {
        use std::sync::OnceLock;
        static KEYS: OnceLock<Keypair> = OnceLock::new();
        KEYS.get_or_init(|| generate().expect("keygen"))
    }

    #[test]
    fn a_generated_key_is_the_pem_the_fediverse_reads() {
        let k = keys();
        assert!(
            k.public_pem.starts_with("-----BEGIN PUBLIC KEY-----"),
            "the published half must be SPKI: it goes into `publicKeyPem` verbatim"
        );
        assert!(
            k.private_pem.starts_with("-----BEGIN PRIVATE KEY-----"),
            "PKCS#8 for the private half"
        );
    }

    #[test]
    fn a_signature_verifies_against_its_own_key_and_nothing_else() {
        let k = keys();
        let msg = b"(request-target): post /users/ash/inbox";
        let sig = sign(&k.private_pem, msg).expect("sign");

        assert!(verify(&k.public_pem, msg, &sig));
        assert!(
            !verify(&k.public_pem, b"(request-target): post /users/eve/inbox", &sig),
            "a different message must not verify"
        );

        let other = generate().expect("keygen");
        assert!(
            !verify(&other.public_pem, msg, &sig),
            "somebody else's key must not verify our signature"
        );
    }

    /// Every one of these is a refusal, and the point of the test is that none
    /// of them is anything *else* — a panic here would be a remote server
    /// taking this process down with a malformed header.
    #[test]
    fn garbage_is_refused_rather_than_fatal() {
        let k = keys();
        let sig = sign(&k.private_pem, b"hello").expect("sign");

        assert!(!verify("not a pem at all", b"hello", &sig));
        assert!(!verify(&k.public_pem, b"hello", "!!! not base64 !!!"));
        assert!(
            !verify(&k.public_pem, b"hello", &B64.encode([0u8; 16])),
            "a short signature is refused, not unwrapped"
        );
        assert!(!verify("", b"hello", ""));
    }
}
