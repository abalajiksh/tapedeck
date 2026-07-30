//! Encryption for credentials Tapedeck has to keep recoverable.
//!
//! Most secrets here are *hashes* — passwords (Argon2) and API tokens (SHA-256)
//! are never needed in plaintext, so they're never stored that way. But some
//! credentials must be replayed to a third party on every request: Last.fm
//! session keys, ListenBrainz tokens, Plex tokens, and a Navidrome password
//! (Subsonic hashes `password + salt` per call, so the password itself is
//! required). Those live in the database and are encrypted here.
//!
//! **Threat model, stated honestly.** The key sits on the same host as the
//! database, so this does *not* protect against host compromise — anyone who can
//! read the key can read the credentials. What it does protect is the database
//! file travelling somewhere the key doesn't: `GET /api/v1/backup` hands out a
//! whole-database snapshot, and those get copied to laptops and cloud storage.
//! The key is therefore kept outside the database and never included in a
//! backup.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    Key, XChaCha20Poly1305, XNonce,
};
use rand::rngs::OsRng;
use rand::TryRngCore;
use std::path::Path;
use tracing::{info, warn};

/// Marks a value as encrypted, and by which scheme. Plaintext left over from
/// before encryption existed has no prefix, which is how `decrypt` tells them
/// apart and why the migration can be re-run safely.
const PREFIX: &str = "tdenc:v1:";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24; // XChaCha20 — wide enough to pick nonces at random

#[derive(Clone)]
pub struct SecretBox {
    cipher: XChaCha20Poly1305,
}

impl SecretBox {
    /// Load the key from `TAPEDECK_SECRET_KEY` (base64, 32 bytes) or a keyfile,
    /// generating the keyfile on first run.
    ///
    /// Losing the key means losing every stored credential — users would have to
    /// reconnect their services. It is not recoverable from the database, which
    /// is the entire point, so the log line says where it lives.
    pub fn load(keyfile: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Ok(encoded) = std::env::var("TAPEDECK_SECRET_KEY") {
            let raw = B64.decode(encoded.trim())?;
            if raw.len() != KEY_LEN {
                return Err(format!("TAPEDECK_SECRET_KEY must decode to {KEY_LEN} bytes").into());
            }
            info!("🔐 Credential key loaded from TAPEDECK_SECRET_KEY");
            return Ok(Self::from_bytes(&raw));
        }

        if keyfile.exists() {
            let encoded = std::fs::read_to_string(keyfile)?;
            let raw = B64.decode(encoded.trim())?;
            if raw.len() != KEY_LEN {
                return Err(format!(
                    "{} is corrupt — expected {KEY_LEN} key bytes",
                    keyfile.display()
                )
                .into());
            }
            return Ok(Self::from_bytes(&raw));
        }

        let mut raw = [0u8; KEY_LEN];
        OsRng.try_fill_bytes(&mut raw)?;
        std::fs::write(keyfile, B64.encode(raw))?;
        restrict_permissions(keyfile);
        info!(
            "🔐 Generated credential key at {} — back this up separately from the database, \
               and never inside it",
            keyfile.display()
        );
        Ok(Self::from_bytes(&raw))
    }

    fn from_bytes(raw: &[u8]) -> Self {
        let key = Key::try_from(raw).expect("key length checked by callers");
        Self {
            cipher: XChaCha20Poly1305::new(&key),
        }
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String, Box<dyn std::error::Error>> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.try_fill_bytes(&mut nonce)?;
        let ciphertext = self
            .cipher
            .encrypt(
                &XNonce::try_from(&nonce[..]).expect("fixed-size nonce"),
                plaintext.as_bytes(),
            )
            .map_err(|_| "credential encryption failed")?;

        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Ok(format!("{PREFIX}{}", B64.encode(blob)))
    }

    /// Decrypt, passing through anything that isn't ours.
    ///
    /// Rows written before encryption existed are returned as-is so an upgrade
    /// keeps working while the migration catches up. That also makes the
    /// migration idempotent: a second run sees the prefix and skips.
    pub fn decrypt(&self, stored: &str) -> Result<String, Box<dyn std::error::Error>> {
        let Some(encoded) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_string());
        };
        let blob = B64.decode(encoded)?;
        if blob.len() <= NONCE_LEN {
            return Err("stored credential is truncated".into());
        }
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let plaintext = self
            .cipher
            .decrypt(
                &XNonce::try_from(nonce).map_err(|_| "bad nonce length")?,
                ciphertext,
            )
            // Almost always the wrong key rather than tampering — say so, since
            // that's the recoverable case.
            .map_err(|_| "could not decrypt credential — wrong TAPEDECK_SECRET_KEY or keyfile?")?;
        Ok(String::from_utf8(plaintext)?)
    }

    pub fn is_encrypted(stored: &str) -> bool {
        stored.starts_with(PREFIX)
    }

    /// Fixed key for tests, so a test database doesn't touch the real keyfile.
    #[cfg(test)]
    pub fn test_key() -> Self {
        Self::from_bytes(&[7u8; KEY_LEN])
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        warn!(
            "Could not restrict permissions on {}: {}",
            path.display(),
            e
        );
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_with_test_key() -> SecretBox {
        SecretBox::from_bytes(&[7u8; KEY_LEN])
    }

    #[test]
    fn round_trips() {
        let sb = box_with_test_key();
        let enc = sb.encrypt("hunter2").unwrap();
        assert!(SecretBox::is_encrypted(&enc));
        assert!(!enc.contains("hunter2"));
        assert_eq!(sb.decrypt(&enc).unwrap(), "hunter2");
    }

    #[test]
    fn same_plaintext_encrypts_differently() {
        let sb = box_with_test_key();
        // Random nonce per call, so equal credentials aren't equal ciphertext —
        // otherwise the table would leak which users share a password.
        assert_ne!(sb.encrypt("same").unwrap(), sb.encrypt("same").unwrap());
    }

    #[test]
    fn passes_through_legacy_plaintext() {
        let sb = box_with_test_key();
        let legacy = r#"{"token":"abc"}"#;
        assert_eq!(sb.decrypt(legacy).unwrap(), legacy);
    }

    #[test]
    fn wrong_key_fails_closed() {
        let enc = box_with_test_key().encrypt("secret").unwrap();
        let other = SecretBox::from_bytes(&[9u8; KEY_LEN]);
        assert!(other.decrypt(&enc).is_err());
    }

    #[test]
    fn rejects_tampering() {
        let sb = box_with_test_key();
        let enc = sb.encrypt("secret").unwrap();
        let mut bytes = B64.decode(enc.strip_prefix(PREFIX).unwrap()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(sb
            .decrypt(&format!("{PREFIX}{}", B64.encode(&bytes)))
            .is_err());
    }
}
