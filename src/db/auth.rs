//! Password hashing, login, and browser sessions.
//!
//! The activity stamp is throttled, not written per request — see `users.rs`.

use super::*;

impl Database {
    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| format!("Password hashing failed: {}", e))
    }
    pub fn verify_password(password: &str, hash: &str) -> bool {
        if hash == "not-used-yet" {
            return false;
        }
        let Ok(parsed) = PasswordHash::new(hash) else {
            return false;
        };
        Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
    }
    /// Returns true if no user has a real password (first-run or pre-auth upgrade).
    pub async fn needs_setup(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM users WHERE password_hash != 'not-used-yet'")
                .fetch_one(&self.pool)
                .await?;
        Ok(count.0 == 0)
    }
    /// Verify username + password, return user info if valid.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<AuthUser>, sqlx::Error> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, username, password_hash, role FROM users WHERE username = ? AND disabled = 0"
        ).bind(username).fetch_optional(&self.pool).await?;

        if let Some((user_id, uname, hash, role)) = row {
            if Self::verify_password(password, &hash) {
                return Ok(Some(AuthUser { user_id, username: uname, is_admin: role == "admin" }));
            }
        }
        Ok(None)
    }
    /// Look up a user id by username (case-insensitive). Used to map a polled
    /// Plex account to the matching Tapedeck user.
    pub async fn get_user_id_by_username(
        &self,
        username: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM users WHERE LOWER(username) = LOWER(?)")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }
    /// Resolve a username to a *published* deck, or nothing.
    ///
    /// **An unknown username and a user who has not published are the same
    /// answer, deliberately.** This is the one unauthenticated read path in the
    /// application, so anything that distinguishes the two turns it into a
    /// membership oracle for a private, invite-only instance — type names at it
    /// and learn who has an account here. It is the rule
    /// `server::pairing` already follows for expired versus unknown device
    /// codes, arriving in the one other place it matters.
    ///
    /// A disabled account never publishes, whatever its switch says.
    pub async fn public_np_target(
        &self,
        username: &str,
    ) -> Result<Option<(i64, Option<String>, PublicNpFields)>, sqlx::Error> {
        let row: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, display_name, public_np_fields
               FROM users
              WHERE LOWER(username) = LOWER(?)
                AND public_now_playing = 1
                AND disabled = 0",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, name, fields)| (id, name, PublicNpFields::from_json(fields.as_deref()))))
    }
    /// The owner's own view of their publish switch.
    pub async fn get_public_np_settings(
        &self,
        user_id: i64,
    ) -> Result<(bool, PublicNpFields), sqlx::Error> {
        let row: Option<(i64, Option<String>)> =
            sqlx::query_as("SELECT public_now_playing, public_np_fields FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(match row {
            Some((on, fields)) => (on != 0, PublicNpFields::from_json(fields.as_deref())),
            None => (false, PublicNpFields::default()),
        })
    }
    /// Write the publish switch. Both halves always move together: a request
    /// that turns publishing on without saying which fields it wants gets the
    /// base payload, never the last set somebody ticked and turned off.
    pub async fn set_public_np_settings(
        &self,
        user_id: i64,
        enabled: bool,
        fields: PublicNpFields,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET public_now_playing = ?, public_np_fields = ? WHERE id = ?")
            .bind(i64::from(enabled))
            .bind(fields.to_json())
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Update the password for user_id=1 during setup.
    pub async fn setup_admin(
        &self,
        username: &str,
        display_name: Option<&str>,
        password_hash: &str,
    ) -> Result<i64, sqlx::Error> {
        let has_users = self.has_users().await?;

        if has_users {
            // Update existing user_id=1
            sqlx::query(
                "UPDATE users SET username = ?, display_name = ?, password_hash = ?, role = 'admin' WHERE id = 1"
            )
            .bind(username).bind(display_name).bind(password_hash)
            .execute(&self.pool).await?;
            Ok(1)
        } else {
            // Create new admin
            self.create_user(username, display_name, password_hash, "admin").await
        }
    }
    fn hash_session(token: &str) -> String {
        Self::hash_token(token)
    }
    /// Create a new web session. Returns the raw session token (for the cookie).
    pub async fn create_web_session(&self, user_id: i64) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 32] = rand::rng().random();
        let session_token = hex::encode(raw);
        let session_hash = Self::hash_session(&session_token);
        let now = Self::now_secs();
        let expires_at = now + 7 * 86400; // 7 days

        sqlx::query(
            "INSERT INTO web_sessions (user_id, session_hash, created_at, expires_at, last_active_at)
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind(user_id).bind(&session_hash).bind(now).bind(expires_at).bind(now)
        .execute(&self.pool).await?;

        Ok(session_token)
    }
    pub async fn validate_session(
        &self,
        session_token: &str,
    ) -> Result<Option<AuthUser>, sqlx::Error> {
        let session_hash = Self::hash_session(session_token);
        let now = Self::now_secs();

        // The stamp comes back with the row it lives on, so deciding whether
        // to write costs nothing.
        let row: Option<(i64, i64, String, String, i64)> = sqlx::query_as(
            "SELECT ws.id, u.id, u.username, u.role, ws.last_active_at FROM web_sessions ws
             JOIN users u ON ws.user_id = u.id
             WHERE ws.session_hash = ? AND ws.expires_at > ? AND u.disabled = 0",
        )
        .bind(&session_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((session_id, user_id, username, role, last_active)) = row {
            // Skipped entirely rather than made conditional in SQL: an `UPDATE`
            // that matches no rows still opens a write transaction, so the only
            // way not to pay for it is not to issue it.
            if now - last_active >= Self::ACTIVITY_STAMP_INTERVAL_SECS {
                let _ = sqlx::query("UPDATE web_sessions SET last_active_at = ? WHERE id = ?")
                    .bind(now)
                    .bind(session_id)
                    .execute(&self.pool)
                    .await;
            }
            Ok(Some(AuthUser { user_id, username, is_admin: role == "admin" }))
        } else {
            Ok(None)
        }
    }
    /// Delete a session by its raw token (logout).
    pub async fn delete_web_session(&self, session_token: &str) -> Result<(), sqlx::Error> {
        let session_hash = Self::hash_session(session_token);
        sqlx::query("DELETE FROM web_sessions WHERE session_hash = ?")
            .bind(&session_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
