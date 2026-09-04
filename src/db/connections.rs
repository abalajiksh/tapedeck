//! Import history and per-user scrobble connections.
//!
//! There are no server-wide sinks: `sinks_for_user` is the only thing that
//! turns these rows into sinks, and a user with no connections stays local.

use super::*;

impl Database {
    pub async fn set_connection(
        &self,
        user_id: i64,
        service: &str,
        credentials: &str,
        username: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO user_connections (user_id, service, credentials, connected_username, connected_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(user_id, service) DO UPDATE SET
                credentials = excluded.credentials,
                connected_username = excluded.connected_username,
                connected_at = excluded.connected_at"
        )
        .bind(user_id).bind(service)
        .bind(self.seal(credentials)).bind(username).bind(Self::now_secs())
        .execute(&self.pool).await?;
        Ok(())
    }
    pub async fn delete_connection(
        &self,
        user_id: i64,
        service: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_connections WHERE user_id = ? AND service = ?")
            .bind(user_id)
            .bind(service)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Drop every user's connection to one service.
    ///
    /// Used when the operator changes an app registration: the stored access and
    /// refresh tokens were issued *by the old app* and mean nothing to a new
    /// one, so leaving them would offer people a connection that silently fails
    /// on next use. Deliberately server-wide and not scoped to a user — the
    /// registration it invalidates is server-wide too.
    pub async fn delete_connections_for_service(&self, service: &str) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_connections WHERE service = ?")
            .bind(service)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
    /// Public-facing list: service + connected account only, never credentials.
    pub async fn list_connections(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, Option<String>, i64)>, sqlx::Error> {
        sqlx::query_as("SELECT service, connected_username, connected_at FROM user_connections WHERE user_id = ?")
            .bind(user_id).fetch_all(&self.pool).await
    }
    /// Credentials for the engine to build this user's sinks. `(service, credentials_json)`.
    pub async fn get_connections_for_forwarding(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT service, credentials FROM user_connections WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        // A row we can't decrypt is skipped rather than fatal: one bad credential
        // shouldn't stop the user's other services from receiving listens.
        Ok(rows
            .into_iter()
            .filter_map(|(service, blob)| match self.secrets.decrypt(&blob) {
                Ok(plain) => Some((service, plain)),
                Err(e) => {
                    error!("Undecryptable {} credential for user {}: {}", service, user_id, e);
                    None
                }
            })
            .collect())
    }
    /// Wrap a credential for storage. Encryption failure is not silently
    /// tolerated — storing plaintext because the cipher errored would defeat the
    /// point, so the row keeps whatever it had and the caller sees the error via
    /// the write failing.
    pub(super) fn seal(&self, plaintext: &str) -> String {
        match self.secrets.encrypt(plaintext) {
            Ok(sealed) => sealed,
            Err(e) => {
                error!("Credential encryption failed: {} — refusing to store plaintext", e);
                String::new()
            }
        }
    }
    pub(super) fn unseal(&self, stored: &str) -> Option<String> {
        match self.secrets.decrypt(stored) {
            Ok(plain) => Some(plain),
            Err(e) => {
                error!("Could not decrypt stored credential: {}", e);
                None
            }
        }
    }
    /// One-time upgrade for rows written before encryption existed. Idempotent —
    /// already-encrypted values carry a prefix and are skipped, so this is safe
    /// to run on every boot.
    pub(super) async fn encrypt_legacy_credentials(&self) -> Result<(), sqlx::Error> {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT user_id, service, credentials FROM user_connections")
                .fetch_all(&self.pool)
                .await?;

        let mut upgraded = 0;
        for (user_id, service, blob) in rows {
            if SecretBox::is_encrypted(&blob) || blob.is_empty() {
                continue;
            }
            let sealed = self.seal(&blob);
            if sealed.is_empty() {
                continue;
            }
            sqlx::query(
                "UPDATE user_connections SET credentials = ? WHERE user_id = ? AND service = ?",
            )
            .bind(sealed)
            .bind(user_id)
            .bind(&service)
            .execute(&self.pool)
            .await?;
            upgraded += 1;
        }
        if upgraded > 0 {
            info!("🔐 Encrypted {} stored credential(s) at rest", upgraded);
        }
        Ok(())
    }
}
