//! The Radio page's storage: presets, tuning sessions and the air log.
//!
//! Every query is scoped by `user_id`. None of them reads `scrobbles` — a
//! radio listen is written through `save_scrobble` like every other, and the
//! air log only remembers the `source_id` it was written under.

use super::*;
use serde::Serialize;

/// A station the listener saved, where it sits on their dial.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RadioPreset {
    pub id: i64,
    /// `<gcc>.<pi>`, the directory's id.
    pub station_id: String,
    pub name: String,
    pub short_name: Option<String>,
    /// `fm` or `dab`.
    pub band: String,
    /// MHz for FM (`95.8`), the channel block for DAB (`5C`).
    pub dial: Option<String>,
    pub city: Option<String>,
    /// Order on the preset row, from 1.
    pub position: i64,
}

/// One stretch of listening to one station.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RadioSession {
    pub id: i64,
    pub station_id: String,
    pub station_name: String,
    pub band: String,
    pub dial: Option<String>,
    pub city: Option<String>,
    pub chain_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
}

/// One thing a station said while you were tuned in.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AirLogEntry {
    pub id: i64,
    pub session_id: i64,
    pub station_id: String,
    pub station_name: String,
    pub band: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// Exactly what the station sent.
    pub raw: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// `track`, `guess`, `break` or `other`.
    pub kind: String,
    /// `ICY` today.
    pub source: String,
    /// `playing`, `scrobbled`, `review`, `kept`, `dropped`, `held`,
    /// `partial`, `skipped` or `logged`.
    pub status: String,
    /// The listen this became, as `scrobbles.source_id`.
    pub source_id: Option<String>,
    pub mbid_recording: Option<String>,
}

/// What a new air-log line carries.
pub struct NewAirLine<'a> {
    pub session_id: i64,
    pub station_id: &'a str,
    pub station_name: &'a str,
    pub band: &'a str,
    pub started_at: i64,
    pub raw: &'a str,
    pub artist: Option<&'a str>,
    pub title: Option<&'a str>,
    pub kind: &'a str,
    pub source: &'a str,
    pub status: &'a str,
}

const PRESET_COLUMNS: &str = "id, station_id, name, short_name, band, dial, city, position";
const SESSION_COLUMNS: &str =
    "id, station_id, station_name, band, dial, city, chain_id, started_at, ended_at, end_reason";
const AIR_COLUMNS: &str =
    "id, session_id, station_id, station_name, band, started_at, ended_at, raw, \
     artist, title, kind, source, status, source_id, mbid_recording";

impl Database {
    pub async fn radio_presets(&self, user_id: i64) -> Result<Vec<RadioPreset>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT {PRESET_COLUMNS} FROM radio_presets WHERE user_id = ? ORDER BY position, id"
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn radio_preset(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<RadioPreset>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT {PRESET_COLUMNS} FROM radio_presets WHERE user_id = ? AND id = ?"
        )))
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Save a station to the end of the preset row. `Ok(None)` when it is
    /// already a preset on that band.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_radio_preset(
        &self,
        user_id: i64,
        station_id: &str,
        name: &str,
        short_name: Option<&str>,
        band: &str,
        dial: Option<&str>,
        city: Option<&str>,
    ) -> Result<Option<RadioPreset>, sqlx::Error> {
        let next: Option<(Option<i64>,)> = sqlx::query_as(&crate::sql!(
            "SELECT MAX(position) FROM radio_presets WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let position = next.and_then(|(m,)| m).unwrap_or(0) + 1;
        // `fetch_all` — see `create_mcp_connection`: the row must be visible to
        // the next read on another pool connection.
        let rows: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO radio_presets
                (user_id, station_id, name, short_name, band, dial, city, position, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING RETURNING id"
        ))
        .bind(user_id)
        .bind(station_id)
        .bind(name)
        .bind(short_name)
        .bind(band)
        .bind(dial)
        .bind(city)
        .bind(position)
        .bind(Self::now_secs())
        .fetch_all(&self.pool)
        .await?;
        match rows.first() {
            Some((id,)) => self.radio_preset(user_id, *id).await,
            None => Ok(None),
        }
    }

    /// Change where a preset sits. Each `Some` is a change (`Some(None)`
    /// clears); `None` leaves the field alone.
    pub async fn update_radio_preset(
        &self,
        user_id: i64,
        id: i64,
        dial: Option<Option<&str>>,
        city: Option<Option<&str>>,
        position: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let Some(current) = self.radio_preset(user_id, id).await? else { return Ok(false) };
        let dial = dial.unwrap_or(current.dial.as_deref());
        let city = city.unwrap_or(current.city.as_deref());
        let position = position.unwrap_or(current.position);
        sqlx::query(&crate::sql!(
            "UPDATE radio_presets SET dial = ?, city = ?, position = ? WHERE user_id = ? AND id = ?"
        ))
        .bind(dial)
        .bind(city)
        .bind(position)
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn delete_radio_preset(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res =
            sqlx::query(&crate::sql!("DELETE FROM radio_presets WHERE user_id = ? AND id = ?"))
                .bind(user_id)
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn start_radio_session(
        &self,
        user_id: i64,
        preset: &RadioPreset,
        chain_id: Option<i64>,
        started_at: i64,
    ) -> Result<i64, sqlx::Error> {
        let rows: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO radio_sessions
                (user_id, station_id, station_name, band, dial, city, chain_id, started_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(&preset.station_id)
        .bind(&preset.name)
        .bind(&preset.band)
        .bind(&preset.dial)
        .bind(&preset.city)
        .bind(chain_id)
        .bind(started_at)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn end_radio_session(
        &self,
        user_id: i64,
        id: i64,
        ended_at: i64,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE radio_sessions SET ended_at = ?, end_reason = ?
             WHERE user_id = ? AND id = ? AND ended_at IS NULL"
        ))
        .bind(ended_at)
        .bind(reason)
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The last stations tuned, newest first, one row per station and band.
    pub async fn recent_radio(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<RadioSession>, sqlx::Error> {
        // Read a bounded tail and fold in Rust: "latest row per group" is a
        // window function on one backend and a GROUP BY trick on the other,
        // and fifty rows is nothing.
        let rows: Vec<RadioSession> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT {SESSION_COLUMNS} FROM radio_sessions WHERE user_id = ?
             ORDER BY started_at DESC LIMIT 50"
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out: Vec<RadioSession> = Vec::new();
        for r in rows {
            if !out.iter().any(|o| o.station_id == r.station_id && o.band == r.band) {
                out.push(r);
            }
            if out.len() == limit {
                break;
            }
        }
        Ok(out)
    }

    pub async fn add_air_line(
        &self,
        user_id: i64,
        line: &NewAirLine<'_>,
    ) -> Result<i64, sqlx::Error> {
        let rows: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO radio_air_log
                (user_id, session_id, station_id, station_name, band, started_at, raw,
                 artist, title, kind, source, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(line.session_id)
        .bind(line.station_id)
        .bind(line.station_name)
        .bind(line.band)
        .bind(line.started_at)
        .bind(line.raw)
        .bind(line.artist)
        .bind(line.title)
        .bind(line.kind)
        .bind(line.source)
        .bind(line.status)
        .fetch_all(&self.pool)
        .await?;
        rows.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }

    /// Settle a line: what it turned out to be, and the listen it became.
    #[allow(clippy::too_many_arguments)]
    pub async fn settle_air_line(
        &self,
        user_id: i64,
        id: i64,
        kind: &str,
        status: &str,
        ended_at: Option<i64>,
        source_id: Option<&str>,
        mbid_recording: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE radio_air_log
             SET kind = ?, status = ?, ended_at = COALESCE(?, ended_at),
                 source_id = COALESCE(?, source_id), mbid_recording = COALESCE(?, mbid_recording)
             WHERE user_id = ? AND id = ?"
        ))
        .bind(kind)
        .bind(status)
        .bind(ended_at)
        .bind(source_id)
        .bind(mbid_recording)
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn air_log(&self, user_id: i64, limit: i64) -> Result<Vec<AirLogEntry>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT {AIR_COLUMNS} FROM radio_air_log WHERE user_id = ?
             ORDER BY started_at DESC, id DESC LIMIT ?"
        )))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Lines waiting for a yes, oldest first — however far back they are, so
    /// the queue never loses one off the bottom of the log.
    pub async fn air_review_queue(&self, user_id: i64) -> Result<Vec<AirLogEntry>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT {AIR_COLUMNS} FROM radio_air_log WHERE user_id = ? AND status = 'review'
             ORDER BY started_at, id LIMIT 200"
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn air_line(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<AirLogEntry>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT {AIR_COLUMNS} FROM radio_air_log WHERE user_id = ? AND id = ?"
        )))
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn radio_session(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<RadioSession>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT {SESSION_COLUMNS} FROM radio_sessions WHERE user_id = ? AND id = ?"
        )))
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Close whatever a restart left open. A tuner lives in memory, so a
    /// session still open at startup ended when the process did; it is closed
    /// at its last logged line, and a line still `playing` became `partial` —
    /// nobody can say how much of it was heard.
    pub async fn close_dangling_radio(&self) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE radio_sessions
             SET end_reason = 'restart',
                 ended_at = COALESCE(
                     (SELECT MAX(COALESCE(a.ended_at, a.started_at)) FROM radio_air_log a
                      WHERE a.session_id = radio_sessions.id),
                     started_at)
             WHERE ended_at IS NULL"
        ))
        .execute(&self.pool)
        .await?;
        sqlx::query(&crate::sql!(
            "UPDATE radio_air_log SET status = 'partial', ended_at = started_at WHERE status = 'playing'"
        ))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(all(test, not(feature = "postgres")))]
mod tests {
    use super::*;

    async fn testdb() -> (Database, i64) {
        let path = std::env::temp_dir().join(format!(
            "td-radio-{}-{}.db",
            std::process::id(),
            rand::random::<u32>()
        ));
        let db =
            Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
                .await
                .unwrap();
        let uid = db.create_user("ashwin", None, "x", "admin").await.unwrap();
        (db, uid)
    }

    #[tokio::test]
    async fn presets_append_refuse_duplicates_and_stay_with_their_owner() {
        let (db, me) = testdb().await;
        let other = db.create_user("guest", None, "x", "user").await.unwrap();
        let a = db
            .add_radio_preset(
                me,
                "de0.d3c3",
                "MDR KULTUR",
                Some("KULTUR"),
                "fm",
                Some("94.4"),
                Some("Leipzig"),
            )
            .await
            .unwrap()
            .unwrap();
        let b = db
            .add_radio_preset(me, "de0.d3c3", "MDR KULTUR", None, "dab", Some("5C"), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((a.position, b.position), (1, 2));
        assert!(
            db.add_radio_preset(me, "de0.d3c3", "MDR KULTUR", None, "fm", None, None)
                .await
                .unwrap()
                .is_none(),
            "the same station on the same band is one preset"
        );
        assert!(db.radio_preset(other, a.id).await.unwrap().is_none());
        assert!(!db.delete_radio_preset(other, a.id).await.unwrap());
        assert!(!db.update_radio_preset(other, a.id, Some(None), None, None).await.unwrap());

        // Absent leaves alone, Some(None) clears.
        assert!(db.update_radio_preset(me, a.id, None, Some(None), None).await.unwrap());
        let a2 = db.radio_preset(me, a.id).await.unwrap().unwrap();
        assert_eq!((a2.dial.as_deref(), a2.city.as_deref()), (Some("94.4"), None));
    }

    #[tokio::test]
    async fn a_restart_closes_open_sessions_at_their_last_line() {
        let (db, me) = testdb().await;
        let p = db
            .add_radio_preset(me, "de0.d3c3", "MDR KULTUR", None, "fm", None, None)
            .await
            .unwrap()
            .unwrap();
        let s = db.start_radio_session(me, &p, None, 1000).await.unwrap();
        let line = |started_at, status| NewAirLine {
            session_id: s,
            station_id: "de0.d3c3",
            station_name: "MDR KULTUR",
            band: "fm",
            started_at,
            raw: "A - B",
            artist: Some("A"),
            title: Some("B"),
            kind: "track",
            source: "ICY",
            status,
        };
        db.add_air_line(me, &line(1100, "scrobbled")).await.unwrap();
        let playing = db.add_air_line(me, &line(1300, "playing")).await.unwrap();

        db.close_dangling_radio().await.unwrap();
        let session = db.radio_session(me, s).await.unwrap().unwrap();
        assert_eq!(
            (session.ended_at, session.end_reason.as_deref()),
            (Some(1300), Some("restart"))
        );
        assert_eq!(db.air_line(me, playing).await.unwrap().unwrap().status, "partial");

        let recent = db.recent_radio(me, 3).await.unwrap();
        assert_eq!(recent.len(), 1);
    }

    #[tokio::test]
    async fn a_user_with_radio_history_can_still_be_deleted() {
        let (db, _) = testdb().await;
        let gone = db.create_user("leaving", None, "x", "user").await.unwrap();
        let p = db
            .add_radio_preset(gone, "de0.d3c3", "MDR KULTUR", None, "fm", None, None)
            .await
            .unwrap()
            .unwrap();
        let s = db.start_radio_session(gone, &p, None, 1000).await.unwrap();
        db.add_air_line(
            gone,
            &NewAirLine {
                session_id: s,
                station_id: "de0.d3c3",
                station_name: "MDR KULTUR",
                band: "fm",
                started_at: 1001,
                raw: "x",
                artist: None,
                title: None,
                kind: "other",
                source: "ICY",
                status: "logged",
            },
        )
        .await
        .unwrap();
        assert!(db.delete_user(gone).await.unwrap());
    }
}
