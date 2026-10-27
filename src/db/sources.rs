//! Per-user music sources — several of a kind per user, told apart by `label`.
//!
//! `record_source_poll` and `set_source_credential` must **not** touch
//! `updated_at` — that is the stamp `reconcile_sources` watches, and bumping
//! it rebuilds the source every tick.

use super::*;

impl Database {
    /// Every enabled source across all users — what the engine reconciles its
    /// registry against each tick. Credentials come back decrypted; a row we
    /// can't decrypt is dropped rather than failing the whole tick.
    ///
    /// A source with a token made on it is in plugin mode and left out: its
    /// listens arrive through that token, and polling as well would race them
    /// with bare copies. Removing the token brings it back on the next tick.
    pub async fn list_enabled_sources(&self) -> Result<Vec<UserSource>, sqlx::Error> {
        let rows: Vec<UserSourceRow> = sqlx::query_as(&crate::sql!(
            "SELECT s.id, s.user_id, s.kind, s.label, s.enabled, s.url, s.credentials,
                    s.remote_account, s.poll_interval_secs, s.options, s.last_polled_at,
                    s.last_error, s.updated_at, COALESCE(u.role, '') AS owner_role
             FROM user_sources s LEFT JOIN users u ON u.id = s.user_id
             WHERE s.enabled = 1
               AND NOT EXISTS (SELECT 1 FROM tokens t WHERE t.source_ref = s.id)
             ORDER BY s.user_id, s.kind",
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(|r| self.hydrate_source(r)).collect())
    }
    /// This user's sources, for the Settings screen. Credentials are decrypted
    /// here too — the handler decides what is safe to send to the browser.
    pub async fn list_sources_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<UserSource>, sqlx::Error> {
        let rows: Vec<UserSourceRow> = sqlx::query_as(&crate::sql!(
            "SELECT s.id, s.user_id, s.kind, s.label, s.enabled, s.url, s.credentials,
                    s.remote_account, s.poll_interval_secs, s.options, s.last_polled_at,
                    s.last_error, s.updated_at, COALESCE(u.role, '') AS owner_role
             FROM user_sources s LEFT JOIN users u ON u.id = s.user_id
             WHERE s.user_id = ? ORDER BY s.kind",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(|r| self.hydrate_source(r)).collect())
    }
    /// Every source on the instance and when it was last polled, for
    /// Settings → Operations.
    ///
    /// Instance-wide and therefore admin-only at the handler — this is the one
    /// source query not scoped by `user_id`, because "is anything on this box
    /// polling?" is not a question any one account can answer. Deliberately
    /// selects **no credentials and no URL**: the readout needs to say which
    /// source and how late it is, and nothing more.
    ///
    /// Everything here was already being written by `record_source_poll`; only
    /// the reading was missing. On 2026-08-24 "Plex last polled 78 minutes ago"
    /// against a 5-second interval was true all day and visible nowhere.
    pub async fn source_health(&self) -> Result<Vec<SourceHealth>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT s.id, s.user_id, COALESCE(u.username, '') AS username, s.kind, s.label,
                    s.enabled, s.poll_interval_secs, s.last_polled_at, s.last_error,
                    EXISTS (SELECT 1 FROM tokens t WHERE t.source_ref = s.id) AS plugin
             FROM user_sources s
             LEFT JOIN users u ON u.id = s.user_id
             ORDER BY s.user_id, s.kind",
        ))
        .fetch_all(&self.pool)
        .await
    }
    /// One source by id, scoped to its owner so an id from another user is a
    /// miss rather than a leak.
    pub async fn get_source(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<UserSource>, sqlx::Error> {
        let row: Option<UserSourceRow> = sqlx::query_as(&crate::sql!(
            "SELECT s.id, s.user_id, s.kind, s.label, s.enabled, s.url, s.credentials,
                    s.remote_account, s.poll_interval_secs, s.options, s.last_polled_at,
                    s.last_error, s.updated_at, COALESCE(u.role, '') AS owner_role
             FROM user_sources s LEFT JOIN users u ON u.id = s.user_id
             WHERE s.user_id = ? AND s.id = ?",
        ))
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| self.hydrate_source(r)))
    }
    fn hydrate_source(&self, row: UserSourceRow) -> Option<UserSource> {
        let credentials =
            if row.credentials.is_empty() { String::new() } else { self.unseal(&row.credentials)? };
        Some(UserSource {
            id: row.id,
            user_id: row.user_id,
            kind: row.kind,
            label: row.label,
            enabled: row.enabled,
            url: row.url,
            credentials,
            remote_account: row.remote_account,
            poll_interval_secs: row.poll_interval_secs,
            options: row.options,
            last_polled_at: row.last_polled_at,
            last_error: row.last_error,
            updated_at: row.updated_at,
            owner_is_admin: row.owner_role == "admin",
        })
    }
    /// Create a source, returning its id. `updated_at` doubles as the change
    /// stamp the engine watches, so a save is picked up on the next tick.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_source(
        &self,
        user_id: i64,
        kind: &str,
        label: &str,
        enabled: bool,
        url: &str,
        credentials: &str,
        remote_account: Option<&str>,
        poll_interval_secs: Option<i64>,
        options: &str,
    ) -> Result<i64, sqlx::Error> {
        let sealed = if credentials.is_empty() { String::new() } else { self.seal(credentials) };
        let res = sqlx::query_as::<_, (i64,)>(&crate::sql!(
            "INSERT INTO user_sources
                (user_id, kind, label, enabled, url, credentials, remote_account,
                 poll_interval_secs, options, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(kind)
        .bind(label)
        .bind(enabled)
        .bind(url)
        .bind(sealed)
        .bind(remote_account)
        .bind(poll_interval_secs)
        .bind(options)
        .bind(Self::now_secs())
        .fetch_all(&self.pool)
        .await?;
        res.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn update_source(
        &self,
        user_id: i64,
        id: i64,
        label: &str,
        enabled: bool,
        url: &str,
        credentials: &str,
        remote_account: Option<&str>,
        poll_interval_secs: Option<i64>,
        options: &str,
    ) -> Result<bool, sqlx::Error> {
        let sealed = if credentials.is_empty() { String::new() } else { self.seal(credentials) };
        let res = sqlx::query(&crate::sql!(
            "UPDATE user_sources SET
                label = ?, enabled = ?, url = ?, credentials = ?, remote_account = ?,
                poll_interval_secs = ?, options = ?, updated_at = ?
             WHERE id = ? AND user_id = ?"
        ))
        .bind(label)
        .bind(enabled)
        .bind(url)
        .bind(sealed)
        .bind(remote_account)
        .bind(poll_interval_secs)
        .bind(options)
        .bind(Self::now_secs())
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn delete_source(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Artist pictures chosen from this server go with it: the proxy could
        // only ever answer them with a fallback, and the picker would keep
        // calling a picture "yours" that nothing can show.
        sqlx::query(&crate::sql!(
            "DELETE FROM artist_pictures WHERE user_id = ? AND source_ref = ?"
        ))
        .bind(user_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        Self::delete_source_tokens_in(&mut tx, user_id, id).await?;
        let res =
            sqlx::query(&crate::sql!("DELETE FROM user_sources WHERE user_id = ? AND id = ?"))
                .bind(user_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }
    /// Make the token a plugin on this source submits with, replacing any
    /// earlier one. Returns the raw token, which is shown once.
    ///
    /// The caller has already checked the source belongs to `user_id`.
    pub async fn create_source_token(
        &self,
        user_id: i64,
        source_id: i64,
        name: &str,
        scopes: &str,
    ) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let mut tx = self.pool.begin().await?;
        Self::delete_source_tokens_in(&mut tx, user_id, source_id).await?;
        sqlx::query(&crate::sql!(
            "INSERT INTO tokens (user_id, token_hash, name, scopes, created_at, source_ref)
             VALUES (?, ?, ?, ?, ?, ?)"
        ))
        .bind(user_id)
        .bind(Self::hash_token(&token))
        .bind(name)
        .bind(scopes)
        .bind(Self::now_secs())
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        info!("🔑 Created plugin token for source {} (user_id {})", source_id, user_id);
        Ok(token)
    }
    /// Remove a source's plugin token, which puts it back to polling.
    pub async fn delete_source_tokens(
        &self,
        user_id: i64,
        source_id: i64,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let n = Self::delete_source_tokens_in(&mut tx, user_id, source_id).await?;
        tx.commit().await?;
        Ok(n > 0)
    }
    /// Same unlinking as `delete_token`: sessions keep the history and stop
    /// naming the token, or the foreign key refuses the delete.
    async fn delete_source_tokens_in(
        tx: &mut sqlx::Transaction<'_, Db>,
        user_id: i64,
        source_id: i64,
    ) -> Result<u64, sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE sessions SET token_id = NULL
             WHERE user_id = ? AND token_id IN (SELECT id FROM tokens WHERE source_ref = ?)"
        ))
        .bind(user_id)
        .bind(source_id)
        .execute(&mut **tx)
        .await?;
        let res =
            sqlx::query(&crate::sql!("DELETE FROM tokens WHERE user_id = ? AND source_ref = ?"))
                .bind(user_id)
                .bind(source_id)
                .execute(&mut **tx)
                .await?;
        Ok(res.rows_affected())
    }
    /// A source's plugin token, if it has one: `(scopes, created_at,
    /// last_used_at)`. The token itself is only a hash by now.
    pub async fn source_token(
        &self,
        user_id: i64,
        source_id: i64,
    ) -> Result<Option<(String, i64, Option<i64>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT COALESCE(scopes, ''), created_at, last_used_at FROM tokens
             WHERE user_id = ? AND source_ref = ? ORDER BY id DESC LIMIT 1"
        ))
        .bind(user_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await
    }
    /// The user's one source of `kind`, or `None` when they have none or
    /// several — a token made under Settings → API Tokens names no source, and
    /// a guess between two servers would point the art proxy at the wrong one.
    pub async fn sole_source_of_kind(
        &self,
        user_id: i64,
        kind: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let ids: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT id FROM user_sources WHERE user_id = ? AND kind = ? LIMIT 2"
        ))
        .bind(user_id)
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;
        Ok(if ids.len() == 1 { Some(ids[0].0) } else { None })
    }
    /// Record the outcome of a poll. Deliberately does *not* touch
    /// `updated_at` — that would look like a config change and make the engine
    /// rebuild the source on every tick.
    pub async fn record_source_poll(
        &self,
        id: i64,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE user_sources SET last_polled_at = ?, last_error = ? WHERE id = ?",
        ))
        .bind(Self::now_secs())
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Merge one key into a source's sealed credential blob, leaving the rest
    /// alone.
    ///
    /// Exists for credentials the *remote* issues rather than the user: Roon
    /// hands back a pairing token when the extension is approved, and may
    /// reissue it, so the engine has to write one back mid-poll. Everything
    /// else in `user_sources.credentials` is typed by a person into a form and
    /// goes through `update_source`.
    ///
    /// **`updated_at` is deliberately untouched**, for the same reason
    /// `record_source_poll` leaves it alone: it is the change stamp
    /// `reconcile_sources` watches, so bumping it here would make the engine
    /// tear down and rebuild the source — re-running the Roon handshake — on
    /// every tick that persisted a token.
    pub async fn set_source_credential(
        &self,
        id: i64,
        key: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        let stored: Option<String> =
            sqlx::query_scalar(&crate::sql!("SELECT credentials FROM user_sources WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();

        let plain = match stored {
            Some(s) if !s.is_empty() => self.unseal(&s).unwrap_or_default(),
            _ => String::new(),
        };
        let mut bag: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&plain).unwrap_or_default();
        bag.insert(key.to_string(), serde_json::Value::String(value.to_string()));

        let sealed = self.seal(&serde_json::Value::Object(bag).to_string());
        // `seal` returns empty rather than plaintext when encryption fails.
        // Writing that would silently wipe the source's other credentials, so
        // the write is skipped and the in-memory token stays usable until the
        // next restart.
        if sealed.is_empty() {
            return Err(sqlx::Error::Protocol(
                "refusing to store an unencryptable credential".into(),
            ));
        }
        sqlx::query(&crate::sql!("UPDATE user_sources SET credentials = ? WHERE id = ?"))
            .bind(sealed)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// The stored Plex config, or `None` before first-run seeding.
    pub async fn get_plex_settings(&self) -> Result<Option<PlexSettings>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT enabled, url, token, users_allow, users_block, devices_allow,
                    devices_block, libraries_allow, libraries_block, updated_at
             FROM plex_settings WHERE id = 1"
        ))
        .fetch_optional(&self.pool)
        .await
    }
    /// Write the single settings row. `updated_at` doubles as the change stamp
    /// the engine watches to decide when to rebuild its Plex source.
    pub async fn save_plex_settings(&self, s: &PlexSettings) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO plex_settings
                (id, enabled, url, token, users_allow, users_block, devices_allow,
                 devices_block, libraries_allow, libraries_block, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                enabled=excluded.enabled, url=excluded.url, token=excluded.token,
                users_allow=excluded.users_allow, users_block=excluded.users_block,
                devices_allow=excluded.devices_allow, devices_block=excluded.devices_block,
                libraries_allow=excluded.libraries_allow, libraries_block=excluded.libraries_block,
                updated_at=excluded.updated_at"
        ))
        .bind(s.enabled)
        .bind(&s.url)
        .bind(&s.token)
        .bind(&s.users_allow)
        .bind(&s.users_block)
        .bind(&s.devices_allow)
        .bind(&s.devices_block)
        .bind(&s.libraries_allow)
        .bind(&s.libraries_block)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Record a new import as `running` and return its row id. Persisting at
    /// start (not just completion) means an interrupted import leaves a durable
    /// record instead of a job that vanishes with the process.
    pub async fn create_import_job(&self, user_id: i64, source: &str) -> Result<i64, sqlx::Error> {
        let res = sqlx::query_as::<_, (i64,)>(&crate::sql!("INSERT INTO import_jobs (user_id, source, state, fetched, imported, status, created_at)
             VALUES (?, ?, 'running', 0, 0, 'started', ?) RETURNING id")).bind(user_id).bind(source).bind(Self::now_secs()).fetch_all(&self.pool).await?;
        res.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    /// Update a job to its final state on completion.
    pub async fn finish_import_job(
        &self,
        id: i64,
        state: &str,
        fetched: i64,
        imported: i64,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE import_jobs SET state = ?, fetched = ?, imported = ?, status = ? WHERE id = ?",
        ))
        .bind(state)
        .bind(fetched)
        .bind(imported)
        .bind(status)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// On startup, any import still marked `running` died with the previous
    /// process — mark it interrupted so history is accurate and nothing lingers.
    pub async fn mark_running_imports_interrupted(&self) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("UPDATE import_jobs SET state = 'interrupted', status = 'interrupted by shutdown' WHERE state = 'running'")).execute(&self.pool).await?;
        Ok(res.rows_affected())
    }
    pub async fn list_imports(&self, user_id: i64) -> Result<Vec<ImportHistoryRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT * FROM import_jobs WHERE user_id = ? ORDER BY created_at DESC LIMIT 50",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Bulk-reassign the chain for a set of listens (History bulk edit).
    /// Retag a batch of listens with a chain — **and always with a variant**,
    /// even when that variant is `None`.
    ///
    /// Writing the chain and leaving `chain_variant_id` alone is the bug this
    /// signature exists to stop: a listen moved from chain A to chain B would
    /// keep a variant belonging to A, pairing it with components that were
    /// never fitted together. `equipment_seconds` ignores a mismatched pairing,
    /// so the *hours* stayed right and only the displayed setup was wrong —
    /// which is exactly the kind of wrongness nobody notices. Sessions have
    /// rewritten both together since v0.54.0; listens did not.
    ///
    /// The caller checks that the variant belongs to the chain before calling.
    /// Mark, or unmark, a set of listens as heard in company.
    ///
    /// Scoped by `user_id` in the `WHERE`, so a body naming somebody else's
    /// listens changes nothing rather than half-succeeding.
    ///
    /// Deliberately **does not** touch `joint_session_id`. That column is the
    /// join key saying *this exact play happened on several decks at once*, and
    /// only a Shared Spool can know it; letting a hand edit write one would
    /// invent a sitting that never took place. The two answer different
    /// questions and only one of them is a matter of opinion.
    pub async fn bulk_set_scrobble_company(
        &self,
        user_id: i64,
        ids: &[i64],
        company: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE scrobbles SET company = ? WHERE user_id = ? AND id IN ({placeholders})"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query(&stmt).bind(company).bind(user_id);
        for id in ids {
            q = q.bind(id);
        }
        Ok(q.execute(&self.pool).await?.rows_affected())
    }
    pub async fn bulk_set_scrobble_chain(
        &self,
        user_id: i64,
        ids: &[i64],
        chain_id: Option<i64>,
        variant_id: Option<i64>,
    ) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE scrobbles SET chain_id = ?, chain_variant_id = ? WHERE user_id = ? AND id IN ({placeholders})"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query(&stmt).bind(chain_id).bind(variant_id).bind(user_id);
        for id in ids {
            q = q.bind(id);
        }
        Ok(q.execute(&self.pool).await?.rows_affected())
    }
}
