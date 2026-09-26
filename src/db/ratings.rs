//! Star ratings lifted off a media server, and the threshold that turns them
//! into loves.
//!
//! Two jobs, kept apart on purpose. Storing a rating records what a server
//! holds; applying a threshold records what *this* instance decided about it.
//! Because those are separate, moving the threshold is a knob rather than a
//! ratchet — see [`Database::apply_rating_loves`], which withdraws the loves it
//! made last time and is structurally incapable of touching one made by hand.

use super::*;
use crate::ratings::{LoveRule, Rating, SourceScale};

/// A rating on its way in, as a source read it off the wire.
///
/// `rating` and `scale_max` travel together and neither is useful alone. The
/// keys are computed here rather than by each source, so a rating is folded by
/// exactly the same two functions an entity is and the two cannot drift.
#[derive(Debug, Clone)]
pub struct IncomingRating {
    /// `recording`, `release` or `artist` — the three entity kinds a love can
    /// point at.
    pub kind: String,
    /// The server's own id for the item. Part of the uniqueness, so re-pulling
    /// updates rather than duplicating.
    pub remote_id: String,
    pub artist: String,
    /// Title for a recording, album name for a release, empty for an artist —
    /// the shape `Database::entity_key` already expects.
    pub name: String,
    pub album: Option<String>,
    pub mbid: Option<String>,
    /// The number the server gave, verbatim, and what it was out of.
    pub rating: Option<Rating>,
    /// Subsonic's `starred`, Jellyfin's and Emby's `IsFavorite`. Already the
    /// statement a love makes, so it carries no scale and takes no threshold.
    pub starred: bool,
    /// When the server says it was rated, where it says so at all.
    pub rated_at: Option<i64>,
}

/// A stored rating, as it goes back out to a screen.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct LibraryRatingRow {
    pub id: i64,
    pub source_id: i64,
    pub source_kind: String,
    pub kind: String,
    pub artist: String,
    pub name: String,
    pub album: Option<String>,
    /// The raw number, exactly as the server gave it.
    pub rating: Option<f64>,
    /// What that number was out of. Meaningless apart.
    pub scale_max: f64,
    pub starred: bool,
    pub rated_at: Option<i64>,
}

impl LibraryRatingRow {
    /// Where this sits on its own scale, 0..=1 — the one comparable number.
    pub fn fraction(&self) -> f32 {
        match self.rating {
            Some(v) if v > 0.0 && self.scale_max > 0.0 => {
                Rating::new(v as f32, SourceScale::new(self.scale_max as f32)).fraction()
            }
            _ => 0.0,
        }
    }
}

/// What is held, per source, so a screen can say where the numbers came from
/// without listing them.
#[derive(Debug, Clone, FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct RatingSourceSummary {
    pub source_id: i64,
    pub source_kind: String,
    pub label: String,
    pub rated: i64,
    pub starred: i64,
    pub scale_max: f64,
    pub fetched_at: Option<i64>,
}

/// What applying a threshold did, or would do.
///
/// `matched` and `unmatched` are stated separately for the reason every paired
/// count in this codebase is: a rating on something never played is real data
/// and is kept, but it does not become a love, and a screen reporting only the
/// loves would silently understate what the library holds.
#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct RatingApply {
    /// Ratings that clear the bar.
    pub qualifying: usize,
    /// …of which this many are things this user has actually listened to.
    pub matched: usize,
    /// …and this many are not, so they are not loved. See the note on the rule.
    pub unmatched: usize,
    /// Loves newly made by this run.
    pub loved: usize,
    /// Loves withdrawn because their rating no longer clears the bar.
    pub withdrawn: usize,
    /// Already loved, by hand or by an earlier run. Untouched.
    pub already: usize,
    /// Whether anything was actually written.
    pub dry_run: bool,
}

impl Database {
    /// Store what a source just read, replacing whatever that source said last
    /// time.
    ///
    /// One transaction rather than a commit per row: a library of a few
    /// thousand ratings is a few thousand WAL writes otherwise, which is the
    /// shape that made `rebuild_sessions` run past a proxy timeout on the real
    /// host — no statement slow, simply fifty thousand of them.
    ///
    /// `ON CONFLICT DO UPDATE` on `(user_id, source_id, kind, remote_id)`, so
    /// **a re-pull tracks the server**: a rating raised, lowered or cleared
    /// there is raised, lowered or cleared here. That is the right direction —
    /// the server is the authority on its own ratings, and this table is a copy
    /// of them.
    pub async fn upsert_library_ratings(
        &self,
        user_id: i64,
        source_id: i64,
        source_kind: &str,
        rows: &[IncomingRating],
    ) -> Result<usize, sqlx::Error> {
        if rows.is_empty() {
            return Ok(0);
        }
        let now = Self::now_secs();
        let mut tx = self.pool.begin().await?;
        let mut written = 0usize;
        for r in rows {
            let entity_key = Self::entity_key(&r.kind, &r.artist, &r.name);
            let match_key = Self::match_key(&r.kind, &r.artist, &r.name);
            let (value, scale_max) = match r.rating {
                Some(v) => (Some(v.value as f64), v.scale.max as f64),
                // A starred item with no number still needs a scale recorded,
                // or the row cannot say what a later rating on it would mean.
                None => (
                    None,
                    crate::ratings::scale_for_source(source_kind)
                        .map(|s| s.max as f64)
                        .unwrap_or(5.0),
                ),
            };
            sqlx::query(&crate::sql!(
                "INSERT INTO library_ratings
                   (user_id, source_id, source_kind, kind, remote_id, artist, name, album,
                    entity_key, match_key, mbid, rating, scale_max, starred, rated_at, fetched_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(user_id, source_id, kind, remote_id) DO UPDATE SET
                    artist = excluded.artist, name = excluded.name, album = excluded.album,
                    entity_key = excluded.entity_key, match_key = excluded.match_key,
                    mbid = COALESCE(excluded.mbid, library_ratings.mbid),
                    rating = excluded.rating, scale_max = excluded.scale_max,
                    starred = excluded.starred, rated_at = excluded.rated_at,
                    fetched_at = excluded.fetched_at"
            ))
            .bind(user_id)
            .bind(source_id)
            .bind(source_kind)
            .bind(&r.kind)
            .bind(&r.remote_id)
            .bind(&r.artist)
            .bind(&r.name)
            .bind(r.album.as_deref())
            .bind(&entity_key)
            .bind(&match_key)
            .bind(r.mbid.as_deref())
            .bind(value)
            .bind(scale_max)
            .bind(r.starred)
            .bind(r.rated_at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            written += 1;
        }
        tx.commit().await?;
        Ok(written)
    }

    /// Everything this source last reported, so a re-pull can drop what the
    /// server no longer rates.
    ///
    /// Called before a pull rather than after: a rating cleared on the server
    /// simply stops appearing in its answer, so the only way to notice is to
    /// compare. Scoped to the one source — another server's ratings of the same
    /// record are its own business.
    pub async fn forget_source_ratings(
        &self,
        user_id: i64,
        source_id: i64,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "DELETE FROM library_ratings WHERE user_id = ? AND source_id = ?"
        ))
        .bind(user_id)
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// Forget every rating this user holds. Does **not** touch loves: a love
    /// already made is a statement that was made, and this is a request to
    /// forget the ratings, not to un-love what they produced.
    pub async fn clear_library_ratings(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!("DELETE FROM library_ratings WHERE user_id = ?"))
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// What is held, per source.
    pub async fn rating_sources(
        &self,
        user_id: i64,
    ) -> Result<Vec<RatingSourceSummary>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT r.source_id, r.source_kind, COALESCE(s.label, '') AS label,
                    SUM(CASE WHEN r.rating IS NOT NULL AND r.rating > 0 THEN 1 ELSE 0 END) AS rated,
                    SUM(CASE WHEN r.starred THEN 1 ELSE 0 END) AS starred,
                    MAX(r.scale_max) AS scale_max, MAX(r.fetched_at) AS fetched_at
               FROM library_ratings r
               LEFT JOIN user_sources s ON s.id = r.source_id
              WHERE r.user_id = ?
              GROUP BY r.source_id, r.source_kind, s.label
              ORDER BY r.source_kind"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// The rated items themselves, highest first, for a preview.
    pub async fn list_library_ratings(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<LibraryRatingRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, source_id, source_kind, kind, artist, name, album,
                    rating, scale_max, starred, rated_at
               FROM library_ratings
              WHERE user_id = ?
              ORDER BY (CASE WHEN scale_max > 0 THEN COALESCE(rating, 0) / scale_max ELSE 0 END)
                       DESC, artist, name
              LIMIT ?"
        ))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Every identity key this user has actually listened to, exact and folded.
    ///
    /// One grouped scan per entity kind and the folding done in Rust, which is
    /// the shape `loves_detailed` and `list_crate` already use: the alternative
    /// is a lookup per rating, and a join key that is a *function* of a column
    /// can never be indexed — the quadratic that made Rediscovery take two
    /// minutes on the real host.
    ///
    /// **Skips are excluded**, and that is the meaning rather than the habit: a
    /// love points at something you listened to, and a skip is the one thing on
    /// record that says you did not.
    async fn history_keys(&self, user_id: i64) -> Result<HashSet<String>, sqlx::Error> {
        let mut keys: HashSet<String> = HashSet::new();
        for (kind, name_col) in [("recording", "title"), ("release", "album")] {
            let rows: Vec<(String, String)> = sqlx::query_as(&crate::sql!(&format!(
                "SELECT artist, {name_col} FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE
                    AND {name_col} IS NOT NULL AND TRIM({name_col}) != ''
                    AND TRIM(artist) != ''
                  GROUP BY LOWER(TRIM(artist)), LOWER(TRIM({name_col}))"
            )))
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
            for (artist, name) in rows {
                keys.insert(Self::entity_key(kind, &artist, &name));
                keys.insert(Self::match_key(kind, &artist, &name));
            }
        }
        let artists: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT artist FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND TRIM(artist) != ''
              GROUP BY LOWER(TRIM(artist))"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        for (artist,) in artists {
            keys.insert(Self::entity_key("artist", "", &artist));
            keys.insert(Self::match_key("artist", "", &artist));
        }
        Ok(keys)
    }

    /// Apply a threshold: love what clears it, and withdraw what no longer
    /// does.
    ///
    /// Three rules, and the second is the one everything rests on.
    ///
    /// **A rating on something never played is kept and not loved.** That is
    /// the rule `loves_pull` already follows in as many words — loves point at
    /// entities, entities are things you have actually listened to, and
    /// manufacturing rows for records with no listens behind them puts things
    /// on the Loved page that render as a column of zeroes. The count comes
    /// back as `unmatched` so it is visible rather than silently absent.
    ///
    /// **Only loves this instance derived are ever withdrawn.** A love pressed
    /// by hand or pulled from Last.fm carries NULL in `source`, and the query
    /// that decides what *may* be taken back asks for `source = 'rating'` — so
    /// nothing else is ever a candidate, however far the threshold moves. And
    /// because the insert is `ON CONFLICT DO NOTHING`, a track that was
    /// *already* loved by hand keeps its NULL and stays that way: the person's
    /// statement outranks the derived one, the same call `lyrics_cache.edited`
    /// and the shelf's typed-in running times both make.
    ///
    /// The `DELETE` below repeats that clause, and it is worth knowing that the
    /// repetition is a **second lock rather than the first**: with the
    /// selection correct, a hand-made love never reaches it, so
    /// `raising_the_threshold_takes_back_only_what_it_gave` passes with the
    /// delete's clause removed and fails with the selection's — verified both
    /// ways. It stays because this is the destructive half and the cost is one
    /// line, but do not remove the clause on the *selection* on the grounds
    /// that the delete covers it. It does not; nothing would reach it.
    ///
    /// **`dry_run` writes nothing at all**, including no entities — which is
    /// why it matches against the history keys instead of calling
    /// `upsert_entity`, since that creates. A preview that created rows would
    /// not be a preview.
    pub async fn apply_rating_loves(
        &self,
        user_id: i64,
        rule: LoveRule,
        dry_run: bool,
    ) -> Result<RatingApply, sqlx::Error> {
        let mut out = RatingApply { dry_run, ..Default::default() };

        /// `(kind, artist, name, mbid, rating, scale_max, starred)` — the row
        /// as the threshold needs it, named so the tuple does not have to be
        /// read twice to know which end is which.
        type RatingTuple = (String, String, String, Option<String>, Option<f64>, f64, bool);

        let rows: Vec<RatingTuple> = sqlx::query_as(&crate::sql!(
            "SELECT kind, artist, name, mbid, rating, scale_max, starred
                   FROM library_ratings WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let history = self.history_keys(user_id).await?;

        // Fold to one decision per *thing*, not per rating row: two servers can
        // both hold the same record, and either of them clearing the bar is
        // enough. Keyed on the exact identity, which is what an entity is
        // created under.
        let mut wanted: HashMap<String, (String, String, String, Option<String>)> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (kind, artist, name, mbid, rating, scale_max, starred) in rows {
            let value = rating.filter(|v| *v > 0.0).map(|v| {
                Rating::new(
                    v as f32,
                    SourceScale::new(if scale_max > 0.0 { scale_max as f32 } else { 5.0 }),
                )
            });
            if !rule.loves(value, starred) {
                continue;
            }
            let key = Self::entity_key(&kind, &artist, &name);
            if seen.insert(key.clone()) {
                out.qualifying += 1;
                if history.contains(&key)
                    || history.contains(&Self::match_key(&kind, &artist, &name))
                {
                    out.matched += 1;
                    wanted.insert(key, (kind, artist, name, mbid));
                } else {
                    out.unmatched += 1;
                }
            }
        }

        // What the last run left behind, so this one can take it back.
        let existing: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT l.entity_id, e.entity_key
               FROM loves l JOIN entities e ON e.id = l.entity_id
              WHERE l.user_id = ? AND l.source = 'rating'"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        for (entity_id, key) in &existing {
            if !wanted.contains_key(key) {
                out.withdrawn += 1;
                if !dry_run {
                    sqlx::query(&crate::sql!(
                        "DELETE FROM loves
                          WHERE user_id = ? AND entity_id = ? AND source = 'rating'"
                    ))
                    .bind(user_id)
                    .bind(entity_id)
                    .execute(&self.pool)
                    .await?;
                }
            }
        }

        for (kind, artist, name, mbid) in wanted.into_values() {
            if dry_run {
                // Counting against the keys the last run left is the closest a
                // preview can get without creating the entity it would need to
                // ask about.
                let key = Self::entity_key(&kind, &artist, &name);
                if existing.iter().any(|(_, k)| k == &key) {
                    out.already += 1;
                } else {
                    out.loved += 1;
                }
                continue;
            }
            let entity = self.upsert_entity(&kind, &name, &artist, mbid.as_deref()).await?;
            if self.is_loved(user_id, entity).await? {
                out.already += 1;
                continue;
            }
            self.set_love_from(user_id, entity, true, Some("rating")).await?;
            out.loved += 1;
        }

        Ok(out)
    }

    /// [`Database::set_love`] with provenance.
    ///
    /// `ON CONFLICT DO NOTHING` is load-bearing rather than tidiness: a track
    /// already loved by hand keeps its NULL `source` and therefore stays out of
    /// every future withdrawal. Overwriting it with `'rating'` would quietly
    /// put somebody's own love inside the reach of a threshold.
    pub async fn set_love_from(
        &self,
        user_id: i64,
        entity_id: i64,
        loved: bool,
        source: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        if !loved {
            return self.set_love(user_id, entity_id, false).await;
        }
        sqlx::query(&crate::sql!(
            "INSERT INTO loves (user_id, entity_id, source, created_at)
             VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING"
        ))
        .bind(user_id)
        .bind(entity_id)
        .bind(source)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
