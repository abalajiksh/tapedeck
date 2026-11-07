//! Where a listen came from, which box a reporter is, outputs per box, and
//! the Signal Chains inbox that asks about plays that landed in no chain.
//!
//! **The inbox is computed on read.** Its questions come from the listens
//! themselves (`chain_id IS NULL`, the last 30 days) grouped by where they came
//! from; only "not now" is stored. Nothing here ever assigns a chain or ties a
//! reporter to a box without an answer: a match on a name is a suggestion.
//!
//! Every query is scoped by `user_id`; skips are never asked about.

use super::*;

/// How far back the inbox looks, and therefore how many plays an answer's
/// "also file them" moves: exactly the ones it counted.
pub const INBOX_WINDOW_SECS: i64 = 30 * 86_400;
/// How long "not now" puts a question off.
pub const INBOX_SNOOZE_SECS: i64 = 30 * 86_400;

/// Gear types a reporter can be: the boxes plays start from.
const SOURCE_TYPES: &[&str] =
    &["phone", "computer", "dap", "source", "transport", "turntable", "radio", "receiver"];

/// Unfiled plays from one place: device, token, output, source name, then
/// how many and the first and last of them.
type UnfiledGroup = (Option<i64>, Option<i64>, Option<String>, String, i64, i64, i64);
/// A device's id, machine id, name, platform, product and box.
type DeviceNames = (i64, String, Option<String>, Option<String>, Option<String>, Option<i64>);
/// A piece of gear's id, name, type, model and aliases.
type GearNames = (i64, String, String, Option<String>, Option<String>);

/// Where a listen came from, as ingest saw it.
pub struct ListenOrigin<'a> {
    pub token_id: Option<i64>,
    pub output: Option<&'a str>,
    /// The ladder rung that decided the chain: `explicit`, `device_output`,
    /// `output_binding`, `token_default`, `device_default` or `none`.
    pub chain_source: &'a str,
}

/// An output seen from one box, and the chain it is mapped to.
#[derive(Debug, FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct DeviceBindingRow {
    pub id: i64,
    /// The box it was seen from.
    pub equipment_id: i64,
    pub equipment_name: String,
    pub identifier: String,
    pub interface: Option<String>,
    /// Null when seen but not mapped yet.
    pub chain_id: Option<i64>,
    /// Which setup of that chain it carries; null is the chain as wired.
    pub chain_variant_id: Option<i64>,
    pub last_seen: i64,
    pub hits: i64,
}

/// One device or token, as the inbox names it.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InboxReporter {
    /// `device` or `token`.
    pub kind: &'static str,
    pub id: i64,
    pub name: String,
    /// What it is: the platform for a device, "a token" for a token.
    pub sub: String,
    /// The box it is already tied to, if any.
    pub equipment_id: Option<i64>,
}

/// One question in the inbox.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InboxQuestion {
    /// Stable for as long as the question stands; what "not now" stores.
    pub key: String,
    /// `same_box` — are these reporters this box? `which_box` — which box is
    /// this reporter? `where` — plays landed nowhere; where were you?
    /// `merge` — two chains differ at one box; make it an option? `moods` —
    /// named setups that change no gear; make them session titles? `spool` —
    /// Shared Spool plays on no chain; file them At a friend's?
    pub kind: &'static str,
    /// `same_box`: the box suggested, by a name or model it matches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_name: Option<String>,
    /// `same_box`: every reporter not yet tied to a box that matches it.
    /// `which_box`: the one reporter asked about.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reporters: Vec<InboxReporter>,
    /// `where`: how many unfiled plays, and over what span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plays: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<i64>,
    /// `where`: the start of the window counted. Send it back with the answer
    /// so "also file them" moves exactly the plays that were counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// `where`: what the plays came through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<InboxReporter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<InboxReporter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    /// `where`: what an answer will set so future plays find the chain:
    /// `device_output` (an output of a box), `output` (an output name from
    /// anything), `token`, `device`, or `none` when nothing identifies the
    /// source and only the counted plays can be filed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<&'static str>,
    /// `merge`: the two chains and the step they differ at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge: Option<super::InboxMerge>,
    /// `moods`: every setup that changes no gear.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub setups: Vec<super::InboxSetup>,
    /// `spool`: accepted Shared Spool plays on no chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spool: Option<super::InboxSpool>,
}

impl InboxQuestion {
    pub(super) fn new(key: String, kind: &'static str) -> Self {
        InboxQuestion {
            key,
            kind,
            equipment_id: None,
            equipment_name: None,
            reporters: Vec::new(),
            plays: None,
            first: None,
            last: None,
            since: None,
            device: None,
            token: None,
            output: None,
            source_name: None,
            rule: None,
            merge: None,
            setups: Vec::new(),
            spool: None,
        }
    }
}

/// What a `where` answer did.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct WhereAnswered {
    /// The rule written, as in `InboxQuestion.rule`.
    pub rule: &'static str,
    /// How many counted plays were filed. Zero unless asked.
    pub moved: u64,
}

/// Lowercase letters and digits only, so "XQ-DQ54", "xq dq54" and
/// "TIDAL on XQ-DQ54" can be compared.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Whether any of a box's names appears in any of a reporter's. Names under
/// three characters never match: "M3" is in too many things.
fn names_match(box_names: &[String], reporter_names: &[String]) -> bool {
    box_names
        .iter()
        .map(|n| squash(n))
        .filter(|n| n.chars().count() >= 3)
        .any(|needle| reporter_names.iter().any(|h| squash(h).contains(&needle)))
}

/// A box's aliases from the stored JSON array.
pub fn parse_aliases(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|r| serde_json::from_str::<Vec<String>>(r).ok()).unwrap_or_default()
}

impl Database {
    /// Note where a just-saved listen came from. A separate statement, like
    /// `set_listen_playback`, rather than three more arguments on
    /// `save_scrobble`.
    pub async fn set_listen_origin(
        &self,
        user_id: i64,
        source_id: &str,
        source_name: &str,
        origin: &ListenOrigin<'_>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET token_id = ?, output_identifier = ?, chain_source = ?
              WHERE user_id = ? AND source_id = ? AND source_name = ?"
        ))
        .bind(origin.token_id)
        .bind(origin.output)
        .bind(origin.chain_source)
        .bind(user_id)
        .bind(source_id)
        .bind(source_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The box a listen's reporter is tied to: the device's, else the token's.
    /// The device is the finer statement — one token can serve several
    /// machines, one machine is one box.
    pub async fn source_equipment(
        &self,
        user_id: i64,
        device_id: Option<i64>,
        token_id: Option<i64>,
    ) -> Result<Option<i64>, sqlx::Error> {
        if let Some(d) = device_id {
            let eq: Option<Option<i64>> = sqlx::query_scalar(&crate::sql!(
                "SELECT equipment_id FROM devices WHERE id = ? AND user_id = ?"
            ))
            .bind(d)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(Some(eq)) = eq {
                return Ok(Some(eq));
            }
        }
        if let Some(t) = token_id {
            let eq: Option<Option<i64>> = sqlx::query_scalar(&crate::sql!(
                "SELECT equipment_id FROM tokens WHERE id = ? AND user_id = ?"
            ))
            .bind(t)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
            if let Some(Some(eq)) = eq {
                return Ok(Some(eq));
            }
        }
        Ok(None)
    }

    /// `record_output` for a reporter tied to a box: the output is learned per
    /// box, so two phones' "Phone speaker" are two rows.
    pub async fn record_device_output(
        &self,
        user_id: i64,
        equipment_id: i64,
        identifier: &str,
        interface: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO device_bindings
                    (user_id, equipment_id, identifier, interface, chain_id, last_seen, hits)
             VALUES (?, ?, ?, ?, NULL, ?, 1)
             ON CONFLICT(user_id, equipment_id, identifier) DO UPDATE SET
                hits = device_bindings.hits + 1, last_seen = excluded.last_seen,
                interface = COALESCE(excluded.interface, device_bindings.interface)",
        ))
        .bind(user_id)
        .bind(equipment_id)
        .bind(identifier)
        .bind(interface)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rung 2 of the ladder: an output of this box mapped to a chain.
    pub async fn resolve_device_binding(
        &self,
        user_id: i64,
        equipment_id: i64,
        identifier: &str,
    ) -> Result<Option<(i64, Option<i64>)>, sqlx::Error> {
        let row: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT chain_id, chain_variant_id FROM device_bindings
              WHERE user_id = ? AND equipment_id = ? AND identifier = ?"
        ))
        .bind(user_id)
        .bind(equipment_id)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(c, v)| c.map(|c| (c, v))))
    }

    pub async fn list_device_bindings(
        &self,
        user_id: i64,
    ) -> Result<Vec<DeviceBindingRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT b.id, b.equipment_id, e.name AS equipment_name, b.identifier, b.interface,
                    b.chain_id, b.chain_variant_id, b.last_seen, b.hits
               FROM device_bindings b JOIN equipment e ON e.id = b.equipment_id
              WHERE b.user_id = ?
              ORDER BY e.name, b.chain_id IS NOT NULL, b.last_seen DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Map (or clear) an output of a box. Creates the row if the output has not
    /// been seen yet. The chain and setup are written together, as
    /// `set_binding` writes them; the caller checks both are the user's and fit.
    pub async fn set_device_binding(
        &self,
        user_id: i64,
        equipment_id: i64,
        identifier: &str,
        chain_id: Option<i64>,
        chain_variant_id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let chain_variant_id = chain_id.and(chain_variant_id);
        sqlx::query(&crate::sql!(
            "INSERT INTO device_bindings
                    (user_id, equipment_id, identifier, chain_id, chain_variant_id, last_seen, hits)
             VALUES (?, ?, ?, ?, ?, ?, 0)
             ON CONFLICT(user_id, equipment_id, identifier) DO UPDATE SET
                chain_id = excluded.chain_id, chain_variant_id = excluded.chain_variant_id",
        ))
        .bind(user_id)
        .bind(equipment_id)
        .bind(identifier)
        .bind(chain_id)
        .bind(chain_variant_id)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_device_binding(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res =
            sqlx::query(&crate::sql!("DELETE FROM device_bindings WHERE id = ? AND user_id = ?"))
                .bind(id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Whether this box is the user's.
    pub async fn owns_equipment(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let n: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM equipment WHERE id = ? AND user_id = ?"
        ))
        .bind(id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    /// Tie devices and tokens to a box (or untie them, with `None`). Ids that
    /// are not the user's are ignored. Hours never move: grouping changes how
    /// the page draws reporters, not where any listen was attributed.
    pub async fn tie_reporters(
        &self,
        user_id: i64,
        equipment_id: Option<i64>,
        device_ids: &[i64],
        token_ids: &[i64],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for (table, ids) in [("devices", device_ids), ("tokens", token_ids)] {
            for id in ids {
                sqlx::query(&crate::sql!(&format!(
                    "UPDATE {table} SET equipment_id = ? WHERE id = ? AND user_id = ?"
                )))
                .bind(equipment_id)
                .bind(id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Remember another name a box reports as. Blank and repeated names
    /// (ignoring case and punctuation) are not added twice.
    pub async fn add_alias(
        &self,
        user_id: i64,
        equipment_id: i64,
        alias: &str,
    ) -> Result<(), sqlx::Error> {
        let alias = alias.split_whitespace().collect::<Vec<_>>().join(" ");
        if squash(&alias).is_empty() {
            return Ok(());
        }
        let raw: Option<Option<String>> = sqlx::query_scalar(&crate::sql!(
            "SELECT aliases FROM equipment WHERE id = ? AND user_id = ?"
        ))
        .bind(equipment_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(raw) = raw else { return Ok(()) };
        let mut list = parse_aliases(raw.as_deref());
        if list.iter().any(|a| squash(a) == squash(&alias)) {
            return Ok(());
        }
        list.push(alias);
        self.set_aliases(user_id, equipment_id, &list).await
    }

    /// Replace a box's aliases.
    pub async fn set_aliases(
        &self,
        user_id: i64,
        equipment_id: i64,
        aliases: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut clean: Vec<String> = Vec::new();
        for a in aliases {
            let a = a.split_whitespace().collect::<Vec<_>>().join(" ");
            if !squash(&a).is_empty() && !clean.iter().any(|c| squash(c) == squash(&a)) {
                clean.push(a);
            }
        }
        let json = (!clean.is_empty()).then(|| serde_json::to_string(&clean).unwrap_or_default());
        sqlx::query(&crate::sql!("UPDATE equipment SET aliases = ? WHERE id = ? AND user_id = ?"))
            .bind(json)
            .bind(equipment_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Put a question off.
    pub async fn dismiss_inbox(&self, user_id: i64, key: &str) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO chain_inbox_dismissed (user_id, key, until) VALUES (?, ?, ?)
             ON CONFLICT(user_id, key) DO UPDATE SET until = excluded.until"
        ))
        .bind(user_id)
        .bind(key)
        .bind(Self::now_secs() + INBOX_SNOOZE_SECS)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The questions to ask, most useful first: which box a reporter is, then
    /// where unfiled plays came from, by how many.
    pub async fn chain_inbox(&self, user_id: i64) -> Result<Vec<InboxQuestion>, sqlx::Error> {
        let now = Self::now_secs();
        let since = now - INBOX_WINDOW_SECS;
        // A Shared Spool play is another person's system; "At a friend's"
        // (phase 5) is where those questions belong, not here.
        let groups: Vec<UnfiledGroup> = sqlx::query_as(&crate::sql!(
            "SELECT device_id, token_id, output_identifier, source_name,
                        COUNT(*), MIN(timestamp), MAX(timestamp)
                   FROM scrobbles
                  WHERE user_id = ? AND chain_id IS NULL AND skipped = FALSE
                    AND timestamp >= ? AND source_name <> 'Shared Spool'
                  GROUP BY device_id, token_id, output_identifier, source_name
                  ORDER BY COUNT(*) DESC"
        ))
        .bind(user_id)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        if groups.is_empty() {
            return Ok(Vec::new());
        }

        let dismissed: Vec<String> = sqlx::query_scalar(&crate::sql!(
            "SELECT key FROM chain_inbox_dismissed WHERE user_id = ? AND until > ?"
        ))
        .bind(user_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        let devices: Vec<DeviceNames> = sqlx::query_as(&crate::sql!(
            "SELECT id, machine_id, name, platform, product, equipment_id
                   FROM devices WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let tokens: Vec<(i64, String, Option<i64>)> = sqlx::query_as(&crate::sql!(
            "SELECT id, name, equipment_id FROM tokens WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let gear: Vec<GearNames> = sqlx::query_as(&crate::sql!(
            "SELECT id, name, type, model, aliases FROM equipment WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let boxes: Vec<(i64, String, Vec<String>)> = gear
            .iter()
            .filter(|(_, _, ty, _, _)| SOURCE_TYPES.contains(&ty.as_str()))
            .map(|(id, name, _, model, aliases)| {
                let mut names = vec![name.clone()];
                names.extend(model.clone());
                names.extend(parse_aliases(aliases.as_deref()));
                (*id, name.clone(), names)
            })
            .collect();

        let device_rep = |id: i64| {
            devices.iter().find(|d| d.0 == id).map(|(id, mid, name, platform, product, eq)| {
                (
                    InboxReporter {
                        kind: "device",
                        id: *id,
                        name: name
                            .clone()
                            .or_else(|| product.clone())
                            .unwrap_or_else(|| mid.clone()),
                        sub: platform.clone().unwrap_or_else(|| "a player".into()),
                        equipment_id: *eq,
                    },
                    [name.clone(), product.clone(), Some(mid.clone())]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>(),
                )
            })
        };
        let token_rep = |id: i64| {
            tokens.iter().find(|t| t.0 == id).map(|(id, name, eq)| {
                (
                    InboxReporter {
                        kind: "token",
                        id: *id,
                        name: name.clone(),
                        sub: "A token you made".into(),
                        equipment_id: *eq,
                    },
                    vec![name.clone()],
                )
            })
        };
        // Every reporter not yet tied to a box, with the names it goes by.
        let loose: Vec<(InboxReporter, Vec<String>)> = devices
            .iter()
            .filter(|d| d.5.is_none())
            .filter_map(|d| device_rep(d.0))
            .chain(tokens.iter().filter(|t| t.2.is_none()).filter_map(|t| token_rep(t.0)))
            .collect();

        let mut box_qs: Vec<InboxQuestion> = Vec::new();
        let mut where_qs: Vec<InboxQuestion> = Vec::new();
        let mut asked_box = std::collections::HashSet::new();
        for (device_id, token_id, output, source_name, plays, first, last) in groups {
            let device = device_id.and_then(device_rep);
            let token = token_id.and_then(token_rep);

            // First, which box sent it — when the reporter is not tied to one.
            let reporter = device.clone().or(token.clone());
            if let Some((rep, names)) = reporter.as_ref().filter(|(r, _)| r.equipment_id.is_none())
            {
                if let Some((eq, eq_name, _)) =
                    boxes.iter().find(|(_, _, bnames)| names_match(bnames, names))
                {
                    let key = format!("box:{eq}");
                    if asked_box.insert(key.clone()) && !dismissed.contains(&key) {
                        let mut q = InboxQuestion::new(key, "same_box");
                        q.equipment_id = Some(*eq);
                        q.equipment_name = Some(eq_name.clone());
                        q.reporters = loose
                            .iter()
                            .filter(|(_, n)| {
                                names_match(&boxes.iter().find(|b| b.0 == *eq).unwrap().2, n)
                            })
                            .map(|(r, _)| r.clone())
                            .collect();
                        box_qs.push(q);
                    }
                } else {
                    let key = format!("which:{}{}", &rep.kind[..1], rep.id);
                    if asked_box.insert(key.clone())
                        && !dismissed.contains(&key)
                        && !boxes.is_empty()
                    {
                        let mut q = InboxQuestion::new(key, "which_box");
                        q.reporters = vec![rep.clone()];
                        box_qs.push(q);
                    }
                }
            }

            // Then where the plays should have gone.
            let key = format!(
                "where:{}:{}:{}:{}",
                device_id.map(|d| d.to_string()).unwrap_or_default(),
                token_id.map(|t| t.to_string()).unwrap_or_default(),
                output.clone().unwrap_or_default(),
                source_name
            );
            if dismissed.contains(&key) {
                continue;
            }
            let has_box = reporter.as_ref().is_some_and(|(r, _)| r.equipment_id.is_some());
            let mut q = InboxQuestion::new(key, "where");
            q.rule = Some(match (&output, token_id, device_id) {
                (Some(_), _, _) if has_box => "device_output",
                (Some(_), _, _) => "output",
                (None, Some(_), _) => "token",
                (None, None, Some(_)) => "device",
                _ => "none",
            });
            q.plays = Some(plays);
            q.first = Some(first);
            q.last = Some(last);
            q.since = Some(since);
            q.device = device.map(|(r, _)| r);
            q.token = token.map(|(r, _)| r);
            q.output = output;
            q.source_name = Some(source_name);
            where_qs.push(q);
        }
        box_qs.extend(where_qs);
        Ok(box_qs)
    }

    /// Answer a `where` question: write the most specific rule the plays allow,
    /// so future ones find the chain, and file the counted plays if asked.
    ///
    /// The rule follows the ladder: an output (of a box when the reporter is
    /// tied to one, otherwise by name), else the token's default chain, else
    /// the device's. With none of those there is nothing to remember, and only
    /// the counted plays can be filed. A setup is only ever remembered on an
    /// output, the one rung that may name one.
    #[allow(clippy::too_many_arguments)]
    pub async fn answer_where(
        &self,
        user_id: i64,
        device_id: Option<i64>,
        token_id: Option<i64>,
        output: Option<&str>,
        source_name: &str,
        since: i64,
        chain_id: i64,
        chain_variant_id: Option<i64>,
        backfill: bool,
    ) -> Result<WhereAnswered, sqlx::Error> {
        let eq = self.source_equipment(user_id, device_id, token_id).await?;
        let rule = match (output, token_id, device_id) {
            (Some(out), _, _) => {
                if let Some(eq) = eq {
                    self.set_device_binding(user_id, eq, out, Some(chain_id), chain_variant_id)
                        .await?;
                    "device_output"
                } else {
                    self.set_binding(user_id, out, Some(chain_id), chain_variant_id).await?;
                    "output"
                }
            }
            (None, Some(t), _) => {
                sqlx::query(&crate::sql!(
                    "UPDATE tokens SET default_chain_id = ? WHERE id = ? AND user_id = ?"
                ))
                .bind(chain_id)
                .bind(t)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
                "token"
            }
            (None, None, Some(d)) => {
                sqlx::query(&crate::sql!(
                    "UPDATE devices SET default_chain_id = ? WHERE id = ? AND user_id = ?"
                ))
                .bind(chain_id)
                .bind(d)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
                "device"
            }
            _ => "none",
        };
        let variant = if output.is_some() { chain_variant_id } else { None };
        let moved = if backfill {
            // The same population the question counted: unfiled, not skipped,
            // from this source, inside the window it showed.
            sqlx::query(&crate::sql!(
                "UPDATE scrobbles SET chain_id = ?, chain_variant_id = ?, chain_source = 'inbox'
                  WHERE user_id = ? AND chain_id IS NULL AND skipped = FALSE AND timestamp >= ?
                    AND source_name = ?
                    AND (device_id = ? OR (device_id IS NULL AND ? IS NULL))
                    AND (token_id = ? OR (token_id IS NULL AND ? IS NULL))
                    AND (output_identifier = ? OR (output_identifier IS NULL AND ? IS NULL))"
            ))
            .bind(chain_id)
            .bind(variant)
            .bind(user_id)
            .bind(since)
            .bind(source_name)
            .bind(device_id)
            .bind(device_id)
            .bind(token_id)
            .bind(token_id)
            .bind(output)
            .bind(output)
            .execute(&self.pool)
            .await?
            .rows_affected()
        } else {
            0
        };
        Ok(WhereAnswered { rule, moved })
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn a_model_number_matches_however_it_is_spelled() {
        let phone = vec!["Xperia 1 V".to_string(), "XQ-DQ54".to_string()];
        assert!(names_match(&phone, &["TIDAL on XQ-DQ54".into()]));
        assert!(names_match(&phone, &["xperia 1 v".into()]));
        assert!(!names_match(&phone, &["Chrome".into()]));
        // Too short to mean anything on its own.
        assert!(!names_match(&["M3".into()], &["MacBook Pro M3".into()]));
    }
}
