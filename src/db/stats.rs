//! Ranged analytics.
//!
//! Paired counts must share a population. `FIDELITY_KNOWN` is one constant so
//! a fifth fidelity query cannot get it wrong, and it is NULL-safe because the
//! same predicate is used under a `NOT`.

use super::*;

impl Database {
    /// `after` = None means all-time. Genres/languages/regions/decades need
    /// MusicBrainz enrichment we don't have yet, so they return empty arrays
    /// (the UI hides empty breakdowns). totals/top_artists/heatmap are real.
    pub async fn get_ranged_stats(
        &self,
        user_id: i64,
        after: Option<i64>,
        by_hours: bool,
    ) -> Result<serde_json::Value, sqlx::Error> {
        // The top lists are cut to 50 rows, so the sort has to happen in SQL:
        // re-sorting the returned 50 by hours in the browser would rank within
        // the top 50 *by plays*, and silently miss a long record played a few
        // times — exactly the row switching to Hours exists to surface.
        //
        // `SUM(duration)` is NULL for listens whose source reported no length,
        // so it is coalesced to 0; those sort last under Hours, which is right
        // — an unknown running time is not a large one.
        let order = if by_hours { "COALESCE(secs, 0) DESC, plays DESC" } else { "plays DESC" };
        let plays: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let total = plays.0;

        let secs: (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let hours = secs.0.unwrap_or(0) as f64 / 3600.0;
        // Hours is a sum over the listens that *have* a length, so it is a floor
        // rather than a total — a source that reported no duration contributes
        // nothing and says nothing. Cheaper to state the coverage than to
        // pretend a listen of unknown length lasted zero seconds.
        let duration_known: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;

        let artists: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let albums: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT album) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND album IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let tracks: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT title || '|' || artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let avg_quality: (Option<f64>,) = sqlx::query_as(
            "SELECT AVG(quality_score) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND quality_score IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        // The average's own denominator. Paired counts must share a population,
        // and this one is *not* `total` — see `quality_known` in the payload.
        let quality_known: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND quality_score IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let lossless: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND is_lossless = TRUE"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let dsd: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND format_type = 'dsd'"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let fidelity_known: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                AND {FIDELITY_KNOWN}"
        ))
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
        let skips: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = TRUE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;

        // "Since" is a fact about the whole history and is therefore *not*
        // ranged — asking it of a one-week window would answer with the start
        // of that window, which is a restatement of the control rather than an
        // answer. Skips are excluded: the first thing you skipped past is not
        // the day you started listening.
        let first_listen: (Option<i64>,) = sqlx::query_as(
            "SELECT MIN(timestamp) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        let loves: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM loves WHERE user_id = ?")
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
        let top_rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(&format!(
            "SELECT MAX(sa.name) AS artist, COUNT(*) as plays, SUM(s.duration) as secs
             FROM scrobble_artists sa
             JOIN scrobbles s ON s.id = sa.scrobble_id
             WHERE s.user_id = ? AND s.skipped = FALSE AND (? IS NULL OR s.timestamp >= ?)
             GROUP BY sa.artist_key ORDER BY {} LIMIT 50",
            order
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let names: Vec<String> = top_rows.iter().map(|(n, _, _)| n.clone()).collect();
        let images = self.artist_images(&names).await.unwrap_or_default();
        let top_artists: Vec<serde_json::Value> = top_rows
            .into_iter()
            .map(|(name, plays, secs)| {
                let image = images.get(&Self::artist_key(&name)).cloned();
                serde_json::json!({
                    "name": name, "plays": plays,
                    "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0,
                    "image_url": image,
                })
            })
            .collect();

        let album_rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(&format!(
            "SELECT album, artist, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND album IS NOT NULL AND album != ''
             GROUP BY album, artist ORDER BY {} LIMIT 50",
            order
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let top_albums: Vec<serde_json::Value> = album_rows
            .into_iter()
            .map(|(name, artist, plays, secs)| {
                serde_json::json!({ "name": name, "artist": artist, "plays": plays,
                "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0 })
            })
            .collect();

        // Grouped case-insensitively on title+artist, like every other listing
        // query — the same song submitted as "Xtal" and "XTAL" by two clients is
        // one track, not two entries competing for the same slot.
        let track_rows: Vec<(String, String, Option<String>, i64, Option<i64>)> = sqlx::query_as(
            &format!(            "SELECT title, artist, MAX(album) AS album, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist)) ORDER BY {} LIMIT 50",
                order
            ),
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let top_tracks: Vec<serde_json::Value> = track_rows
            .into_iter()
            .map(|(title, artist, album, plays, secs)| {
                serde_json::json!({ "title": title, "artist": artist, "album": album,
                "plays": plays,
                "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0 })
            })
            .collect();

        // Fidelity mix, bucketed the way the badges are: DSD > lossless > lossy,
        // with anything unlabelled called out rather than silently dropped.
        //
        // `Unknown` is written as the complement of `FIDELITY_KNOWN` rather than
        // as its own `ELSE`, so this bucket and the shares above are provably
        // the same split of the same rows. Written the other way round they were
        // *almost* the same, which is worse than different.
        let fidelity_rows: Vec<(String, i64)> = sqlx::query_as(&format!(
            "SELECT CASE
                    WHEN NOT {FIDELITY_KNOWN} THEN 'Unknown'
                    WHEN format_type = 'dsd' THEN 'DSD'
                    WHEN is_lossless = TRUE THEN 'Lossless'
                    ELSE 'Lossy'
                END AS tier, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY tier ORDER BY c DESC"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let fidelity: Vec<serde_json::Value> = fidelity_rows
            .into_iter()
            .map(|(name, c)| serde_json::json!({ "name": name, "plays": c, "pct": pct(c) }))
            .collect();

        // Release decades. Only listens whose release year has been resolved by
        // the MusicBrainz backfill count — the rest are simply absent.
        let decade_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT (release_year / 10) * 10 AS decade, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND release_year IS NOT NULL AND release_year > 1000
             GROUP BY decade ORDER BY decade",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let decade_total: i64 = decade_rows.iter().map(|(_, c)| c).sum();
        let decades: Vec<serde_json::Value> = decade_rows.into_iter().map(|(d, c)| {
            serde_json::json!({ "name": format!("{}s", d), "plays": c,
                "pct": if decade_total > 0 { (c as f64 / decade_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        // Genre breakdown. One listen carries several tags, so `json_each`
        // fans the array out and each tag is counted once per listen that has
        // it — which means the percentages are a share of *tag mentions*, not
        // of listens, and cannot be read as "40% of what I played was rock".
        // Tombstoned rows (`[]`) contribute nothing and drop out naturally.
        let genre_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT g.value AS name, COUNT(*) AS c
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (? IS NULL OR s.timestamp >= ?) AND s.genres IS NOT NULL
              GROUP BY LOWER(g.value) ORDER BY c DESC LIMIT 12",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let genre_total: i64 = genre_rows.iter().map(|(_, c)| c).sum();
        let genres: Vec<serde_json::Value> = genre_rows.into_iter().map(|(name, c)| {
            serde_json::json!({ "name": name, "plays": c,
                "pct": if genre_total > 0 { (c as f64 / genre_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        // Language breakdown (only listens that have been classified).
        // `skipped = FALSE` was missing here: a skip is stored but must never be
        // counted, and this is a counting query like any other.
        let lang_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT language, COUNT(*) as c FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND language IS NOT NULL
             GROUP BY language ORDER BY c DESC LIMIT 12",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let lang_total: i64 = lang_rows.iter().map(|(_, c)| c).sum();
        let languages: Vec<serde_json::Value> = lang_rows.into_iter().map(|(name, c)| {
            serde_json::json!({ "name": name, "pct": if lang_total > 0 { (c as f64 / lang_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        Ok(serde_json::json!({
            "totals": {
                "plays": total, "hours": (hours * 10.0).round() / 10.0,
                // What that sum is over. A listen with no reported length adds
                // nothing to it, so the figure is a floor.
                "duration_known": duration_known.0,
                "artists": artists.0, "albums": albums.0, "tracks": tracks.0,
                "avg_quality": avg_quality.0.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
                // How many listens that average is actually over. `compute_score`
                // only runs at ingest and in the Plex/Jellyfin sources, so an
                // imported listen has no score — the average has the same narrow
                // population the fidelity shares do, and the card must say so
                // rather than presenting a figure drawn from 5% of a history as
                // a fact about all of it.
                "quality_known": quality_known.0,
                "lossless_pct": fpct(lossless.0), "dsd_pct": fpct(dsd.0),
                // The two shares above are of this, not of `plays`.
                "fidelity_known": fidelity_known.0,
                "skips": skips.0,
                "per_day": per_day,
                "first_listen": first_listen.0,
                "loves": loves.0,
            },
            "top_artists": top_artists,
            "top_albums": top_albums,
            "top_tracks": top_tracks,
            "fidelity": fidelity,
            "decades": decades,
            "genres": genres, "languages": languages, "regions": [],
            // heatmap is filled in by the handler in the user's local timezone
            "heatmap": Vec::<Vec<i64>>::new(),
        }))
    }
}
