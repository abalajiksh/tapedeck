//! Devices, equipment and chain variants.
//!
//! `equipment_seconds` folds in Rust rather than SQL because a variant
//! override means *which chain* no longer answers *what was fitted*, and gear
//! must be counted as a **set** per chain or a chain naming one box twice
//! charges it twice.

use super::*;

impl Database {
    pub async fn upsert_device(
        &self,
        user_id: i64,
        machine_id: &str,
        name: Option<&str>,
        platform: Option<&str>,
        product: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
                .bind(user_id)
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((id,)) = existing {
            sqlx::query("UPDATE devices SET last_seen = ?, total_listens = total_listens + 1,
                 name = COALESCE(?, name), platform = COALESCE(?, platform), product = COALESCE(?, product) WHERE id = ?")
            .bind(now).bind(name).bind(platform).bind(product).bind(id).execute(&self.pool).await?;
            Ok(id)
        } else {
            let result = sqlx::query("INSERT INTO devices (user_id, machine_id, name, platform, product, first_seen, last_seen, total_listens)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 1)")
            .bind(user_id).bind(machine_id).bind(name).bind(platform).bind(product).bind(now).bind(now)
            .execute(&self.pool).await?;
            Ok(result.last_insert_rowid())
        }
    }
    /// Look a device up without creating or touching it.
    ///
    /// `upsert_device` is the normal path and it *writes* — it bumps
    /// `last_seen` and `total_listens`. A dry-run submit has to resolve the
    /// chain ladder's device rung without leaving a trace, or "tell me what
    /// would happen" would itself change what happens.
    pub async fn find_device(
        &self,
        user_id: i64,
        machine_id: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
                .bind(user_id)
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }
    pub async fn get_devices(&self, user_id: i64) -> Result<Vec<DeviceRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM devices WHERE user_id = ? ORDER BY last_seen DESC")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
    }
    pub async fn upsert_equipment(
        &self,
        user_id: i64,
        name: &str,
        equipment_type: &str,
        brand: Option<&str>,
        model: Option<&str>,
        hours_to_add: f64,
        purchased_at: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM equipment WHERE user_id = ? AND name = ?")
                .bind(user_id)
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((id,)) = existing {
            sqlx::query(
                "UPDATE equipment SET total_hours = total_hours + ?, last_used = ? WHERE id = ?",
            )
            .bind(hours_to_add)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            let result = sqlx::query("INSERT INTO equipment (user_id, name, type, brand, model, total_hours, first_used, last_used, purchased_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(user_id).bind(name).bind(equipment_type).bind(brand).bind(model)
            .bind(hours_to_add).bind(now).bind(now).bind(purchased_at).execute(&self.pool).await?;
            Ok(result.last_insert_rowid())
        }
    }
    /// Resolve a `[[gear:…]]` shortcode to one of the user's equipment rows.
    ///
    /// Unlike `resolve_reference` this never *creates* anything: gear is
    /// something you own and catalogue deliberately, so a note naming a piece
    /// you haven't added should render as unresolved rather than conjuring an
    /// empty record with no type or brand. Matches the name first, then the
    /// model, so "[[gear:HD 650]]" finds it whichever field it was entered in.
    pub async fn find_equipment_by_name(
        &self,
        user_id: i64,
        needle: &str,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(None);
        }
        sqlx::query_as(
            "SELECT id, name FROM equipment
             WHERE user_id = ? AND (LOWER(TRIM(name)) = ? OR LOWER(TRIM(model)) = ?)
             ORDER BY (LOWER(TRIM(name)) = ?) DESC, id ASC LIMIT 1",
        )
        .bind(user_id)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .fetch_optional(&self.pool)
        .await
    }
    pub async fn list_variants(&self, user_id: i64) -> Result<Vec<ChainVariant>, sqlx::Error> {
        sqlx::query_as::<_, ChainVariantRow>(
            "SELECT v.id, v.user_id, v.chain_id, v.name, v.overrides, v.created_at,
                    CAST(COALESCE((
                        SELECT SUM(sc.duration) FROM scrobbles sc
                         WHERE sc.user_id = v.user_id AND sc.chain_variant_id = v.id
                           AND sc.skipped = FALSE
                    ), 0) AS REAL) / 3600.0 AS total_hours
               FROM chain_variants v
              WHERE v.user_id = ?
              ORDER BY v.chain_id, v.name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }
    pub async fn create_variant(
        &self,
        user_id: i64,
        chain_id: i64,
        name: &str,
        overrides: &[VariantOverride],
    ) -> Result<i64, sqlx::Error> {
        let json = serde_json::to_string(overrides).unwrap_or_else(|_| "[]".into());
        let now = chrono::Utc::now().timestamp();
        let res = sqlx::query(
            "INSERT INTO chain_variants (user_id, chain_id, name, overrides, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(chain_id)
        .bind(name)
        .bind(json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }
    pub async fn update_variant(
        &self,
        user_id: i64,
        id: i64,
        name: Option<&str>,
        overrides: Option<&[VariantOverride]>,
    ) -> Result<bool, sqlx::Error> {
        let mut sets: Vec<&str> = Vec::new();
        if name.is_some() {
            sets.push("name = ?");
        }
        if overrides.is_some() {
            sets.push("overrides = ?");
        }
        if sets.is_empty() {
            return Ok(true);
        }
        let sql =
            format!("UPDATE chain_variants SET {} WHERE user_id = ? AND id = ?", sets.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(v) = name {
            q = q.bind(v.to_string());
        }
        if let Some(v) = overrides {
            q = q.bind(serde_json::to_string(v).unwrap_or_else(|_| "[]".into()));
        }
        let res = q.bind(user_id).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }
    /// Delete a variant, clearing the listens that point at it first.
    ///
    /// Those listens keep their `chain_id` — the chain was still what they went
    /// through — and simply fall back to its base components, which is what a
    /// null variant already means everywhere. Deleting the row without this is
    /// a foreign-key violation, the same trap `delete_chain` documents.
    pub async fn delete_variant(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for table in ["scrobbles", "physical_plays", "physical_releases"] {
            sqlx::query(&format!(
                "UPDATE {table} SET chain_variant_id = NULL
                  WHERE chain_variant_id = ? AND user_id = ?"
            ))
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }
        let res = sqlx::query("DELETE FROM chain_variants WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }
    /// Is this variant the caller's, and does it belong to that chain?
    ///
    /// Both halves matter: a variant naming another chain would resolve to
    /// components that were never fitted, so `equipment_seconds` ignores such a
    /// pairing — better to refuse it at the door than store it and silently
    /// drop it later.
    pub async fn variant_fits_chain(
        &self,
        user_id: i64,
        variant_id: i64,
        chain_id: i64,
    ) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM chain_variants WHERE id = ? AND user_id = ? AND chain_id = ?",
        )
        .bind(variant_id)
        .bind(user_id)
        .bind(chain_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }
    /// Fold one chain into another as a variant of it, moving its listening
    /// with it.
    ///
    /// **This is the half v0.54.0 left out.** Variants shipped as a
    /// forward-looking feature: you build one, and listens tagged with it from
    /// then on credit the gear that was fitted. But the case the feature was
    /// *measured* against already existed as two chains — "Commute Phone"
    /// (Xperia → HD 569) and "Summer Commute" (Xperia → Hexa), differing by one
    /// box, each holding real hours. Creating the variant did nothing to them:
    /// the duplicate chain kept its own listens, its own total, and its own row
    /// on a page whose whole complaint was near-duplicate rows. So the one
    /// situation variants were designed for was the one they could not resolve.
    ///
    /// Every listen, side, session and binding of `from_chain_id` is repointed
    /// at `into_chain_id` carrying `variant_id`, and the source chain is then
    /// deleted. `equipment_seconds` resolves the overrides per `(chain,
    /// variant)` pair, so the moved hours land on the gear that was actually
    /// fitted — which is what makes the two totals add up instead of sitting in
    /// two rows that cannot be compared.
    ///
    /// Four rules, all load-bearing:
    ///
    /// * **A chain with its own variants is refused.** Their `position`s index
    ///   into the source chain's components, and those are about to stop being
    ///   the base. Re-deriving them against the target is possible and would be
    ///   a *guess* about which box was fitted — the one thing this whole
    ///   feature exists to stop guessing at.
    /// * **The variant must belong to the target chain.** The same rule
    ///   `variant_fits_chain` enforces at the door, for the same reason: a
    ///   variant paired with another chain resolves to components that were
    ///   never fitted together, and `equipment_seconds` would ignore it —
    ///   silently crediting the base gear for every listen just moved.
    /// * **Sessions carry no variant**, by design: a session's variant is read
    ///   off its listens, so moving the listens moves it and the two cannot
    ///   disagree.
    /// * **A device default, a token default and a binding carry no variant
    ///   either** — they are standing statements about a *chain*, and there is
    ///   no slot for which stylus is in the headshell. They follow the chain.
    ///
    /// Not reversible. The caller says so before running it.
    pub async fn fold_chain_into_variant(
        &self,
        user_id: i64,
        from_chain_id: i64,
        into_chain_id: i64,
        variant: FoldTarget<'_>,
    ) -> Result<Result<FoldOutcome, String>, sqlx::Error> {
        if from_chain_id == into_chain_id {
            return Ok(Err("A chain cannot be folded into itself".into()));
        }

        let from = self.get_chain(user_id, from_chain_id).await?;
        let into = self.get_chain(user_id, into_chain_id).await?;
        let (Some(from), Some(into)) = (from, into) else {
            return Ok(Err("No such chain".into()));
        };

        // Its variants index into components that are about to stop being the
        // base. Refused rather than re-derived.
        let own: Vec<(i64,)> =
            sqlx::query_as("SELECT id FROM chain_variants WHERE chain_id = ? AND user_id = ?")
                .bind(from_chain_id)
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        if !own.is_empty() {
            return Ok(Err(format!(
                "\"{}\" has {} variant(s) of its own. Their overrides index into its components, \
                 which folding replaces — delete them first, or fold the other way round.",
                from.name,
                own.len()
            )));
        }

        let mut tx = self.pool.begin().await?;
        let now = Self::now_secs();

        let (variant_id, variant_name, created) = match variant {
            FoldTarget::Existing(id) => {
                let row: Option<(i64, String)> = sqlx::query_as(
                    "SELECT id, name FROM chain_variants
                      WHERE id = ? AND user_id = ? AND chain_id = ?",
                )
                .bind(id)
                .bind(user_id)
                .bind(into_chain_id)
                .fetch_optional(&mut *tx)
                .await?;
                match row {
                    Some((id, name)) => (id, name, false),
                    None => {
                        return Ok(Err(
                            "That variant isn't one of the chain you're folding into".into()
                        ))
                    }
                }
            }
            // Derived from the two chains, never from the request: the whole
            // point is that the moved listens credit the gear the source chain
            // actually named.
            FoldTarget::New(name) => {
                let name = name.trim();
                if name.is_empty() {
                    return Ok(Err("The new variant needs a name".into()));
                }
                let overrides = crate::models::derive_overrides(&into.components, &from.components);
                let json = serde_json::to_string(&overrides).unwrap_or_else(|_| "[]".into());
                let res = sqlx::query(
                    "INSERT INTO chain_variants (user_id, chain_id, name, overrides, created_at)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(user_id)
                .bind(into_chain_id)
                .bind(name)
                .bind(json)
                .bind(now)
                .execute(&mut *tx)
                .await;
                match res {
                    Ok(r) => (r.last_insert_rowid(), name.to_string(), true),
                    // `UNIQUE(chain_id, name)`, so this is the ordinary
                    // collision rather than a fault.
                    Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                        return Ok(Err(format!(
                            "\"{}\" already has a variant called \"{name}\"",
                            into.name
                        )))
                    }
                    Err(e) => return Err(e),
                }
            }
        };

        let mut out = FoldOutcome {
            variant_id,
            variant_name,
            variant_created: created,
            ..Default::default()
        };

        // The listening, which is the whole point: chain *and* variant, so the
        // hours resolve against the gear that was fitted.
        for (table, slot) in [
            ("scrobbles", &mut out.listens),
            ("physical_plays", &mut out.sides),
            ("physical_releases", &mut out.releases),
        ] {
            let res = sqlx::query(&format!(
                "UPDATE {table} SET chain_id = ?, chain_variant_id = ?
                  WHERE chain_id = ? AND user_id = ?"
            ))
            .bind(into_chain_id)
            .bind(variant_id)
            .bind(from_chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            *slot = res.rows_affected();
        }

        // Everything that names a chain and has no variant slot. A session's
        // variant is read off its listens, which have just moved.
        for (table, column, slot) in [
            ("sessions", "chain_id", &mut out.sessions),
            ("devices", "default_chain_id", &mut out.devices),
            ("chain_bindings", "chain_id", &mut out.bindings),
            ("tokens", "default_chain_id", &mut out.tokens),
        ] {
            let res = sqlx::query(&format!(
                "UPDATE {table} SET {column} = ? WHERE {column} = ? AND user_id = ?"
            ))
            .bind(into_chain_id)
            .bind(from_chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            *slot = res.rows_affected();
        }

        // No counts reported for these two: a spin is the deck as it is right
        // now and a pairing request is a credential in flight. Both still have
        // to let go before the source chain can be deleted.
        for (table, column) in
            [("physical_spins", "chain_id"), ("pairing_requests", "default_chain_id")]
        {
            sqlx::query(&format!("UPDATE {table} SET {column} = ? WHERE {column} = ?"))
                .bind(into_chain_id)
                .bind(from_chain_id)
                .execute(&mut *tx)
                .await?;
        }

        sqlx::query("DELETE FROM signal_chains WHERE id = ? AND user_id = ?")
            .bind(from_chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(Ok(out))
    }
    /// Chains that differ from another of the caller's by only a step or two.
    ///
    /// The discovery half of the fold, and the same shape the sanitiser uses:
    /// this reports, it never acts. `distance` is how many steps disagree, so 0
    /// is the same rig under two names and 1 is the swapped-box case the whole
    /// feature exists for.
    ///
    /// Ordered so the chain with **more listening** is the base and the smaller
    /// one folds into it. That is only a suggestion — the endpoint takes an
    /// explicit direction — but it is the right default: the base keeps its id,
    /// and repointing the smaller side moves fewer rows.
    pub async fn near_duplicate_chains(
        &self,
        user_id: i64,
    ) -> Result<Vec<FoldSuggestion>, sqlx::Error> {
        // Deliberately **not** `get_chains` + `list_variants`, which is what
        // this was first written as. Both derive hours with a correlated
        // subquery per row, and the join key is `chain_id`, which no index
        // serves — so that version cost one full scan of the history per chain
        // *and* per variant, and the page already pays for one set of those in
        // `get_chains`. On a Pi with the database on an SD card that is the
        // difference this codebase has been caught by twice: no individual
        // statement is slow, there are simply far too many of them.
        //
        // Components and overrides need no scan at all, and the hours come from
        // one grouped pass — the shape `equipment_seconds` already uses.
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, name, components FROM signal_chains WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        let variant_rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT id, chain_id, name, overrides FROM chain_variants WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let totals: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT chain_id, SUM(duration) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND chain_id IS NOT NULL
              GROUP BY chain_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let hours: HashMap<i64, f64> = totals
            .into_iter()
            .filter_map(|(c, s)| Some((c?, s.unwrap_or(0) as f64 / 3600.0)))
            .collect();
        struct Chain {
            id: i64,
            name: String,
            components: Vec<ChainComponent>,
            hours: f64,
        }
        let chains: Vec<Chain> = rows
            .into_iter()
            .map(|(id, name, json)| Chain {
                id,
                name,
                components: serde_json::from_str(&json).unwrap_or_default(),
                hours: hours.get(&id).copied().unwrap_or(0.0),
            })
            .collect();
        let variants: Vec<(i64, i64, String, Vec<VariantOverride>)> = variant_rows
            .into_iter()
            .map(|(id, chain_id, name, json)| {
                (id, chain_id, name, serde_json::from_str(&json).unwrap_or_default())
            })
            .collect();

        let mut out: Vec<FoldSuggestion> = Vec::new();
        for (i, a) in chains.iter().enumerate() {
            for b in chains.iter().skip(i + 1) {
                let (a_id, b_id) = (a.id, b.id);
                let distance = crate::models::chain_distance(&a.components, &b.components);
                // Two steps is where "a near-duplicate" stops being a useful
                // description — past that they are simply different rigs, and a
                // variant that overrides half the chain is a copy wearing an
                // override's clothes.
                if distance > 2 {
                    continue;
                }
                // The bigger history is the base: it keeps its id, and fewer
                // rows move.
                let (base, dupe, base_id, dupe_id) =
                    if a.hours >= b.hours { (a, b, a_id, b_id) } else { (b, a, b_id, a_id) };
                // A chain carrying variants of its own cannot be the one that
                // folds — say so here rather than letting the fold refuse it
                // after the user has picked it.
                let dupe_has_variants = variants.iter().any(|(_, chain, _, _)| *chain == dupe_id);
                let overrides = crate::models::derive_overrides(&base.components, &dupe.components);
                out.push(FoldSuggestion {
                    base_chain_id: base_id,
                    base_name: base.name.clone(),
                    base_hours: base.hours,
                    fold_chain_id: dupe_id,
                    fold_name: dupe.name.clone(),
                    fold_hours: dupe.hours,
                    distance: distance as i64,
                    overrides: overrides.clone(),
                    // A variant of the base that already wires it the way the
                    // duplicate does — which is exactly the state someone is in
                    // when they set variants up and found the old chain still
                    // sitting there.
                    matching_variant: variants
                        .iter()
                        .find(|(_, chain, _, o)| *chain == base_id && *o == overrides)
                        .map(|(id, _, name, _)| (*id, name.clone())),
                    fold_blocked_by_variants: dupe_has_variants,
                });
            }
        }
        out.sort_by(|x, y| {
            x.distance.cmp(&y.distance).then(
                (y.base_hours + y.fold_hours)
                    .partial_cmp(&(x.base_hours + x.fold_hours))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        Ok(out)
    }
    /// Seconds of listening attributable to each piece of gear.
    ///
    /// This used to be one SQL expression: sum the listens of every chain whose
    /// components name the gear. Variants break that, and breaking it is the
    /// point — with two cartridges on one turntable, "which chain was it" no
    /// longer answers "what was fitted". A listen's effective components are
    /// the chain's, **with that listen's variant applied**, so the question is
    /// per `(chain, variant)` pair rather than per chain.
    ///
    /// Folded in Rust rather than pushed into SQL: chains and variants are tens
    /// of rows, the grouped scan is one query, and expressing override
    /// resolution in SQLite would be a `json_each` correlated against a second
    /// `json_each` — unreadable, and this is the query that decides whether a
    /// stylus is due for replacement.
    ///
    /// Skips are excluded, like every other counting query here.
    #[allow(clippy::type_complexity)]
    pub async fn equipment_seconds(
        &self,
        user_id: i64,
    ) -> Result<(HashMap<i64, i64>, HashMap<(i64, i64, Option<i64>), i64>), sqlx::Error> {
        // Every chain's own components, and every variant's overrides.
        let chains: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, components FROM signal_chains WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        let variants: Vec<(i64, i64, String)> =
            sqlx::query_as("SELECT id, chain_id, overrides FROM chain_variants WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;

        let base: HashMap<i64, Vec<ChainComponent>> = chains
            .into_iter()
            .map(|(id, json)| (id, serde_json::from_str(&json).unwrap_or_default()))
            .collect();
        let overrides: HashMap<i64, (i64, Vec<VariantOverride>)> = variants
            .into_iter()
            .map(|(id, chain_id, json)| {
                (id, (chain_id, serde_json::from_str(&json).unwrap_or_default()))
            })
            .collect();

        let grouped: Vec<(Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT chain_id, chain_variant_id, SUM(duration)
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND chain_id IS NOT NULL
              GROUP BY chain_id, chain_variant_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out: HashMap<i64, i64> = HashMap::new();
        // The same walk, remembered rather than only summed: `(gear, chain,
        // variant) -> seconds`. A total is not checkable on its own, and with a
        // variant in play the hours genuinely arrive from more than one chain.
        let mut split: HashMap<(i64, i64, Option<i64>), i64> = HashMap::new();
        for (chain_id, variant_id, secs) in grouped {
            let (Some(chain_id), Some(secs)) = (chain_id, secs) else { continue };
            let Some(components) = base.get(&chain_id) else { continue };
            // A variant belonging to a different chain is ignored rather than
            // applied — it can only mean the listen's chain was repointed after
            // the fact, and applying it would credit gear that was never fitted.
            let applied = variant_id
                .and_then(|v| overrides.get(&v))
                .filter(|(owner, _)| *owner == chain_id)
                .map(|(_, o)| o.as_slice())
                .unwrap_or(&[]);

            // One entry per distinct piece of gear: a chain naming the same
            // thing at two steps must not count its running time twice — the
            // trap `get_equipment` hit when it used a join instead of EXISTS.
            let mut fitted: HashSet<i64> = HashSet::new();
            for (i, c) in components.iter().enumerate() {
                match applied.iter().find(|o| o.position == i as i64) {
                    // An override replaces the step, so the gear it replaced
                    // gets no hours for these listens. That is the whole
                    // feature: the elliptical stylus does not accrue wear while
                    // the microlinear is in the headshell.
                    Some(o) => {
                        if let Some(id) = o.equipment_id {
                            fitted.insert(id);
                        }
                    }
                    None => {
                        if let Some(id) = c.equipment_id {
                            fitted.insert(id);
                        }
                    }
                }
            }
            // An override may also *add* a step the base chain does not have —
            // a cartridge on a chain that never named one.
            for o in applied {
                if o.position as usize >= components.len() {
                    if let Some(id) = o.equipment_id {
                        fitted.insert(id);
                    }
                }
            }

            for id in fitted {
                *out.entry(id).or_insert(0) += secs;
                // Keyed on the variant *as applied*, so a variant paired with
                // the wrong chain — which the `filter` above already refuses to
                // apply — is not reported as a source either.
                let applied_variant = (!applied.is_empty()).then_some(variant_id).flatten();
                *split.entry((id, chain_id, applied_variant)).or_insert(0) += secs;
            }
        }
        Ok((out, split))
    }
    pub async fn get_equipment(&self, user_id: i64) -> Result<Vec<EquipmentRow>, sqlx::Error> {
        let (seconds, split) = self.equipment_seconds(user_id).await?;
        let mut rows: Vec<EquipmentRow> = sqlx::query_as(
            "SELECT e.id, e.user_id, e.name, e.type, e.brand, e.model,
                    0.0 AS total_hours,
                    e.first_used, e.last_used, e.notes, e.purchased_at, e.status
               FROM equipment e
              WHERE e.user_id = ?",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // Names for the breakdown, so a source reads "Commute Phone / Summer"
        // rather than a pair of ids. Two small tables, fetched once.
        let chain_names: HashMap<i64, String> =
            sqlx::query_as("SELECT id, name FROM signal_chains WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();
        let variant_names: HashMap<i64, String> =
            sqlx::query_as("SELECT id, name FROM chain_variants WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();

        for r in &mut rows {
            r.total_hours = seconds.get(&r.id).copied().unwrap_or(0) as f64 / 3600.0;
            let mut sources: Vec<GearHours> = split
                .iter()
                .filter(|((gear, _, _), _)| *gear == r.id)
                .filter_map(|((_, chain, variant), secs)| {
                    Some(GearHours {
                        chain_id: *chain,
                        chain_name: chain_names.get(chain)?.clone(),
                        variant_id: *variant,
                        variant_name: variant.and_then(|v| variant_names.get(&v).cloned()),
                        hours: *secs as f64 / 3600.0,
                    })
                })
                .collect();
            // Largest first, then by name so the order is stable between loads
            // rather than following the hash map's.
            sources.sort_by(|a, b| {
                b.hours
                    .partial_cmp(&a.hours)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.chain_name.cmp(&b.chain_name))
                    .then_with(|| a.variant_name.cmp(&b.variant_name))
            });
            r.sources = sources;
        }
        // Same order the SQL produced, kept so the gear table does not reshuffle.
        rows.sort_by(|a, b| {
            b.total_hours
                .partial_cmp(&a.total_hours)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(rows)
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn update_equipment(
        &self,
        user_id: i64,
        id: i64,
        name: &str,
        equipment_type: &str,
        brand: Option<&str>,
        model: Option<&str>,
        notes: Option<&str>,
        purchased_at: Option<i64>,
        status: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE equipment SET name = ?, type = ?, brand = ?, model = ?, notes = ?,
                    purchased_at = ?, status = ?
              WHERE id = ? AND user_id = ?",
        )
        .bind(name)
        .bind(equipment_type)
        .bind(brand)
        .bind(model)
        .bind(notes)
        .bind(purchased_at)
        .bind(status)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn delete_equipment(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM equipment WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn assign_session(
        &self,
        user_id: i64,
        timestamp: i64,
        duration_secs: i64,
        device_id: Option<i64>,
        chain_id: Option<i64>,
        quality_score: Option<f64>,
        listening_context: &str,
        session_gap_seconds: i64,
        token_id: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        // A session is contiguous listening *through one setup*. The gap is the
        // usual test, but a change of submitting token ends it too: one token
        // per app is how these are organised, so "fooyin stopped, Pano started"
        // is a different setup even if it happened a minute later. A browser
        // submission carries no token and is treated as its own bucket.
        let recent: Option<(i64, i64, Option<i64>)> = sqlx::query_as(
            "SELECT id, ended_at, token_id FROM sessions WHERE user_id = ? AND ended_at >= ? - ? ORDER BY ended_at DESC LIMIT 1"
        ).bind(user_id).bind(timestamp).bind(session_gap_seconds).fetch_optional(&self.pool).await?;
        let ended_at = timestamp + duration_secs;
        let recent = recent.filter(|(_, _, prev_token)| *prev_token == token_id);
        if let Some((session_id, _, _)) = recent {
            sqlx::query(
                "UPDATE sessions SET ended_at = MAX(ended_at, ?), track_count = track_count + 1,
                    total_duration = total_duration + ?,
                    avg_quality_score = CASE WHEN ? IS NOT NULL THEN
                        (COALESCE(avg_quality_score, 0) * track_count + ?) / (track_count + 1)
                    ELSE avg_quality_score END WHERE id = ?",
            )
            .bind(ended_at)
            .bind(duration_secs)
            .bind(quality_score)
            .bind(quality_score)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
            Ok(session_id)
        } else {
            let result = sqlx::query(
                "INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                    track_count, total_duration, avg_quality_score, listening_context, token_id,
                    uuid, updated_at)
                 VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(device_id)
            .bind(chain_id)
            .bind(timestamp)
            .bind(ended_at)
            .bind(duration_secs)
            .bind(quality_score)
            .bind(listening_context)
            .bind(token_id)
            // Every session gets its stable id at birth — see the migration.
            .bind(new_uuid())
            .bind(Self::now_secs())
            .execute(&self.pool)
            .await?;
            Ok(result.last_insert_rowid())
        }
    }
}
