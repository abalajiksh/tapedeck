//! What's on now: slots, the picks on them, and when each pick went on.
//!
//! A **slot** is a chain step with two or more `options` (the HD 569, the
//! Hexa, the S08 on a commute). Picking one is not a new chain and not a named
//! setup: it is a fitting, a row in `chain_fittings` saying "from `since`, the
//! chain ran with these picks". The picks are stored as a `chain_variants` row
//! (`auto = TRUE`, never named on screen), because listens already carry
//! `chain_variant_id` and `equipment_seconds` already splits gear hours by
//! `(chain, variant)`. So filing a listen with the right variant is the whole
//! job, and no hours are ever computed a second way.
//!
//! Switching never retires an option: the person switches back and forth, and
//! each option keeps the hours filed to it. A switch can be backdated ("since
//! when?"): the plays from that moment **up to the next recorded switch of the
//! same step** move to the new pick, and later switches stay as they were
//! (Ashwin, 2026-11-07). Other steps' picks are never changed, and a play an
//! output picked by itself (a binding naming a variant) never moves, because
//! only plays filed with the pick in effect at their time are touched.
//!
//! Every query is scoped by `user_id`.

use super::*;
use crate::models::{ChainComponent, ComponentType, VariantOverride};
use std::collections::BTreeMap;

/// What is on at each slot: position → equipment id.
pub type Picks = BTreeMap<i64, i64>;

/// One slot of a chain, as the "what's on now" picker shows it.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct Slot {
    /// The step's index in the chain's components.
    pub position: i64,
    pub slot_name: Option<String>,
    /// The gear the step is wired with, which is what "as wired" and every
    /// listen before the first switch mean.
    pub wired: i64,
    /// The gear on now.
    pub on: i64,
    /// When the gear on now went on, if a switch says so. Null when it has
    /// been on since the chain began (no switch of this step recorded).
    pub on_since: Option<i64>,
    pub options: Vec<SlotOption>,
}

/// One piece of gear that can be on at a slot.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct SlotOption {
    pub equipment_id: i64,
    pub name: String,
    pub brand: Option<String>,
    pub equipment_type: Option<String>,
    /// When you bought it, if the gear record says.
    pub purchased_at: Option<i64>,
    /// Its first play through any chain, skips excluded. "In use since" is
    /// the purchase date when there is one, else this.
    pub first_play: Option<i64>,
    /// Outputs that pick this option by themselves (a Bluetooth name that
    /// says which earbuds), by their reported name.
    pub picked_by: Vec<String>,
}

/// A chain's slots, and what a "since when?" needs to offer.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct WhatsOn {
    pub chain_id: i64,
    pub slots: Vec<Slot>,
    /// When the last play through this chain ended: "right after the last
    /// play here". Null when it has none.
    pub last_play_end: Option<i64>,
}

/// What a switch did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct FitOutcome {
    /// Plays (skips excluded) that are, or would be, filed with the new pick.
    pub moved: i64,
    /// From when the pick counts.
    pub since: i64,
    /// Where the change stops: the next recorded switch of the same step.
    /// Null when there is none, so the new pick is what's on now.
    pub until: Option<i64>,
    /// The new pick is what's on now.
    pub now_on: bool,
}

/// Why a switch was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitRefusal {
    NoChain,
    NotASlot,
    NotAnOption,
    InFuture,
}

/// The steps that are slots: (position, wired gear, options).
pub fn slots_of(components: &[ChainComponent]) -> Vec<(i64, i64, Vec<i64>)> {
    components
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let wired = c.equipment_id?;
            let opts = c.options.as_ref()?;
            (opts.len() >= 2).then(|| (i as i64, wired, opts.clone()))
        })
        .collect()
}

/// The picks a variant's overrides describe, at every slot. A slot the
/// variant does not override is on its wired gear.
pub fn picks_of(components: &[ChainComponent], overrides: &[VariantOverride]) -> Picks {
    slots_of(components)
        .into_iter()
        .map(|(pos, wired, _)| {
            let on = overrides
                .iter()
                .find(|o| o.position == pos)
                .and_then(|o| o.equipment_id)
                .unwrap_or(wired);
            (pos, on)
        })
        .collect()
}

/// A variant's overrides as a comparable set of (position, gear), or None
/// when it does anything a slot pick cannot (adds or removes a step, names
/// free text, overrides a step that is not a slot).
fn pick_set(components: &[ChainComponent], overrides: &[VariantOverride]) -> Option<Picks> {
    let slots: HashMap<i64, i64> =
        slots_of(components).into_iter().map(|(p, w, _)| (p, w)).collect();
    let mut out = Picks::new();
    for o in overrides {
        let wired = slots.get(&o.position)?;
        let eq = o.equipment_id?;
        if eq != *wired {
            out.insert(o.position, eq);
        }
    }
    Some(out)
}

/// Is this step one a near-duplicate chain may differ at and still be the
/// same chain? A different player is a different chain: plays find a chain
/// by what they came from.
fn swappable(c: &ChainComponent) -> bool {
    c.equipment_id.is_some()
        && !matches!(c.component_type, ComponentType::Source | ComponentType::Transport)
}

/// Gear facts the picker needs: name, brand, type, purchased.
type GearFact = (String, Option<String>, Option<String>, Option<i64>);
type GearFacts = HashMap<i64, GearFact>;

/// The `mood` setups the inbox offers to turn into session titles.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InboxSetup {
    pub id: i64,
    pub name: String,
    pub chain_id: i64,
    pub chain_name: String,
    /// Plays filed with it, skips excluded.
    pub plays: i64,
    /// Sittings those plays are in that have no title yet.
    pub sessions: i64,
}

/// Two chains that differ only at one swappable step.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InboxMerge {
    /// The chain kept: the one with more hours.
    pub chain_id: i64,
    pub chain_name: String,
    pub chain_hours: f64,
    /// The chain that becomes an option on it.
    pub other_chain_id: i64,
    pub other_chain_name: String,
    pub other_hours: f64,
    pub position: i64,
    /// The gear at that step on each.
    pub gear: String,
    pub other_gear: String,
}

/// What retiring mood setups did.
#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct MoodsRetired {
    /// Sittings that gained a title.
    pub sessions_titled: i64,
    /// Setups removed.
    pub removed: i64,
}

impl Database {
    /// The pick in effect for a chain at a moment: the variant of the newest
    /// switch at or before `ts`. None when no switch covers it (the chain as
    /// wired). Applied by `save_scrobble` to every listen that arrives with a
    /// chain and no variant.
    pub async fn fitted_variant(
        &self,
        user_id: i64,
        chain_id: i64,
        ts: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(&crate::sql!(
            "SELECT variant_id FROM chain_fittings
              WHERE user_id = ? AND chain_id = ? AND since <= ?
              ORDER BY since DESC, id DESC LIMIT 1"
        ))
        .bind(user_id)
        .bind(chain_id)
        .bind(ts)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    /// Every chain's variants, overrides parsed.
    async fn chain_variant_overrides(
        &self,
        user_id: i64,
        chain_id: i64,
    ) -> Result<HashMap<i64, Vec<VariantOverride>>, sqlx::Error> {
        let rows: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, overrides FROM chain_variants WHERE user_id = ? AND chain_id = ?"
        ))
        .bind(user_id)
        .bind(chain_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, j)| (id, serde_json::from_str(&j).unwrap_or_default()))
            .collect())
    }

    /// A chain's switches, oldest first: (since, variant).
    async fn fittings_of(
        &self,
        user_id: i64,
        chain_id: i64,
    ) -> Result<Vec<(i64, i64, Option<i64>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, since, variant_id FROM chain_fittings
              WHERE user_id = ? AND chain_id = ? ORDER BY since, id"
        ))
        .bind(user_id)
        .bind(chain_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn gear_facts(&self, user_id: i64) -> Result<GearFacts, sqlx::Error> {
        type GearRow = (i64, String, Option<String>, Option<String>, Option<i64>);
        let rows: Vec<GearRow> = sqlx::query_as(&crate::sql!(
            "SELECT id, name, brand, type, purchased_at FROM equipment WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id, n, b, t, p)| (id, (n, b, t, p))).collect())
    }

    /// What is on now at every slot of every chain that has one, for the
    /// chain list and the graph.
    pub async fn current_picks(&self, user_id: i64) -> Result<HashMap<i64, Picks>, sqlx::Error> {
        let chains: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, components FROM signal_chains WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let variants: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, overrides FROM chain_variants WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let variants: HashMap<i64, Vec<VariantOverride>> = variants
            .into_iter()
            .map(|(id, j)| (id, serde_json::from_str(&j).unwrap_or_default()))
            .collect();
        let now = Self::now_secs();
        let latest: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT chain_id, since, variant_id FROM chain_fittings
              WHERE user_id = ? AND since <= ? ORDER BY chain_id, since, id"
        ))
        .bind(user_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        let mut on_now: HashMap<i64, Option<i64>> = HashMap::new();
        for (chain, _, v) in latest {
            on_now.insert(chain, v);
        }
        let mut out = HashMap::new();
        for (id, json) in chains {
            let comps: Vec<ChainComponent> = serde_json::from_str(&json).unwrap_or_default();
            if slots_of(&comps).is_empty() {
                continue;
            }
            let ov = on_now
                .get(&id)
                .copied()
                .flatten()
                .and_then(|v| variants.get(&v))
                .cloned()
                .unwrap_or_default();
            out.insert(id, picks_of(&comps, &ov));
        }
        Ok(out)
    }

    /// One chain's slots, each option's dates, and when its last play ended.
    pub async fn whats_on(
        &self,
        user_id: i64,
        chain_id: i64,
    ) -> Result<Option<WhatsOn>, sqlx::Error> {
        let Some(chain) = self.get_chain(user_id, chain_id).await? else {
            return Ok(None);
        };
        let comps = chain.components;
        let variants = self.chain_variant_overrides(user_id, chain_id).await?;
        let fittings = self.fittings_of(user_id, chain_id).await?;
        let now = Self::now_secs();
        let picks_at = |v: Option<i64>| {
            picks_of(&comps, v.and_then(|v| variants.get(&v)).map(Vec::as_slice).unwrap_or(&[]))
        };
        let facts = self.gear_facts(user_id).await?;
        let first_plays = self.gear_first_plays(user_id).await?;

        // Outputs that pick a variant by themselves, per (position, gear).
        let pinned: Vec<(String, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT identifier, chain_variant_id FROM chain_bindings
              WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NOT NULL
             UNION ALL
             SELECT identifier, chain_variant_id FROM device_bindings
              WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NOT NULL"
        ))
        .bind(user_id)
        .bind(chain_id)
        .bind(user_id)
        .bind(chain_id)
        .fetch_all(&self.pool)
        .await?;

        let mut slots = Vec::new();
        for (pos, wired, options) in slots_of(&comps) {
            // Walk the switches, noting when this step's pick last changed.
            let mut on = wired;
            let mut on_since = None;
            for (_, since, v) in fittings.iter().filter(|f| f.1 <= now) {
                let p = picks_at(*v).get(&pos).copied().unwrap_or(wired);
                if p != on {
                    on = p;
                    on_since = Some(*since);
                }
            }
            let options = options
                .iter()
                .map(|&eq| {
                    let (name, brand, ty, purchased) = facts
                        .get(&eq)
                        .cloned()
                        .unwrap_or_else(|| ("Gone from your gear".into(), None, None, None));
                    let picked_by = pinned
                        .iter()
                        .filter(|(_, v)| picks_at(*v).get(&pos) == Some(&eq) && eq != wired)
                        .map(|(id, _)| id.clone())
                        .collect();
                    SlotOption {
                        equipment_id: eq,
                        name,
                        brand,
                        equipment_type: ty,
                        purchased_at: purchased,
                        first_play: first_plays.get(&eq).copied(),
                        picked_by,
                    }
                })
                .collect();
            slots.push(Slot {
                position: pos,
                slot_name: comps[pos as usize].slot_name.clone(),
                wired,
                on,
                on_since,
                options,
            });
        }

        let last: Option<(Option<i64>,)> = sqlx::query_as(&crate::sql!(
            "SELECT MAX(timestamp + COALESCE(duration, 0)) FROM scrobbles
              WHERE user_id = ? AND chain_id = ? AND skipped = FALSE"
        ))
        .bind(user_id)
        .bind(chain_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(Some(WhatsOn { chain_id, slots, last_play_end: last.and_then(|r| r.0) }))
    }

    /// Each piece of gear's first play through any chain, skips excluded.
    ///
    /// One grouped pass for the earliest listen per (chain, variant), each
    /// pair resolved to the gear it ran through. A box added to a chain later
    /// (a component `since`) counts from that date, not from the chain's
    /// first play.
    async fn gear_first_plays(&self, user_id: i64) -> Result<HashMap<i64, i64>, sqlx::Error> {
        let firsts: Vec<(Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT chain_id, chain_variant_id, MIN(timestamp) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND chain_id IS NOT NULL
              GROUP BY chain_id, chain_variant_id"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let chains: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, components FROM signal_chains WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let chains: HashMap<i64, Vec<ChainComponent>> = chains
            .into_iter()
            .map(|(id, j)| (id, serde_json::from_str(&j).unwrap_or_default()))
            .collect();
        let variants: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, overrides FROM chain_variants WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let variants: HashMap<i64, Vec<VariantOverride>> = variants
            .into_iter()
            .map(|(id, j)| (id, serde_json::from_str(&j).unwrap_or_default()))
            .collect();
        let mut out: HashMap<i64, i64> = HashMap::new();
        for (chain, variant, first) in firsts {
            let (Some(chain), Some(first)) = (chain, first) else { continue };
            let Some(base) = chains.get(&chain) else { continue };
            let comps = match variant.and_then(|v| variants.get(&v)) {
                Some(ov) => crate::models::resolve_variant(base, ov),
                None => base.clone(),
            };
            for c in comps {
                if let Some(eq) = c.equipment_id {
                    let ts = first.max(c.since.unwrap_or(first));
                    out.entry(eq).and_modify(|t| *t = (*t).min(ts)).or_insert(ts);
                }
            }
        }
        Ok(out)
    }

    /// The variant for a set of picks, made if no variant of the chain says
    /// exactly that yet. None for the chain as wired.
    async fn variant_for_picks(
        &self,
        user_id: i64,
        chain_id: i64,
        comps: &[ChainComponent],
        picks: &Picks,
        create: bool,
    ) -> Result<Option<i64>, sqlx::Error> {
        let slots: HashMap<i64, i64> =
            slots_of(comps).into_iter().map(|(p, w, _)| (p, w)).collect();
        let target: Picks = picks
            .iter()
            .filter(|(p, eq)| slots.get(p).is_some_and(|w| w != *eq))
            .map(|(p, eq)| (*p, *eq))
            .collect();
        if target.is_empty() {
            return Ok(None);
        }
        let variants = self.chain_variant_overrides(user_id, chain_id).await?;
        let mut ids: Vec<&i64> = variants.keys().collect();
        ids.sort();
        for id in ids {
            if pick_set(comps, &variants[id]).as_ref() == Some(&target) {
                return Ok(Some(*id));
            }
        }
        if !create {
            return Ok(None);
        }
        let facts = self.gear_facts(user_id).await?;
        let overrides: Vec<VariantOverride> = target
            .iter()
            .map(|(p, eq)| VariantOverride {
                position: *p,
                equipment_id: Some(*eq),
                name: facts.get(eq).map(|f| f.0.clone()),
            })
            .collect();
        // Named after the gear it puts on, because a few older screens and
        // the session API print the name as it is; unique per chain only
        // because the column is.
        let stem = overrides
            .iter()
            .map(|o| o.name.clone().unwrap_or_else(|| "gear".into()))
            .collect::<Vec<_>>()
            .join(" + ");
        let json = serde_json::to_string(&overrides).unwrap_or_else(|_| "[]".into());
        for n in 1..50 {
            let name = if n == 1 { stem.clone() } else { format!("{stem} ({n})") };
            let res = sqlx::query_as::<_, (i64,)>(&crate::sql!(
                "INSERT INTO chain_variants (uuid, user_id, chain_id, name, overrides, created_at, auto)
                 VALUES (?, ?, ?, ?, ?, ?, TRUE) RETURNING id"
            ))
            .bind(new_uuid())
            .bind(user_id)
            .bind(chain_id)
            .bind(&name)
            .bind(&json)
            .bind(Self::now_secs())
            .fetch_all(&self.pool)
            .await;
            match res {
                Ok(r) => return Ok(r.first().map(|r| r.0)),
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(e) => return Err(e),
            }
        }
        Err(sqlx::Error::Protocol("could not name a variant for those picks".into()))
    }

    /// Put `equipment_id` on at a slot from `since` (now when None).
    ///
    /// With `dry_run`, nothing is written and the outcome says how many plays
    /// would move. Otherwise the switch is recorded and the plays from `since`
    /// up to the next recorded switch of this step are filed with the new
    /// pick: only those filed with the pick in effect at their time, so a play
    /// an output picked by itself stays put.
    pub async fn fit(
        &self,
        user_id: i64,
        chain_id: i64,
        position: i64,
        equipment_id: i64,
        since: Option<i64>,
        dry_run: bool,
    ) -> Result<Result<FitOutcome, FitRefusal>, sqlx::Error> {
        let now = Self::now_secs();
        let t = since.unwrap_or(now);
        if t > now + 60 {
            return Ok(Err(FitRefusal::InFuture));
        }
        let Some(chain) = self.get_chain(user_id, chain_id).await? else {
            return Ok(Err(FitRefusal::NoChain));
        };
        let comps = chain.components;
        let Some((_, wired, options)) = slots_of(&comps).into_iter().find(|s| s.0 == position)
        else {
            return Ok(Err(FitRefusal::NotASlot));
        };
        if !options.contains(&equipment_id) {
            return Ok(Err(FitRefusal::NotAnOption));
        }
        let variants = self.chain_variant_overrides(user_id, chain_id).await?;
        let fittings = self.fittings_of(user_id, chain_id).await?;
        let picks_at = |v: Option<i64>| {
            picks_of(&comps, v.and_then(|v| variants.get(&v)).map(Vec::as_slice).unwrap_or(&[]))
        };
        let effective_at = |ts: i64| -> Option<i64> {
            fittings.iter().rev().find(|f| f.1 <= ts).and_then(|f| f.2)
        };

        // Where the change stops: the first later switch at which this step's
        // pick changes, read on the history as it stands.
        let mut prev = picks_at(effective_at(t)).get(&position).copied().unwrap_or(wired);
        let mut until = None;
        for (_, s, v) in fittings.iter().filter(|f| f.1 > t) {
            let p = picks_at(*v).get(&position).copied().unwrap_or(wired);
            if p != prev {
                until = Some(*s);
                break;
            }
            prev = p;
        }
        let in_range = |ts: i64| ts >= t && until.is_none_or(|u| ts < u);

        // The stretches of [t, until) and the pick each ran with.
        let mut starts: Vec<i64> = vec![t];
        starts.extend(fittings.iter().map(|f| f.1).filter(|s| *s > t && in_range(*s)));
        starts.dedup();
        let mut segments: Vec<(i64, Option<i64>, Option<i64>)> = Vec::new();
        for (i, s) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().or(until);
            segments.push((*s, end, effective_at(*s)));
        }
        let with_pick = |v: Option<i64>| {
            let mut p = picks_at(v);
            p.insert(position, equipment_id);
            p
        };

        let mut out = FitOutcome { since: t, until, now_on: until.is_none(), ..Default::default() };

        // Count first: the same `WHERE` the move uses, skips left out.
        let changes = |v: Option<i64>| with_pick(v) != picks_at(v);
        for (s, end, was) in segments.iter().filter(|seg| changes(seg.2)) {
            let was_sql =
                if was.is_some() { "chain_variant_id = ?" } else { "chain_variant_id IS NULL" };
            let end_sql = if end.is_some() { " AND timestamp < ?" } else { "" };
            let count_sql = format!(
                "SELECT COUNT(*) FROM scrobbles
                  WHERE user_id = ? AND chain_id = ? AND skipped = FALSE
                    AND timestamp >= ?{end_sql} AND {was_sql}"
            );
            let stmt = crate::sql!(&count_sql);
            let mut q = sqlx::query_scalar::<_, i64>(&stmt).bind(user_id).bind(chain_id).bind(*s);
            if let Some(e) = end {
                q = q.bind(*e);
            }
            if let Some(v) = was {
                q = q.bind(*v);
            }
            out.moved += q.fetch_one(&self.pool).await?;
        }
        if dry_run {
            return Ok(Ok(out));
        }

        // Every variant the move needs, made before the transaction: on
        // SQLite a write through the pool would wait on the transaction's own
        // lock.
        let mut needed: Vec<Option<i64>> = segments.iter().map(|seg| seg.2).collect();
        needed.extend(fittings.iter().filter(|f| in_range(f.1)).map(|f| f.2));
        needed.push(effective_at(t));
        let mut remap: HashMap<Option<i64>, Option<i64>> = HashMap::new();
        for was in needed {
            if let std::collections::hash_map::Entry::Vacant(e) = remap.entry(was) {
                let new = self
                    .variant_for_picks(user_id, chain_id, &comps, &with_pick(was), true)
                    .await?;
                e.insert(new);
            }
        }

        let mut tx = self.pool.begin().await?;
        for (s, end, was) in segments.iter().filter(|seg| changes(seg.2)) {
            let new = remap[was];
            let was_sql =
                if was.is_some() { "chain_variant_id = ?" } else { "chain_variant_id IS NULL" };
            for (table, col) in [("scrobbles", "timestamp"), ("physical_plays", "played_at")] {
                let end_sql = if end.is_some() { format!(" AND {col} < ?") } else { String::new() };
                let sql = format!(
                    "UPDATE {table} SET chain_variant_id = ?
                      WHERE user_id = ? AND chain_id = ? AND {col} >= ?{end_sql} AND {was_sql}"
                );
                let stmt = crate::sql!(&sql);
                let mut q = sqlx::query(&stmt).bind(new).bind(user_id).bind(chain_id).bind(*s);
                if let Some(e) = end {
                    q = q.bind(*e);
                }
                if let Some(v) = was {
                    q = q.bind(*v);
                }
                q.execute(&mut *tx).await?;
            }
        }
        // The switches inside the range now carry the new pick too, and one
        // marks where it starts.
        for (id, _, v) in fittings.iter().filter(|f| in_range(f.1)) {
            sqlx::query(&crate::sql!(
                "UPDATE chain_fittings SET variant_id = ? WHERE id = ? AND user_id = ?"
            ))
            .bind(remap[v])
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }
        if !fittings.iter().any(|f| f.1 == t) {
            sqlx::query(&crate::sql!(
                "INSERT INTO chain_fittings (user_id, chain_id, variant_id, since, created_at)
                 VALUES (?, ?, ?, ?, ?)"
            ))
            .bind(user_id)
            .bind(chain_id)
            .bind(remap[&effective_at(t)])
            .bind(t)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Ok(out))
    }

    /// Tidy the slot options a save sends: the wired gear first, your own
    /// gear only, no repeats, and fewer than two means not a slot. A step that
    /// arrives with no `options` keeps the ones it had (matched by gear).
    pub(super) async fn normalise_slots(
        &self,
        user_id: i64,
        components: &[ChainComponent],
        was: &[ChainComponent],
    ) -> Result<Vec<ChainComponent>, sqlx::Error> {
        let mine: HashSet<i64> = sqlx::query_scalar::<_, i64>(&crate::sql!(
            "SELECT id FROM equipment WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        Ok(components
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut c = c.clone();
                if c.options.is_none() {
                    let before =
                        was.get(i).filter(|w| w.equipment_id == c.equipment_id).or_else(|| {
                            was.iter().find(|w| {
                                w.equipment_id.is_some() && w.equipment_id == c.equipment_id
                            })
                        });
                    if let Some(w) = before {
                        c.options = w.options.clone();
                        if c.slot_name.is_none() {
                            c.slot_name = w.slot_name.clone();
                        }
                    }
                }
                let Some(wired) = c.equipment_id else {
                    c.options = None;
                    c.slot_name = None;
                    return c;
                };
                let mut opts = vec![wired];
                for id in c.options.iter().flatten() {
                    if mine.contains(id) && !opts.contains(id) {
                        opts.push(*id);
                    }
                }
                if opts.len() >= 2 {
                    c.options = Some(opts);
                    c.slot_name = c
                        .slot_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(|n| n.chars().take(60).collect());
                } else {
                    c.options = None;
                    c.slot_name = None;
                }
                c
            })
            .collect())
    }

    /// Turn named setups that swap gear into slot options (phase 3's one-time
    /// migration, run on read so it also catches a setup made through the old
    /// API). Each overriding gear joins the options of the step it overrides,
    /// and the setup becomes an `auto` variant: its row and every listen filed
    /// with it stay, so no hour moves. Setups that change no gear are left for
    /// the inbox to offer as session titles; ones that add or remove a step
    /// are left as they are.
    pub async fn adopt_setups(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        let named: Vec<(i64, i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, chain_id, overrides FROM chain_variants
              WHERE user_id = ? AND auto = FALSE"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut adopted = 0;
        for (id, chain_id, json) in named {
            let overrides: Vec<VariantOverride> = serde_json::from_str(&json).unwrap_or_default();
            let Some(chain) = self.get_chain(user_id, chain_id).await? else { continue };
            let mut comps = chain.components;
            let swaps_gear = !overrides.is_empty()
                && overrides.iter().all(|o| {
                    comps.get(o.position as usize).is_some_and(|c| {
                        c.equipment_id.is_some()
                            && o.equipment_id.is_some()
                            && o.equipment_id != c.equipment_id
                    })
                });
            if !swaps_gear {
                continue;
            }
            for o in &overrides {
                let c = &mut comps[o.position as usize];
                let mut opts = c.options.clone().unwrap_or_else(|| vec![c.equipment_id.unwrap()]);
                let eq = o.equipment_id.unwrap();
                if !opts.contains(&eq) {
                    opts.push(eq);
                }
                c.options = Some(opts);
            }
            let mut tx = self.pool.begin().await?;
            sqlx::query(&crate::sql!(
                "UPDATE signal_chains SET components = ? WHERE id = ? AND user_id = ?"
            ))
            .bind(serde_json::to_string(&comps).unwrap_or_else(|_| "[]".into()))
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(&crate::sql!(
                "UPDATE chain_variants SET auto = TRUE WHERE id = ? AND user_id = ?"
            ))
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            adopted += 1;
        }
        Ok(adopted)
    }

    /// Named setups that change no gear (a mood, not a box), with how much
    /// was filed with them.
    pub async fn mood_setups(&self, user_id: i64) -> Result<Vec<InboxSetup>, sqlx::Error> {
        let named: Vec<(i64, i64, String, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, chain_id, name, overrides FROM chain_variants
              WHERE user_id = ? AND auto = FALSE ORDER BY chain_id, name"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for (id, chain_id, name, json) in named {
            let overrides: Vec<VariantOverride> = serde_json::from_str(&json).unwrap_or_default();
            let Some(chain) = self.get_chain(user_id, chain_id).await? else { continue };
            let no_gear = overrides.iter().all(|o| {
                chain
                    .components
                    .get(o.position as usize)
                    .is_some_and(|c| c.equipment_id.is_some() && o.equipment_id == c.equipment_id)
            });
            if !no_gear {
                continue;
            }
            let (plays, sessions): (i64, i64) = sqlx::query_as(&crate::sql!(
                "SELECT COUNT(*), COUNT(DISTINCT sc.session_id) FROM scrobbles sc
                   LEFT JOIN sessions se ON se.id = sc.session_id
                  WHERE sc.user_id = ? AND sc.chain_variant_id = ? AND sc.skipped = FALSE
                    AND (sc.session_id IS NULL OR se.title IS NULL)"
            ))
            .bind(user_id)
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
            out.push(InboxSetup {
                id,
                name,
                chain_id,
                chain_name: chain.name.clone(),
                plays,
                sessions,
            });
        }
        Ok(out)
    }

    /// Carry mood setups onto their sittings as titles (only sittings with
    /// none), then remove them. Their plays keep their chain; the gear was
    /// the same, so no hour moves.
    pub async fn retire_mood_setups(
        &self,
        user_id: i64,
        ids: &[i64],
    ) -> Result<MoodsRetired, sqlx::Error> {
        let moods = self.mood_setups(user_id).await?;
        let mut out = MoodsRetired::default();
        for m in moods.iter().filter(|m| ids.contains(&m.id)) {
            out.sessions_titled += sqlx::query(&crate::sql!(
                "UPDATE sessions SET title = ?, updated_at = ?
                  WHERE user_id = ? AND title IS NULL AND id IN (
                        SELECT session_id FROM scrobbles
                         WHERE user_id = ? AND chain_variant_id = ? AND skipped = FALSE
                           AND session_id IS NOT NULL)"
            ))
            .bind(&m.name)
            .bind(Self::now_secs())
            .bind(user_id)
            .bind(user_id)
            .bind(m.id)
            .execute(&self.pool)
            .await?
            .rows_affected() as i64;
            if self.delete_variant(user_id, m.id).await? {
                out.removed += 1;
            }
        }
        Ok(out)
    }

    /// Chains that differ from another only at one swappable step: the case
    /// a slot exists for. The one with more hours is kept.
    pub async fn merge_candidates(&self, user_id: i64) -> Result<Vec<InboxMerge>, sqlx::Error> {
        let suggestions = self.near_duplicate_chains(user_id).await?;
        let chains = self.get_chains(user_id).await?;
        let comps: HashMap<i64, &Vec<ChainComponent>> =
            chains.iter().filter_map(|c| Some((c.id?, &c.components))).collect();
        let mut out = Vec::new();
        for s in suggestions {
            if s.distance != 1 || s.fold_blocked_by_variants || s.overrides.len() != 1 {
                continue;
            }
            let (Some(a), Some(b)) = (comps.get(&s.base_chain_id), comps.get(&s.fold_chain_id))
            else {
                continue;
            };
            let pos = s.overrides[0].position as usize;
            let (Some(x), Some(y)) = (a.get(pos), b.get(pos)) else { continue };
            if a.len() != b.len() || !swappable(x) || !swappable(y) {
                continue;
            }
            out.push(InboxMerge {
                chain_id: s.base_chain_id,
                chain_name: s.base_name,
                chain_hours: s.base_hours,
                other_chain_id: s.fold_chain_id,
                other_chain_name: s.fold_name,
                other_hours: s.fold_hours,
                position: pos as i64,
                gear: x.name.clone(),
                other_gear: y.name.clone(),
            });
        }
        Ok(out)
    }

    /// Merge `from` into `into` as an option at the one step they differ at.
    /// Its listens, outputs, rooms and defaults move, filed with that option,
    /// and `from` is deleted. Not reversible.
    pub async fn merge_as_option(
        &self,
        user_id: i64,
        into: i64,
        from: i64,
    ) -> Result<Result<FoldOutcome, String>, sqlx::Error> {
        let Some(m) = self.merge_candidates(user_id).await?.into_iter().find(|m| {
            (m.chain_id == into && m.other_chain_id == from)
                || (m.chain_id == from && m.other_chain_id == into)
        }) else {
            return Ok(Err("Those two chains differ at more than one step, or at the \
                           player. Only a chain that swaps one box can become an option."
                .into()));
        };
        let Some(base) = self.get_chain(user_id, into).await? else {
            return Ok(Err("No such chain".into()));
        };
        let Some(other) = self.get_chain(user_id, from).await? else {
            return Ok(Err("No such chain".into()));
        };
        let pos = m.position;
        let eq = other.components[pos as usize].equipment_id.unwrap_or_default();
        // The option on the kept chain first, so the variant is a slot pick.
        let mut comps = base.components.clone();
        {
            let c = &mut comps[pos as usize];
            let mut opts = c.options.clone().unwrap_or_else(|| vec![c.equipment_id.unwrap()]);
            if !opts.contains(&eq) {
                opts.push(eq);
            }
            c.options = Some(opts);
        }
        sqlx::query(&crate::sql!(
            "UPDATE signal_chains SET components = ? WHERE id = ? AND user_id = ?"
        ))
        .bind(serde_json::to_string(&comps).unwrap_or_else(|_| "[]".into()))
        .bind(into)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        let picks: Picks = picks_of(&comps, &[])
            .into_iter()
            .map(|(p, w)| (p, if p == pos { eq } else { w }))
            .collect();
        let variant = self.variant_for_picks(user_id, into, &comps, &picks, true).await?;
        let Some(variant) = variant else {
            return Ok(Err("That chain is wired the same at that step".into()));
        };
        self.fold_chain_into_variant(user_id, from, into, FoldTarget::Existing(variant)).await
    }

    /// The inbox's questions about slots, after the ones about unfiled plays:
    /// chains that should be one chain with an option (`merge`), then the
    /// named setups that change no gear (`moods`). Both are asked once; "not
    /// now" puts them off like any other.
    pub async fn slot_questions(&self, user_id: i64) -> Result<Vec<InboxQuestion>, sqlx::Error> {
        let dismissed: HashSet<String> = sqlx::query_scalar::<_, String>(&crate::sql!(
            "SELECT key FROM chain_inbox_dismissed WHERE user_id = ? AND until > ?"
        ))
        .bind(user_id)
        .bind(Self::now_secs())
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        let mut out = Vec::new();
        for m in self.merge_candidates(user_id).await? {
            let key = format!("merge:{}:{}", m.chain_id, m.other_chain_id);
            if dismissed.contains(&key) {
                continue;
            }
            let mut q = InboxQuestion::new(key, "merge");
            q.merge = Some(m);
            out.push(q);
        }
        let moods = self.mood_setups(user_id).await?;
        if !moods.is_empty() && !dismissed.contains("moods") {
            let mut q = InboxQuestion::new("moods".into(), "moods");
            q.setups = moods;
            out.push(q);
        }
        Ok(out)
    }
}
