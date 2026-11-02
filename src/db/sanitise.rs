//! Preview-then-apply metadata repair. **Never automatic at ingest** — a
//! 15-way split with a 4-play plurality is a weak signal the operator should
//! see before it is applied.

use super::*;

impl Database {
    /// Albums where the listens disagree about which release they are.
    ///
    /// Returns (artist, album, total plays, distinct release MBIDs, the winning
    /// The release MBID the most listens of this album claim, if any do.
    ///
    /// A plurality rather than a lookup because a tagger works per *track*, so
    /// one listen out of twelve routinely carries a compilation's MBID — the
    /// exact split `albums_with_mixed_releases` exists to clean up. Anything
    /// wanting "which record is this" should ask the listens, not the first row
    /// it happens to find.
    pub async fn album_release_mbid(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT mbid_release
               FROM scrobbles
              WHERE user_id = ? AND LOWER(TRIM(artist)) = LOWER(TRIM(?))
                AND LOWER(TRIM(album)) = LOWER(TRIM(?))
                AND mbid_release IS NOT NULL AND TRIM(mbid_release) != ''
              GROUP BY mbid_release
              ORDER BY COUNT(*) DESC
              LIMIT 1"
        ))
        .bind(user_id)
        .bind(artist)
        .bind(album)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(m,)| m))
    }
    /// MBID, how many rows carry it).
    pub async fn albums_with_mixed_releases(
        &self,
        user_id: i64,
    ) -> Result<Vec<MixedAlbum>, sqlx::Error> {
        // One flat query, grouped in Rust. The previous version did the
        // majority vote in SQL with correlated subqueries and could only report
        // the winner; per-edit review needs every candidate and its play count,
        // because deciding whether a 4-of-29 plurality is trustworthy is
        // exactly the judgement being handed to the operator.
        let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT artist, album, mbid_release, COUNT(*) AS n
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND album IS NOT NULL AND TRIM(album) != ''
              GROUP BY LOWER(TRIM(artist)), LOWER(TRIM(album)), mbid_release
              ORDER BY n DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // Key on the case-folded pair; keep the first spelling seen for display.
        let mut groups: HashMap<(String, String), MixedAlbum> = HashMap::new();
        for (artist, album, mbid, n) in rows {
            let key = (artist.trim().to_lowercase(), album.trim().to_lowercase());
            let entry = groups.entry(key).or_insert_with(|| MixedAlbum {
                artist: artist.clone(),
                album: album.clone(),
                plays: 0,
                variants: 0,
                winner: None,
                winner_plays: 0,
                untagged: 0,
                candidates: Vec::new(),
            });
            entry.plays += n;
            match mbid.filter(|m| !m.trim().is_empty()) {
                Some(m) => {
                    entry.variants += 1;
                    // Rows arrive most-played first, so the first is the winner.
                    if entry.winner.is_none() {
                        entry.winner = Some(m.clone());
                        entry.winner_plays = n;
                    }
                    entry.candidates.push(ReleaseCandidate { mbid: m, plays: n });
                }
                // Listens with no release at all. Unifying stamps these too,
                // which is worth showing rather than burying in the total.
                None => entry.untagged += n,
            }
        }

        let mut out: Vec<MixedAlbum> = groups.into_values().filter(|g| g.variants > 1).collect();
        out.sort_by(|a, b| b.plays.cmp(&a.plays));
        Ok(out)
    }
    /// Force every listen of one album onto a single release MBID.
    ///
    /// Artwork is cleared alongside it — the resolved cover belongs to the
    /// release being replaced, and `artwork_url`/`caa_id` are the queue keys for
    /// the backfill, so nulling them is what re-fetches the right one.
    /// Returns how many rows changed.
    pub async fn unify_album_release(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
        mbid_release: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE scrobbles
                SET mbid_release = ?, artwork_url = NULL, caa_id = NULL, caa_release_mbid = NULL
              WHERE user_id = ?
                AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
                AND (mbid_release IS NULL OR mbid_release != ?)"
        ))
        .bind(mbid_release)
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .bind(mbid_release)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
    /// Artist strings that carry a featured credit, with the primary artist they
    /// would fold into and how many listens are affected.
    ///
    /// `Cœur de pirate feat. Loud` is a *different artist* to every query in the
    /// app, so one track drops out of its own album and off its own artist page.
    /// Only the explicit `feat.` / `ft.` / `featuring` / `with` forms are
    /// matched — `&` and `,` are left alone, because "Simon & Garfunkel" and
    /// "Earth, Wind & Fire" are names, not credits.
    pub async fn artists_with_featured_credits(
        &self,
        user_id: i64,
    ) -> Result<Vec<FeaturedArtist>, sqlx::Error> {
        let rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY LOWER(TRIM(artist)) ORDER BY plays DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(artist, plays)| {
                let primary = split_featured(&artist)?;
                Some(FeaturedArtist { artist, primary, plays })
            })
            .collect())
    }
    /// Artist names that differ only in punctuation or spacing.
    ///
    /// "A.R. Rahman" and "A. R. Rahman" are one person but two rows in every
    /// grouping query, so they appear twice in the same top-50 and split their
    /// own play count. Grouped by a key with all punctuation and whitespace
    /// removed; the spelling with the most plays wins, on the theory that the
    /// one you've scrobbled most is the one your library actually uses.
    ///
    /// Only *variants of one name* are matched — this can't merge two artists
    /// who are genuinely different, because the normalised forms would differ.
    pub async fn artists_with_spelling_variants(
        &self,
        user_id: i64,
    ) -> Result<Vec<VariantArtist>, sqlx::Error> {
        // Grouped by the **exact** string, not by `LOWER(TRIM(artist))`.
        //
        // That was the bug: the Rust grouping below also lowercases, so
        // anything it would merge on case alone, the SQL had already merged —
        // SQLite returned one arbitrarily-chosen spelling per lowercased name
        // and the group came back with a single member. Case-only variants
        // were therefore invisible *by construction*: on a 21k-listen history
        // this reported nothing while thirteen real groups sat in the data.
        // The half that worked — punctuation and spacing, "A.R. Rahman"
        // against "A R Rahman" — survives the collapse, which is why it
        // looked correct.
        let rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY artist ORDER BY plays DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut groups: HashMap<String, Vec<(String, i64)>> = HashMap::new();
        for (artist, plays) in rows {
            groups.entry(normalise_artist(&artist)).or_default().push((artist, plays));
        }

        let mut out: Vec<VariantArtist> = groups
            .into_values()
            .filter(|g| g.len() > 1)
            .map(|mut g| {
                // Already ordered by plays from the query; be explicit anyway.
                g.sort_by(|a, b| b.1.cmp(&a.1));
                let (primary, primary_plays) = g[0].clone();
                // Whether these differ by anything a reader would notice.
                // Most read paths already lowercase — `chart()` groups on
                // `LOWER(TRIM(artist))`, `entity_key` lowercases — so a
                // case-only group has correct counts and merely displays
                // inconsistently. Flagged rather than hidden: it is a real
                // variant, just a cosmetic one, and burying it with the
                // count-splitting kind would make the list unreadable.
                let case_only = g.iter().all(|(n, _)| n.to_lowercase() == primary.to_lowercase());
                VariantArtist {
                    primary,
                    primary_plays,
                    variants: g[1..].iter().map(|(n, p)| (n.clone(), *p)).collect(),
                    case_only,
                }
            })
            .collect();
        // The ones that actually split a play count first; cosmetic ones last.
        out.sort_by(|a, b| {
            a.case_only.cmp(&b.case_only).then(b.primary_plays.cmp(&a.primary_plays))
        });
        Ok(out)
    }
    /// Joined artist strings in the history, and how each would split.
    ///
    /// Runs `normalise_credit` — the rule every new listen gets at insert — over
    /// what is already stored, so an old listen ends up exactly as it would
    /// have if it arrived today. Strings it cannot split but which *look*
    /// joined (a `,`, `&`, ` x `, ` and ` or ` + `) are returned with no
    /// names, for a hand split; "Simon & Garfunkel" is among them and simply
    /// stays unticked.
    pub async fn joined_artist_credits(
        &self,
        user_id: i64,
    ) -> Result<Vec<JoinedCredit>, sqlx::Error> {
        let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT artist, artist_credit, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY artist, artist_credit ORDER BY plays DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::new();
        for (artist, credit, plays) in rows {
            let mut play =
                Play::new(String::new(), artist.clone(), 0, String::new(), String::new());
            // The stored list — or, when that is one entry, the string the older
            // featured-credit fold kept there ("Daft Punk feat. Romanthony",
            // with "Daft Punk" already in `artist`), cut the same way a new
            // listen's would be.
            play.artists = credit.as_deref().map(|c| {
                let listed = split_stored_credit(c);
                if listed.len() > 1 {
                    listed
                } else {
                    split_credit(c)
                }
            });
            let renamed = normalise_credit(&mut play).is_some();
            let names = play.artists.clone().unwrap_or_default();
            // A rewrite of the credit alone counts too: that is the fold's
            // leftover, still credited in `scrobble_artists` as one phantom.
            let recredited =
                names.len() > 1 && credit.as_deref() != Some(names.join("; ").as_str());
            if renamed || recredited {
                out.push(JoinedCredit { artist, credit, names, plays });
            } else if credit.is_none() && !is_various_artists(&artist) && looks_joined(&artist) {
                out.push(JoinedCredit { artist, credit, names: Vec::new(), plays });
            }
        }
        Ok(out)
    }

    /// Split every listen stored as exactly `(artist, credit)` onto `names`,
    /// primary first, and re-credit them in `scrobble_artists`.
    ///
    /// The original string goes into `artist_display` (unless one is already
    /// there), which is what `get_pending_scrobbles` forwards — so a listen
    /// still waiting to go out reaches Last.fm as it always would have. The
    /// caller has already checked `names` (derived, or `credit_covers`).
    pub async fn split_artist_credit(
        &self,
        user_id: i64,
        artist: &str,
        credit: Option<&str>,
        names: &[String],
    ) -> Result<u64, sqlx::Error> {
        let Some(primary) = names.first() else { return Ok(0) };
        let mut tx = self.pool.begin().await?;
        let ids: Vec<(i64, Option<String>)> = sqlx::query_as(&crate::sql!(
            "SELECT id, mbid_artist FROM scrobbles
             WHERE user_id = ? AND artist = ? AND COALESCE(artist_credit, '') = ?"
        ))
        .bind(user_id)
        .bind(artist)
        .bind(credit.unwrap_or(""))
        .fetch_all(&mut *tx)
        .await?;

        for (id, mbid_artist) in &ids {
            sqlx::query(&crate::sql!(
                "UPDATE scrobbles
                    SET artist_display = COALESCE(artist_display, artist),
                        artist = ?, artist_credit = ?
                  WHERE id = ?"
            ))
            .bind(primary)
            .bind(names.join("; "))
            .bind(id)
            .execute(&mut *tx)
            .await?;

            // Rebuilt rather than patched: the old rows credit the joined
            // string itself, which is the phantom artist this removes.
            sqlx::query(&crate::sql!("DELETE FROM scrobble_artists WHERE scrobble_id = ?"))
                .bind(id)
                .execute(&mut *tx)
                .await?;
            let mut play =
                Play::new(String::new(), primary.clone(), 0, String::new(), String::new());
            play.artists = Some(names.to_vec());
            play.mbid_artist = mbid_artist.as_deref().and_then(|m| serde_json::from_str(m).ok());
            for (position, (name, mbid)) in credited_artists(&play).into_iter().enumerate() {
                sqlx::query(&crate::sql!(
                    "INSERT INTO scrobble_artists (scrobble_id, artist_key, name, position, mbid)
                     VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING"
                ))
                .bind(id)
                .bind(Database::artist_key(&name))
                .bind(&name)
                .bind(position as i64)
                .bind(mbid)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(ids.len() as u64)
    }
}

/// `artist_credit` as stored: everyone, joined with `"; "`.
fn split_stored_credit(c: &str) -> Vec<String> {
    c.split(';').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect()
}

/// A string a person might have to split by hand. Deliberately loose — it only
/// decides what is *shown*, never what is changed.
fn looks_joined(artist: &str) -> bool {
    let a = artist.to_lowercase();
    [", ", " & ", " x ", " and ", " + ", " vs. ", " vs "].iter().any(|j| a.contains(j))
}
