//! Ranged analytics.
//!
//! Paired counts must share a population. `FIDELITY_KNOWN` is one constant so
//! a fifth fidelity query cannot get it wrong, and it is NULL-safe because the
//! same predicate is used under a `NOT`.

use super::*;

/// Ranged statistics: totals, top lists (50 rows each), fidelity mix, decades,
/// a 7×24 listening heatmap, breadth, genres and languages.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RangedStats {
    pub totals: RangedTotals,
    pub top_artists: Vec<TopArtist>,
    pub top_albums: Vec<TopAlbum>,
    pub top_tracks: Vec<TopTrack>,
    /// `DSD`, `Lossless`, `Lossy` and `Unknown`, largest first. A share of
    /// `plays`, because it carries the `Unknown` bucket of its own.
    pub fidelity: Vec<Share>,
    /// Release decades, `1970s` and so on, oldest first. Only listens whose
    /// release year has been resolved count, so `pct` is a share of those.
    pub decades: Vec<Share>,
    /// The twelve most-mentioned genres. **A share of tag mentions, not
    /// listens** — one track carries several tags. Never present it as a
    /// percentage of plays.
    pub genres: Vec<Share>,
    /// The twelve largest languages, over the listens that have been
    /// classified.
    pub languages: Vec<LanguageShare>,
    /// Always empty; reserved.
    #[schema(value_type = Vec<Object>)]
    pub regions: Vec<serde_json::Value>,
    /// Seven rows of 24, counts of plays by hour, in your own timezone. Row 0
    /// is Monday.
    pub heatmap: Vec<Vec<i64>>,
    /// Absent until the genre map is fetched and genres backfilled — "you play
    /// 0 genres" would be a lie about the listening rather than a statement
    /// about the data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breadth: Option<crate::server::everynoise::Breadth>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RangedTotals {
    pub plays: i64,
    /// Summed over the listens with a known length — see `duration_known`.
    pub hours: f64,
    /// Listens `hours` is summed over. A listen whose source reported no
    /// length contributes nothing, so `hours` is a floor rather than a total.
    pub duration_known: i64,
    pub artists: i64,
    pub albums: i64,
    pub tracks: i64,
    /// 0–100, averaged over `quality_known` listens only; 0 when there are none.
    pub avg_quality: f64,
    /// Listens `avg_quality` is averaged over. Only a live source ever writes a
    /// quality score, so an imported history has none and the average is over
    /// the same narrow population the fidelity shares are. Present it with its
    /// coverage or not at all; at zero there is no score to show, not a score
    /// of zero.
    pub quality_known: i64,
    /// Share of `fidelity_known`, not of `plays`. A whole percentage.
    pub lossless_pct: i64,
    /// Share of `fidelity_known`, not of `plays`. A whole percentage.
    pub dsd_pct: i64,
    /// Listens in this window whose format is known — the denominator of the
    /// two shares above. The `fidelity` array is a share of `plays` instead,
    /// because it carries an `Unknown` bucket of its own.
    pub fidelity_known: i64,
    /// Skipped plays in this window. Every other count in this object excludes
    /// them; this one is about them. Not every source reports skips, so zero
    /// means "nothing told us", not "nothing skipped".
    pub skips: i64,
    /// Plays per **calendar** day of the window, including days with no
    /// listening. Dividing by active days instead reports a busy afternoon's
    /// rate as the year's.
    pub per_day: f64,
    /// Earliest non-skipped listen, unix seconds. A fact about the whole
    /// history and therefore **not** ranged — asking it of a one-week window
    /// would only restate the window.
    pub first_listen: Option<i64>,
    /// Loved entities: tracks, records and artists together. All time, not
    /// ranged.
    pub loves: i64,
}

/// A top artist. Counted over everyone credited, the way the charts count.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct TopArtist {
    pub name: String,
    pub plays: i64,
    /// One decimal place.
    pub hours: f64,
    pub image_url: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct TopAlbum {
    pub name: String,
    pub artist: String,
    pub plays: i64,
    /// One decimal place.
    pub hours: f64,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct TopTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub plays: i64,
    /// One decimal place.
    pub hours: f64,
}

/// One slice of a breakdown.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Share {
    pub name: String,
    pub plays: i64,
    /// A whole percentage of the breakdown's own total — which total, each
    /// breakdown says.
    pub pct: i64,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct LanguageShare {
    pub name: String,
    /// A whole percentage of the classified listens.
    pub pct: i64,
}

impl Database {
    /// `after` = None means all-time. Genres/languages/regions/decades need
    /// MusicBrainz enrichment we don't have yet, so they return empty arrays
    /// (the UI hides empty breakdowns). totals/top_artists/heatmap are real.
    pub async fn get_ranged_stats(
        &self,
        user_id: i64,
        after: Option<i64>,
        by_hours: bool,
    ) -> Result<RangedStats, sqlx::Error> {
        // The top lists are cut to 50 rows, so the sort has to happen in SQL:
        // re-sorting the returned 50 by hours in the browser would rank within
        // the top 50 *by plays*, and silently miss a long record played a few
        // times — exactly the row switching to Hours exists to surface.
        //
        // `SUM(duration)` is NULL for listens whose source reported no length,
        // so it is coalesced to 0; those sort last under Hours, which is right
        // — an unknown running time is not a large one.
        let order = if by_hours { "COALESCE(secs, 0) DESC, plays DESC" } else { "plays DESC" };
        let plays: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let total = plays.0;

        let secs: (Option<i64>,) = sqlx::query_as(&crate::sql!("SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let hours = secs.0.unwrap_or(0) as f64 / 3600.0;
        // Hours is a sum over the listens that *have* a length, so it is a floor
        // rather than a total — a source that reported no duration contributes
        // nothing and says nothing. Cheaper to state the coverage than to
        // pretend a listen of unknown length lasted zero seconds.
        let duration_known: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;

        let artists: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let albums: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(DISTINCT album) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND album IS NOT NULL")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let tracks: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(DISTINCT title || '|' || artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let avg_quality: (Option<f64>,) = sqlx::query_as(&crate::sql!("SELECT AVG(quality_score) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND quality_score IS NOT NULL")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        // The average's own denominator. Paired counts must share a population,
        // and this one is *not* `total` — see `quality_known` in the payload.
        let quality_known: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND quality_score IS NOT NULL")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let lossless: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND is_lossless = TRUE")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let dsd: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND format_type = 'dsd'")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let fidelity_known: (i64,) = sqlx::query_as(&crate::sql!(&format!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                AND {FIDELITY_KNOWN}"
        )))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_one(&self.pool)
        .await?;
        // ── The four "shape of the listening" figures ──
        //
        // Deliberately *not* filtered to `skipped = FALSE`: this is the one
        // query in the file whose whole subject is the skips. Everything that
        // counts listens still filters them out, and `plays` above already did.
        let skips: (i64,) = sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = TRUE AND (? IS NULL OR timestamp >= ?)")).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;

        // "Since" is a fact about the whole history and is therefore *not*
        // ranged — asking it of a one-week window would answer with the start
        // of that window, which is a restatement of the control rather than an
        // answer. Skips are excluded: the first thing you skipped past is not
        // the day you started listening.
        let first_listen: (Option<i64>,) = sqlx::query_as(&crate::sql!(
            "SELECT MIN(timestamp) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        let loves: (i64,) =
            sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM loves WHERE user_id = ?"))
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;

        // Plays per **calendar** day, not per day you happened to listen — 846
        // plays over five active days reads as 169/day when the month's real
        // rate is 27. Same trap `reports::avg_daily` documents.
        //
        // `range_after` builds rolling windows (`now - 7 days`) rather than
        // calendar periods, so the span is simply how far back `after` reaches;
        // all-time measures from the first listen. Both are clamped to at least
        // a day, or a history a few hours old divides by nearly zero and
        // reports a spectacular rate.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let span_secs = match after {
            Some(a) => now - a,
            None => first_listen.0.map(|f| now - f).unwrap_or(0),
        };
        let days = (span_secs as f64 / 86_400.0).max(1.0);
        let per_day = if total > 0 { (total as f64 / days * 10.0).round() / 10.0 } else { 0.0 };

        let pct = |n: i64| {
            if total > 0 {
                (n as f64 / total as f64 * 100.0).round() as i64
            } else {
                0
            }
        };
        // The fidelity shares have their own denominator, and it is not `total`
        // — see `FIDELITY_KNOWN`. The mix below keeps `pct`, because it carries
        // an `Unknown` bucket and so really is a share of everything.
        let fpct = |n: i64| {
            if fidelity_known.0 > 0 {
                (n as f64 / fidelity_known.0 as f64 * 100.0).round() as i64
            } else {
                0
            }
        };

        // Over the credits, exactly as `chart()` does. These two answer the
        // same question on two screens, and the whole reason the handoff
        // between them exists is that they must not disagree.
        let top_rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT MAX(sa.name) AS artist, COUNT(*) as plays, SUM(s.duration) as secs
             FROM scrobble_artists sa
             JOIN scrobbles s ON s.id = sa.scrobble_id
             WHERE s.user_id = ? AND s.skipped = FALSE AND (? IS NULL OR s.timestamp >= ?)
             GROUP BY sa.artist_key ORDER BY {} LIMIT 50",
            order
        )))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let names: Vec<String> = top_rows.iter().map(|(n, _, _)| n.clone()).collect();
        let images = self.artist_images(user_id, &names).await.unwrap_or_default();
        let top_artists: Vec<TopArtist> = top_rows
            .into_iter()
            .map(|(name, plays, secs)| {
                let image_url = images.get(&Self::artist_key(&name)).cloned();
                TopArtist {
                    name,
                    plays,
                    hours: (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0,
                    image_url,
                }
            })
            .collect();

        let album_rows: Vec<(String, String, i64, Option<i64>)> =
            sqlx::query_as(&crate::sql!(&format!(
                "SELECT album, artist, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND album IS NOT NULL AND album != ''
             GROUP BY album, artist ORDER BY {} LIMIT 50",
                order
            )))
            .bind(user_id)
            .bind(after)
            .bind(after)
            .fetch_all(&self.pool)
            .await?;
        let top_albums: Vec<TopAlbum> = album_rows
            .into_iter()
            .map(|(name, artist, plays, secs)| TopAlbum {
                name,
                artist,
                plays,
                hours: (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0,
            })
            .collect();

        // Grouped case-insensitively on title+artist, like every other listing
        // query — the same song submitted as "Xtal" and "XTAL" by two clients is
        // one track, not two entries competing for the same slot.
        let track_rows: Vec<(String, String, Option<String>, i64, Option<i64>)> = sqlx::query_as(&crate::sql!(&format!(            "SELECT title, artist, MAX(album) AS album, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist)) ORDER BY {} LIMIT 50",
                order
            ),))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let top_tracks: Vec<TopTrack> = track_rows
            .into_iter()
            .map(|(title, artist, album, plays, secs)| TopTrack {
                title,
                artist,
                album,
                plays,
                hours: (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0,
            })
            .collect();

        // Fidelity mix, bucketed the way the badges are: DSD > lossless > lossy,
        // with anything unlabelled called out rather than silently dropped.
        //
        // `Unknown` is written as the complement of `FIDELITY_KNOWN` rather than
        // as its own `ELSE`, so this bucket and the shares above are provably
        // the same split of the same rows. Written the other way round they were
        // *almost* the same, which is worse than different.
        let fidelity_rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT CASE
                    WHEN NOT {FIDELITY_KNOWN} THEN 'Unknown'
                    WHEN format_type = 'dsd' THEN 'DSD'
                    WHEN is_lossless = TRUE THEN 'Lossless'
                    ELSE 'Lossy'
                END AS tier, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY tier ORDER BY c DESC"
        )))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let fidelity: Vec<Share> = fidelity_rows
            .into_iter()
            .map(|(name, c)| Share { name, plays: c, pct: pct(c) })
            .collect();

        // Release decades. Only listens whose release year has been resolved by
        // the MusicBrainz backfill count — the rest are simply absent.
        let decade_rows: Vec<(i64, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT (release_year / 10) * 10 AS decade, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND release_year IS NOT NULL AND release_year > 1000
             GROUP BY decade ORDER BY decade"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let decade_total: i64 = decade_rows.iter().map(|(_, c)| c).sum();
        let decades: Vec<Share> = decade_rows
            .into_iter()
            .map(|(d, c)| Share {
                name: format!("{}s", d),
                plays: c,
                pct: if decade_total > 0 {
                    (c as f64 / decade_total as f64 * 100.0).round() as i64
                } else {
                    0
                },
            })
            .collect();

        // Genre breakdown. One listen carries several tags, so `json_each`
        // fans the array out and each tag is counted once per listen that has
        // it — which means the percentages are a share of *tag mentions*, not
        // of listens, and cannot be read as "40% of what I played was rock".
        // Tombstoned rows (`[]`) contribute nothing and drop out naturally.
        let genre_rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT g.value AS name, COUNT(*) AS c
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (? IS NULL OR s.timestamp >= ?) AND s.genres IS NOT NULL
              GROUP BY LOWER(g.value) ORDER BY c DESC LIMIT 12"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let genre_total: i64 = genre_rows.iter().map(|(_, c)| c).sum();
        let genres: Vec<Share> = genre_rows
            .into_iter()
            .map(|(name, c)| Share {
                name,
                plays: c,
                pct: if genre_total > 0 {
                    (c as f64 / genre_total as f64 * 100.0).round() as i64
                } else {
                    0
                },
            })
            .collect();

        // Language breakdown (only listens that have been classified).
        // `skipped = FALSE` was missing here: a skip is stored but must never be
        // counted, and this is a counting query like any other.
        let lang_rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT language, COUNT(*) as c FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND language IS NOT NULL
             GROUP BY language ORDER BY c DESC LIMIT 12"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let lang_total: i64 = lang_rows.iter().map(|(_, c)| c).sum();
        let languages: Vec<LanguageShare> = lang_rows
            .into_iter()
            .map(|(name, c)| LanguageShare {
                name,
                pct: if lang_total > 0 {
                    (c as f64 / lang_total as f64 * 100.0).round() as i64
                } else {
                    0
                },
            })
            .collect();

        Ok(RangedStats {
            totals: RangedTotals {
                plays: total,
                hours: (hours * 10.0).round() / 10.0,
                // What that sum is over. A listen with no reported length adds
                // nothing to it, so the figure is a floor.
                duration_known: duration_known.0,
                artists: artists.0,
                albums: albums.0,
                tracks: tracks.0,
                avg_quality: avg_quality.0.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
                // How many listens that average is actually over. `compute_score`
                // only runs at ingest and in the Plex/Jellyfin sources, so an
                // imported listen has no score — the average has the same narrow
                // population the fidelity shares do, and the card must say so
                // rather than presenting a figure drawn from 5% of a history as
                // a fact about all of it.
                quality_known: quality_known.0,
                lossless_pct: fpct(lossless.0),
                dsd_pct: fpct(dsd.0),
                // The two shares above are of this, not of `plays`.
                fidelity_known: fidelity_known.0,
                skips: skips.0,
                per_day,
                first_listen: first_listen.0,
                loves: loves.0,
            },
            top_artists,
            top_albums,
            top_tracks,
            fidelity,
            decades,
            genres,
            languages,
            regions: Vec::new(),
            // heatmap is filled in by the handler in the user's local timezone
            heatmap: Vec::new(),
            breadth: None,
        })
    }
}
