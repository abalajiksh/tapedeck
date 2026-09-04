//! Preferences, signal chains, and output-device bindings.
//!
//! Chain and gear hours are **derived from the listens** (`CHAIN_COLUMNS`),
//! never accumulated — an accumulator cannot take hours back when a chain is
//! un-assigned, and only a derivation is retroactive.

use super::*;

impl Database {
    /// Returns the user's `settings` JSON object (defaults to `{}`).
    pub async fn get_preferences(&self, user_id: i64) -> Result<serde_json::Value, sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT settings FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        let json = row
            .and_then(|(s,)| s)
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        Ok(json)
    }
    /// Every user's scrobble threshold, in one query.
    ///
    /// One query for everybody rather than one per user: the engine refreshes
    /// this on a clock of its own and would otherwise be doing a lookup per user
    /// per tick to read two numbers that almost never change. Users who have
    /// never touched the setting are simply absent — the caller falls back to
    /// the default, so this returns nothing at all on a fresh instance.
    pub async fn scrobble_thresholds(
        &self,
    ) -> Result<HashMap<i64, crate::models::ScrobbleThreshold>, sqlx::Error> {
        let rows: Vec<(i64, Option<String>)> =
            sqlx::query_as("SELECT id, settings FROM users WHERE settings IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, settings)| {
                let json = serde_json::from_str::<serde_json::Value>(&settings?).ok()?;
                // Only users who actually set one, so the default stays the
                // single source of truth for everybody else.
                if json.get("scrobble_threshold_percent").is_none()
                    && json.get("scrobble_threshold_secs").is_none()
                {
                    return None;
                }
                Some((id, crate::models::ScrobbleThreshold::from_prefs(&json)))
            })
            .collect())
    }
    /// Shallow-merge `patch` into the user's stored preferences and persist.
    /// Returns the merged object.
    pub async fn merge_preferences(
        &self,
        user_id: i64,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value, sqlx::Error> {
        let mut current = self.get_preferences(user_id).await?;
        if let (Some(obj), Some(p)) = (current.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                obj.insert(k.clone(), v.clone());
            }
        }
        sqlx::query("UPDATE users SET settings = ? WHERE id = ?")
            .bind(current.to_string())
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(current)
    }
    pub async fn create_chain(&self, chain: &SignalChain) -> Result<i64, sqlx::Error> {
        let components_json =
            serde_json::to_string(&chain.components).unwrap_or_else(|_| "[]".into());
        let now = Self::now_secs();
        let result = sqlx::query(
            "INSERT INTO signal_chains (user_id, name, description, components, listening_context, icon, is_active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        ).bind(chain.user_id).bind(&chain.name).bind(&chain.description)
        .bind(&components_json).bind(chain.listening_context.as_str()).bind(&chain.icon)
        .bind(chain.is_active).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }
    pub async fn get_chains(&self, user_id: i64) -> Result<Vec<SignalChain>, sqlx::Error> {
        let rows: Vec<ChainRow> = sqlx::query_as(&format!(
            "SELECT {} FROM signal_chains s WHERE s.user_id = ? ORDER BY s.name ASC",
            Self::CHAIN_COLUMNS
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.into()).collect())
    }
    pub async fn get_chain(
        &self,
        user_id: i64,
        chain_id: i64,
    ) -> Result<Option<SignalChain>, sqlx::Error> {
        let row: Option<ChainRow> = sqlx::query_as(&format!(
            "SELECT {} FROM signal_chains s WHERE s.id = ? AND s.user_id = ?",
            Self::CHAIN_COLUMNS
        ))
        .bind(chain_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.into()))
    }
    /// Resolve a chain *name* (as sent by clients in `tapedeck_chain.chain_id`)
    /// to its id for this user. Case-insensitive.
    /// Every side played of one pressing, as `(side, played_at)`.
    ///
    /// The key a shelf import dedups on: re-importing the same archive must not
    /// double the wear recorded against a stylus.
    pub async fn plays_for_release(
        &self,
        user_id: i64,
        release_id: i64,
    ) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT side, played_at FROM physical_plays
              WHERE user_id = ? AND release_id = ?",
        )
        .bind(user_id)
        .bind(release_id)
        .fetch_all(&self.pool)
        .await
    }
    pub async fn get_chain_by_name(
        &self,
        user_id: i64,
        name: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM signal_chains WHERE user_id = ? AND LOWER(name) = LOWER(?)",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }
    /// A source device's configured default chain, if any (fallback for listens
    /// that don't carry an explicit chain or output binding).
    pub async fn get_device_default_chain(
        &self,
        user_id: i64,
        device_id: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT default_chain_id FROM devices WHERE id = ? AND user_id = ?")
                .bind(device_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(c,)| c))
    }
    /// The chain a device defaults to, looked up by the **key the source
    /// reports** rather than by our row id.
    ///
    /// Exists so a *now-playing* entry can carry the chain its eventual listen
    /// will be attributed to, which is what lets Patch apply the shared-chain
    /// rule to a live deck. The scrobble path resolves the same thing through
    /// `upsert_device`, and that is a **write** — fine once per banked listen,
    /// wrong every five seconds for something still playing. This one only
    /// reads: a device that has never been seen simply has no chain yet, which
    /// is the honest answer for a source we have not banked anything from.
    pub async fn device_chain_by_key(
        &self,
        user_id: i64,
        machine_id: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT default_chain_id FROM devices WHERE user_id = ? AND machine_id = ?",
        )
        .bind(user_id)
        .bind(machine_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(c,)| c))
    }
    /// Point a source device at a chain (or clear it with `None`).
    pub async fn set_device_default_chain(
        &self,
        user_id: i64,
        device_id: i64,
        chain_id: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("UPDATE devices SET default_chain_id = ? WHERE id = ? AND user_id = ?")
                .bind(chain_id)
                .bind(device_id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }
    pub async fn update_chain(
        &self,
        user_id: i64,
        chain_id: i64,
        name: &str,
        description: Option<&str>,
        components: &[ChainComponent],
        listening_context: &str,
        icon: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let components_json = serde_json::to_string(components).unwrap_or_else(|_| "[]".into());
        let res = sqlx::query(
            // `shared` is deliberately absent. This is a whole-record replace
            // of everything the editor owns, and sharing is not the editor's —
            // it is switched from the chain drawer or Patch → Decks. Adding it
            // here would un-publish a chain as a side effect of renaming it,
            // which is the shape that silently erased gear notes in v0.50.0.
            "UPDATE signal_chains SET name = ?, description = ?, components = ?, listening_context = ?, icon = ?
             WHERE id = ? AND user_id = ?"
        ).bind(name).bind(description).bind(&components_json).bind(listening_context).bind(icon)
         .bind(chain_id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }
    /// Delete a chain, first unlinking (not deleting) anything that references
    /// it — listens and sessions keep their data with `chain_id` cleared. Done
    /// in a transaction so we never leave dangling FKs (sqlx enables them).
    ///
    /// **Ten tables carry a chain reference and this used to clear four**, so
    /// deleting a chain that a token pointed at, or that a record on the shelf
    /// named, or that had a *variant*, failed outright with a foreign-key
    /// violation. The variant case is the one that bit: `chain_variants.chain_id`
    /// is `NOT NULL`, so the moment anyone set up the feature v0.54.0 shipped,
    /// the chain it belonged to became undeletable — and the error surfaces as a
    /// 500 with nothing saying which reference is in the way.
    ///
    /// Variants are **deleted** rather than unlinked, unlike everything else
    /// here: a variant is a set of overrides indexed into a chain's components,
    /// so without that chain it is not a degraded row, it is a meaningless one.
    /// The listens that pointed at those variants keep their own data and lose
    /// only the variant, which is what a null variant already means everywhere.
    pub async fn delete_chain(&self, user_id: i64, chain_id: i64) -> Result<bool, sqlx::Error> {
        // Ownership is settled **first**, so everything below can clear by chain
        // id alone. That matters for `pairing_requests`, whose `user_id` is
        // nullable — scoping its clear by user would skip exactly the rows that
        // then block the delete.
        let owned: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM signal_chains WHERE id = ? AND user_id = ?")
                .bind(chain_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        if owned.is_none() {
            return Ok(false);
        }

        let mut tx = self.pool.begin().await?;

        // Anything pointing at a *variant* of this chain has to let go before
        // the variants themselves can be deleted.
        for table in ["scrobbles", "physical_plays", "physical_releases"] {
            sqlx::query(&format!(
                "UPDATE {table} SET chain_variant_id = NULL
                  WHERE chain_variant_id IN
                        (SELECT id FROM chain_variants WHERE chain_id = ?)"
            ))
            .bind(chain_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM chain_variants WHERE chain_id = ?")
            .bind(chain_id)
            .execute(&mut *tx)
            .await?;

        // Everything that merely *names* the chain keeps its row and loses the
        // reference.
        for (table, column) in [
            ("scrobbles", "chain_id"),
            ("sessions", "chain_id"),
            ("devices", "default_chain_id"),
            ("chain_bindings", "chain_id"),
            ("tokens", "default_chain_id"),
            ("physical_plays", "chain_id"),
            ("physical_releases", "chain_id"),
            ("physical_spins", "chain_id"),
            ("pairing_requests", "default_chain_id"),
        ] {
            sqlx::query(&format!("UPDATE {table} SET {column} = NULL WHERE {column} = ?"))
                .bind(chain_id)
                .execute(&mut *tx)
                .await?;
        }

        let res = sqlx::query("DELETE FROM signal_chains WHERE id = ? AND user_id = ?")
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }
    /// Record that we saw a listen come through this output device. Inserts a
    /// new unmapped row (chain_id NULL) the first time, else bumps hits/last_seen
    /// and refreshes the interface. Never overwrites an existing chain mapping.
    pub async fn record_output(
        &self,
        user_id: i64,
        identifier: &str,
        interface: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Self::now_secs();
        sqlx::query(
            "INSERT INTO chain_bindings (user_id, identifier, interface, chain_id, last_seen, hits)
             VALUES (?, ?, ?, NULL, ?, 1)
             ON CONFLICT(user_id, identifier) DO UPDATE SET
                hits = hits + 1, last_seen = excluded.last_seen,
                interface = COALESCE(excluded.interface, chain_bindings.interface)",
        )
        .bind(user_id)
        .bind(identifier)
        .bind(interface)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Resolve an output-device identifier to its bound chain, if mapped.
    pub async fn resolve_binding(
        &self,
        user_id: i64,
        identifier: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT chain_id FROM chain_bindings WHERE user_id = ? AND identifier = ?",
        )
        .bind(user_id)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(c,)| c))
    }
    pub async fn list_bindings(&self, user_id: i64) -> Result<Vec<BindingRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM chain_bindings WHERE user_id = ? ORDER BY chain_id IS NOT NULL, last_seen DESC")
            .bind(user_id).fetch_all(&self.pool).await
    }
    /// Assign (or clear, with None) the chain for an output identifier. Creates
    /// the binding if the identifier hasn't been seen yet (manual add).
    pub async fn set_binding(
        &self,
        user_id: i64,
        identifier: &str,
        chain_id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let now = Self::now_secs();
        sqlx::query(
            "INSERT INTO chain_bindings (user_id, identifier, chain_id, last_seen, hits)
             VALUES (?, ?, ?, ?, 0)
             ON CONFLICT(user_id, identifier) DO UPDATE SET chain_id = excluded.chain_id",
        )
        .bind(user_id)
        .bind(identifier)
        .bind(chain_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn delete_binding(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM chain_bindings WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
