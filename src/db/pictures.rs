//! The picture each person chose for an artist — see `init_artist_pictures`.
//!
//! The shared `artists` row keeps what the backfill found; a row here is one
//! person's choice and beats it for them alone. Everything that shows an artist
//! reads `COALESCE(pick.url, artists.image_url)`, which is why `url` is stored
//! ready to load rather than worked out per reader.

use super::*;

/// One person's choice of picture for one artist.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArtistPicture {
    pub source: String,
    pub source_ref: Option<i64>,
    pub image_key: String,
    pub provider: Option<String>,
    pub url: String,
}

/// A choice about to be stored. `url` is what the browser will load.
pub struct NewArtistPicture<'a> {
    pub source: &'a str,
    pub source_ref: Option<i64>,
    pub image_key: &'a str,
    pub provider: Option<&'a str>,
    pub url: &'a str,
}

impl Database {
    /// This person's picture of this kind for this artist, if they chose one.
    pub async fn artist_picture(
        &self,
        user_id: i64,
        artist_id: i64,
        kind: &str,
    ) -> Result<Option<ArtistPicture>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT source, source_ref, image_key, provider, url
               FROM artist_pictures WHERE user_id = ? AND artist_id = ? AND kind = ?"
        ))
        .bind(user_id)
        .bind(artist_id)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await
    }

    /// Store a choice, replacing whatever this person had chosen before.
    /// Returns what it replaced, so an uploaded file it supersedes can go.
    pub async fn set_artist_picture(
        &self,
        user_id: i64,
        artist_id: i64,
        kind: &str,
        pick: &NewArtistPicture<'_>,
    ) -> Result<Option<ArtistPicture>, sqlx::Error> {
        let previous = self.artist_picture(user_id, artist_id, kind).await?;
        sqlx::query(&crate::sql!(
            "INSERT INTO artist_pictures
                (user_id, artist_id, kind, source, source_ref, image_key, provider, url, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, artist_id, kind) DO UPDATE SET
                source = excluded.source,
                source_ref = excluded.source_ref,
                image_key = excluded.image_key,
                provider = excluded.provider,
                url = excluded.url,
                updated_at = excluded.updated_at"
        ))
        .bind(user_id)
        .bind(artist_id)
        .bind(kind)
        .bind(pick.source)
        .bind(pick.source_ref)
        .bind(pick.image_key)
        .bind(pick.provider)
        .bind(pick.url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(previous)
    }

    /// Go back to the shared picture. Returns what was removed.
    pub async fn clear_artist_picture(
        &self,
        user_id: i64,
        artist_id: i64,
        kind: &str,
    ) -> Result<Option<ArtistPicture>, sqlx::Error> {
        let previous = self.artist_picture(user_id, artist_id, kind).await?;
        sqlx::query(&crate::sql!(
            "DELETE FROM artist_pictures WHERE user_id = ? AND artist_id = ? AND kind = ?"
        ))
        .bind(user_id)
        .bind(artist_id)
        .bind(kind)
        .execute(&self.pool)
        .await?;
        Ok(previous)
    }
}

/// Where an uploaded picture is served from — `server::artwork::UPLOAD_PREFIX`,
/// repeated because the database layer does not reach into the server.
const UPLOAD_PREFIX: &str = "/api/v1/art/upload/";

/// Was this value on the shared row put there by a person, and if so how?
///
/// Before v0.124.0 the hand override and the backfill wrote the same column,
/// so this is a reading of the value rather than a record of who wrote it. An
/// upload is certain. The library proxy path is what the portrait backfill
/// writes, and a Deezer CDN link is what the artwork chain writes, so both are
/// automatic. Anything else was pasted. A wide picture has no backfill at all,
/// so for `wide` every value was somebody's choice.
pub(crate) fn hand_set_source(kind: &str, url: &str) -> Option<&'static str> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with(UPLOAD_PREFIX) {
        return Some("upload");
    }
    if kind == "portrait" {
        if url.starts_with("/api/v1/artists/img/") {
            return None;
        }
        let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string));
        if host.is_some_and(|h| h == "dzcdn.net" || h.ends_with(".dzcdn.net")) {
            return None;
        }
    }
    Some("link")
}

impl Database {
    /// Move the pictures people set by hand on the shared row onto the first
    /// admin, and hand the shared row back to the backfill. Returns how many
    /// moved, or `None` when there is no admin to give them to yet.
    ///
    /// The admin, because before v0.124.0 there was no record of who set any of
    /// them and the operator is the one who answers for the instance.
    pub(crate) async fn move_hand_set_artist_pictures(&self) -> Result<Option<u64>, sqlx::Error> {
        let admin: Option<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT id FROM users WHERE role = 'admin' ORDER BY id LIMIT 1"
        ))
        .fetch_optional(&self.pool)
        .await?;
        let Some((admin,)) = admin else { return Ok(None) };

        let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(&crate::sql!(
            "SELECT id, image_url, landscape_url FROM artists
              WHERE (image_url IS NOT NULL AND TRIM(image_url) != '')
                 OR (landscape_url IS NOT NULL AND TRIM(landscape_url) != '')"
        ))
        .fetch_all(&self.pool)
        .await?;

        let mut moved = 0u64;
        let mut tx = self.pool.begin().await?;
        for (artist_id, portrait, wide) in rows {
            for (kind, value, column) in
                [("portrait", portrait, "image_url"), ("wide", wide, "landscape_url")]
            {
                let Some(url) = value else { continue };
                let Some(source) = hand_set_source(kind, &url) else { continue };
                let url = url.trim();
                // An admin who already chose one keeps theirs.
                sqlx::query(&crate::sql!(
                    "INSERT INTO artist_pictures
                        (user_id, artist_id, kind, source, source_ref, image_key, provider,
                         url, updated_at)
                     VALUES (?, ?, ?, ?, NULL, ?, NULL, ?, ?)
                     ON CONFLICT(user_id, artist_id, kind) DO NOTHING"
                ))
                .bind(admin)
                .bind(artist_id)
                .bind(kind)
                .bind(source)
                .bind(url)
                .bind(url)
                .bind(Self::now_secs())
                .execute(&mut *tx)
                .await?;
                let sql = format!("UPDATE artists SET {column} = NULL WHERE id = ?");
                let stmt = crate::sql!(&sql);
                sqlx::query(&stmt).bind(artist_id).execute(&mut *tx).await?;
                moved += 1;
            }
        }
        tx.commit().await?;
        Ok(Some(moved))
    }
}
