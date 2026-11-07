//! At a friend's: the built-in chain for listening on someone else's system.
//!
//! A play accepted out of a Shared Spool used to land on whatever chain the
//! person had named in "Rooms you sit in", or on none. Now it lands here by
//! default: one chain per person, `signal_chains.kind = 'friend'`, made the
//! first time it is needed and kept in the built-in "Elsewhere" room. It has
//! no steps, so it credits no gear (it was not your gear), and it starts
//! "Only me" like every chain (Ashwin, 2026-11-06). A chain named the old way
//! (`users.spool_chain_id`) is still honoured, and the friend chain's drawer
//! says so.
//!
//! Every query is scoped by `user_id`.

use super::rooms::ROOM_ELSEWHERE;
use super::*;
use crate::models::{ListeningContext, SignalChain};

/// The name the friend chain is made with. Renaming it is fine; it is found
/// by `kind`, never by name.
pub const FRIEND_CHAIN_NAME: &str = "At a friend's";

/// The source name accepted Shared Spool plays carry.
pub const SPOOL_SOURCE: &str = "Shared Spool";

/// Shared Spool plays that landed on no chain.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InboxSpool {
    /// Plays, skips excluded.
    pub plays: i64,
    pub first: Option<i64>,
    pub last: Option<i64>,
}

impl Database {
    /// This person's At a friend's chain, if it has been made.
    pub async fn friend_chain(&self, user_id: i64) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT id FROM signal_chains WHERE user_id = ? AND kind = 'friend' ORDER BY id LIMIT 1"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// This person's At a friend's chain, made now (in the Elsewhere room) if
    /// it does not exist yet.
    pub async fn ensure_friend_chain(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        if let Some(id) = self.friend_chain(user_id).await? {
            return Ok(id);
        }
        let id = self
            .create_chain(&SignalChain {
                id: None,
                user_id,
                name: FRIEND_CHAIN_NAME.into(),
                description: Some("Someone else's system".into()),
                components: Vec::new(),
                listening_context: ListeningContext::Active,
                icon: Some("speakers".into()),
                is_active: true,
                shared: false,
                total_hours: 0.0,
                created_at: 0,
            })
            .await?;
        sqlx::query(&crate::sql!(
            "UPDATE signal_chains SET kind = 'friend' WHERE id = ? AND user_id = ?"
        ))
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        let room = self.ensure_builtin_room(user_id, ROOM_ELSEWHERE).await?;
        let _ = self.set_chain_rooms(user_id, id, &[room]).await?;
        Ok(id)
    }

    /// Where an accepted Shared Spool play lands: the chain named the old way
    /// if there is one, else At a friend's.
    pub async fn spool_target(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        match self.spool_chain_id(user_id).await? {
            Some(id) => Ok(id),
            None => self.ensure_friend_chain(user_id).await,
        }
    }

    /// Accepted Shared Spool plays on no chain (accepted before At a
    /// friend's existed), for the inbox. None when there are none.
    pub async fn unfiled_spool(&self, user_id: i64) -> Result<Option<InboxSpool>, sqlx::Error> {
        let (plays, first, last): (i64, Option<i64>, Option<i64>) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*), MIN(timestamp), MAX(timestamp) FROM scrobbles
              WHERE user_id = ? AND chain_id IS NULL AND skipped = FALSE AND source_name = ?"
        ))
        .bind(user_id)
        .bind(SPOOL_SOURCE)
        .fetch_one(&self.pool)
        .await?;
        Ok((plays > 0).then_some(InboxSpool { plays, first, last }))
    }

    /// File every accepted Shared Spool play on no chain under `chain_id`, or
    /// At a friend's when none is given. Returns the plays filed (skips
    /// excluded) and the chain.
    pub async fn file_spool_plays(
        &self,
        user_id: i64,
        chain_id: Option<i64>,
    ) -> Result<Result<(i64, i64), String>, sqlx::Error> {
        let target = match chain_id {
            Some(id) => {
                if self.get_chain(user_id, id).await?.is_none() {
                    return Ok(Err("No such chain".into()));
                }
                id
            }
            None => self.ensure_friend_chain(user_id).await?,
        };
        let moved: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND chain_id IS NULL AND skipped = FALSE AND source_name = ?"
        ))
        .bind(user_id)
        .bind(SPOOL_SOURCE)
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET chain_id = ?, chain_source = 'spool'
              WHERE user_id = ? AND chain_id IS NULL AND source_name = ?"
        ))
        .bind(target)
        .bind(user_id)
        .bind(SPOOL_SOURCE)
        .execute(&self.pool)
        .await?;
        Ok(Ok((moved, target)))
    }
}
