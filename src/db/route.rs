//! Saving a chain's route from the route editor.
//!
//! Chain and gear hours are derived from the listens, so changing a chain's
//! steps changes what every past listen is taken to have run through. The
//! editor makes that a choice:
//!
//! * **From now on** (the default): plays already logged keep the route they
//!   had. The old route is kept as a hidden `auto` variant expressed against
//!   the new one, every listen filed "as wired" is filed with it, every other
//!   variant is re-expressed so it names the same gear it always did, and a
//!   switch at the moment of saving starts the new route with whatever was on
//!   at its slots. The machinery `promote_variant` already uses, so nothing
//!   about gear hours is computed a second way.
//! * **Rewrite past plays too**: the steps are replaced and every play ever
//!   logged here is read through the new route. Swaps follow their gear to
//!   wherever it sits now. Meant for fixing a mistake.
//!
//! Every query is scoped by `user_id`.

use super::fittings::{slots_of, Picks};
use super::*;
use crate::models::{overrides_against, resolve_variant, ChainComponent, VariantOverride};

/// What a route save did.
#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct RouteSaved {
    /// `keep` (past plays keep the old route) or `rewrite`.
    pub past: &'static str,
    /// Plays (skips excluded) now filed with the old route. 0 for `rewrite`.
    pub kept: i64,
    /// The route did not change; only slot options or names were saved.
    pub unchanged: bool,
}

/// The picks a resolved route makes at the new route's slots: whichever of
/// a slot's options the route runs through, else the wired gear.
fn picks_in(new: &[ChainComponent], route: &[ChainComponent]) -> Picks {
    slots_of(new)
        .into_iter()
        .map(|(pos, wired, options)| {
            let on = options
                .iter()
                .copied()
                .find(|eq| *eq != wired && route.iter().any(|c| c.equipment_id == Some(*eq)))
                .unwrap_or(wired);
            (pos, on)
        })
        .collect()
}

/// Two routes are the same wiring: the same steps in the same order.
fn same_route(a: &[ChainComponent], b: &[ChainComponent]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| crate::models::component_matches(x, y))
}

impl Database {
    /// Replace a chain's steps. `keep` is "from now on"; otherwise every
    /// past play is re-read through the new route. Refusals come back as
    /// `Ok(Err(reason))`.
    pub async fn save_route(
        &self,
        user_id: i64,
        chain_id: i64,
        components: &[ChainComponent],
        keep: bool,
    ) -> Result<Result<RouteSaved, String>, sqlx::Error> {
        let Some(chain) = self.get_chain(user_id, chain_id).await? else {
            return Ok(Err("No such chain".into()));
        };
        if components.iter().all(|c| c.name.trim().is_empty()) {
            return Ok(Err("A route needs at least one box".into()));
        }
        let mine: HashSet<i64> = sqlx::query_scalar::<_, i64>(&crate::sql!(
            "SELECT id FROM equipment WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        if components.iter().any(|c| c.equipment_id.is_some_and(|id| !mine.contains(&id))) {
            return Ok(Err("The route names gear that isn't yours".into()));
        }
        let old = chain.components.clone();
        let new = self.normalise_slots(user_id, components, &old).await?;
        let new_json = serde_json::to_string(&new).unwrap_or_else(|_| "[]".into());

        let variant_rows: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, overrides FROM chain_variants WHERE user_id = ? AND chain_id = ?"
        ))
        .bind(user_id)
        .bind(chain_id)
        .fetch_all(&self.pool)
        .await?;
        let variants: Vec<(i64, Vec<VariantOverride>)> = variant_rows
            .into_iter()
            .map(|(id, j)| (id, serde_json::from_str(&j).unwrap_or_default()))
            .collect();

        // Same wiring, or a rewrite: the steps change in place.
        let unchanged = same_route(&old, &new);
        if unchanged || !keep {
            let mut tx = self.pool.begin().await?;
            sqlx::query(&crate::sql!(
                "UPDATE signal_chains SET components = ? WHERE id = ? AND user_id = ?"
            ))
            .bind(&new_json)
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            if !unchanged {
                // A swap follows the gear it swapped out to wherever that gear
                // sits now; a swap of gear that is gone goes with it.
                for (id, ov) in &variants {
                    let moved: Vec<VariantOverride> = ov
                        .iter()
                        .filter_map(|o| {
                            let p = o.position as usize;
                            match old.get(p) {
                                Some(was) => {
                                    let q = new.iter().position(|c| {
                                        if was.equipment_id.is_some() {
                                            c.equipment_id == was.equipment_id
                                        } else {
                                            crate::models::component_matches(c, was)
                                        }
                                    })?;
                                    Some(VariantOverride { position: q as i64, ..o.clone() })
                                }
                                // A step it added past the end stays past the end.
                                None => Some(VariantOverride {
                                    position: (new.len() + p - old.len()) as i64,
                                    ..o.clone()
                                }),
                            }
                        })
                        .collect();
                    if &moved != ov {
                        sqlx::query(&crate::sql!(
                            "UPDATE chain_variants SET overrides = ? WHERE id = ? AND user_id = ?"
                        ))
                        .bind(serde_json::to_string(&moved).unwrap_or_else(|_| "[]".into()))
                        .bind(id)
                        .bind(user_id)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
            tx.commit().await?;
            return Ok(Ok(RouteSaved {
                past: if keep { "keep" } else { "rewrite" },
                kept: 0,
                unchanged,
            }));
        }

        // ── From now on ──
        let now = Self::now_secs();
        let resolved: HashMap<i64, Vec<ChainComponent>> =
            variants.iter().map(|(id, ov)| (*id, resolve_variant(&old, ov))).collect();
        let fittings: Vec<(i64, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT since, variant_id FROM chain_fittings
              WHERE user_id = ? AND chain_id = ? ORDER BY since, id"
        ))
        .bind(user_id)
        .bind(chain_id)
        .fetch_all(&self.pool)
        .await?;
        let on_now = fittings.iter().rev().find(|f| f.0 <= now).and_then(|f| f.1);

        // The rows the move needs, made first: a write through the pool would
        // wait on the transaction's own lock on SQLite. Unreferenced until the
        // transaction commits, and hidden, so a failure leaves nothing visible.
        let facts: HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(&crate::sql!(
            "SELECT id, name FROM equipment WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        let day = chrono::DateTime::from_timestamp(now, 0)
            .map(|d| d.format("%-d %b %Y").to_string())
            .unwrap_or_default();
        let history = self
            .insert_auto_variant(
                user_id,
                chain_id,
                &format!("Route until {day}"),
                &overrides_against(&new, &old),
            )
            .await?;
        // What is on now, and what each output picks by itself, carried onto
        // the new route by gear.
        let mut pick_variant: HashMap<Vec<(i64, i64)>, Option<i64>> = HashMap::new();
        let for_route = |route: &[ChainComponent]| -> Vec<(i64, i64)> {
            picks_in(&new, route)
                .into_iter()
                .filter(|(p, eq)| new[*p as usize].equipment_id != Some(*eq))
                .collect()
        };
        let current_key = for_route(&match on_now.and_then(|v| resolved.get(&v)) {
            Some(r) => r.clone(),
            None => old.clone(),
        });
        let pinned: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT DISTINCT chain_variant_id FROM (
                 SELECT chain_variant_id FROM chain_bindings
                  WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NOT NULL
                 UNION ALL
                 SELECT chain_variant_id FROM device_bindings
                  WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NOT NULL
                 UNION ALL
                 SELECT chain_variant_id FROM physical_releases
                  WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NOT NULL
             ) t"
        ))
        .bind(user_id)
        .bind(chain_id)
        .bind(user_id)
        .bind(chain_id)
        .bind(user_id)
        .bind(chain_id)
        .fetch_all(&self.pool)
        .await?;
        let mut keys: Vec<Vec<(i64, i64)>> = vec![current_key.clone()];
        let mut pinned_key: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
        for (v,) in &pinned {
            if let Some(r) = resolved.get(v) {
                let k = for_route(r);
                pinned_key.insert(*v, k.clone());
                keys.push(k);
            }
        }
        for k in keys {
            if pick_variant.contains_key(&k) {
                continue;
            }
            let id = if k.is_empty() {
                None
            } else {
                let ov: Vec<VariantOverride> = k
                    .iter()
                    .map(|(p, eq)| VariantOverride {
                        position: *p,
                        equipment_id: Some(*eq),
                        name: facts.get(eq).cloned(),
                    })
                    .collect();
                let stem = ov.iter().filter_map(|o| o.name.clone()).collect::<Vec<_>>().join(" + ");
                Some(self.insert_auto_variant(user_id, chain_id, &stem, &ov).await?)
            };
            pick_variant.insert(k, id);
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!(
            "UPDATE signal_chains SET components = ? WHERE id = ? AND user_id = ?"
        ))
        .bind(&new_json)
        .bind(chain_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        // Every variant goes on naming the gear it named.
        for (id, _) in &variants {
            let ov = overrides_against(&new, &resolved[id]);
            sqlx::query(&crate::sql!(
                "UPDATE chain_variants SET overrides = ? WHERE id = ? AND user_id = ?"
            ))
            .bind(serde_json::to_string(&ov).unwrap_or_else(|_| "[]".into()))
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }
        // The plays as wired ran through the old route.
        let kept: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NULL AND skipped = FALSE"
        ))
        .bind(user_id)
        .bind(chain_id)
        .fetch_one(&mut *tx)
        .await?;
        for table in ["scrobbles", "physical_plays", "chain_fittings"] {
            let col = if table == "chain_fittings" { "variant_id" } else { "chain_variant_id" };
            sqlx::query(&crate::sql!(&format!(
                "UPDATE {table} SET {col} = ? WHERE user_id = ? AND chain_id = ? AND {col} IS NULL"
            )))
            .bind(history)
            .bind(user_id)
            .bind(chain_id)
            .execute(&mut *tx)
            .await?;
        }
        // The time before the first switch ran as wired, which is now the
        // old route; a switch from the start says so, so a listen imported
        // later with an old timestamp takes it too and a pick cannot be
        // backdated onto it.
        if fittings.first().is_none_or(|f| f.0 > 0) {
            sqlx::query(&crate::sql!(
                "INSERT INTO chain_fittings (user_id, chain_id, variant_id, since, created_at)
                 VALUES (?, ?, ?, 0, ?)"
            ))
            .bind(user_id)
            .bind(chain_id)
            .bind(history)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(&crate::sql!(
            "INSERT INTO chain_fittings (user_id, chain_id, variant_id, since, created_at)
             VALUES (?, ?, ?, ?, ?)"
        ))
        .bind(user_id)
        .bind(chain_id)
        .bind(pick_variant[&current_key])
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        // Outputs and records that pick gear by themselves keep picking it.
        for (v, k) in &pinned_key {
            for table in ["chain_bindings", "device_bindings", "physical_releases"] {
                sqlx::query(&crate::sql!(&format!(
                    "UPDATE {table} SET chain_variant_id = ?
                      WHERE user_id = ? AND chain_id = ? AND chain_variant_id = ?"
                )))
                .bind(pick_variant[k])
                .bind(user_id)
                .bind(chain_id)
                .bind(v)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(Ok(RouteSaved { past: "keep", kept, unchanged: false }))
    }
}
