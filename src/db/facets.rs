//! What kind of listen a listen is: the classifier's work list and writes, and
//! every ranking the Classical, Scores & Ambient and seasonal tabs read.
//!
//! The decision lives in `recording_facts` (shared, one per recording as heard
//! on a release) and is copied onto `scrobbles.facets` with the listener's own
//! overrides applied. Rankings filter on the copy and join the decision only
//! where they need its piece, composer or performers.

use super::{ChartRow, Cover, Database, COVER_PICK};
use crate::classify::{ArtistDates, Outcome, PerformerRow};
use serde::Serialize;
use sqlx::FromRow;
use utoipa::ToSchema;

/// A listen as the classifier needs it.
#[derive(Debug, Clone, FromRow)]
pub struct FacetListen {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub artist: String,
    pub artist_credit: Option<String>,
    pub album: Option<String>,
    pub album_filed_under: Option<String>,
    pub mbid_recording: Option<String>,
    pub mbid_release: Option<String>,
    pub mbid_release_group: Option<String>,
    pub mbid_artist: Option<String>,
}

fn present(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

impl FacetListen {
    /// Which decision this listen shares. A recording MusicBrainz knows is
    /// decided once per release it was heard on — the same recording is score
    /// on the *TRON: Legacy* album and a single on its own — and a listen
    /// known only by name is decided by its names.
    pub fn fact_key(&self) -> String {
        match present(&self.mbid_recording) {
            Some(r) => format!("r:{}|{}", r, present(&self.mbid_release).unwrap_or("")),
            None => format!(
                "n:{}\u{1f}{}\u{1f}{}\u{1f}{}",
                fold(&self.title),
                fold(&self.artist),
                fold(self.album.as_deref().unwrap_or("")),
                present(&self.mbid_release).unwrap_or("")
            ),
        }
    }

    /// The album as Tapedeck files it: its folded name and who it is filed
    /// under. What an album-level override is keyed by.
    pub fn album_key(&self) -> Option<String> {
        let album = present(&self.album)?;
        let filed = present(&self.album_filed_under).unwrap_or(self.artist.as_str());
        Some(format!("{}\u{1f}{}", fold(album), fold(filed)))
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct StoredFact {
    pub facets: i64,
    pub piece_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FacetOverride {
    pub target_kind: String,
    pub target_key: String,
    pub facet: i64,
    pub value: bool,
}

/// One row of a Classical, Scores or lens ranking.
#[derive(Debug, Clone, Default, FromRow, Serialize, ToSchema)]
pub struct InsightRow {
    /// What a filter or an expanded row passes back: a piece key, a composer's
    /// MBID, a performer's folded name.
    pub key: String,
    pub name: String,
    /// The secondary line: a piece's composer, a performer's instrument.
    pub sub: Option<String>,
    #[sqlx(default)]
    pub album: Option<String>,
    #[sqlx(default)]
    pub album_artist: Option<String>,
    pub plays: i64,
    pub seconds: i64,
    /// Pieces: how many recordings you heard it in. Composers and performers:
    /// how many pieces. Songs in a lens: how many artists' versions.
    #[sqlx(default)]
    pub versions: Option<i64>,
    /// Complete hearings: every movement in one sitting. Pieces and composers.
    #[sqlx(default)]
    pub hearings: Option<i64>,
    /// 1 where this performer's role was inferred rather than stated.
    #[sqlx(default)]
    pub inferred: Option<i64>,
    #[sqlx(default)]
    pub artwork_url: Option<String>,
    #[sqlx(default)]
    pub caa_id: Option<i64>,
    #[sqlx(default)]
    pub caa_release_mbid: Option<String>,
    #[serde(skip)]
    #[sqlx(default)]
    pub cover_pick: Option<String>,
}

resolves_cover!(InsightRow);

/// Filters a ranking can stack: Bach → Partita No. 2 → violinists.
#[derive(Debug, Clone, Default)]
pub struct InsightFilter {
    pub after: Option<i64>,
    pub before: Option<i64>,
    /// A composer's MBID, or their folded name when MusicBrainz has none.
    pub composer: Option<String>,
    pub piece: Option<String>,
    /// A performer's folded name.
    pub performer: Option<String>,
    pub instrument: Option<String>,
}

enum Bind {
    I(i64),
    O(Option<i64>),
    S(String),
}

/// One classical listen with what the statistics need about its piece and
/// composer.
#[derive(Debug, Clone, FromRow)]
pub struct ClassicalStatRow {
    pub seconds: i64,
    pub timestamp: i64,
    pub composer_key: Option<String>,
    pub composer_name: Option<String>,
    pub composed_year: Option<i64>,
    pub form: Option<String>,
    pub born: Option<String>,
    pub died: Option<String>,
    pub ended: Option<i64>,
    pub gender: Option<String>,
    pub country: Option<String>,
}

/// A performer on a classical listen: `(artist_key, name, role, country,
/// composer key, piece key, plays)`.
pub type PerformerStatRow = (String, String, String, Option<String>, Option<String>, String, i64);

/// A piece for the timeline: where it stands and how much you played it.
#[derive(Debug, Clone, FromRow)]
pub struct TimelineRow {
    pub piece_key: String,
    pub piece_title: String,
    pub composer_key: Option<String>,
    pub composer_name: Option<String>,
    pub composed_year: Option<i64>,
    pub born: Option<String>,
    pub plays: i64,
    pub seconds: i64,
}

/// An artist in somebody's history with the dates anniversaries need.
#[derive(Debug, Clone, FromRow)]
pub struct DatedArtist {
    pub mbid: String,
    pub name: String,
    pub kind: Option<String>,
    pub begin: Option<String>,
    pub end_date: Option<String>,
    pub ended: i64,
    pub plays: i64,
}

/// How much of a classical window the performer charts can speak for.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct RoleCoverage {
    pub seconds: i64,
    /// Seconds whose performers MusicBrainz states.
    pub stated: i64,
    /// Seconds with roles only Tapedeck inferred.
    pub inferred: i64,
}

impl Database {
    // ── The classifier's side ──

    /// Listens decided by an older classifier, newest first — those
    /// MusicBrainz describes ahead of those known only by name, so a
    /// performer's stated role ("Hilary Hahn, violin") is on record before a
    /// name-only listen of them asks for it.
    pub async fn listens_needing_facets(
        &self,
        version: i64,
        user_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<FacetListen>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, user_id, title, artist, artist_credit, album, album_filed_under,
                    mbid_recording, mbid_release, mbid_release_group, mbid_artist
               FROM scrobbles
              WHERE facets_version < ? AND skipped = FALSE AND (? IS NULL OR user_id = ?)
              ORDER BY CASE WHEN mbid_recording IS NULL OR TRIM(mbid_recording) = '' THEN 1 ELSE 0 END,
                       id DESC
              LIMIT ?"
        ))
        .bind(version)
        .bind(user_id)
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// How many of a listener's listens the classifier has not reached yet —
    /// the "still sorting N listens" line on every tab.
    pub async fn facets_pending(&self, user_id: i64, version: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND facets_version < ?"
        ))
        .bind(user_id)
        .bind(version)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn stored_fact(
        &self,
        key: &str,
        version: i64,
    ) -> Result<Option<StoredFact>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT facets, piece_key FROM recording_facts WHERE fact_key = ? AND version = ?"
        ))
        .bind(key)
        .bind(version)
        .fetch_optional(&self.pool)
        .await
    }

    /// Write one decision and who played it. `inferred` performers are kept
    /// apart from stated ones so the tab can mark them.
    pub async fn store_fact(
        &self,
        key: &str,
        version: i64,
        o: &Outcome,
        inferred: &[PerformerRow],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!(
            "INSERT INTO recording_facts (fact_key, version, facets, piece_key, piece_title, movement,
                    composer_name, composer_mbid, leaf_key, part_count, composed_year, form,
                    evidence, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (fact_key) DO UPDATE SET
                version = excluded.version, facets = excluded.facets,
                piece_key = excluded.piece_key, piece_title = excluded.piece_title,
                movement = excluded.movement, composer_name = excluded.composer_name,
                composer_mbid = excluded.composer_mbid, leaf_key = excluded.leaf_key,
                part_count = excluded.part_count, composed_year = excluded.composed_year,
                form = excluded.form, evidence = excluded.evidence,
                updated_at = excluded.updated_at"
        ))
        .bind(key)
        .bind(version)
        .bind(o.facets)
        .bind(&o.piece_key)
        .bind(&o.piece_title)
        .bind(&o.movement)
        .bind(&o.composer_name)
        .bind(&o.composer_mbid)
        .bind(&o.leaf_key)
        .bind(o.part_count)
        .bind(o.composed_year.map(i64::from))
        .bind(&o.form)
        .bind(serde_json::to_string(&o.evidence).unwrap_or_else(|_| "[]".into()))
        .bind(Self::now_secs())
        .execute(&mut *tx)
        .await?;
        sqlx::query(&crate::sql!("DELETE FROM recording_performers WHERE fact_key = ?"))
            .bind(key)
            .execute(&mut *tx)
            .await?;
        let rows = o.performers.iter().map(|p| (p, 0i64)).chain(inferred.iter().map(|p| (p, 1)));
        for (p, inferred) in rows {
            sqlx::query(&crate::sql!(
                "INSERT INTO recording_performers (fact_key, artist_key, name, mbid, role, instrument, country, inferred)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (fact_key, artist_key, role) DO NOTHING"
            ))
            .bind(key)
            .bind(Self::artist_key(&p.name))
            .bind(&p.name)
            .bind(&p.mbid)
            .bind(&p.role)
            .bind(&p.instrument)
            .bind(&p.country)
            .bind(inferred)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    pub async fn set_listen_facets(
        &self,
        id: i64,
        key: &str,
        facets: i64,
        version: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET fact_key = ?, facets = ?, facets_version = ? WHERE id = ?"
        ))
        .bind(key)
        .bind(facets)
        .bind(version)
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    /// The role MusicBrainz most often states for a performer, on recordings
    /// it does describe — how "Hilary Hahn" on a name-only listen becomes a
    /// violinist.
    pub async fn stated_role(
        &self,
        artist_key: &str,
    ) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT role, instrument FROM recording_performers
              WHERE artist_key = ? AND inferred = 0
              GROUP BY role, instrument ORDER BY COUNT(*) DESC LIMIT 1"
        ))
        .bind(artist_key)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn facet_overrides(&self, user_id: i64) -> Result<Vec<FacetOverride>, sqlx::Error> {
        let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT target_kind, target_key, facet, value FROM facet_overrides WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(target_kind, target_key, facet, value)| FacetOverride {
                target_kind,
                target_key,
                facet,
                value: value != 0,
            })
            .collect())
    }

    /// This listener's overrides on one target: `(facet, value)`.
    pub async fn facet_overrides_on(
        &self,
        user_id: i64,
        kind: &str,
        key: &str,
    ) -> Result<Vec<(i64, bool)>, sqlx::Error> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT facet, value FROM facet_overrides
              WHERE user_id = ? AND target_kind = ? AND target_key = ?"
        ))
        .bind(user_id)
        .bind(kind)
        .bind(key)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(f, v)| (f, v != 0)).collect())
    }

    /// Set or remove one override, and send the listens it touches back to
    /// the classifier — which re-applies from cached decisions, no lookups.
    /// Returns how many listens were requeued.
    pub async fn set_facet_override(
        &self,
        user_id: i64,
        kind: &str,
        key: &str,
        facet: i64,
        value: Option<bool>,
    ) -> Result<u64, sqlx::Error> {
        match value {
            Some(v) => {
                sqlx::query(&crate::sql!(
                    "INSERT INTO facet_overrides (user_id, target_kind, target_key, facet, value, created_at)
                     VALUES (?, ?, ?, ?, ?, ?)
                     ON CONFLICT (user_id, target_kind, target_key, facet)
                     DO UPDATE SET value = excluded.value, created_at = excluded.created_at"
                ))
                .bind(user_id)
                .bind(kind)
                .bind(key)
                .bind(facet)
                .bind(i64::from(v))
                .bind(Self::now_secs())
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query(&crate::sql!(
                    "DELETE FROM facet_overrides
                      WHERE user_id = ? AND target_kind = ? AND target_key = ? AND facet = ?"
                ))
                .bind(user_id)
                .bind(kind)
                .bind(key)
                .bind(facet)
                .execute(&self.pool)
                .await?;
            }
        }
        let res = if kind == "artist" {
            sqlx::query(&crate::sql!(
                "UPDATE scrobbles SET facets_version = 0
                  WHERE user_id = ? AND id IN (SELECT scrobble_id FROM scrobble_artists WHERE artist_key = ?)"
            ))
            .bind(user_id)
            .bind(key)
            .execute(&self.pool)
            .await?
        } else if kind == "work" {
            sqlx::query(&crate::sql!(
                "UPDATE scrobbles SET facets_version = 0
                  WHERE user_id = ? AND fact_key IN (SELECT fact_key FROM recording_facts WHERE piece_key = ?)"
            ))
            .bind(user_id)
            .bind(key)
            .execute(&self.pool)
            .await?
        } else {
            let (album, filed) = key.split_once('\u{1f}').unwrap_or((key, ""));
            sqlx::query(&crate::sql!(
                "UPDATE scrobbles SET facets_version = 0
                  WHERE user_id = ? AND LOWER(TRIM(album)) = ?
                    AND LOWER(TRIM(COALESCE(album_filed_under, artist))) = ?"
            ))
            .bind(user_id)
            .bind(album)
            .bind(filed)
            .execute(&self.pool)
            .await?
        };
        Ok(res.rows_affected())
    }

    // ── Rankings ──

    /// The window and the filters, as SQL and binds, for a query over
    /// `scrobbles s JOIN recording_facts f` carrying `flag`.
    fn insight_where(user_id: i64, flag: i64, f: &InsightFilter) -> (String, Vec<Bind>) {
        let mut sql = String::from(
            "s.user_id = ? AND s.skipped = FALSE AND (s.facets & ?) != 0
             AND (? IS NULL OR s.timestamp >= ?) AND (? IS NULL OR s.timestamp < ?)",
        );
        let mut binds = vec![
            Bind::I(user_id),
            Bind::I(flag),
            Bind::O(f.after),
            Bind::O(f.after),
            Bind::O(f.before),
            Bind::O(f.before),
        ];
        if let Some(c) = &f.composer {
            sql.push_str(" AND COALESCE(f.composer_mbid, LOWER(f.composer_name)) = ?");
            binds.push(Bind::S(c.clone()));
        }
        if let Some(p) = &f.piece {
            sql.push_str(" AND f.piece_key = ?");
            binds.push(Bind::S(p.clone()));
        }
        if let Some(p) = &f.performer {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM recording_performers px
                               WHERE px.fact_key = s.fact_key AND px.artist_key = ?)",
            );
            binds.push(Bind::S(p.clone()));
        }
        (sql, binds)
    }

    async fn insight_rows(
        &self,
        sql: &str,
        binds: Vec<Bind>,
    ) -> Result<Vec<InsightRow>, sqlx::Error> {
        let stmt = crate::sql!(sql);
        let mut q = sqlx::query_as::<_, InsightRow>(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        let mut rows = q.fetch_all(&self.pool).await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }

    /// One Classical ranking: `pieces`, `movements` (of `filter.piece`),
    /// `composers`, `performances`, or performers by `conductor`,
    /// `orchestra`, `ensemble`, `soloist` (narrowed by `filter.instrument`).
    pub async fn classical_chart(
        &self,
        user_id: i64,
        rank: &str,
        f: &InsightFilter,
        by_hours: bool,
        limit: i64,
    ) -> Result<Vec<InsightRow>, sqlx::Error> {
        let (cond, mut binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let order = if by_hours { "seconds DESC, plays DESC" } else { "plays DESC, seconds DESC" };
        let from = "scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key";
        let sql = match rank {
            "movements" => format!(
                "SELECT COALESCE(f.leaf_key, LOWER(COALESCE(f.movement, s.title))) AS key,
                        MAX(COALESCE(f.movement, s.title)) AS name, MAX(f.piece_title) AS sub,
                        COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds,
                        COUNT(DISTINCT s.fact_key) AS versions, {COVER_PICK} AS cover_pick
                   FROM {from} WHERE s.skipped = FALSE AND {cond}
                  GROUP BY COALESCE(f.leaf_key, LOWER(COALESCE(f.movement, s.title)))
                  ORDER BY {order} LIMIT ?"
            ),
            "composers" => format!(
                "SELECT COALESCE(f.composer_mbid, LOWER(f.composer_name)) AS key,
                        MAX(f.composer_name) AS name, NULL AS sub,
                        COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds,
                        COUNT(DISTINCT f.piece_key) AS versions, NULL AS cover_pick
                   FROM {from} WHERE s.skipped = FALSE AND {cond} AND f.composer_name IS NOT NULL
                  GROUP BY COALESCE(f.composer_mbid, LOWER(f.composer_name))
                  ORDER BY {order} LIMIT ?"
            ),
            "performances" => format!(
                "SELECT f.piece_key || CHAR(31) || LOWER(TRIM(COALESCE(s.album, ''))) AS key,
                        MAX(f.piece_title) AS name, MAX(s.artist) AS sub,
                        MAX(s.album) AS album, MAX(COALESCE(s.album_filed_under, s.artist)) AS album_artist,
                        COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds,
                        {COVER_PICK} AS cover_pick
                   FROM {from} WHERE s.skipped = FALSE AND {cond}
                  GROUP BY f.piece_key, LOWER(TRIM(COALESCE(s.album, ''))),
                           LOWER(TRIM(COALESCE(s.album_filed_under, s.artist)))
                  ORDER BY {order} LIMIT ?"
            ),
            "conductor" | "orchestra" | "ensemble" | "soloist" => {
                let mut extra = String::from(" AND rp.role = ?");
                binds.push(Bind::S(rank.to_string()));
                if let Some(i) = &f.instrument {
                    extra.push_str(" AND rp.instrument = ?");
                    binds.push(Bind::S(i.clone()));
                }
                // The role binds come after the window's, so the join's
                // conditions live in WHERE rather than ON.
                format!(
                    "SELECT rp.artist_key AS key, MAX(rp.name) AS name, MAX(rp.instrument) AS sub,
                            COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds,
                            COUNT(DISTINCT f.piece_key) AS versions, MAX(rp.inferred) AS inferred,
                            NULL AS cover_pick
                       FROM {from} JOIN recording_performers rp ON rp.fact_key = s.fact_key
                      WHERE s.skipped = FALSE AND {cond}{extra}
                      GROUP BY rp.artist_key
                      ORDER BY {order} LIMIT ?"
                )
            }
            _ => format!(
                "SELECT f.piece_key AS key, MAX(f.piece_title) AS name, MAX(f.composer_name) AS sub,
                        COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds,
                        COUNT(DISTINCT s.fact_key) AS versions, {COVER_PICK} AS cover_pick
                   FROM {from} WHERE s.skipped = FALSE AND {cond}
                  GROUP BY f.piece_key
                  ORDER BY {order} LIMIT ?"
            ),
        };
        binds.push(Bind::I(limit));
        self.insight_rows(&sql, binds).await
    }

    /// The listens complete hearings are counted from: classical, with a
    /// sitting, and a piece MusicBrainz knows. `(session, piece, leaf,
    /// parts, composer key)`.
    #[allow(clippy::type_complexity)]
    pub async fn classical_hearing_rows(
        &self,
        user_id: i64,
        f: &InsightFilter,
    ) -> Result<Vec<(Option<i64>, String, Option<String>, Option<i64>, Option<String>)>, sqlx::Error>
    {
        let (cond, binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let sql = format!(
            "SELECT s.session_id, f.piece_key, f.leaf_key, f.part_count,
                    COALESCE(f.composer_mbid, LOWER(f.composer_name))
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
              WHERE s.skipped = FALSE AND {cond} AND f.piece_key LIKE 'w:%'"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        q.fetch_all(&self.pool).await
    }

    /// Instruments the soloists in this window played, most-heard first.
    pub async fn classical_instruments(
        &self,
        user_id: i64,
        f: &InsightFilter,
    ) -> Result<Vec<(String, i64)>, sqlx::Error> {
        let (cond, binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let sql = format!(
            "SELECT rp.instrument, COUNT(*) AS n
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
               JOIN recording_performers rp ON rp.fact_key = s.fact_key
              WHERE s.skipped = FALSE AND {cond} AND rp.role = 'soloist' AND rp.instrument IS NOT NULL
              GROUP BY rp.instrument ORDER BY n DESC LIMIT 12"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        q.fetch_all(&self.pool).await
    }

    /// Of the classical seconds in a window, how many the performer charts
    /// rest on stated roles, and how many on inferred ones.
    pub async fn classical_role_coverage(
        &self,
        user_id: i64,
        f: &InsightFilter,
    ) -> Result<RoleCoverage, sqlx::Error> {
        let (cond, binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let sql = format!(
            "SELECT COALESCE(SUM(s.duration), 0),
                    COALESCE(SUM(CASE WHEN EXISTS (SELECT 1 FROM recording_performers rp
                          WHERE rp.fact_key = s.fact_key AND rp.inferred = 0) THEN s.duration ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN NOT EXISTS (SELECT 1 FROM recording_performers rp
                          WHERE rp.fact_key = s.fact_key AND rp.inferred = 0)
                        AND EXISTS (SELECT 1 FROM recording_performers rp
                          WHERE rp.fact_key = s.fact_key AND rp.inferred = 1) THEN s.duration ELSE 0 END), 0)
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
              WHERE s.skipped = FALSE AND {cond}"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (i64, i64, i64)>(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        let (seconds, stated, inferred) = q.fetch_one(&self.pool).await?;
        Ok(RoleCoverage { seconds, stated, inferred })
    }

    /// Songs in a lens, covers grouped: every version of "Last Christmas" is
    /// one row when MusicBrainz knows the work, with how many artists' versions.
    pub async fn lens_songs(
        &self,
        user_id: i64,
        flag: i64,
        f: &InsightFilter,
        by_hours: bool,
        limit: i64,
    ) -> Result<Vec<InsightRow>, sqlx::Error> {
        let (cond, mut binds) = Self::insight_where(user_id, flag, f);
        let order = if by_hours { "seconds DESC, plays DESC" } else { "plays DESC, seconds DESC" };
        let sql = format!(
            "SELECT f.piece_key AS key, MAX(f.piece_title) AS name, MAX(s.artist) AS sub,
                    MAX(s.album) AS album, MAX(COALESCE(s.album_filed_under, s.artist)) AS album_artist,
                    COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds,
                    COUNT(DISTINCT LOWER(TRIM(s.artist))) AS versions, {COVER_PICK} AS cover_pick
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
              WHERE s.skipped = FALSE AND {cond}
              GROUP BY f.piece_key
              ORDER BY {order} LIMIT ?"
        );
        binds.push(Bind::I(limit));
        self.insight_rows(&sql, binds).await
    }

    /// The versions of one song or piece you heard, by artist.
    pub async fn piece_versions(
        &self,
        user_id: i64,
        piece_key: &str,
        limit: i64,
    ) -> Result<Vec<InsightRow>, sqlx::Error> {
        let sql = format!(
            "SELECT LOWER(TRIM(s.artist)) AS key, MAX(s.artist) AS name, MAX(s.title) AS sub,
                    MAX(s.album) AS album, MAX(COALESCE(s.album_filed_under, s.artist)) AS album_artist,
                    COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds, {COVER_PICK} AS cover_pick
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
              WHERE s.user_id = ? AND s.skipped = FALSE AND f.piece_key = ?
              GROUP BY LOWER(TRIM(s.artist))
              ORDER BY plays DESC LIMIT ?"
        );
        self.insight_rows(
            &sql,
            vec![Bind::I(user_id), Bind::S(piece_key.to_string()), Bind::I(limit)],
        )
        .await
    }

    /// Albums the classifier thinks are score but will not move without the
    /// listener's word: soundtracks credited to their composer alone, with
    /// nothing saying the tracks are instrumental.
    pub async fn score_suggestions(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<InsightRow>, sqlx::Error> {
        let sql = format!(
            "SELECT LOWER(TRIM(s.album)) || CHAR(31) || LOWER(TRIM(COALESCE(s.album_filed_under, s.artist))) AS key,
                    MAX(s.album) AS name, MAX(COALESCE(s.album_filed_under, s.artist)) AS sub,
                    MAX(s.album) AS album, MAX(COALESCE(s.album_filed_under, s.artist)) AS album_artist,
                    COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds, {COVER_PICK} AS cover_pick
               FROM scrobbles s
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (s.facets & ?) != 0 AND (s.facets & ?) = 0
                AND s.album IS NOT NULL AND TRIM(s.album) != ''
              GROUP BY LOWER(TRIM(s.album)), LOWER(TRIM(COALESCE(s.album_filed_under, s.artist)))
              ORDER BY plays DESC LIMIT ?"
        );
        self.insight_rows(
            &sql,
            vec![
                Bind::I(user_id),
                Bind::I(crate::classify::SCORE_SUGGESTED),
                Bind::I(crate::classify::SCORE),
                Bind::I(limit),
            ],
        )
        .await
    }

    /// `(timestamp, seconds, facets)` for every listen in a window — for the
    /// season curve and the composition chart, which bucket by the listener's
    /// own calendar days and months and so have to do it in Rust.
    pub async fn facet_timeline(
        &self,
        user_id: i64,
        after: Option<i64>,
        before: Option<i64>,
        cap: i64,
    ) -> Result<Vec<(i64, i64, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT timestamp, COALESCE(duration, 0), facets FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND (? IS NULL OR timestamp >= ?) AND (? IS NULL OR timestamp < ?)
              ORDER BY timestamp DESC LIMIT ?"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .bind(before)
        .bind(before)
        .bind(cap)
        .fetch_all(&self.pool)
        .await
    }

    /// When every listen carrying `flag` happened — for the seasons a lens
    /// can show and the listens that fell outside any season.
    pub async fn facet_timestamps(&self, user_id: i64, flag: i64) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT timestamp FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND (facets & ?) != 0"
        ))
        .bind(user_id)
        .bind(flag)
        .fetch_all(&self.pool)
        .await
    }

    /// All-time listens per flag, for B-Sides' cards: `(classical, score,
    /// ambient, christmas, halloween, easter)`.
    pub async fn facet_counts(&self, user_id: i64) -> Result<[i64; 6], sqlx::Error> {
        use crate::classify::*;
        let row: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(&crate::sql!(
            "SELECT COALESCE(SUM(CASE WHEN (facets & ?) != 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN (facets & ?) != 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN (facets & ?) != 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN (facets & ?) != 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN (facets & ?) != 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN (facets & ?) != 0 THEN 1 ELSE 0 END), 0)
               FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND facets != 0"
        ))
        .bind(CLASSICAL)
        .bind(SCORE)
        .bind(AMBIENT)
        .bind(CHRISTMAS)
        .bind(HALLOWEEN)
        .bind(EASTER)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok([row.0, row.1, row.2, row.3, row.4, row.5])
    }

    /// Top Charts' chart with this listener's splits taken out.
    pub async fn top_chart(
        &self,
        user_id: i64,
        kind: &str,
        after: Option<i64>,
        limit: i64,
        by_hours: bool,
        exclude: i64,
    ) -> Result<Vec<ChartRow>, sqlx::Error> {
        self.chart_filtered(user_id, kind, after, None, limit, by_hours, exclude, 0).await
    }

    /// How many listens in a window a split took out of Top Charts.
    pub async fn split_out(
        &self,
        user_id: i64,
        after: Option<i64>,
        exclude: i64,
    ) -> Result<i64, sqlx::Error> {
        if exclude == 0 {
            return Ok(0);
        }
        sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                AND (facets & ?) != 0"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .bind(exclude)
        .fetch_one(&self.pool)
        .await
    }

    /// One artist's plays by record, for the album spread on their page:
    /// `(album, filed under, plays)`, with the unnamed records as `None`.
    pub async fn artist_album_plays(
        &self,
        user_id: i64,
        artist: &str,
    ) -> Result<Vec<(Option<String>, Option<String>, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT MAX(NULLIF(TRIM(s.album), '')), MAX(COALESCE(s.album_filed_under, s.artist)), COUNT(*)
               FROM scrobble_artists sa JOIN scrobbles s ON s.id = sa.scrobble_id
              WHERE sa.artist_key = ? AND s.user_id = ? AND s.skipped = FALSE
              GROUP BY LOWER(TRIM(COALESCE(s.album, ''))), LOWER(TRIM(COALESCE(s.album_filed_under, s.artist)))"
        ))
        .bind(Self::artist_key(artist))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    // ── Dates, statistics, the timeline and anniversaries ──

    /// Artists in anybody's history, and composers of pieces anybody heard,
    /// whose dates have not been looked up yet.
    pub async fn artists_needing_dates(&self, limit: i64) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT mbid FROM (
                SELECT DISTINCT sa.mbid AS mbid FROM scrobble_artists sa
                 WHERE sa.mbid IS NOT NULL AND sa.mbid != ''
                UNION
                SELECT DISTINCT f.composer_mbid AS mbid FROM recording_facts f
                 WHERE f.composer_mbid IS NOT NULL AND f.composer_mbid != ''
             ) m
             WHERE NOT EXISTS (SELECT 1 FROM artist_dates d WHERE d.mbid = m.mbid)
             LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn store_artist_dates(&self, mbid: &str, a: &ArtistDates) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO artist_dates (mbid, name, kind, gender, country, begin, end_date, ended, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (mbid) DO UPDATE SET
                name = excluded.name, kind = excluded.kind, gender = excluded.gender,
                country = excluded.country, begin = excluded.begin, end_date = excluded.end_date,
                ended = excluded.ended, updated_at = excluded.updated_at"
        ))
        .bind(mbid)
        .bind(&a.name)
        .bind(&a.kind)
        .bind(&a.gender)
        .bind(&a.country)
        .bind(&a.begin)
        .bind(&a.end)
        .bind(i64::from(a.ended))
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    /// Every classical listen in a window, with its composer's dates.
    pub async fn classical_stat_rows(
        &self,
        user_id: i64,
        f: &InsightFilter,
    ) -> Result<Vec<ClassicalStatRow>, sqlx::Error> {
        let (cond, binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let sql = format!(
            "SELECT COALESCE(s.duration, 0) AS seconds, s.timestamp AS timestamp,
                    COALESCE(f.composer_mbid, LOWER(f.composer_name)) AS composer_key,
                    f.composer_name AS composer_name, f.composed_year AS composed_year, f.form AS form,
                    d.begin AS born, d.end_date AS died, d.ended AS ended, d.gender AS gender,
                    d.country AS country
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
               LEFT JOIN artist_dates d ON d.mbid = f.composer_mbid
              WHERE s.skipped = FALSE AND {cond}"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, ClassicalStatRow>(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        q.fetch_all(&self.pool).await
    }

    /// Performers on classical listens in a window, grouped by who, in what
    /// role, from where, playing whose piece.
    pub async fn classical_performer_rows(
        &self,
        user_id: i64,
        f: &InsightFilter,
    ) -> Result<Vec<PerformerStatRow>, sqlx::Error> {
        let (cond, binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let sql = format!(
            "SELECT rp.artist_key, MAX(rp.name), rp.role, MAX(rp.country),
                    COALESCE(f.composer_mbid, LOWER(f.composer_name)), f.piece_key, COUNT(*)
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
               JOIN recording_performers rp ON rp.fact_key = s.fact_key
              WHERE s.skipped = FALSE AND {cond}
              GROUP BY rp.artist_key, rp.role, COALESCE(f.composer_mbid, LOWER(f.composer_name)), f.piece_key"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, PerformerStatRow>(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        q.fetch_all(&self.pool).await
    }

    /// Every classical piece in a window, for the timeline.
    pub async fn classical_timeline(
        &self,
        user_id: i64,
        f: &InsightFilter,
    ) -> Result<Vec<TimelineRow>, sqlx::Error> {
        let (cond, binds) = Self::insight_where(user_id, crate::classify::CLASSICAL, f);
        let sql = format!(
            "SELECT f.piece_key AS piece_key, MAX(f.piece_title) AS piece_title,
                    MAX(COALESCE(f.composer_mbid, LOWER(f.composer_name))) AS composer_key,
                    MAX(f.composer_name) AS composer_name, MAX(f.composed_year) AS composed_year,
                    MAX(d.begin) AS born, COUNT(*) AS plays, COALESCE(SUM(s.duration), 0) AS seconds
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
               LEFT JOIN artist_dates d ON d.mbid = f.composer_mbid
              WHERE s.skipped = FALSE AND {cond}
              GROUP BY f.piece_key"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, TimelineRow>(&stmt);
        for b in binds {
            q = match b {
                Bind::I(v) => q.bind(v),
                Bind::O(v) => q.bind(v),
                Bind::S(v) => q.bind(v),
            };
        }
        q.fetch_all(&self.pool).await
    }

    /// Artists credited on this listener's listens, and composers of pieces
    /// they heard, with their dates — at least `min_plays` listens each.
    pub async fn dated_artists(
        &self,
        user_id: i64,
        min_plays: i64,
    ) -> Result<Vec<DatedArtist>, sqlx::Error> {
        let mut credited: Vec<DatedArtist> = sqlx::query_as(&crate::sql!(
            "SELECT d.mbid AS mbid, MAX(d.name) AS name, MAX(d.kind) AS kind, MAX(d.begin) AS begin,
                    MAX(d.end_date) AS end_date, MAX(d.ended) AS ended, COUNT(*) AS plays
               FROM scrobble_artists sa JOIN scrobbles s ON s.id = sa.scrobble_id
               JOIN artist_dates d ON d.mbid = sa.mbid
              WHERE s.user_id = ? AND s.skipped = FALSE
              GROUP BY d.mbid HAVING COUNT(*) >= ?"
        ))
        .bind(user_id)
        .bind(min_plays)
        .fetch_all(&self.pool)
        .await?;
        let composers: Vec<DatedArtist> = sqlx::query_as(&crate::sql!(
            "SELECT d.mbid AS mbid, MAX(d.name) AS name, MAX(d.kind) AS kind, MAX(d.begin) AS begin,
                    MAX(d.end_date) AS end_date, MAX(d.ended) AS ended, COUNT(*) AS plays
               FROM scrobbles s JOIN recording_facts f ON f.fact_key = s.fact_key
               JOIN artist_dates d ON d.mbid = f.composer_mbid
              WHERE s.user_id = ? AND s.skipped = FALSE
              GROUP BY d.mbid HAVING COUNT(*) >= ?"
        ))
        .bind(user_id)
        .bind(min_plays)
        .fetch_all(&self.pool)
        .await?;
        for c in composers {
            match credited.iter_mut().find(|a| a.mbid == c.mbid) {
                Some(a) => a.plays = a.plays.max(c.plays),
                None => credited.push(c),
            }
        }
        Ok(credited)
    }

    /// Albums in this listener's history with a release year:
    /// `(album, filed under, earliest year, plays, last played)`.
    #[allow(clippy::type_complexity)]
    pub async fn dated_albums(
        &self,
        user_id: i64,
        min_plays: i64,
    ) -> Result<Vec<(String, String, i64, i64, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT MAX(album), MAX(COALESCE(album_filed_under, artist)), MIN(release_year), COUNT(*), MAX(timestamp)
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND release_year IS NOT NULL AND release_year > 0
                AND album IS NOT NULL AND TRIM(album) != ''
              GROUP BY LOWER(TRIM(album)), LOWER(TRIM(COALESCE(album_filed_under, artist)))
             HAVING COUNT(*) >= ?"
        ))
        .bind(user_id)
        .bind(min_plays)
        .fetch_all(&self.pool)
        .await
    }

    /// One artist's dates, by MBID or, failing that, by name.
    pub async fn artist_dates_for(
        &self,
        mbid: Option<&str>,
        name: &str,
    ) -> Result<Option<DatedArtist>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT mbid, name, kind, begin, end_date, ended, 0 AS plays FROM artist_dates
              WHERE (? IS NOT NULL AND mbid = ?) OR LOWER(name) = ?
              ORDER BY CASE WHEN mbid = ? THEN 0 ELSE 1 END LIMIT 1"
        ))
        .bind(mbid)
        .bind(mbid)
        .bind(name.trim().to_lowercase())
        .bind(mbid.unwrap_or(""))
        .fetch_optional(&self.pool)
        .await
    }
}
