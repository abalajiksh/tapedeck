//! Rooms: where a person's chains live.
//!
//! A room holds chains, not gear. A chain may be in several rooms (a Bluetooth
//! speaker carried between two), so membership is the `chain_rooms` link table.
//! Rooms never touch attribution: they group chains on the page and nothing
//! else reads them. Every query is scoped by `user_id`.

use super::*;

/// The built-in room for chains that go with you.
pub const ROOM_PORTABLE: &str = "portable";
/// The built-in room for someone else's system ("At a friend's", phase 5).
pub const ROOM_ELSEWHERE: &str = "elsewhere";

/// A room's name on screen when it is a built-in. The person never types it.
fn builtin_name(kind: &str) -> &'static str {
    match kind {
        ROOM_ELSEWHERE => "Elsewhere",
        _ => "On the go",
    }
}

/// Longest room name accepted. A room is a label on a chip, not a description.
pub const ROOM_NAME_MAX: usize = 60;

/// One room and the chains in it.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct Room {
    pub id: i64,
    pub name: String,
    /// A glyph name from the UI's set, or null.
    pub icon: Option<String>,
    /// `room` for your own, `portable` (On the go) or `elsewhere` for the
    /// built-ins, which cannot be renamed or deleted.
    pub kind: String,
    pub sort: i64,
    pub created_at: i64,
    /// The chains in this room, by id. A chain may be in several rooms.
    pub chain_ids: Vec<i64>,
}

#[derive(Debug, FromRow)]
struct RoomRow {
    id: i64,
    name: String,
    icon: Option<String>,
    kind: String,
    sort: i64,
    created_at: i64,
}

/// Why a room change was refused. Each maps to one status code at the handler.
#[derive(Debug, PartialEq)]
pub enum RoomRefusal {
    /// No such room of yours (or chain, for membership).
    NotFound,
    /// A built-in room cannot be renamed or deleted.
    BuiltIn,
    /// You already have a room with that name.
    Duplicate,
    /// Empty, or longer than [`ROOM_NAME_MAX`].
    BadName,
}

fn clean_name(raw: &str) -> Option<String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty() && name.chars().count() <= ROOM_NAME_MAX).then_some(name)
}

impl Database {
    /// Every room of this user's, in their order, with the chains in each.
    ///
    /// Creates the built-in "On the go" room the first time it is asked for, so
    /// it is there to pick without anybody having made it. "Elsewhere" waits
    /// for the friend chain that will live in it.
    pub async fn list_rooms(&self, user_id: i64) -> Result<Vec<Room>, sqlx::Error> {
        self.ensure_builtin_room(user_id, ROOM_PORTABLE).await?;
        let rows: Vec<RoomRow> = sqlx::query_as(&crate::sql!(
            "SELECT id, name, icon, kind, sort, created_at FROM rooms
              WHERE user_id = ? ORDER BY sort, id"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let links: Vec<(i64, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT room_id, chain_id FROM chain_rooms WHERE user_id = ? ORDER BY chain_id"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Room {
                chain_ids: links.iter().filter(|(rid, _)| *rid == r.id).map(|(_, c)| *c).collect(),
                id: r.id,
                name: r.name,
                icon: r.icon,
                kind: r.kind,
                sort: r.sort,
                created_at: r.created_at,
            })
            .collect())
    }

    /// Make sure a built-in room exists, and return its id.
    ///
    /// A room of the person's own that already carries the built-in's name is
    /// adopted as the built-in rather than refused: it is the room they meant.
    pub async fn ensure_builtin_room(&self, user_id: i64, kind: &str) -> Result<i64, sqlx::Error> {
        if let Some(id) = sqlx::query_scalar::<_, i64>(&crate::sql!(
            "SELECT id FROM rooms WHERE user_id = ? AND kind = ?"
        ))
        .bind(user_id)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(id);
        }
        let name = builtin_name(kind);
        if let Some(id) = sqlx::query_scalar::<_, i64>(&crate::sql!(
            "SELECT id FROM rooms WHERE user_id = ? AND name = ?"
        ))
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        {
            sqlx::query(&crate::sql!("UPDATE rooms SET kind = ? WHERE id = ? AND user_id = ?"))
                .bind(kind)
                .bind(id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
            return Ok(id);
        }
        // Built-ins sort after the person's own rooms.
        let ids: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO rooms (uuid, user_id, name, icon, kind, sort, created_at)
             VALUES (?, ?, ?, NULL, ?, 1000, ?) RETURNING id"
        ))
        .bind(new_uuid())
        .bind(user_id)
        .bind(name)
        .bind(kind)
        .bind(Self::now_secs())
        .fetch_all(&self.pool)
        .await?;
        Ok(ids[0].0)
    }

    /// A new room of the person's own, placed after their others.
    pub async fn create_room(
        &self,
        user_id: i64,
        name: &str,
        icon: Option<&str>,
    ) -> Result<Result<i64, RoomRefusal>, sqlx::Error> {
        let Some(name) = clean_name(name) else {
            return Ok(Err(RoomRefusal::BadName));
        };
        if self.room_named(user_id, &name, None).await? {
            return Ok(Err(RoomRefusal::Duplicate));
        }
        // After the person's own rooms and before the built-ins, which are
        // moved down one when the new room would otherwise land among them
        // (a reorder can leave a built-in anywhere in the list).
        let next: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COALESCE(MAX(sort), -1) + 1 FROM rooms WHERE user_id = ? AND kind = 'room'"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!(
            "UPDATE rooms SET sort = sort + 1 WHERE user_id = ? AND kind <> 'room' AND sort >= ?"
        ))
        .bind(user_id)
        .bind(next)
        .execute(&mut *tx)
        .await?;
        let ids: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO rooms (uuid, user_id, name, icon, kind, sort, created_at)
             VALUES (?, ?, ?, ?, 'room', ?, ?) RETURNING id"
        ))
        .bind(new_uuid())
        .bind(user_id)
        .bind(&name)
        .bind(icon)
        .bind(next)
        .bind(Self::now_secs())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(ids[0].0))
    }

    /// Whether this user already has a room called `name` (case-insensitively),
    /// other than `except`.
    async fn room_named(
        &self,
        user_id: i64,
        name: &str,
        except: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let n: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM rooms
              WHERE user_id = ? AND LOWER(name) = LOWER(?) AND (? IS NULL OR id <> ?)"
        ))
        .bind(user_id)
        .bind(name)
        .bind(except)
        .bind(except)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    async fn room_kind(&self, user_id: i64, room_id: i64) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!("SELECT kind FROM rooms WHERE id = ? AND user_id = ?"))
            .bind(room_id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Rename a room, change its icon, or move it in the order. `None` leaves a
    /// field alone; `icon: Some(None)` clears the icon. A built-in may be
    /// reordered and given an icon, never renamed.
    pub async fn update_room(
        &self,
        user_id: i64,
        room_id: i64,
        name: Option<&str>,
        icon: Option<Option<&str>>,
        sort: Option<i64>,
    ) -> Result<Result<(), RoomRefusal>, sqlx::Error> {
        let Some(kind) = self.room_kind(user_id, room_id).await? else {
            return Ok(Err(RoomRefusal::NotFound));
        };
        let mut tx = self.pool.begin().await?;
        if let Some(raw) = name {
            if kind != "room" {
                return Ok(Err(RoomRefusal::BuiltIn));
            }
            let Some(clean) = clean_name(raw) else {
                return Ok(Err(RoomRefusal::BadName));
            };
            if self.room_named(user_id, &clean, Some(room_id)).await? {
                return Ok(Err(RoomRefusal::Duplicate));
            }
            sqlx::query(&crate::sql!("UPDATE rooms SET name = ? WHERE id = ? AND user_id = ?"))
                .bind(&clean)
                .bind(room_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(icon) = icon {
            sqlx::query(&crate::sql!("UPDATE rooms SET icon = ? WHERE id = ? AND user_id = ?"))
                .bind(icon)
                .bind(room_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(sort) = sort {
            sqlx::query(&crate::sql!("UPDATE rooms SET sort = ? WHERE id = ? AND user_id = ?"))
                .bind(sort)
                .bind(room_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Delete a room of the person's own. Its chains lose the room and nothing
    /// else: deleting a room never deletes a chain.
    pub async fn delete_room(
        &self,
        user_id: i64,
        room_id: i64,
    ) -> Result<Result<(), RoomRefusal>, sqlx::Error> {
        match self.room_kind(user_id, room_id).await?.as_deref() {
            None => return Ok(Err(RoomRefusal::NotFound)),
            Some("room") => {}
            Some(_) => return Ok(Err(RoomRefusal::BuiltIn)),
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!("DELETE FROM chain_rooms WHERE room_id = ? AND user_id = ?"))
            .bind(room_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&crate::sql!("DELETE FROM rooms WHERE id = ? AND user_id = ?"))
            .bind(room_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Put a chain in exactly these rooms; an empty list takes it out of every
    /// room. Refused whole when the chain or any room is not this user's, so a
    /// half-applied change never happens.
    pub async fn set_chain_rooms(
        &self,
        user_id: i64,
        chain_id: i64,
        room_ids: &[i64],
    ) -> Result<Result<(), RoomRefusal>, sqlx::Error> {
        let owned: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM signal_chains WHERE id = ? AND user_id = ?"
        ))
        .bind(chain_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        if owned == 0 {
            return Ok(Err(RoomRefusal::NotFound));
        }
        let mut wanted: Vec<i64> = room_ids.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        for rid in &wanted {
            if self.room_kind(user_id, *rid).await?.is_none() {
                return Ok(Err(RoomRefusal::NotFound));
            }
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!("DELETE FROM chain_rooms WHERE chain_id = ? AND user_id = ?"))
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for rid in &wanted {
            sqlx::query(&crate::sql!(
                "INSERT INTO chain_rooms (user_id, chain_id, room_id) VALUES (?, ?, ?)"
            ))
            .bind(user_id)
            .bind(chain_id)
            .bind(rid)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Ok(()))
    }
}
