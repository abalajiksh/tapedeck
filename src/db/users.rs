//! Accounts, sessions, tokens, device pairing, MCP connections and OAuth 2.1.
//!
//! Two rules that must not drift: an activity stamp is throttled to
//! `ACTIVITY_STAMP_INTERVAL_SECS` rather than written on every authenticated
//! request (v0.92.1 — it was a write per request, and nothing reads the session
//! stamp at all), and `TokenAuth::has_scope` matches whole tokens so a scope of
//! `rewrite` does not grant `write`.

use super::*;

impl Database {
    pub(super) fn hash_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    }
    pub fn now_secs() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }
    /// Resolve a raw API token to its owner **and** the signal chain that token
    /// defaults to. The chain rides along because the ingest path needs it on
    /// every submission and this is already the one query that touches the row.
    pub async fn validate_token(&self, token: &str) -> Result<Option<TokenAuth>, sqlx::Error> {
        let token_hash = Self::hash_token(token);
        let now = Self::now_secs();
        #[allow(clippy::type_complexity)]
        let row: Option<(
            i64,
            String,
            String,
            i64,
            Option<i64>,
            Option<String>,
            Option<bool>,
            Option<i64>,
        )> = sqlx::query_as(&crate::sql!("SELECT u.id, u.username, u.role, t.id, t.default_chain_id, t.scopes,
                        t.imports_are_live, t.last_used_at
             FROM tokens t JOIN users u ON t.user_id = u.id
             WHERE t.token_hash = ? AND (t.expires_at IS NULL OR t.expires_at > ?) AND u.disabled = 0"))
        .bind(&token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((
            user_id,
            username,
            role,
            token_id,
            default_chain_id,
            scopes,
            imports_are_live,
            last_used,
        )) = row
        {
            // Same throttle as the session stamp. Settings shows this as a
            // relative time ("2 hours ago"), so five minutes of slack costs the
            // reader nothing — and a scrobble client submitting every few
            // minutes stops taking the write lock for it every time.
            if last_used.is_none_or(|t| now - t >= Self::ACTIVITY_STAMP_INTERVAL_SECS) {
                let _ = sqlx::query(&crate::sql!(
                    "UPDATE tokens SET last_used_at = ? WHERE token_hash = ?"
                ))
                .bind(now)
                .bind(&token_hash)
                .execute(&self.pool)
                .await;
            }
            Ok(Some(TokenAuth {
                user: AuthUser { user_id, username, is_admin: role == "admin" },
                token_id,
                default_chain_id,
                scopes: scopes.unwrap_or_default(),
                imports_are_live: imports_are_live.unwrap_or(false),
            }))
        } else {
            Ok(None)
        }
    }
    /// Create an account.
    ///
    /// **`share_all_with_patched` starts on, and it is written here rather than
    /// left to the column default.** The column defaults to `0` because an
    /// account that predates it accepted its patches under the old rule — an
    /// unshared chain published nothing — and nobody consents to more by their
    /// operator upgrading a binary. That obligation is to *those* patches and
    /// it expires: it is no reason for an account created afterwards, whose
    /// every patch will be accepted under this rule, to start closed.
    ///
    /// Setting it at the insert rather than moving the column default is what
    /// keeps the two kinds of database agreeing. `init()` creates the column
    /// with `DEFAULT 0` and `migrate()` adds it with `DEFAULT 0`, so flipping
    /// the default in `init()` would make a fresh instance and a migrated one
    /// behave differently for every account created after the upgrade — the
    /// worst possible split, because it depends on when the file was made
    /// rather than on anything anybody decided.
    ///
    /// It widens what a **patched** deck sees and nothing else: the Instance
    /// tab, Discover and a Shared Spool's captures are all still narrowed to
    /// shared chains. See `PATCH_VISIBLE_LISTENS`.
    pub async fn create_user(
        &self,
        username: &str,
        display_name: Option<&str>,
        password_hash: &str,
        role: &str,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let result = sqlx::query_as::<_, (i64,)>(&crate::sql!(
            "INSERT INTO users (username, display_name, password_hash, role, created_at, \
                                share_all_with_patched) VALUES (?, ?, ?, ?, ?, 1) RETURNING id",
        ))
        .bind(username)
        .bind(display_name)
        .bind(password_hash)
        .bind(role)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        result.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    /// Returns (role, disabled) for a user, if they exist.
    pub async fn get_user_role_disabled(
        &self,
        id: i64,
    ) -> Result<Option<(String, bool)>, sqlx::Error> {
        let row: Option<(String, bool)> =
            sqlx::query_as(&crate::sql!("SELECT role, disabled FROM users WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }
    /// Count admins who can actually log in (enabled). Used to guard against
    /// leaving the instance with no reachable admin.
    pub async fn count_active_admins(&self) -> Result<i64, sqlx::Error> {
        let c: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0",
        ))
        .fetch_one(&self.pool)
        .await?;
        Ok(c.0)
    }
    pub async fn update_user_role(&self, id: i64, role: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("UPDATE users SET role = ? WHERE id = ?"))
            .bind(role)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn set_user_disabled(&self, id: i64, disabled: bool) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("UPDATE users SET disabled = ? WHERE id = ?"))
            .bind(disabled)
            .bind(id)
            .execute(&self.pool)
            .await?;
        // Disabling a user invalidates their live web sessions immediately.
        if disabled {
            let _ = sqlx::query(&crate::sql!("DELETE FROM web_sessions WHERE user_id = ?"))
                .bind(id)
                .execute(&self.pool)
                .await;
        }
        Ok(res.rows_affected() > 0)
    }
    pub async fn update_user_password(
        &self,
        id: i64,
        password_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("UPDATE users SET password_hash = ? WHERE id = ?"))
            .bind(password_hash)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Drop every browser session a user holds.
    ///
    /// Called on a password change: that is normally a reaction to "someone
    /// else may have access", and a new password that leaves the old sessions
    /// logged in does not actually take anything away from them.
    pub async fn clear_user_sessions(&self, id: i64) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("DELETE FROM web_sessions WHERE user_id = ?"))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
    /// A user's own profile, for the account screen.
    pub async fn get_profile(&self, id: i64) -> Result<Option<UserProfile>, sqlx::Error> {
        sqlx::query_as::<_, UserProfile>(&crate::sql!(
            "SELECT id, username, display_name, role, created_at, avatar_url, bio, disabled
               FROM users WHERE id = ?",
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
    /// Update the parts of a profile the owner controls.
    ///
    /// Each field is `Option<Option<..>>` for the reason recorded against
    /// `bulk-chain` and `default_chain_id`: on an endpoint where a field
    /// chooses between "change this" and "leave it alone", a bare `Option`
    /// cannot tell absent from null, and a misspelled key silently clears
    /// whatever it failed to name. Absent leaves alone; explicit null clears.
    ///
    /// Note what is *not* here: `role`, `disabled`, `username` and
    /// `password_hash`. Those are either admin decisions or need the current
    /// password, and neither belongs on the same call as a bio.
    pub async fn update_profile(
        &self,
        id: i64,
        display_name: Option<Option<String>>,
        bio: Option<Option<String>>,
        avatar_url: Option<Option<String>>,
    ) -> Result<bool, sqlx::Error> {
        let mut changed = false;
        if let Some(v) = display_name {
            sqlx::query(&crate::sql!("UPDATE users SET display_name = ? WHERE id = ?"))
                .bind(v)
                .bind(id)
                .execute(&self.pool)
                .await?;
            changed = true;
        }
        if let Some(v) = bio {
            sqlx::query(&crate::sql!("UPDATE users SET bio = ? WHERE id = ?"))
                .bind(v)
                .bind(id)
                .execute(&self.pool)
                .await?;
            changed = true;
        }
        if let Some(v) = avatar_url {
            sqlx::query(&crate::sql!("UPDATE users SET avatar_url = ? WHERE id = ?"))
                .bind(v)
                .bind(id)
                .execute(&self.pool)
                .await?;
            changed = true;
        }
        Ok(changed)
    }
    /// Verify a password against the stored hash for one user.
    ///
    /// This is what makes "change my password" different from an admin reset:
    /// the caller already holds a session, so without it a borrowed browser is
    /// enough to lock the owner out of their own account.
    pub async fn check_user_password(&self, id: i64, password: &str) -> bool {
        let row: Result<(String,), _> = sqlx::query_as(&crate::sql!(
            "SELECT password_hash FROM users WHERE id = ? AND disabled = 0"
        ))
        .bind(id)
        .fetch_one(&self.pool)
        .await;
        matches!(row, Ok((hash,)) if Self::verify_password(password, &hash))
    }
    /// Delete a user and everything they own, in one transaction. Every listen,
    /// token, chain, device, session, binding, and import record scoped to the
    /// user is removed first so foreign keys stay consistent.
    pub async fn delete_user(&self, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Revisions reference notes, and notes reference scrobbles, so both have
        // to be cleared before the rows they point at. Entities are shared
        // metadata and stay; the loves and notes that pointed at them go.
        sqlx::query(&crate::sql!(
            "DELETE FROM note_revisions WHERE note_id IN (SELECT id FROM notes WHERE user_id = ?)",
        ))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        // Credits point at listens, so they go before them — the same rule as
        // the notes above, and the one the loop below cannot express because
        // this table has no `user_id` of its own.
        sqlx::query(&crate::sql!(
            "DELETE FROM scrobble_artists WHERE scrobble_id IN
                 (SELECT id FROM scrobbles WHERE user_id = ?)"
        ))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        // Playlist tracks point at playlists and have no `user_id` of their
        // own, so they cannot go in the loop below — the same shape as
        // `scrobble_artists` above, and the same reason.
        sqlx::query(&crate::sql!(
            "DELETE FROM playlist_tracks WHERE playlist_id IN
                 (SELECT id FROM playlists WHERE user_id = ?)"
        ))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        for table in [
            "notes",
            "loves",
            // The air log points at a radio session, so it clears first.
            "radio_air_log",
            "radio_sessions",
            "radio_presets",
            "scrobbles",
            // The MCP tables clear before `users`, and in this order: audit
            // rows and playlists both reference a connection, so the
            // connection cannot go first.
            "mcp_audit",
            "playlists",
            // References mcp_connections, so it clears before them.
            "recommendations",
            "mcp_connections",
            "oauth_codes",
            // The shelf goes before `equipment` and `signal_chains`, which both
            // of these point at — a play records the turntable it ran on, and a
            // pressing records what it is normally played through. Clearing the
            // gear first is a foreign-key violation, which with `physical_plays`
            // absent from this list is what deleting anyone with a shelf did.
            "physical_images",
            "physical_plays",
            // Points at a release, a chain and a piece of gear, so it clears
            // before all three — same trap `physical_plays` was missing from.
            "physical_spins",
            "physical_releases",
            // A session points at the token, device and chain it ran
            // through, so it clears before all three. It used to come after
            // them, and deleting anyone who had ever submitted with a token
            // failed on the foreign key.
            "sessions",
            "tokens",
            "signal_chains",
            "devices",
            "equipment",
            "chain_bindings",
            "import_jobs",
            "web_sessions",
            "user_connections",
            "artist_pictures",
        ] {
            sqlx::query(&crate::sql!(&format!("DELETE FROM {} WHERE user_id = ?", table)))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let res = sqlx::query(&crate::sql!("DELETE FROM users WHERE id = ?"))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }
    /// Start a pairing. Returns `(device_code, user_code)`.
    ///
    /// The device code is the bearer secret the client polls with and is stored
    /// hashed; the user code is what a human carries between two screens.
    ///
    /// `started_by` is the account whose session asked, when one did. Only
    /// that account may approve it: approving mints a token for the
    /// approver and hands it to whoever holds the device code, so a code one
    /// account started and another approved is the second account's
    /// credential delivered to the first — the Strix scan of v0.120.0
    /// collected an admin token that way.
    pub async fn start_pairing(
        &self,
        client_name: &str,
        scopes: &str,
        started_by: Option<i64>,
    ) -> Result<(String, String), sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let device_code = format!("tdp_{}", hex::encode(raw));

        // No vowels and no 0/O/1/I/L: the whole point is reading it off one
        // screen and typing it into another without a transcription error, and
        // excluding those also means it cannot accidentally spell anything.
        const ALPHABET: &[u8] = b"BCDFGHJKMNPQRSTVWXYZ23456789";
        let now = Self::now_secs();
        // Retry on the (vanishingly unlikely) collision rather than failing the
        // request — the unique index is what makes that safe.
        for _ in 0..8 {
            let code: String = (0..8)
                .map(|i| {
                    let c = ALPHABET[rand::rng().random_range(0..ALPHABET.len())] as char;
                    if i == 4 {
                        format!("-{c}")
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            let res = sqlx::query(&crate::sql!(
                "INSERT INTO pairing_requests
                    (device_code_hash, user_code, client_name, scopes, user_id, created_at,
                     expires_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            ))
            .bind(Self::hash_token(&device_code))
            .bind(&code)
            .bind(client_name)
            .bind(scopes)
            .bind(started_by)
            .bind(now)
            .bind(now + Self::PAIRING_TTL_SECS)
            .execute(&self.pool)
            .await;
            match res {
                Ok(_) => return Ok((device_code, code)),
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(e) => return Err(e),
            }
        }
        Err(sqlx::Error::Protocol("could not allocate a pairing code".into()))
    }
    /// What a user code refers to, for the approval screen. Expired requests
    /// are not returned.
    pub async fn pairing_by_user_code(
        &self,
        user_code: &str,
    ) -> Result<Option<PairingDescription>, sqlx::Error> {
        #[allow(clippy::type_complexity)]
        let row: Option<(String, String, Option<i64>, Option<i64>, Option<String>, i64)> =
            sqlx::query_as(&crate::sql!(
                "SELECT p.client_name, p.scopes, p.approved_at, p.user_id, u.username,
                        p.created_at
                   FROM pairing_requests p LEFT JOIN users u ON u.id = p.user_id
                  WHERE p.user_code = ? AND p.expires_at > ?",
            ))
            .bind(user_code.trim().to_uppercase())
            .bind(Self::now_secs())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(client_name, scopes, approved_at, user_id, username, created_at)| {
            // Approval overwrites `user_id` with the approver, so it only
            // names the starter while the request is still pending.
            let pending = approved_at.is_none();
            PairingDescription {
                client_name,
                scopes,
                approved: !pending,
                started_by: user_id.filter(|_| pending),
                started_by_name: username.filter(|_| pending),
                created_at,
            }
        }))
    }
    /// Approve a pairing and mint its token.
    ///
    /// The token is stored on the request for the client to collect exactly
    /// once. Unavailable when the code is unknown, expired, or already
    /// approved — a second approval must not mint a second token — and
    /// refused when another account's session started it.
    pub async fn approve_pairing(
        &self,
        user_code: &str,
        user_id: i64,
        default_chain_id: Option<i64>,
    ) -> Result<PairingApproval, sqlx::Error> {
        let now = Self::now_secs();
        let row: Option<(i64, String, String, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT id, client_name, scopes, user_id FROM pairing_requests
              WHERE user_code = ? AND expires_at > ? AND approved_at IS NULL"
        ))
        .bind(user_code.trim().to_uppercase())
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, client_name, scopes, started_by)) = row else {
            return Ok(PairingApproval::Unavailable);
        };
        if started_by.is_some_and(|starter| starter != user_id) {
            return Ok(PairingApproval::OtherAccount);
        }

        // A paired client starts as a backfill source like any other; the
        // operator flips it live from Settings once they know what it is.
        let token =
            self.create_token(user_id, &client_name, &scopes, default_chain_id, false).await?;
        sqlx::query(&crate::sql!(
            "UPDATE pairing_requests
                SET user_id = ?, default_chain_id = ?, token = ?, approved_at = ?
              WHERE id = ? AND approved_at IS NULL"
        ))
        .bind(user_id)
        .bind(default_chain_id)
        .bind(&token)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(PairingApproval::Approved)
    }
    /// What the polling client should be told.
    pub async fn poll_pairing(&self, device_code: &str) -> Result<PairingPoll, sqlx::Error> {
        let hash = Self::hash_token(device_code);
        let row: Option<(i64, Option<String>, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT id, token, expires_at FROM pairing_requests WHERE device_code_hash = ?",
        ))
        .bind(&hash)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, token, expires_at)) = row else {
            return Ok(PairingPoll::Unknown);
        };
        if let Some(token) = token {
            // Handed over once, then cleared: the request row must not remain a
            // second copy of a live credential.
            sqlx::query(&crate::sql!("DELETE FROM pairing_requests WHERE id = ?"))
                .bind(id)
                .execute(&self.pool)
                .await?;
            return Ok(PairingPoll::Approved(token));
        }
        if expires_at <= Self::now_secs() {
            return Ok(PairingPoll::Expired);
        }
        Ok(PairingPoll::Pending)
    }
    /// Drop expired, unapproved requests. Cheap, and keeps a guessable code
    /// from lingering.
    pub async fn purge_expired_pairings(&self) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "DELETE FROM pairing_requests WHERE expires_at <= ? AND token IS NULL",
        ))
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
    pub async fn create_token(
        &self,
        user_id: i64,
        name: &str,
        scopes: &str,
        default_chain_id: Option<i64>,
        imports_are_live: bool,
    ) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let token_hash = Self::hash_token(&token);
        let now = Self::now_secs();
        sqlx::query(&crate::sql!("INSERT INTO tokens (user_id, token_hash, name, scopes, created_at, default_chain_id, imports_are_live) VALUES (?, ?, ?, ?, ?, ?, ?)"))
            .bind(user_id).bind(&token_hash).bind(name).bind(scopes).bind(now)
            .bind(default_chain_id)
            .bind(imports_are_live)
            .execute(&self.pool).await?;
        info!("🔑 Created token '{}' for user_id {}", name, user_id);
        Ok(token)
    }
    /// Rename a token or repoint its default chain. The token itself is never
    /// re-issued — the whole point is that you can fix the chain after the fact
    /// without touching the client you already pasted it into.
    pub async fn update_token(
        &self,
        user_id: i64,
        token_id: i64,
        name: &str,
        default_chain_id: Option<i64>,
        imports_are_live: bool,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(&crate::sql!(
            "UPDATE tokens SET name = ?, default_chain_id = ?, imports_are_live = ?
             WHERE id = ? AND user_id = ?",
        ))
        .bind(name)
        .bind(default_chain_id)
        .bind(imports_are_live)
        .bind(token_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
    /// Re-issue a token in place: same id, name, scopes and chain, new secret.
    /// Only the hash is stored, so a lost token can't be shown again — this is
    /// the recovery path instead. Everything referencing the token id (sessions,
    /// the default chain) keeps pointing at the same row.
    pub async fn rotate_token(
        &self,
        user_id: i64,
        token_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let result = sqlx::query(&crate::sql!(
            "UPDATE tokens SET token_hash = ? WHERE id = ? AND user_id = ?"
        ))
        .bind(Self::hash_token(&token))
        .bind(token_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        info!("🔑 Rotated token {} for user_id {}", token_id, user_id);
        Ok(Some(token))
    }
    /// Revoke a token. Its sessions are the user's listening history and stay;
    /// they stop naming the token, which is what lets the row go — a bare
    /// delete failed on the foreign key for every token that had ever
    /// submitted, and left it authenticating.
    pub async fn delete_token(&self, user_id: i64, token_id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!(
            "UPDATE sessions SET token_id = NULL WHERE token_id = ? AND user_id = ?"
        ))
        .bind(token_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(&crate::sql!("DELETE FROM tokens WHERE id = ? AND user_id = ?"))
            .bind(token_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }
    pub async fn has_users(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM users"))
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0 > 0)
    }
    pub async fn list_users(&self) -> Result<Vec<UserRecord>, sqlx::Error> {
        sqlx::query_as(&crate::sql!("SELECT id, username, display_name, role, created_at, disabled FROM users ORDER BY id ASC"))
            .fetch_all(&self.pool).await
    }
    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<TokenRecord>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT t.id, t.name, t.scopes, t.created_at, t.last_used_at, t.default_chain_id,
                    t.imports_are_live, c.name AS default_chain_name
             FROM tokens t LEFT JOIN signal_chains c ON c.id = t.default_chain_id
             WHERE t.user_id = ? ORDER BY t.created_at ASC",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// The shape of a history, and how much of it carries each kind of
    /// metadata.
    ///
    /// This exists for one reader — the MCP surface's `listening_profile`, the
    /// tool a model is told to call before it says anything with a number in
    /// it — and it is the single most useful query on that surface. Not because
    /// the totals are interesting, but because **the coverage figures are the
    /// only thing standing between a sparse column and a confident sentence
    /// about it.**
    ///
    /// The lesson is v0.41.2's, arriving somewhere it can be spoken aloud.
    /// Format is known for 2.6% of a real imported history, so a lossless share
    /// computed over every play was reporting the import ratio while looking
    /// like a verdict on the listening. A card can be made to confess that; a
    /// chatbot cannot be, unless it is handed the denominator.
    ///
    /// Every count here shares one population — `skipped = FALSE`, the rule
    /// every counting query in this file follows — so the fractions are
    /// fractions of the same whole. Paired counts that do not share a
    /// population is the bug this codebase has now found five times.
    pub async fn listening_coverage(&self, user_id: i64) -> Result<ListeningCoverage, sqlx::Error> {
        let row: ListeningCoverage = sqlx::query_as(&crate::sql!(&format!(
            "SELECT
                COUNT(*) AS plays,
                COUNT(DISTINCT LOWER(TRIM(artist))) AS artists,
                COUNT(DISTINCT LOWER(TRIM(album))) AS albums,
                MIN(timestamp) AS first_listen,
                MAX(timestamp) AS last_listen,
                SUM(CASE WHEN mbid_recording IS NOT NULL AND mbid_recording != ''
                         THEN 1 ELSE 0 END) AS with_mbid,
                -- The empty string is the tombstone -- asked, and this
                -- recording has none -- so it is not coverage. Counting it
                -- would report a track nobody can look up as identified.
                SUM(CASE WHEN isrc IS NOT NULL AND isrc != '' THEN 1 ELSE 0 END) AS with_isrc,
                SUM(CASE WHEN {fidelity} THEN 1 ELSE 0 END) AS with_format,
                SUM(CASE WHEN duration IS NOT NULL THEN 1 ELSE 0 END) AS with_duration,
                SUM(CASE WHEN chain_id IS NOT NULL THEN 1 ELSE 0 END) AS with_chain,
                SUM(CASE WHEN quality_score IS NOT NULL THEN 1 ELSE 0 END) AS with_quality
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
            fidelity = FIDELITY_KNOWN
        )))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }
    /// Genre coverage is a separate query because genres live on the *artist*,
    /// not the listen — which is also the reason a genre position is
    /// artist-level and the MCP instructions say so.
    pub async fn genre_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let row: (i64, i64) = sqlx::query_as(&crate::sql!(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN a.genres IS NOT NULL AND a.genres != '[]' THEN 1 ELSE 0 END)
             FROM (SELECT DISTINCT LOWER(TRIM(artist)) AS k FROM scrobbles
                   WHERE user_id = ? AND skipped = FALSE) s
             LEFT JOIN artists a ON LOWER(TRIM(a.name)) = s.k"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }
    /// Where the listens came from, biggest first. The honest answer to "is
    /// this history imported or observed?", which decides how much of the rest
    /// of the profile can be trusted.
    pub async fn source_breakdown(&self, user_id: i64) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT source_name, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY source_name ORDER BY plays DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Mint a connection. Returns the bearer secret and, when the grant came
    /// through OAuth, a refresh secret — both plaintext, both returned exactly
    /// once and stored only as hashes.
    ///
    /// `ttl_secs` is `None` for a connection created by hand in Settings: it is
    /// a credential the user pasted somewhere and expiring it silently would
    /// break their client on a random Tuesday for no security gain they asked
    /// for. An OAuth grant expires, because the client can refresh it without
    /// anybody being present.
    ///
    /// **`fetch_all`, not `fetch_one` — and that is a correctness fix, not a
    /// style choice.** sqlx steps a statement only as far as the rows the
    /// caller asks for, so `fetch_one` on an `INSERT … RETURNING` stops at the
    /// first row and never reaches `SQLITE_DONE`. The implicit write
    /// transaction then stays open until that pooled connection happens to run
    /// something else — and until it does, **the row is invisible to every
    /// other connection in the pool**.
    ///
    /// Measured, not theorised: 226 of 300 inserts were missing from a read
    /// issued immediately afterwards. `fetch_all` and plain `execute` both lost
    /// none. Nothing is *lost* — the write commits when that connection is next
    /// used — but "create it, then read it back" is exactly the shape that
    /// misses, which for a freshly minted token means a client's first request
    /// getting a spurious 401.
    ///
    /// `a_row_written_with_returning_is_visible_immediately` pins it.
    pub async fn create_mcp_connection(
        &self,
        user_id: i64,
        name: &str,
        scopes: &str,
        client_id: Option<&str>,
        ttl_secs: Option<i64>,
    ) -> Result<McpCredential, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 32] = rand::rng().random();
        let token = format!("tdmcp_{}", hex::encode(raw));
        let now = Self::now_secs();

        // Only an OAuth grant gets a refresh secret. A pasted credential has
        // nothing to refresh *to* — it never expires — and issuing one would
        // be a second live secret for the same access with no use for it.
        let refresh = ttl_secs.map(|_| {
            let raw: [u8; 32] = rand::rng().random();
            format!("tdmcpr_{}", hex::encode(raw))
        });

        let id: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO mcp_connections
                (user_id, name, token_hash, scopes, client_id, created_at, expires_at, refresh_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(name)
        .bind(Self::hash_token(&token))
        .bind(scopes)
        .bind(client_id)
        .bind(now)
        .bind(ttl_secs.map(|t| now + t))
        .bind(refresh.as_deref().map(Self::hash_token))
        .fetch_all(&self.pool)
        .await?;

        info!("🤖 MCP connection '{}' created for user_id {} [{}]", name, user_id, scopes);
        let id = id.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)?;
        Ok(McpCredential { id, token, refresh, expires_in: ttl_secs })
    }
    /// Resolve a bearer secret to its connection, or `None`.
    ///
    /// Expiry and revocation are checked **here** rather than by the caller, so
    /// there is one place a dead credential can be let through. `last_used_at`
    /// is written on every successful call — it is the column the connections
    /// screen reads to answer "is this thing still talking to me?".
    pub async fn validate_mcp_token(&self, token: &str) -> Result<Option<McpAuth>, sqlx::Error> {
        let hash = Self::hash_token(token);
        let now = Self::now_secs();
        let row: Option<(i64, i64, String, String, String, bool, Option<i64>, Option<i64>)> =
            sqlx::query_as(&crate::sql!(
                "SELECT c.id, u.id, u.username, u.role, c.scopes, u.disabled,
                        c.expires_at, c.revoked_at
                 FROM mcp_connections c JOIN users u ON u.id = c.user_id
                 WHERE c.token_hash = ?"
            ))
            .bind(&hash)
            .fetch_optional(&self.pool)
            .await?;

        let Some((id, user_id, username, role, scopes, disabled, expires_at, revoked_at)) = row
        else {
            return Ok(None);
        };
        if disabled || revoked_at.is_some() || expires_at.is_some_and(|e| e <= now) {
            return Ok(None);
        }

        sqlx::query(&crate::sql!("UPDATE mcp_connections SET last_used_at = ? WHERE id = ?"))
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(Some(McpAuth {
            connection_id: id,
            user: AuthUser { user_id, username, is_admin: role == "admin" },
            scopes,
        }))
    }
    /// Exchange a refresh secret for a fresh pair, rotating both.
    ///
    /// The old access secret dies with the swap, which is the point: a leaked
    /// access token stops working as soon as the legitimate client refreshes.
    pub async fn refresh_mcp_connection(
        &self,
        refresh_token: &str,
        ttl_secs: i64,
    ) -> Result<Option<McpCredential>, sqlx::Error> {
        use rand::Rng;
        let row: Option<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT id FROM mcp_connections WHERE refresh_hash = ? AND revoked_at IS NULL",
        ))
        .bind(Self::hash_token(refresh_token))
        .fetch_optional(&self.pool)
        .await?;
        let Some((id,)) = row else { return Ok(None) };

        let raw: [u8; 32] = rand::rng().random();
        let token = format!("tdmcp_{}", hex::encode(raw));
        let raw: [u8; 32] = rand::rng().random();
        let refresh = format!("tdmcpr_{}", hex::encode(raw));
        let now = Self::now_secs();

        sqlx::query(&crate::sql!(
            "UPDATE mcp_connections SET token_hash = ?, refresh_hash = ?, expires_at = ?
             WHERE id = ?",
        ))
        .bind(Self::hash_token(&token))
        .bind(Self::hash_token(&refresh))
        .bind(now + ttl_secs)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(Some(McpCredential { id, token, refresh: Some(refresh), expires_in: Some(ttl_secs) }))
    }
    pub async fn list_mcp_connections(
        &self,
        user_id: i64,
    ) -> Result<Vec<McpConnectionRecord>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, name, scopes, client_id, created_at, last_used_at, expires_at,
                    (SELECT COUNT(*) FROM mcp_audit a WHERE a.connection_id = c.id) AS calls
             FROM mcp_connections c
             WHERE user_id = ? AND revoked_at IS NULL
             ORDER BY created_at DESC",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Revoke rather than delete, so the audit rows keep pointing at something
    /// with a name. "Which chatbot read my history in March" is a question that
    /// outlives the connection, and a dangling id cannot answer it.
    pub async fn revoke_mcp_connection(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE mcp_connections SET revoked_at = ?, token_hash = 'revoked:' || id,
                    refresh_hash = NULL
             WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
        ))
        .bind(Self::now_secs())
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Record one call. Deliberately infallible from the caller's point of
    /// view — an audit write that fails must not fail the request that was
    /// otherwise fine, and the error is logged rather than propagated.
    #[allow(clippy::too_many_arguments)]
    pub async fn log_mcp_call(
        &self,
        connection_id: Option<i64>,
        user_id: i64,
        method: &str,
        tool: Option<&str>,
        args: Option<&str>,
        ok: bool,
        rows_returned: Option<i64>,
        ms: i64,
    ) {
        // Arguments are truncated rather than stored whole: this is a record of
        // what was asked, not a copy of it, and a tool call carrying a long
        // free-text argument should not turn the audit table into the largest
        // thing in the database.
        let args = args.map(|a| a.chars().take(500).collect::<String>());
        let res = sqlx::query(&crate::sql!(
            "INSERT INTO mcp_audit
                (connection_id, user_id, at, method, tool, args, ok, rows_returned, ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(connection_id)
        .bind(user_id)
        .bind(Self::now_secs())
        .bind(method)
        .bind(tool)
        .bind(args)
        .bind(ok)
        .bind(rows_returned)
        .bind(ms)
        .execute(&self.pool)
        .await;
        if let Err(e) = res {
            tracing::error!("MCP audit write failed: {}", e);
        }
    }
    pub async fn list_mcp_audit(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<McpAuditRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT a.id, a.at, a.method, a.tool, a.args, a.ok, a.rows_returned, a.ms,
                    c.name AS connection_name
             FROM mcp_audit a LEFT JOIN mcp_connections c ON c.id = a.connection_id
             WHERE a.user_id = ? ORDER BY a.at DESC, a.id DESC LIMIT ?",
        ))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// How many calls this connection has made since `since`. The budget check
    /// — one indexed count, run before the tool rather than after.
    pub async fn mcp_calls_since(&self, connection_id: i64, since: i64) -> i64 {
        let row: Result<(i64,), _> = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM mcp_audit WHERE connection_id = ? AND at >= ?",
        ))
        .bind(connection_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await;
        row.map(|r| r.0).unwrap_or(0)
    }
    pub async fn register_oauth_client(
        &self,
        client_id: &str,
        client_name: &str,
        redirect_uris: &str,
        registered_by_url: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO oauth_clients
                (client_id, client_name, redirect_uris, registered_by_url, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(client_id) DO UPDATE SET
                client_name = excluded.client_name,
                redirect_uris = excluded.redirect_uris"
        ))
        .bind(client_id)
        .bind(client_name)
        .bind(redirect_uris)
        .bind(registered_by_url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn get_oauth_client(
        &self,
        client_id: &str,
    ) -> Result<Option<OAuthClient>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT client_id, client_name, redirect_uris, registered_by_url
             FROM oauth_clients WHERE client_id = ?"
        ))
        .bind(client_id)
        .fetch_optional(&self.pool)
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn create_oauth_code(
        &self,
        client_id: &str,
        user_id: i64,
        redirect_uri: &str,
        scopes: &str,
        code_challenge: &str,
        resource: Option<&str>,
        ttl_secs: i64,
    ) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 32] = rand::rng().random();
        let code = hex::encode(raw);
        let now = Self::now_secs();
        sqlx::query(&crate::sql!(
            "INSERT INTO oauth_codes
                (code_hash, client_id, user_id, redirect_uri, scopes, code_challenge,
                 resource, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(Self::hash_token(&code))
        .bind(client_id)
        .bind(user_id)
        .bind(redirect_uri)
        .bind(scopes)
        .bind(code_challenge)
        .bind(resource)
        .bind(now)
        .bind(now + ttl_secs)
        .execute(&self.pool)
        .await?;
        Ok(code)
    }
    /// Take an authorization code, deleting it in the same statement.
    ///
    /// `DELETE … RETURNING` rather than select-then-delete: two statements
    /// would let two simultaneous redemptions of one code both see it, which is
    /// exactly the replay a single-use code exists to stop.
    ///
    /// **`fetch_all`, not `fetch_optional`**, and here the reason is the
    /// security property rather than a stale read. `fetch_optional` stops after
    /// the first row and never steps the statement to `SQLITE_DONE`, so the
    /// delete's implicit transaction stays open until that pooled connection is
    /// next used — and until then the code row is still there for another
    /// connection to find. The single-use guarantee this function exists for
    /// would be resting on SQLite's write locking and on the timing of an
    /// unrelated query, rather than on the delete having happened. See
    /// `create_mcp_connection` for the measurement.
    pub async fn consume_oauth_code(&self, code: &str) -> Result<Option<OAuthCode>, sqlx::Error> {
        let rows: Vec<OAuthCode> = sqlx::query_as(&crate::sql!(
            "DELETE FROM oauth_codes WHERE code_hash = ?
             RETURNING client_id, user_id, redirect_uri, scopes, code_challenge,
                       resource, expires_at"
        ))
        .bind(Self::hash_token(code))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().next().filter(|c| c.expires_at > Self::now_secs()))
    }
    pub async fn purge_expired_oauth_codes(&self) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("DELETE FROM oauth_codes WHERE expires_at < ?"))
            .bind(Self::now_secs())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
