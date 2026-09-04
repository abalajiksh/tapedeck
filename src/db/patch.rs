//! Phase 6 — identity, patches, the Reel, Dubs and Shared Spools.
//!
//! `PATCH_VISIBLE_LISTENS` (in `mod.rs`) is the gate every read here shares:
//! an accepted patch, not a skip, and either the owner shares everything with
//! the decks they have let in or the listen ran through a chain they share.
//! **Narrowings live in the SQL, never in a handler** — a filter applied in a
//! handler is one a second handler can forget.
//!
//! Three screens deliberately stay chain-gated rather than patch-gated — the
//! Instance figures, Discover, and "who else here plays them" — because there
//! is no patch behind those, so the per-chain switch is the only consent.

use super::*;

impl Database {
    /// The outward-facing half of a local user's identity, minted on first need.
    ///
    /// **Lazy rather than in a migration**, so an instance that never federates
    /// never pays for a keypair it will not use — and RSA-2048 generation is
    /// slow enough that doing it for every account at startup would be felt.
    ///
    /// `uri` is written **once, here**, and read from the column ever after.
    /// Deriving it on read from today's `PUBLIC_URL` would silently rename an
    /// actor the moment the operator changed a config value — and a rename is
    /// not a rename to anyone else, it is the old actor vanishing and a
    /// stranger appearing.
    ///
    /// The race is handled rather than assumed away: two concurrent callers
    /// both generate a keypair, one wins the `INSERT`, and **both re-read**.
    /// Returning the loser's key would be worse than an error — it would sign
    /// with a key that was never published, so every signature would verify
    /// against nothing while looking perfectly well-formed.
    pub async fn actor_for_user(&self, user_id: i64, origin: &str) -> Result<Actor, sqlx::Error> {
        if let Some(actor) = self.load_actor(user_id).await? {
            return Ok(actor);
        }

        let username: String =
            sqlx::query_scalar(&crate::sql!("SELECT username FROM users WHERE id = ?"))
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?;

        // CPU-bound for a noticeable fraction of a second. Holding an async
        // worker for that is what makes one request look like a hung server.
        // The error is flattened to a String *inside* the closure: a boxed
        // `dyn Error` is not `Send`, so it cannot cross the task boundary.
        let keys = tokio::task::spawn_blocking(|| {
            crate::patch::keys::generate().map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| sqlx::Error::Protocol(format!("keygen task failed: {e}")))?
        .map_err(|e| sqlx::Error::Protocol(format!("could not generate a keypair: {e}")))?;

        let uri = crate::patch::actor::uri_for(origin, &username);

        sqlx::query(&crate::sql!(
            "INSERT INTO actors (user_id, uri, public_key, private_key, created_at)
             VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        ))
        .bind(user_id)
        .bind(&uri)
        .bind(&keys.public_pem)
        // Sealed like every other replayed secret, so ciphertext never leaves
        // this file.
        .bind(self.seal(&keys.private_pem))
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;

        // **A deck that already has followers is frozen the moment it gains an
        // identity.** Without this the rule would depend on an unrelated
        // switch: `freeze_actor_username` writes to `actors`, so accepting a
        // patch before ever enabling federation recorded nothing, and the
        // username would then look renameable to whoever added a rename path.
        // Consistency matters more than the saved query — it is one read, once
        // per account ever.
        //
        // Note the *sharpest* trigger is narrower than this and does not exist
        // yet: what truly makes a rename unsurvivable is a **remote** follower
        // holding the actor URI, or an activity signed and delivered outbound.
        // Both are Step 3. Freezing on a local accept is the conservative side
        // of that line, and refusing a rename that would have been safe is the
        // cheaper mistake.
        if self.has_accepted_patch(user_id).await? {
            self.freeze_actor_username(user_id).await?;
        }

        self.load_actor(user_id).await?.ok_or(sqlx::Error::RowNotFound)
    }
    /// Has anybody been let in to this deck?
    async fn has_accepted_patch(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let n: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM patches WHERE target_user_id = ? AND state = 'accepted'",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }
    /// Resolve a username to a user **who is willing to be found**.
    ///
    /// A user who has not switched federation on, a disabled account, and a
    /// username that was never real all come back as `None` — one answer,
    /// deliberately. These are the only unauthenticated read paths on the
    /// instance besides the now-playing widget, and they take the same rule:
    /// anything that tells "no such person" apart from "that person is not
    /// published" turns a private, invite-only server into a membership oracle
    /// that anyone can type names at.
    ///
    /// `an_undiscoverable_actor_and_an_unknown_one_are_the_same_answer` is
    /// phrased about the *answer* rather than about this query, because a later
    /// rewrite that splits the cases is a perfectly reasonable-looking change
    /// and a leak.
    ///
    /// Returning the id rather than the actor is what makes the keypair lazy:
    /// the caller mints one through `actor_for_user` if this account has never
    /// needed one. That means a stranger's fetch can trigger a keygen — bounded
    /// at one per federating account, ever, and only for names that are real
    /// and published.
    pub async fn discoverable_user_id(&self, username: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT id FROM users
              WHERE username = ? COLLATE NOCASE AND federation_enabled = 1 AND disabled = 0"
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await
    }
    /// The stored actor for a user, if one has ever been minted — **without
    /// minting one**.
    ///
    /// The distinction from `actor_for_user` is the whole reason this is
    /// public: opening a settings page is not a decision to federate, and a
    /// read path that quietly generates an identity is a read path that has
    /// side effects nobody asked for.
    pub async fn stored_actor(&self, user_id: i64) -> Result<Option<Actor>, sqlx::Error> {
        self.load_actor(user_id).await
    }
    /// The stored actor for a user, unsealed. `None` when no keypair has been
    /// minted yet.
    async fn load_actor(&self, user_id: i64) -> Result<Option<Actor>, sqlx::Error> {
        let row: Option<ActorRow> = sqlx::query_as(&crate::sql!(
            "SELECT a.user_id, u.username, a.uri, a.public_key, a.private_key, a.frozen_at,
                    u.display_name, u.bio, u.avatar_url, u.federation_enabled
               FROM actors a JOIN users u ON u.id = a.user_id
              WHERE a.user_id = ?",
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Actor {
            private_key: self.unseal(&r.private_key).unwrap_or_default(),
            user_id: r.user_id,
            username: r.username,
            uri: r.uri,
            public_key: r.public_key,
            frozen_at: r.frozen_at,
            display_name: r.display_name,
            bio: r.bio,
            avatar_url: r.avatar_url,
            federation_enabled: r.federation_enabled != 0,
        }))
    }
    /// Whether this user's deck answers to strangers at all.
    pub async fn set_federation_enabled(&self, user_id: i64, on: bool) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("UPDATE users SET federation_enabled = ? WHERE id = ?"))
            .bind(if on { 1 } else { 0 })
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Mark the moment this identity left the building.
    ///
    /// Set on the **first accepted patch or the first signed activity,
    /// whichever comes first** — not on going public, because in this design
    /// following happens *while* private: a private deck holds patches in the
    /// inbox for approval, so somebody can have ten followers and never be
    /// public. Idempotent: the first stamp is the true one, and re-stamping
    /// would move a date that other servers' copies are dated against.
    pub async fn freeze_actor_username(&self, user_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE actors SET frozen_at = ? WHERE user_id = ? AND frozen_at IS NULL",
        ))
        .bind(Self::now_secs())
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Is this user's username now load-bearing for somebody else?
    ///
    /// **Nothing calls this yet, and that is a gap rather than a design.**
    /// There is no rename path anywhere in this application — only first-run
    /// setup ever writes `users.username` — so the freeze has nothing to guard.
    /// It is here so that whoever adds an admin rename inherits the refusal
    /// instead of having to know it was needed: a rename after this point is
    /// not a rename to any server holding the actor, it is a deletion and a
    /// stranger. ActivityPub's answer is `Move` + `alsoKnownAs`, which is a
    /// deliberate migration and is not built.
    pub async fn username_is_frozen(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let frozen: Option<Option<i64>> =
            sqlx::query_scalar(&crate::sql!("SELECT frozen_at FROM actors WHERE user_id = ?"))
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(matches!(frozen, Some(Some(_))))
    }
    /// Store or refresh what we know about somebody else's actor.
    ///
    /// Cached rather than fetched per delivery, because an actor document is
    /// the key material a signature is checked against — re-fetching it on
    /// every inbox POST would make every remote server's availability our own,
    /// and would hand anyone who can reach our inbox a way to make us issue
    /// outbound requests.
    pub async fn upsert_remote_actor(
        &self,
        uri: &str,
        handle: Option<&str>,
        name: Option<&str>,
        inbox: &str,
        shared_inbox: Option<&str>,
        public_key: Option<&str>,
        icon_url: Option<&str>,
    ) -> Result<RemoteActor, sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO remote_actors (uri, handle, name, inbox, shared_inbox, public_key,
                                        icon_url, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(uri) DO UPDATE SET
                handle = excluded.handle,
                name = excluded.name,
                inbox = excluded.inbox,
                shared_inbox = excluded.shared_inbox,
                public_key = excluded.public_key,
                icon_url = excluded.icon_url,
                fetched_at = excluded.fetched_at",
        ))
        .bind(uri)
        .bind(handle)
        .bind(name)
        .bind(inbox)
        .bind(shared_inbox)
        .bind(public_key)
        .bind(icon_url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;

        self.remote_actor_by_uri(uri).await?.ok_or(sqlx::Error::RowNotFound)
    }
    pub async fn remote_actor_by_uri(&self, uri: &str) -> Result<Option<RemoteActor>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, uri, handle, name, inbox, shared_inbox, public_key, icon_url, fetched_at
               FROM remote_actors WHERE uri = ?",
        ))
        .bind(uri)
        .fetch_optional(&self.pool)
        .await
    }
    /// File a verified inbound activity.
    ///
    /// **A different table from `activities`, and that is a guarantee rather
    /// than tidiness.** `activities.user_id` is the *author*, and the outbox
    /// serves that table — so an inbound activity stored there would be one
    /// `WHERE` clause away from this server publishing a stranger's activity as
    /// its user's own. Keeping them apart makes that structurally impossible,
    /// the same way the Crate's separate table keeps unowned music out of every
    /// count.
    ///
    /// Returns whether this was new. A redelivery is the *same* activity —
    /// senders retry, and acting on a Follow twice is how one follower becomes
    /// two — so `activity_uri` is unique and a repeat is a no-op, not an error.
    pub async fn record_inbox_activity(
        &self,
        user_id: i64,
        actor_id: i64,
        activity_uri: &str,
        kind: Option<&str>,
        object_ref: Option<&str>,
        payload: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(&crate::sql!(
            "INSERT INTO inbox_activities
                (user_id, actor_id, activity_uri, kind, object_ref, payload, received_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING"
        ))
        .bind(user_id)
        .bind(actor_id)
        .bind(activity_uri)
        .bind(kind)
        .bind(object_ref)
        .bind(payload)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
    /// What this user's outbox publishes.
    ///
    /// **`visibility = 'public'` is the whole of the access control here**, and
    /// it is in the query rather than in the handler because an outbox is
    /// fetched by strangers with no credential: a private or patched-in
    /// activity reaching this collection is published to the internet, and
    /// nothing downstream would notice. Empty until delivery exists — the shape
    /// is served from day one because remote servers probe it to decide whether
    /// an actor is real.
    pub async fn public_outbox(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<(i64, Vec<String>), sqlx::Error> {
        let total: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM activities WHERE user_id = ? AND visibility = 'public'",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        let items: Vec<String> = sqlx::query_scalar(&crate::sql!(
            "SELECT payload FROM activities
              WHERE user_id = ? AND visibility = 'public'
              ORDER BY published DESC LIMIT ?"
        ))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok((total, items))
    }
    /// Resolve a username to a deck that can be patched into.
    ///
    /// Excludes disabled accounts, which `get_user_id_by_username` does not —
    /// that one is used by paths where a disabled user is still a real user
    /// whose rows exist. Here a disabled account is not a deck, and offering a
    /// patch into one would produce an edge that can never be approved.
    ///
    /// Deliberately **not** gated on `federation_enabled`: that switch decides
    /// whether the wider fediverse can find you, and members of a private,
    /// invite-only instance already know each other exist. What protects a deck
    /// locally is that a patch needs approval and a chain publishes nothing
    /// until its owner shares it.
    pub async fn patchable_user_id(&self, username: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT id FROM users WHERE username = ? COLLATE NOCASE AND disabled = 0",
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await
    }
    /// Ask to patch into someone's deck.
    ///
    /// **A patch is a request, not a subscription** — that is the whole reason
    /// `state` exists and is not a boolean. It lands `pending` and shows up in
    /// the target's inbox; nothing is shared until they say yes.
    ///
    /// Idempotent in the direction that matters: asking twice does not mint a
    /// second edge or reset an accepted one. Asking again after a **rejection**
    /// does put it back to pending — a rejection is not a block, and this
    /// design has no block. That is a real consequence rather than an
    /// oversight: it means somebody can ask repeatedly, and on a private
    /// invite-only instance of people who know each other that is the right
    /// trade. Blocking is the missing feature, and this is where it attaches.
    pub async fn request_patch(
        &self,
        follower_user_id: i64,
        target_user_id: i64,
    ) -> Result<String, sqlx::Error> {
        if follower_user_id == target_user_id {
            return Err(sqlx::Error::Protocol("a deck cannot patch into itself".into()));
        }

        let now = Self::now_secs();
        let uuid = new_uuid();

        // `DO UPDATE` rather than `DO NOTHING`, so a re-request after a
        // rejection reopens it — but an **accepted** patch is left alone, or
        // asking again would quietly un-approve a patch its target had already
        // said yes to.
        sqlx::query(&crate::sql!("INSERT INTO patches (uuid, follower_user_id, target_user_id, state, created_at, updated_at)
             VALUES (?, ?, ?, 'pending', ?, ?)
             -- The index this conflicts against is **partial**, and SQLite
             -- rejects an ON CONFLICT target that does not repeat the
             -- predicate — the trap `physical_images` already documents. It
             -- fails at runtime, not at build time, because every query here
             -- is a runtime string.
             ON CONFLICT(follower_user_id, target_user_id)
               WHERE follower_user_id IS NOT NULL AND target_user_id IS NOT NULL
               DO UPDATE SET state = 'pending', updated_at = excluded.updated_at
               WHERE patches.state <> 'accepted'",))
        .bind(&uuid)
        .bind(follower_user_id)
        .bind(target_user_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        sqlx::query_scalar(&crate::sql!(
            "SELECT uuid FROM patches WHERE follower_user_id = ? AND target_user_id = ?",
        ))
        .bind(follower_user_id)
        .bind(target_user_id)
        .fetch_one(&self.pool)
        .await
    }
    /// Say yes or no to a patch request.
    ///
    /// **Scoped to the target on the way in.** `target_user_id` is in the
    /// `WHERE`, not checked by the caller, because this is the one operation in
    /// Phase 6 that widens who may read somebody's listening — and an
    /// authorisation check that lives in a handler is one a second handler can
    /// forget. A patch naming somebody else changes nothing and reports so.
    ///
    /// Accepting **freezes the target's username**: from here their identity is
    /// load-bearing for another party, and a rename would be a deletion and a
    /// stranger rather than a rename. That trigger is why `frozen_at` exists,
    /// and it is here rather than in the handler for the same reason the scope
    /// is.
    pub async fn settle_patch(
        &self,
        uuid: &str,
        target_user_id: i64,
        accept: bool,
    ) -> Result<bool, sqlx::Error> {
        let state = if accept { "accepted" } else { "rejected" };

        let result = sqlx::query(&crate::sql!(
            "UPDATE patches SET state = ?, updated_at = ?
              WHERE uuid = ? AND target_user_id = ? AND state = 'pending'"
        ))
        .bind(state)
        .bind(Self::now_secs())
        .bind(uuid)
        .bind(target_user_id)
        .execute(&self.pool)
        .await?;

        let settled = result.rows_affected() > 0;
        if settled && accept {
            self.freeze_actor_username(target_user_id).await?;
        }
        Ok(settled)
    }
    /// Withdraw a patch you made.
    ///
    /// The **follower's** operation, and it deletes rather than setting a
    /// state: un-patching is not a decision anybody needs to remember, unlike a
    /// rejection, which is the target saying no and worth keeping.
    ///
    /// Deliberately no sweep of anything already delivered. Phase 6's rule:
    /// those listens were shared at the time under a switch their owner set,
    /// and a retroactive sweep would promise an unpatching the protocol cannot
    /// actually deliver.
    pub async fn unpatch(
        &self,
        follower_user_id: i64,
        target_user_id: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(&crate::sql!(
            "DELETE FROM patches WHERE follower_user_id = ? AND target_user_id = ?",
        ))
        .bind(follower_user_id)
        .bind(target_user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
    /// Everyone this user has patched into, or asked to.
    pub async fn patches_out(&self, user_id: i64) -> Result<Vec<PatchPeer>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT p.uuid, p.state, p.created_at, {}
               FROM patches p JOIN users u ON u.id = p.target_user_id
              WHERE p.follower_user_id = ?
              ORDER BY u.username COLLATE NOCASE",
            Self::PATCH_PEER_COLUMNS
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Everyone who has patched into this user, or asked to.
    ///
    /// `pending` rows are included: this is what the inbox reads, and the
    /// distinction is `state`.
    pub async fn patches_in(&self, user_id: i64) -> Result<Vec<PatchPeer>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT p.uuid, p.state, p.created_at, {}
               FROM patches p JOIN users u ON u.id = p.follower_user_id
              WHERE p.target_user_id = ?
              ORDER BY p.state = 'pending' DESC, p.created_at DESC",
            Self::PATCH_PEER_COLUMNS
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Turn a chain's sharing on or off.
    ///
    /// **This is no longer the gate — it is the floor.** A listen reaches a
    /// patched deck when its owner has opened up *or* it ran through a chain
    /// switched to shared here; see `PATCH_VISIBLE_LISTENS`. What this switch
    /// still decides on its own is everything a patch is not behind: the
    /// Instance tab, Discover, "who else here plays them", and whether a Shared
    /// Spool may capture a play into somebody else's history.
    ///
    /// It also decides whether the chain is **named**. `CHAIN_VISIBILITY` draws
    /// three states, not two: a shared chain by name, a private one as "Private
    /// signal chain", and a listen that ran through nothing says nothing. So
    /// this remains the answer to "who may see how I listen", which is a
    /// different question from "who may see what I listen to".
    ///
    /// There is still no per-listen override and there should not be one: the
    /// decision is a standing one about *how you listen* rather than a choice
    /// to make per play.
    pub async fn set_chain_shared(
        &self,
        user_id: i64,
        chain_id: i64,
        shared: bool,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(&crate::sql!(
            "UPDATE signal_chains SET shared = ? WHERE id = ? AND user_id = ?"
        ))
        .bind(shared)
        .bind(chain_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
    /// The Reel: what the decks this user has patched into have been playing.
    ///
    /// **This is the one query in the application that reads another user's
    /// listens**, so every clause in it is load-bearing and none should be
    /// relaxed without saying why:
    ///
    /// * **`state = 'accepted'`** — a pending or rejected patch reads nothing.
    /// * **`c.shared = 1`** — the chain's owner switched it on.
    /// * **`s.chain_id` is joined, not left-joined.** A listen with no chain is
    ///   never shared: nobody switched anything on for it, so the *default* has
    ///   to be exclusion. A `LEFT JOIN` here would publish every unattributed
    ///   listen on the instance, and it would look like a tidy-up.
    /// * **`skipped = FALSE`** — a skip is stored and never counted, and it is
    ///   certainly not something to show other people.
    ///
    /// It is deliberately **not** scoped by `user_id = ?` the way every other
    /// read is; that is the relaxation Phase 6 exists to make, and it is
    /// narrowed by the patch and the chain instead.
    /// `q` narrows on track, artist, album, chain or deck; `deck` pins it to one
    /// username. **Both server-side**, because the Reel is keyset-paged — a
    /// filter applied to the loaded page would silently search only what has
    /// been scrolled to, and say nothing about it.
    pub async fn reel(
        &self,
        user_id: i64,
        limit: i64,
        before: Option<i64>,
        q: Option<&str>,
        deck: Option<&str>,
    ) -> Result<Vec<ReelListen>, sqlx::Error> {
        // A wildcard typed into the box is a literal, the rule `search_notes`
        // already follows. `str::replace` has no capture groups, and reaching
        // for a regex to quote three characters would be its own mistake.
        let needle = q.map(|raw| {
            format!(
                "%{}%",
                raw.trim()
                    .to_lowercase()
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
        sqlx::query_as(&crate::sql!(
            "SELECT s.id, s.timestamp, s.title, s.artist, s.album,
                    s.artwork_url, s.caa_id, s.caa_release_mbid,
                    s.format_type, s.codec, s.bitrate, s.sample_rate, s.bit_depth, s.is_lossless,
                    s.submission_client,
                    u.id AS user_id, u.username, u.display_name, u.avatar_url,
                    a.uri AS actor_uri,
                    -- Named only when its owner shares it; present-but-private
                    -- and absent-entirely are different facts and the row draws
                    -- them differently. See `CHAIN_VISIBILITY`.
                    CASE WHEN c.shared = 1 THEN c.name END AS chain_name,
                    CASE WHEN c.shared = 1 THEN c.icon END AS chain_icon,
                    CASE WHEN c.shared = 1 THEN c.components END AS chain_components,
                    CASE WHEN s.chain_id IS NULL THEN 0 ELSE 1 END AS has_chain
               FROM scrobbles s
               LEFT JOIN signal_chains c ON c.id = s.chain_id
               JOIN users u ON u.id = s.user_id
               JOIN patches p ON p.target_user_id = s.user_id
               LEFT JOIN actors a ON a.user_id = u.id
              WHERE p.follower_user_id = ? AND p.state = 'accepted'
                AND s.skipped = FALSE
                -- The gate, and the whole of it — see `PATCH_VISIBLE_LISTENS`.
                AND (u.share_all_with_patched = 1 OR c.shared = 1)
                AND (? IS NULL OR s.timestamp < ?)
                AND (? IS NULL OR u.username = ?)
                AND (? IS NULL OR
                     LOWER(s.title) LIKE ? ESCAPE '\\' OR
                     LOWER(s.artist) LIKE ? ESCAPE '\\' OR
                     LOWER(COALESCE(s.album, '')) LIKE ? ESCAPE '\\' OR
                     LOWER(c.name) LIKE ? ESCAPE '\\' OR
                     LOWER(u.username) LIKE ? ESCAPE '\\' OR
                     LOWER(COALESCE(u.display_name, '')) LIKE ? ESCAPE '\\')
              ORDER BY s.timestamp DESC
              LIMIT ?"
        ))
        .bind(user_id)
        .bind(before)
        .bind(before)
        .bind(deck)
        .bind(deck)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Decks on this instance that this user could patch into.
    ///
    /// Everyone but themselves and the disabled. **Not gated on
    /// `federation_enabled`**, which is a deliberate separation: that switch
    /// decides whether the wider fediverse can find you, and members of a
    /// private invite-only instance already know each other exist. What
    /// protects a deck here is that a patch needs approval and that a chain
    /// publishes nothing until its owner shares it.
    pub async fn patchable_decks(&self, user_id: i64) -> Result<Vec<PatchPeer>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT NULL AS uuid, NULL AS state, NULL AS created_at, {}
               FROM users u
              WHERE u.id <> ? AND u.disabled = 0
              ORDER BY u.username COLLATE NOCASE",
            Self::PATCH_PEER_COLUMNS
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// How many accepted patches point at this user, or away from them — the
    /// `totalItems` of the `followers` / `following` collections.
    ///
    /// **Counts patches, not actors.** The obvious implementation joins
    /// `actors` to get each end's URI and counts those, and it is wrong: a
    /// local follower who has never switched federation on has no actor row, so
    /// a deck with one follower reported `0`. That makes the number a fact
    /// about who happens to have enabled federation rather than about
    /// followers, which is meaningless to the remote software that reads it.
    ///
    /// Only the **count** is ever served. The roster is what would leak the
    /// shape of a household, and a count is what the protocol actually uses —
    /// see `server::patch::collection`.
    pub async fn patch_count(&self, user_id: i64, followers: bool) -> Result<i64, sqlx::Error> {
        let sql = if followers {
            "SELECT COUNT(*) FROM patches WHERE target_user_id = ? AND state = 'accepted'"
        } else {
            "SELECT COUNT(*) FROM patches WHERE follower_user_id = ? AND state = 'accepted'"
        };
        sqlx::query_scalar(&crate::sql!(sql)).bind(user_id).fetch_one(&self.pool).await
    }
    /// The folded artist keys this user plays, with their own counts.
    ///
    /// **One grouped scan, never a subquery per row.** This exists because the
    /// first version of `patch_artists` asked `COUNT(*) … WHERE
    /// LOWER(TRIM(m.artist)) = LOWER(TRIM(s.artist))` *per artist* — a join key
    /// that is a function of a column, which no index can serve, so it is a
    /// full scan of the whole history per outer row. That is the quadratic this
    /// codebase has now found four times, and in production it exhausted the
    /// connection pool and took the instance down. See the incident note in
    /// CLAUDE.md.
    async fn my_artist_plays(&self, user_id: i64) -> Result<HashMap<String, i64>, sqlx::Error> {
        let rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT LOWER(TRIM(artist)), COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND LOWER(TRIM(artist)) {NOT_VARIOUS}
              GROUP BY LOWER(TRIM(artist))"
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }
    /// Artists across the decks this user has patched into.
    ///
    /// `mine` is what makes the tab worth opening: the overlap is the finding,
    /// and it is shown as *which artists* rather than as a similarity score —
    /// the design's own rule, and the honest one, because a score invites a
    /// ranking of people.
    ///
    /// **Two bounded scans folded in Rust**, not a subquery per artist. See
    /// `my_artist_plays` for why, and what the other shape cost.
    /// `since` bounds the window the ranking is computed over — `None` is all
    /// time. It has to be a *server* parameter rather than a filter applied to
    /// what came back: the query returns the top `limit` by plays, so narrowing
    /// afterwards would rank the last 7 days among artists chosen by their
    /// all-time totals.
    pub async fn patch_artists(
        &self,
        user_id: i64,
        limit: i64,
        since: Option<i64>,
    ) -> Result<Vec<PatchArtist>, sqlx::Error> {
        let mut artists: Vec<PatchArtist> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT s.artist AS name, COUNT(*) AS plays, \
                    COUNT(DISTINCT s.user_id) AS decks, \
                    GROUP_CONCAT(DISTINCT u.username) AS who, \
                    MAX(s.timestamp) AS last_played, \
                    MIN(s.timestamp) AS first_played, 0 AS mine, \
                    NULL AS genre \
               {} AND (?2 IS NULL OR s.timestamp >= ?2) \
                AND LOWER(TRIM(s.artist)) {NOT_VARIOUS} \
            GROUP BY LOWER(TRIM(s.artist)) \
            ORDER BY plays DESC LIMIT ?3",
            Self::PATCH_VISIBLE_LISTENS
        )))
        .bind(user_id)
        .bind(since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mine = self.my_artist_plays(user_id).await?;
        // One grouped read for every genre, folded in Rust — never a lookup per
        // card, which is the quadratic this codebase keeps finding. `artists`
        // is the tag cache the genre map already fills, so this costs nothing
        // beyond the scan.
        let mut genres: HashMap<String, String> = HashMap::new();
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(&crate::sql!(
            "SELECT name, genres FROM artists WHERE genres IS NOT NULL"
        ))
        .fetch_all(&self.pool)
        .await?;
        for (name, raw) in rows {
            if let Some(first) = raw
                .as_deref()
                .and_then(|r| serde_json::from_str::<Vec<String>>(r).ok())
                .and_then(|v| v.into_iter().next())
            {
                genres.insert(name.trim().to_lowercase(), first);
            }
        }
        for a in &mut artists {
            let key = a.name.trim().to_lowercase();
            a.mine = mine.get(&key).copied().unwrap_or(0);
            a.genre = genres.get(&key).cloned();
        }
        Ok(artists)
    }
    /// One deck, as the person patched into it may see it.
    ///
    /// Returns `None` unless there is an accepted patch — the check is in the
    /// query rather than the handler, for the reason every other Phase 6 scope
    /// check is.
    pub async fn deck_view(
        &self,
        viewer_id: i64,
        username: &str,
    ) -> Result<Option<DeckView>, sqlx::Error> {
        let Some(row): Option<(i64, String, Option<String>, Option<String>, Option<String>, i64)> =
            sqlx::query_as(&crate::sql!(
                "SELECT u.id, u.username, u.display_name, u.bio, u.avatar_url, u.created_at
                   FROM users u
                   JOIN patches p ON p.target_user_id = u.id
                  WHERE u.username = ? COLLATE NOCASE
                    AND p.follower_user_id = ? AND p.state = 'accepted'
                    AND u.disabled = 0",
            ))
            .bind(username)
            .bind(viewer_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };

        let (id, username, display_name, bio, avatar_url, created_at) = row;

        // Only the chains they publish. An unshared chain is not theirs to be
        // asked about, and listing it would say what they own without saying
        // what they played — which is worse than saying nothing.
        let chains: Vec<DeckChain> = sqlx::query_as(&crate::sql!(
            "SELECT c.name, c.icon, c.components,
                    CAST(COALESCE((
                        SELECT SUM(sc.duration) FROM scrobbles sc
                         WHERE sc.chain_id = c.id AND sc.skipped = FALSE
                    ), 0) AS REAL) / 3600.0 AS hours
               FROM signal_chains c
              WHERE c.user_id = ? AND c.shared = 1
              ORDER BY hours DESC"
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        // What you both play. Overlap only — no score, no ranking of people.
        //
        // **Two grouped scans intersected in Rust**, not an `EXISTS` correlated
        // on `LOWER(TRIM(artist))`: that join key is a function of a column, so
        // no index can serve it and it becomes a full scan of the viewer's
        // history *per artist of theirs*. See `my_artist_plays`.
        let theirs: Vec<(String, String)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT s.artist, LOWER(TRIM(s.artist)) FROM scrobbles s
               JOIN users u ON u.id = s.user_id
               LEFT JOIN signal_chains c ON c.id = s.chain_id
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (u.share_all_with_patched = 1 OR c.shared = 1)
                AND LOWER(TRIM(s.artist)) {NOT_VARIOUS}
              GROUP BY LOWER(TRIM(s.artist))
              ORDER BY COUNT(*) DESC"
        )))
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        let mine = self.my_artist_plays(viewer_id).await?;
        let both: Vec<String> = theirs
            .into_iter()
            .filter(|(_, key)| mine.contains_key(key))
            .map(|(name, _)| name)
            .take(12)
            .collect();

        let listens: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles s
               JOIN users u ON u.id = s.user_id
               LEFT JOIN signal_chains c ON c.id = s.chain_id
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (u.share_all_with_patched = 1 OR c.shared = 1)"
        ))
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        // The actor endpoints, if they have an identity at all. `None` is a
        // normal state — federation is opt-in — and the page says so rather
        // than printing URLs that would 404.
        let actor: Option<String> =
            sqlx::query_scalar(&crate::sql!("SELECT uri FROM actors WHERE user_id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(Some(DeckView {
            user_id: id,
            username,
            display_name,
            bio,
            avatar_url,
            created_at,
            listens,
            chains,
            both_play: both,
            actor,
        }))
    }
    /// A deck this viewer has an accepted patch **into**, by username.
    ///
    /// Deliberately one-directional, unlike `dub`'s guard. A dub needs a patch
    /// either way because it is a message between two people who know each
    /// other; this answers "may I say I got this off their deck", and only the
    /// direction that lets you *read* their listening can make that true.
    /// Somebody who follows you has told you nothing.
    ///
    /// Returns their id and the name to write down.
    pub async fn deck_i_am_patched_into(
        &self,
        viewer_id: i64,
        username: &str,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        let row: Option<(i64, String, Option<String>)> = sqlx::query_as(&crate::sql!(
            "SELECT u.id, u.username, u.display_name
               FROM users u
               JOIN patches p ON p.target_user_id = u.id
              WHERE u.username = ? COLLATE NOCASE
                AND p.follower_user_id = ? AND p.state = 'accepted'
                AND u.disabled = 0"
        ))
        .bind(username)
        .bind(viewer_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, username, display)| (id, display.unwrap_or(username))))
    }
    /// Everyone on the instance, with what the viewer has in common with them.
    ///
    /// **Overlap is computed against their *shared* chains only**, so this
    /// cannot become a way to learn what somebody plays privately by watching
    /// the suggestion list change.
    ///
    /// **Three bounded scans folded in Rust.** The first version asked, per
    /// user, for their shared artists with an inner `EXISTS` over the viewer's
    /// whole history keyed on `LOWER(TRIM(artist))` — a full scan inside a full
    /// scan, per row. On the real instance that exhausted the connection pool
    /// within a minute of the first patch being accepted and took everything
    /// down with it. See the incident note in CLAUDE.md.
    pub async fn discover_decks(&self, user_id: i64) -> Result<Vec<DiscoverDeck>, sqlx::Error> {
        let mut decks: Vec<DiscoverDeck> = sqlx::query_as(&crate::sql!(
            "SELECT u.id AS user_id, u.username, u.display_name, u.bio, u.avatar_url,
                    u.created_at,
                    (SELECT COUNT(*) FROM signal_chains c
                      WHERE c.user_id = u.id AND c.shared = 1) AS shared_chains,
                    (SELECT state FROM patches p
                      WHERE p.follower_user_id = ? AND p.target_user_id = u.id) AS state,
                    '' AS overlap
               FROM users u
              WHERE u.id <> ? AND u.disabled = 0
              ORDER BY u.created_at DESC",
        ))
        .bind(user_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // Every deck's shared artists, in one pass over the shared listens.
        let shared: Vec<(i64, String, String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT s.user_id, s.artist, LOWER(TRIM(s.artist)), COUNT(*) AS plays
               FROM scrobbles s
               JOIN signal_chains c ON c.id = s.chain_id AND c.shared = 1
              WHERE s.skipped = FALSE
              GROUP BY s.user_id, LOWER(TRIM(s.artist))
              ORDER BY plays DESC",
        ))
        .fetch_all(&self.pool)
        .await?;

        let mine = self.my_artist_plays(user_id).await?;
        let mut by_user: HashMap<i64, Vec<String>> = HashMap::new();
        for (uid, name, key, _) in shared {
            if uid == user_id || !mine.contains_key(&key) {
                continue;
            }
            let entry = by_user.entry(uid).or_default();
            if entry.len() < 6 {
                entry.push(name);
            }
        }

        for d in &mut decks {
            if let Some(names) = by_user.remove(&d.user_id) {
                d.overlap = names.join("\u{1f}");
            }
        }
        Ok(decks)
    }
    /// What the instance has been playing, over shared chains only.
    ///
    /// **A deliberate reversal of the Reports decision that there is no
    /// cross-user percentile**, and recorded as one rather than left to look
    /// like nobody noticed. That rule existed to stop the instance exposing how
    /// much other members play without their consent; a per-chain opt-in *is*
    /// that consent. The limiter is load-bearing: shared chains only, and it
    /// stays inside the instance.
    ///
    /// Note this is **not** narrowed by the caller's patches. The Reel answers
    /// "what have the decks I follow played"; this answers "what is this
    /// instance listening to", which is a fact about the people who chose to
    /// publish a chain, and is the same for everyone who asks.
    pub async fn instance_pulse(&self, since: i64) -> Result<InstancePulse, sqlx::Error> {
        const SHARED: &str = "FROM scrobbles s \
             JOIN signal_chains c ON c.id = s.chain_id AND c.shared = 1 \
            WHERE s.skipped = FALSE";

        let decks: i64 =
            sqlx::query_scalar(&crate::sql!("SELECT COUNT(*) FROM users WHERE disabled = 0"))
                .fetch_one(&self.pool)
                .await?;
        let publishing: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(DISTINCT user_id) FROM signal_chains WHERE shared = 1",
        ))
        .fetch_one(&self.pool)
        .await?;
        let shared_chains: i64 =
            sqlx::query_scalar(&crate::sql!("SELECT COUNT(*) FROM signal_chains WHERE shared = 1"))
                .fetch_one(&self.pool)
                .await?;

        let listens: i64 = sqlx::query_scalar(&crate::sql!(&format!(
            "SELECT COUNT(*) {SHARED} AND s.timestamp >= ?"
        )))
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        let hours: f64 = sqlx::query_scalar(&crate::sql!(&format!(
            "SELECT CAST(COALESCE(SUM(s.duration), 0) AS REAL) / 3600.0 {SHARED} AND s.timestamp >= ?"
        )))
        .bind(since)
        .fetch_one(&self.pool)
        .await?;

        let artists: Vec<PatchArtist> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT s.artist AS name, COUNT(*) AS plays, \
                    COUNT(DISTINCT s.user_id) AS decks, \
                    GROUP_CONCAT(DISTINCT (SELECT username FROM users WHERE id = s.user_id)) AS who, \
                    MAX(s.timestamp) AS last_played, \
                    MIN(s.timestamp) AS first_played, 0 AS mine, NULL AS genre \
               {SHARED} AND s.timestamp >= ? \
                AND LOWER(TRIM(s.artist)) {NOT_VARIOUS} \
            GROUP BY LOWER(TRIM(s.artist)) ORDER BY plays DESC LIMIT 8"
        )))
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        let artists_heard: i64 = sqlx::query_scalar(&crate::sql!(&format!(
            "SELECT COUNT(DISTINCT LOWER(TRIM(s.artist))) {SHARED} AND s.timestamp >= ?"
        )))
        .bind(since)
        .fetch_one(&self.pool)
        .await?;

        let chains: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT c.name, COUNT(*) AS plays {SHARED} AND s.timestamp >= ? \
              GROUP BY c.id ORDER BY plays DESC LIMIT 6"
        )))
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        // Per day, oldest first, as raw (day, hours) — bucketed here by UTC
        // day and re-bucketed by the caller's timezone would be wrong, so the
        // *timestamps* go out and the client draws them. Seven numbers.
        let daily: Vec<(i64, f64)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT s.timestamp, CAST(COALESCE(s.duration, 0) AS REAL) / 3600.0 \
               {SHARED} AND s.timestamp >= ?"
        )))
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        Ok(InstancePulse {
            decks,
            publishing,
            shared_chains,
            listens,
            hours,
            artists_heard,
            artists,
            chains: chains.into_iter().map(|(name, plays)| ChainTally { name, plays }).collect(),
            daily: daily.into_iter().map(|(at, hours)| DailyHour { at, hours }).collect(),
        })
    }
    /// What an admin needs to see about federation.
    ///
    /// Deliberately reports **only what is recorded**. The design's Operations
    /// panel lists signature failures; refusals are logged through `tracing`
    /// and counted nowhere, and inventing a number for a tile would be worse
    /// than an empty one — so the screen says where to look instead.
    pub async fn federation_ops(&self) -> Result<FederationOps, sqlx::Error> {
        let one = |sql: &'static str| async move {
            sqlx::query_scalar::<_, i64>(&crate::sql!(sql)).fetch_one(&self.pool).await
        };
        Ok(FederationOps {
            identities: one("SELECT COUNT(*) FROM actors").await?,
            published: one("SELECT COUNT(*) FROM users WHERE federation_enabled = 1").await?,
            frozen: one("SELECT COUNT(*) FROM actors WHERE frozen_at IS NOT NULL").await?,
            remote_actors: one("SELECT COUNT(*) FROM remote_actors").await?,
            inbox_activities: one("SELECT COUNT(*) FROM inbox_activities").await?,
            deliveries: one("SELECT COUNT(*) FROM activity_deliveries").await?,
        })
    }
    /// Pass a record to someone you are patched with.
    ///
    /// **A dub is a crate entry that came from a person, not a second table.**
    /// It lands on the page they already visit for records they are still
    /// flipping through, and it is scored the same way — `first_played_at` is
    /// derived from their listens, so "you suggested 12 records and they played
    /// 5" falls out with nothing to accumulate.
    ///
    /// Two guards, and they are the crate's own:
    ///
    /// * **`because` is required.** What in *their* listening prompted this.
    ///   Without it a dub is free association, which is the thing the crate
    ///   exists not to be.
    /// * **A record already in their history is refused, with the play count.**
    ///   "You already have this" is Rediscovery under another name, and the
    ///   count is the half that tells the sender something.
    ///
    /// The relationship is checked in both directions being *either* — a dub
    /// travels along a patch, and it is enough that one of you let the other
    /// in. Dubbing at a stranger would be a way to put text on somebody's
    /// screen without their consent.
    pub async fn dub(
        &self,
        from_user_id: i64,
        to_user_id: i64,
        rec: &NewRecommendation,
        from_username: &str,
    ) -> Result<i64, sqlx::Error> {
        let connected: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM patches
              WHERE state = 'accepted'
                AND ((follower_user_id = ? AND target_user_id = ?)
                  OR (follower_user_id = ? AND target_user_id = ?))"
        ))
        .bind(from_user_id)
        .bind(to_user_id)
        .bind(to_user_id)
        .bind(from_user_id)
        .fetch_one(&self.pool)
        .await?;
        if connected == 0 {
            return Err(sqlx::Error::Protocol("not patched with that deck".into()));
        }

        let now = Self::now_secs();
        // `fetch_all`, never `fetch_one` — see `create_mcp_connection`.
        let row: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO recommendations
                (user_id, kind, artist, name, mbid, reason, because, origin,
                 from_user_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(to_user_id)
        .bind(&rec.kind)
        .bind(&rec.artist)
        .bind(&rec.name)
        .bind(&rec.mbid)
        .bind(&rec.reason)
        .bind(&rec.because)
        .bind(from_username)
        .bind(from_user_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        row.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    /// Dubs waiting on this user — crate entries a person put there.
    ///
    /// `from_user_id IS NOT NULL` is the whole filter: it is what separates a
    /// record a friend handed you from one a model suggested, on a page that
    /// deliberately holds both.
    pub async fn dubs_for(&self, user_id: i64) -> Result<Vec<Dub>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT r.id, r.kind, r.artist, r.name, r.reason, r.because, r.status,
                    r.created_at, u.username AS from_username, u.display_name AS from_display_name
               FROM recommendations r
               JOIN users u ON u.id = r.from_user_id
              WHERE r.user_id = ? AND r.from_user_id IS NOT NULL
              ORDER BY r.status = 'open' DESC, r.created_at DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Open a sitting and invite people to it.
    ///
    /// Refuses with `sqlx::Error::Protocol` when an invitee is not patched
    /// with the host — see the guard below.
    ///
    /// One running spool per host, enforced by a partial unique index rather
    /// than by this function — so a second start is an error from the database
    /// rather than a race between two checks.
    pub async fn start_spool(
        &self,
        host_user_id: i64,
        title: Option<&str>,
        with: &[i64],
    ) -> Result<String, sqlx::Error> {
        // **You may only invite a deck you are patched with**, in either
        // direction — the rule `dub` already follows, and for the same reason:
        // an invitation puts a banner on somebody's Patch page, and being able
        // to do that to a stranger is a way to reach them without their
        // consent. It belongs here rather than in the handler so a second
        // caller inherits it instead of having to know it was needed.
        //
        // One query for the whole set rather than one per invitee, and folded
        // in Rust — the pattern `list_crate` and `playlist_tracks_detailed`
        // use. Checked before anything is written, so a sitting never
        // half-starts with the strangers quietly dropped.
        let patched: Vec<i64> = sqlx::query_scalar(&crate::sql!(
            "SELECT CASE WHEN follower_user_id = ? THEN target_user_id ELSE follower_user_id END
               FROM patches
              WHERE state = 'accepted' AND (follower_user_id = ? OR target_user_id = ?)",
        ))
        .bind(host_user_id)
        .bind(host_user_id)
        .bind(host_user_id)
        .fetch_all(&self.pool)
        .await?;
        if with.iter().any(|u| *u != host_user_id && !patched.contains(u)) {
            return Err(sqlx::Error::Protocol("not patched with that deck".into()));
        }

        let uuid = new_uuid();
        let now = Self::now_secs();
        let mut tx = self.pool.begin().await?;

        sqlx::query(&crate::sql!(
            "INSERT INTO spools (uuid, host_user_id, title, started_at) VALUES (?, ?, ?, ?)",
        ))
        .bind(&uuid)
        .bind(host_user_id)
        .bind(title)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let spool_id: i64 =
            sqlx::query_scalar(&crate::sql!("SELECT id FROM spools WHERE uuid = ?"))
                .bind(&uuid)
                .fetch_one(&mut *tx)
                .await?;

        // The host is a member and is already in the room — everyone else is
        // invited, because a spool must not start capturing onto somebody's
        // deck before they have said yes to being in it.
        sqlx::query(&crate::sql!("INSERT INTO spool_members (spool_id, user_id, state, joined_at) VALUES (?, ?, 'joined', ?)",))
        .bind(spool_id)
        .bind(host_user_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        for &uid in with {
            if uid == host_user_id {
                continue;
            }
            sqlx::query(&crate::sql!(
                "INSERT INTO spool_members (spool_id, user_id) VALUES (?, ?) ON CONFLICT DO NOTHING",
            ))
            .bind(spool_id)
            .bind(uid)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(uuid)
    }
    /// Join or leave a sitting you were invited to.
    ///
    /// Returns `None` when there is no invitation of yours with that id, and
    /// `Some(offered)` otherwise — the number of plays the sitting had already
    /// held that joining has now put in your inbox.
    ///
    /// **An ended sitting can still be joined, and that is the point.** The
    /// obvious rule — you may only answer while the room is open — reads as
    /// tidy and is wrong in the common case: the host closes the sitting when
    /// the evening is over, which is exactly when the person who forgot to
    /// press Join finally looks at their screen. Refusing them there throws
    /// away an evening that really happened, and there is no recovery, because
    /// the invitation is gone too.
    ///
    /// Nothing is weakened by allowing it. The invitation is the host's
    /// assertion that you were in the room and the join is yours; neither is
    /// made more or less true by the room having closed since. And joining
    /// still decides nothing on its own — every play arrives as a capture you
    /// have to accept, which is the invariant a live sitting rests on.
    pub async fn set_spool_membership(
        &self,
        uuid: &str,
        user_id: i64,
        state: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let Some(spool_id): Option<i64> =
            sqlx::query_scalar(&crate::sql!("SELECT id FROM spools WHERE uuid = ?"))
                .bind(uuid)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(None);
        };

        let res = sqlx::query(&crate::sql!(
            "UPDATE spool_members SET state = ?, joined_at = COALESCE(joined_at, ?)
              WHERE user_id = ? AND spool_id = ?",
        ))
        .bind(state)
        .bind(Self::now_secs())
        .bind(user_id)
        .bind(spool_id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }

        if state != "joined" {
            return Ok(Some(0));
        }
        Ok(Some(self.offer_spool_backlog(spool_id, uuid, user_id).await?))
    }
    /// Offer the plays a sitting has already held to somebody who has just
    /// joined it.
    ///
    /// Without this, joining is only ever "from now on" — and for a sitting
    /// the host has already ended, "from now on" is nothing at all. The
    /// invitation would be answerable and answering it would do nothing, which
    /// is a worse failure than refusing the join outright because it looks like
    /// it worked.
    ///
    /// Reads the sitting back out of the **listens** rather than out of a
    /// record of what was offered, because there is no such record: a capture
    /// is only ever written for somebody who had already joined. The listens
    /// carry `joint_session_id`, which `capture_into_spool` stamps on every
    /// play made by a member in the room — so they *are* the sitting.
    ///
    /// Three rules, all of them the ones a live capture already follows:
    ///
    /// * **The chain must be shared.** `joint_session_id` is stamped *before*
    ///   the chain gate — deliberately, since nobody but the owner reads it —
    ///   so a listen through a private chain is in this set and must not be
    ///   offered. Re-applied here by joining `signal_chains`, the same way
    ///   `retro_spool_from_session` does. Dropping that join would publish
    ///   every private-chain listen of the evening while looking like a
    ///   simplification.
    /// * **Only members who were actually in the room**, so a listen stamped
    ///   with this uuid by somebody who has since left is not re-published by
    ///   a third person joining.
    /// * **Nothing already offered in *any* sitting**, settled or not — the
    ///   v0.84.0 rule. `spool_captures`' uniqueness is per spool and cannot see
    ///   a sibling, and a live sitting can cover the same plays as a
    ///   reconstruction of the same evening.
    ///
    /// The company snapshot is the room **as it stands now** rather than per
    /// play, because per-play membership is not recorded. That is honest for
    /// the case this exists for — somebody who was there the whole time and
    /// only pressed the button late — and approximate for a genuine mid-sitting
    /// arrival, which is the rarer one.
    ///
    /// Deliberately **does not** rewrite anybody else's listens. A late join
    /// says what the joiner heard; it is not standing to edit another deck's
    /// record of who was in the room at the time.
    async fn offer_spool_backlog(
        &self,
        spool_id: i64,
        uuid: &str,
        user_id: i64,
    ) -> Result<i64, sqlx::Error> {
        let present: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT m.user_id, COALESCE(u.display_name, u.username)
               FROM spool_members m JOIN users u ON u.id = m.user_id
              WHERE m.spool_id = ? AND m.state = 'joined'
              ORDER BY u.username"
        ))
        .bind(spool_id)
        .fetch_all(&self.pool)
        .await?;
        let names: Vec<&str> =
            present.iter().filter(|(id, _)| *id != user_id).map(|(_, n)| n.as_str()).collect();
        let company = (!names.is_empty()).then(|| names.join(", "));

        let plays: Vec<(i64, String, String, Option<String>, Option<i64>, i64)> =
            sqlx::query_as(&crate::sql!(
                "SELECT s.user_id, s.title, s.artist, s.album, s.duration, s.timestamp
               FROM scrobbles s
               JOIN signal_chains c ON c.id = s.chain_id AND c.shared = 1
              WHERE s.joint_session_id = ? AND s.user_id != ? AND s.skipped = FALSE
                AND s.user_id IN (
                      SELECT user_id FROM spool_members
                       WHERE spool_id = ? AND state = 'joined')
              ORDER BY s.timestamp"
            ))
            .bind(uuid)
            .bind(user_id)
            .bind(spool_id)
            .fetch_all(&self.pool)
            .await?;

        let mut offered = 0i64;
        for (from, title, artist, album, duration, played_at) in &plays {
            let res = sqlx::query(&crate::sql!(
                "INSERT INTO spool_captures
                    (spool_id, user_id, from_user_id, title, artist, album, duration, played_at,
                     company)
                 SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?
                  WHERE NOT EXISTS (
                        SELECT 1 FROM spool_captures
                         WHERE user_id = ? AND played_at = ? AND title = ?
                  )"
            ))
            .bind(spool_id)
            .bind(user_id)
            .bind(from)
            .bind(title)
            .bind(artist)
            .bind(album)
            .bind(duration)
            .bind(played_at)
            .bind(&company)
            .bind(user_id)
            .bind(played_at)
            .bind(title)
            .execute(&self.pool)
            .await?;
            offered += res.rows_affected() as i64;
        }
        Ok(offered)
    }
    /// End a sitting. **The host's only.**
    ///
    /// Captures already made are untouched: they are offers somebody has not
    /// answered yet, and ending the room does not withdraw what was played in
    /// it.
    pub async fn end_spool(&self, uuid: &str, host_user_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("UPDATE spools SET ended_at = ? WHERE uuid = ? AND host_user_id = ? AND ended_at IS NULL",))
        .bind(Self::now_secs())
        .bind(uuid)
        .bind(host_user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// The sitting this user is in, if any — hosting or attending — **or an
    /// invitation they have not answered yet.**
    ///
    /// The second half is why an ended sitting is in scope. An invitation is
    /// outstanding until it is answered, and the host closing the room does not
    /// answer it; dropping it at that moment is what made "I forgot to press
    /// Join" unrecoverable, because the banner carrying the only way back in
    /// disappeared along with the sitting. Saying "No thanks" is what dismisses
    /// it, which is the correct way for an invitation to go away.
    ///
    /// A running sitting still wins, so somebody with a stale invitation and a
    /// live room sees the live one.
    pub async fn active_spool(&self, user_id: i64) -> Result<Option<SpoolView>, sqlx::Error> {
        let Some(row): Option<(i64, String, i64, Option<String>, i64, Option<i64>, String)> =
            sqlx::query_as(&crate::sql!(
                "SELECT sp.id, sp.uuid, sp.host_user_id, sp.title, sp.started_at, sp.ended_at,
                        m.state
                   FROM spools sp
                   JOIN spool_members m ON m.spool_id = sp.id AND m.user_id = ?
                  WHERE sp.ended_at IS NULL OR m.state = 'invited'
                  ORDER BY sp.ended_at IS NULL DESC, sp.started_at DESC LIMIT 1",
            ))
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let (id, uuid, host_user_id, title, started_at, ended_at, my_state) = row;

        let members: Vec<SpoolMember> = sqlx::query_as(&crate::sql!(
            "SELECT u.id AS user_id, u.username, u.display_name, m.state
               FROM spool_members m JOIN users u ON u.id = m.user_id
              WHERE m.spool_id = ? ORDER BY m.state = 'joined' DESC, u.username"
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        let captured: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM spool_captures WHERE spool_id = ? AND user_id = ?",
        ))
        .bind(id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(Some(SpoolView {
            uuid,
            viewer_id: user_id,
            title,
            started_at,
            ended_at,
            host_user_id,
            hosting: host_user_id == user_id,
            my_state,
            members,
            captured,
        }))
    }
    /// Hand one of your sittings to somebody you are patched with.
    ///
    /// **Nothing is copied.** This records that they may look at it; the
    /// document is rendered from your listens on every read. So revoking is
    /// real here in a way it can never be once this federates.
    ///
    /// Guarded like `dub` and `start_spool`: patched in either direction, and
    /// refused with `sqlx::Error::Protocol` otherwise. Handing a sitting to a
    /// stranger would put a document on their screen without their consent.
    pub async fn hand_over_session(
        &self,
        from_user_id: i64,
        session_uuid: &str,
        to_user_id: i64,
        note: Option<&str>,
    ) -> Result<String, sqlx::Error> {
        if to_user_id == from_user_id {
            return Err(sqlx::Error::Protocol("hand it to somebody else".into()));
        }
        let connected: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM patches
              WHERE state = 'accepted'
                AND ((follower_user_id = ? AND target_user_id = ?)
                  OR (follower_user_id = ? AND target_user_id = ?))"
        ))
        .bind(from_user_id)
        .bind(to_user_id)
        .bind(to_user_id)
        .bind(from_user_id)
        .fetch_one(&self.pool)
        .await?;
        if connected == 0 {
            return Err(sqlx::Error::Protocol("not patched with that deck".into()));
        }

        // The sitting has to be *yours*. Checked in the query rather than by the
        // handler, so a second caller inherits it.
        let owned: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM sessions WHERE uuid = ? AND user_id = ?",
        ))
        .bind(session_uuid)
        .bind(from_user_id)
        .fetch_one(&self.pool)
        .await?;
        if owned == 0 {
            return Err(sqlx::Error::RowNotFound);
        }

        let uuid = new_uuid();
        // The partial index needs its predicate repeated in the conflict
        // target, or SQLite rejects the statement outright — at runtime, since
        // every query here is a runtime string. The trap `physical_images` and
        // `request_patch` both document.
        //
        // `fetch_all`, never `fetch_one`, on a RETURNING write against a pool.
        let row: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO shared_sessions (uuid, session_uuid, from_user_id, to_user_id, note,
                                          created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(from_user_id, session_uuid, to_user_id) WHERE to_user_id IS NOT NULL
             DO UPDATE SET note = excluded.note
             RETURNING uuid",
        ))
        .bind(&uuid)
        .bind(session_uuid)
        .bind(from_user_id)
        .bind(to_user_id)
        .bind(note)
        .bind(Self::now_secs())
        .fetch_all(&self.pool)
        .await?;
        row.into_iter().next().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    /// **The spool you were both in and neither of you started.**
    ///
    /// Reconstructs a sitting from a past session and offers its plays to
    /// somebody who was actually in the room, as ordinary `spool_captures` —
    /// so from the recipient's side this is indistinguishable from a live
    /// Shared Spool, lands in the same inbox section, and enters their history
    /// on the same accept. That is the point: it *is* the spool, made late.
    ///
    /// The other gesture is `hand_over_session`, which is a document because
    /// the recipient was **not** there. Getting the two confused was the
    /// original design error here: "they were not in the room" is the rarer
    /// case, not the common one — most of the time you sat together and simply
    /// forgot to press the button.
    ///
    /// Returns `(spool uuid, captures offered, listens withheld)`.
    pub async fn retro_spool_from_session(
        &self,
        host_user_id: i64,
        session_uuid: &str,
        to_user_id: i64,
    ) -> Result<(String, i64, i64), sqlx::Error> {
        if to_user_id == host_user_id {
            return Err(sqlx::Error::Protocol("share it with somebody else".into()));
        }
        let connected: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM patches
              WHERE state = 'accepted'
                AND ((follower_user_id = ? AND target_user_id = ?)
                  OR (follower_user_id = ? AND target_user_id = ?))"
        ))
        .bind(host_user_id)
        .bind(to_user_id)
        .bind(to_user_id)
        .bind(host_user_id)
        .fetch_one(&self.pool)
        .await?;
        if connected == 0 {
            return Err(sqlx::Error::Protocol("not patched with that deck".into()));
        }

        let Some((session_id, title, started_at, ended_at)): Option<(
            i64,
            Option<String>,
            i64,
            Option<i64>,
        )> = sqlx::query_as(&crate::sql!(
            "SELECT id, title, started_at, ended_at FROM sessions WHERE uuid = ? AND user_id = ?",
        ))
        .bind(session_uuid)
        .bind(host_user_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Err(sqlx::Error::RowNotFound);
        };

        let now = Self::now_secs();
        let mut tx = self.pool.begin().await?;

        // Find or create the sitting. A second person added to the same evening
        // joins the spool that already exists — one event, however many people
        // were in it — which is what the partial unique index enforces.
        let existing: Option<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, uuid FROM spools WHERE host_user_id = ? AND session_uuid = ?",
        ))
        .bind(host_user_id)
        .bind(session_uuid)
        .fetch_optional(&mut *tx)
        .await?;

        let (spool_id, spool_uuid) = match existing {
            Some(pair) => pair,
            None => {
                let uuid = new_uuid();
                // Created **already ended**, with the sitting's own times. It
                // is not running, so the one-running-spool-per-host index does
                // not apply and a live sitting elsewhere is undisturbed.
                sqlx::query(&crate::sql!(
                    "INSERT INTO spools (uuid, host_user_id, title, started_at, ended_at,
                                         session_uuid)
                     VALUES (?, ?, ?, ?, ?, ?)",
                ))
                .bind(&uuid)
                .bind(host_user_id)
                .bind(&title)
                .bind(started_at)
                .bind(ended_at.unwrap_or(started_at))
                .bind(session_uuid)
                .execute(&mut *tx)
                .await?;
                let id: i64 =
                    sqlx::query_scalar(&crate::sql!("SELECT id FROM spools WHERE uuid = ?"))
                        .bind(&uuid)
                        .fetch_one(&mut *tx)
                        .await?;
                sqlx::query(&crate::sql!(
                    "INSERT INTO spool_members (spool_id, user_id, state, joined_at)
                     VALUES (?, ?, 'joined', ?) ON CONFLICT DO NOTHING",
                ))
                .bind(id)
                .bind(host_user_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                (id, uuid)
            }
        };

        // The recipient goes in as **joined**, because the host is asserting
        // they were in the room. That is safe to assert here for one reason
        // only: it decides nothing on its own. Every play still arrives as a
        // capture they must accept, so nothing reaches their history without
        // their yes — the invariant a live spool rests on, unchanged.
        sqlx::query(&crate::sql!(
            "INSERT INTO spool_members (spool_id, user_id, state, joined_at)
             VALUES (?, ?, 'joined', ?)
             ON CONFLICT(spool_id, user_id) DO UPDATE SET state = 'joined'"
        ))
        .bind(spool_id)
        .bind(to_user_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let host_name: String = sqlx::query_scalar(&crate::sql!(
            "SELECT COALESCE(display_name, username) FROM users WHERE id = ?",
        ))
        .bind(host_user_id)
        .fetch_one(&mut *tx)
        .await?;
        let their_name: String = sqlx::query_scalar(&crate::sql!(
            "SELECT COALESCE(display_name, username) FROM users WHERE id = ?",
        ))
        .bind(to_user_id)
        .fetch_one(&mut *tx)
        .await?;

        // **The same shared-chain rule a live sitting follows**, so a spool made
        // late captures exactly what one started on time would have. What it
        // therefore leaves out is counted and returned, never silently dropped —
        // and unlike a live sitting there is a recovery: share the chain and do
        // it again.
        let plays: Vec<(String, String, Option<String>, Option<i64>, i64)> =
            sqlx::query_as(&crate::sql!(
                "SELECT s.title, s.artist, s.album, s.duration, s.timestamp
               FROM scrobbles s
               JOIN signal_chains c ON c.id = s.chain_id AND c.shared = 1
              WHERE s.session_id = ? AND s.skipped = FALSE
              ORDER BY s.timestamp"
            ))
            .bind(session_id)
            .fetch_all(&mut *tx)
            .await?;

        let total: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles WHERE session_id = ? AND skipped = FALSE",
        ))
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;

        let mut offered = 0i64;
        for (t, a, al, dur, ts) in &plays {
            // **Not offered if this person has already been offered it**, in
            // *any* sitting. `spool_captures`' unique constraint is per spool,
            // so it cannot see across one — and a live sitting running while
            // this reconstruction happens covers exactly the same plays. Found
            // by doing it on a running instance: the same track appeared twice
            // in the inbox, once from each. An already-settled capture matches
            // too, so a play they have accepted or declined is not re-offered.
            let res = sqlx::query(&crate::sql!(
                "INSERT INTO spool_captures
                    (spool_id, user_id, from_user_id, title, artist, album, duration, played_at,
                     company)
                 SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?
                  WHERE NOT EXISTS (
                        SELECT 1 FROM spool_captures
                         WHERE user_id = ? AND played_at = ? AND title = ?
                  )"
            ))
            .bind(spool_id)
            .bind(to_user_id)
            .bind(host_user_id)
            .bind(t)
            .bind(a)
            .bind(&al)
            .bind(dur)
            .bind(ts)
            .bind(&host_name)
            .bind(to_user_id)
            .bind(ts)
            .bind(t)
            .execute(&mut *tx)
            .await?;
            offered += res.rows_affected() as i64;
        }

        // And the host's own listens learn they were not heard alone. Only the
        // ones not already in a sitting: a listen carrying a `joint_session_id`
        // was in a real spool at the time, and that is the better record.
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET joint_session_id = ?, company = COALESCE(company, ?)
              WHERE session_id = ? AND user_id = ? AND joint_session_id IS NULL",
        ))
        .bind(&spool_uuid)
        .bind(&their_name)
        .bind(session_id)
        .bind(host_user_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok((spool_uuid, offered, (total - plays.len() as i64).max(0)))
    }
    /// Withdraw a sitting you handed over. Yours only.
    pub async fn withdraw_session(
        &self,
        uuid: &str,
        from_user_id: i64,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "DELETE FROM shared_sessions WHERE uuid = ? AND from_user_id = ?"
        ))
        .bind(uuid)
        .bind(from_user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Who else on this instance plays a given artist, and which of your own
    /// chains you hear them on.
    ///
    /// **Instance-wide rather than narrowed by your patches**, the same
    /// deliberate reversal the Instance tab makes and for the same reason: the
    /// rule that stops a deck exposing somebody's listening without consent is
    /// the per-chain switch, and switching it on *is* that consent. Shared
    /// chains only, and it stays inside the instance.
    ///
    /// `patched` says whether you already have an accepted patch with them, so
    /// the screen can offer to open their deck or to ask — rather than a button
    /// that 404s, since `deck_view` needs a patch.
    ///
    /// Two grouped scans, never a lookup per row: the folded artist key is a
    /// function of a column and can never be indexed, which is the quadratic
    /// this codebase keeps finding.
    pub async fn patch_artist_context(
        &self,
        user_id: i64,
        artist: &str,
    ) -> Result<(Vec<ArtistDeck>, Vec<ChainTally>), sqlx::Error> {
        let key = artist.trim().to_lowercase();

        let decks: Vec<ArtistDeck> = sqlx::query_as(&crate::sql!(
            "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url,
                    a.uri AS actor_uri, COUNT(*) AS plays,
                    EXISTS (SELECT 1 FROM patches p
                             WHERE p.state = 'accepted'
                               AND ((p.follower_user_id = ?1 AND p.target_user_id = u.id)
                                 OR (p.follower_user_id = u.id AND p.target_user_id = ?1)))
                      AS patched
               FROM scrobbles s
               JOIN signal_chains c ON c.id = s.chain_id AND c.shared = 1
               JOIN users u ON u.id = s.user_id
               LEFT JOIN actors a ON a.user_id = u.id
              WHERE s.skipped = FALSE
                AND LOWER(TRIM(s.artist)) = ?2
                AND s.user_id <> ?1
                AND u.disabled = 0
              GROUP BY u.id ORDER BY plays DESC LIMIT 12",
        ))
        .bind(user_id)
        .bind(&key)
        .fetch_all(&self.pool)
        .await?;

        // **Your own** chains for this artist — no sharing question, it is your
        // listening. Not filtered on `shared`: what you heard them on is a fact
        // about your setup rather than a publication.
        let chains: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT c.name, COUNT(*) AS plays
               FROM scrobbles s JOIN signal_chains c ON c.id = s.chain_id
              WHERE s.user_id = ? AND s.skipped = FALSE AND LOWER(TRIM(s.artist)) = ?
              GROUP BY c.id ORDER BY plays DESC LIMIT 6"
        ))
        .bind(user_id)
        .bind(&key)
        .fetch_all(&self.pool)
        .await?;

        Ok((decks, chains.into_iter().map(|(name, plays)| ChainTally { name, plays }).collect()))
    }
    /// The decks this person has an accepted patch into.
    ///
    /// Just the people — no listens — because the live strip reads what they
    /// are playing from the in-memory registry rather than from the history.
    pub async fn accepted_patch_targets(&self, user_id: i64) -> Result<Vec<DeckPeer>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT u.id AS user_id, u.username, u.display_name, u.avatar_url
               FROM patches p JOIN users u ON u.id = p.target_user_id
              WHERE p.follower_user_id = ? AND p.state = 'accepted'
              ORDER BY u.username"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Whether a deck you have patched with sees **all** your listens.
    pub async fn share_all_with_patched(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query_scalar(&crate::sql!(
            "SELECT share_all_with_patched FROM users WHERE id = ?"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(false))
    }
    /// Open the deck up to everyone patched with you, or close it again.
    ///
    /// Closing takes effect immediately and completely: every read goes back
    /// through `PATCH_VISIBLE_LISTENS`, which is evaluated per query rather
    /// than copied anywhere. Nothing was ever handed over — unlike a spool
    /// capture, which enters somebody's history and stays there — so this is a
    /// switch that genuinely reverses.
    pub async fn set_share_all_with_patched(
        &self,
        user_id: i64,
        on: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("UPDATE users SET share_all_with_patched = ? WHERE id = ?"))
            .bind(on)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// May a patched deck see what this person is playing, and may it be told
    /// what through?
    ///
    /// Two questions, and since 2026-08-26 they have different answers — which
    /// is the whole of what changed. `Visible::No` means the listen is not
    /// shown at all; `Visible::Yes(None)` means it is shown with *"Private
    /// signal chain"* where the chain would be.
    ///
    /// Returns the name rather than a boolean so one round trip answers both
    /// "may this be shown" and "what is it called" — and so a caller cannot
    /// accidentally show the name of a chain it did not check.
    ///
    /// `chain_id` is `None` for a source that reports no chain at all
    /// (Navidrome, an unattributed submission). Those used to be excluded
    /// outright; now they are shown when their owner has opened up, because
    /// "all the music they scrobble" has to mean all of it.
    pub async fn live_visibility(
        &self,
        user_id: i64,
        chain_id: Option<i64>,
    ) -> Result<Visible, sqlx::Error> {
        let all: bool = sqlx::query_scalar(&crate::sql!(
            "SELECT share_all_with_patched FROM users WHERE id = ?"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(false);

        let named: Option<String> = match chain_id {
            Some(cid) => {
                sqlx::query_scalar(&crate::sql!(
                    "SELECT name FROM signal_chains WHERE id = ? AND user_id = ? AND shared = 1",
                ))
                .bind(cid)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?
            }
            None => None,
        };

        // A shared chain is visible however the switch is set — that is the old
        // rule, intact.
        if named.is_some() || all {
            Ok(Visible::Yes(named))
        } else {
            Ok(Visible::No)
        }
    }
    /// How many things are waiting on this person in Patch.
    ///
    /// **One round trip**, because the sidebar asks on every page and four
    /// requests for one number would be four connections out of a pool of
    /// five. The Patch page itself derives the same number from the four lists
    /// it has already loaded rather than calling this — two readers, one
    /// meaning, and the page's version is free where the sidebar's cannot be.
    ///
    /// Each term is the same population its own screen shows: a patch waiting
    /// on a yes, a capture nobody has settled, a dub still open, and a sitting
    /// passed on but not yet opened.
    pub async fn patch_inbox_count(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(&crate::sql!(
            "SELECT (SELECT COUNT(*) FROM patches
                      WHERE target_user_id = ?1 AND state = 'pending')
                  + (SELECT COUNT(*) FROM spool_captures
                      WHERE user_id = ?1 AND state = 'pending')
                  + (SELECT COUNT(*) FROM recommendations
                      WHERE user_id = ?1 AND from_user_id IS NOT NULL AND status = 'open')
                  + (SELECT COUNT(*) FROM shared_sessions
                      WHERE to_user_id = ?1 AND seen_at IS NULL)"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
    }
    /// Sittings handed to this person, newest first.
    pub async fn sittings_for(&self, user_id: i64) -> Result<Vec<HandedSitting>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT ss.uuid, ss.session_uuid, ss.note, ss.created_at, ss.seen_at,
                    u.username AS from_username, u.display_name AS from_display_name,
                    se.title, se.started_at, se.ended_at
               FROM shared_sessions ss
               JOIN users u ON u.id = ss.from_user_id
               LEFT JOIN sessions se ON se.uuid = ss.session_uuid
              WHERE ss.to_user_id = ?
              ORDER BY ss.created_at DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// What you have passed on, and whether it has been read.
    ///
    /// The counterpart of [`Self::sittings_for`], and its absence is why
    /// `withdraw_session` had no caller: a hand-over could be sent and never
    /// listed, so there was no screen from which to take one back.
    ///
    /// **Only documents appear here.** A reconstructed spool writes real
    /// captures into the other person's inbox and no `shared_sessions` row, so
    /// there is nothing to withdraw — which is correct rather than an omission:
    /// those are plays they accept or decline for themselves.
    pub async fn sittings_sent(&self, user_id: i64) -> Result<Vec<SentSitting>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT ss.uuid, ss.session_uuid, ss.note, ss.created_at, ss.seen_at,
                    u.username AS to_username, u.display_name AS to_display_name,
                    se.title, se.started_at, se.ended_at
               FROM shared_sessions ss
               JOIN users u ON u.id = ss.to_user_id
               LEFT JOIN sessions se ON se.uuid = ss.session_uuid
              WHERE ss.from_user_id = ?
              ORDER BY ss.created_at DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Mark a handed-over sitting as seen. There is no accept: it writes
    /// nothing to a history, so there is nothing to consent to — only a
    /// notification to clear.
    pub async fn mark_sitting_seen(&self, uuid: &str, user_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE shared_sessions SET seen_at = ?
              WHERE uuid = ? AND to_user_id = ? AND seen_at IS NULL"
        ))
        .bind(Self::now_secs())
        .bind(uuid)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// The document itself: what played, in order, and what is not shown.
    ///
    /// **Shows exactly what the Reel would**, which since 2026-08-26 means:
    /// every listen if the sender has `share_all_with_patched` on, and
    /// otherwise only those through chains they share. Following the Reel is
    /// the point — a recipient who can already see a listen on the Reel would
    /// find it absurd to have it withheld from a document that person chose to
    /// hand them.
    ///
    /// Whatever is left out is **counted and reported** rather than silently
    /// dropped: a document with invisible holes misrepresents the evening, and
    /// the recipient could never tell.
    ///
    /// Rendered live, so un-sharing a chain afterwards really does take those
    /// listens back out.
    pub async fn handed_sitting(
        &self,
        uuid: &str,
        viewer_id: i64,
    ) -> Result<Option<HandedSittingDetail>, sqlx::Error> {
        // Readable by the recipient, or by whoever handed it over — so you can
        // see what you actually passed on, holes included.
        let Some((session_uuid, note, from_username, from_display_name)): Option<(
            String,
            Option<String>,
            String,
            Option<String>,
        )> = sqlx::query_as(&crate::sql!(
            "SELECT ss.session_uuid, ss.note, u.username, u.display_name
               FROM shared_sessions ss JOIN users u ON u.id = ss.from_user_id
              WHERE ss.uuid = ? AND (ss.to_user_id = ? OR ss.from_user_id = ?)"
        ))
        .bind(uuid)
        .bind(viewer_id)
        .bind(viewer_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let Some((session_id, title, started_at, ended_at)): Option<(
            i64,
            Option<String>,
            i64,
            Option<i64>,
        )> = sqlx::query_as(&crate::sql!(
            "SELECT id, title, started_at, ended_at FROM sessions WHERE uuid = ?",
        ))
        .bind(&session_uuid)
        .fetch_optional(&self.pool)
        .await?
        else {
            // The sitting was regrouped out of existence. An honest empty
            // document beats a 404, which would read as a broken link.
            return Ok(Some(HandedSittingDetail {
                uuid: uuid.to_string(),
                note,
                from_username,
                from_display_name,
                title: None,
                started_at: 0,
                ended_at: None,
                listens: Vec::new(),
                omitted: 0,
                gone: true,
            }));
        };

        let listens: Vec<HandedListen> = sqlx::query_as(&crate::sql!(
            "SELECT s.title, s.artist, s.album, s.timestamp, s.duration, s.artwork_url,
                    s.caa_id, s.caa_release_mbid,
                    CASE WHEN c.shared = 1 THEN c.name END AS chain_name
               FROM scrobbles s
               JOIN users u ON u.id = s.user_id
               LEFT JOIN signal_chains c ON c.id = s.chain_id
              WHERE s.session_id = ? AND s.skipped = FALSE
                AND (u.share_all_with_patched = 1 OR c.shared = 1)
              ORDER BY s.timestamp",
        ))
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        // Everything in the sitting that the reader is not being shown. One
        // count, stated on the document — never a silent difference.
        let total: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles WHERE session_id = ? AND skipped = FALSE",
        ))
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(Some(HandedSittingDetail {
            uuid: uuid.to_string(),
            note,
            from_username,
            from_display_name,
            title,
            started_at,
            ended_at,
            omitted: (total - listens.len() as i64).max(0),
            listens,
            gone: false,
        }))
    }
    /// Mark a listen as having happened in company, and offer it to the others.
    ///
    /// **Two jobs, and the order between them is the whole point.** Stamping
    /// `joint_session_id` says *I was not alone when I heard this* — a fact
    /// about this listener's own listening, which nobody else ever sees.
    /// Fanning out a capture says *here, have this play* — publishing. They
    /// used to be one gated block, so a host who shared no chain had **none of
    /// their own listens marked as social** even though they were demonstrably
    /// hosting a sitting. Two different questions fused because they shared a
    /// function.
    ///
    /// So the stamp happens for anyone in a joined sitting, and only the
    /// fan-out below is gated on the chain.
    ///
    /// Called from `save_scrobble` for **live, unskipped** listens only. An
    /// import must not fan out — twenty thousand rows would put twenty thousand
    /// offers in somebody's inbox, and a backfill is not a sitting. It must not
    /// be *stamped* either: a backfilled row was not heard in company just
    /// because a sitting happens to be running while it imports.
    ///
    /// **For the fan-out the chain must be shared, exactly as for the Reel.**
    /// The other reading was considered and rejected: a spool is a bounded
    /// event both people opted into, so "everyone in the room gets the plays"
    /// is defensible and is probably what a user expects. But the rule this
    /// phase is built on says a listen through an unshared chain is *not
    /// published, not counted and not delivered*, with no per-listen override —
    /// and a capture is publishing. Allowing it would mean two ways a listen
    /// becomes visible while the settings page explains only one, and it would
    /// let somebody start a sitting and quietly hand over a chain they had
    /// deliberately kept private.
    ///
    /// The consequence is real and has to be surfaced rather than discovered: a
    /// host with no shared chain captures nothing, and the screen says so.
    ///
    /// Cheap when nothing is running: one indexed lookup that returns no rows.
    pub(super) async fn capture_into_spool(
        &self,
        user_id: i64,
        play: &Play,
        chain_id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let Some((spool_id, uuid)): Option<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT sp.id, sp.uuid FROM spools sp
               JOIN spool_members m ON m.spool_id = sp.id AND m.user_id = ? AND m.state = 'joined'
              WHERE sp.ended_at IS NULL LIMIT 1"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(());
        };

        // Everyone actually in the room *at this moment*. Read per listen
        // rather than once per sitting, because somebody joining halfway
        // through means the earlier plays really were heard with fewer
        // people — that is accurate, not stale. Joined only: an invitation is
        // not attendance, and naming somebody who never answered as company
        // would be asserting they were there.
        //
        // One read serves every stamp below — this listener's own, and one per
        // recipient, each of which needs the same list minus themselves.
        let present: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT m.user_id, COALESCE(u.display_name, u.username)
               FROM spool_members m JOIN users u ON u.id = m.user_id
              WHERE m.spool_id = ? AND m.state = 'joined'
              ORDER BY u.username"
        ))
        .bind(spool_id)
        .fetch_all(&self.pool)
        .await?;
        let company_for = |excluding: i64| -> Option<String> {
            let names: Vec<&str> = present
                .iter()
                .filter(|(id, _)| *id != excluding)
                .map(|(_, n)| n.as_str())
                .collect();
            (!names.is_empty()).then(|| names.join(", "))
        };
        let company = company_for(user_id);

        // Stamp this listener's own listen with the sitting it belonged to, and
        // with who was in it. The other copies get the same uuid when they are
        // accepted, which is what makes them one joint listen rather than
        // several coincidences.
        //
        // The names are stored rather than derived through `joint_session_id`
        // on read: a history page would otherwise pay a join per listing to
        // recover something that cannot change after the fact. The same call
        // `artist_credit` makes.
        //
        // **Before the chain gate**, and that is deliberate — see the note on
        // this function. Nobody but the owner ever reads these columns, so a
        // publishing switch has no business deciding them.
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET joint_session_id = ?, company = COALESCE(?, company)
              WHERE user_id = ? AND source_id = ? AND source_name = ?",
        ))
        .bind(&uuid)
        .bind(&company)
        .bind(user_id)
        .bind(&play.source_id)
        .bind(&play.source_name)
        .execute(&self.pool)
        .await?;

        // From here down is publishing, and publishing is gated on the chain.
        // No chain is no sharing — the default has to be exclusion, the same
        // call `reel` makes by joining rather than left-joining.
        let Some(chain_id) = chain_id else {
            return Ok(());
        };
        let shared: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT COUNT(*) FROM signal_chains WHERE id = ? AND user_id = ? AND shared = 1",
        ))
        .bind(chain_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        if shared == 0 {
            return Ok(());
        }

        // The capture carries who was in the room **when the play happened**,
        // not when it is eventually accepted. Those can be hours apart, and
        // somebody joining in between was not there for this one.
        for (other, _) in present.iter().filter(|(id, _)| *id != user_id) {
            sqlx::query(&crate::sql!(
                "INSERT INTO spool_captures
                    (spool_id, user_id, from_user_id, title, artist, album, duration, played_at,
                     company)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING"
            ))
            .bind(spool_id)
            .bind(other)
            .bind(user_id)
            .bind(&play.title)
            .bind(&play.artist)
            .bind(&play.album)
            .bind(play.duration.map(|d| d as i64))
            .bind(play.timestamp as i64)
            .bind(company_for(*other))
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }
    /// Captures waiting on this user.
    pub async fn spool_captures(&self, user_id: i64) -> Result<Vec<SpoolCapture>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT cap.id, cap.title, cap.artist, cap.album, cap.duration, cap.played_at,
                    cap.state, u.username AS from_username, u.display_name AS from_display_name,
                    sp.uuid AS spool_uuid, sp.title AS spool_title
               FROM spool_captures cap
               JOIN users u ON u.id = cap.from_user_id
               JOIN spools sp ON sp.id = cap.spool_id
              WHERE cap.user_id = ?
              ORDER BY cap.state = 'pending' DESC, cap.played_at DESC
              LIMIT 200",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Accept or decline captures. **This is the only path by which somebody
    /// else's deck can write a listen into your history**, which is why it is
    /// per capture and explicit.
    ///
    /// Accepting writes a real scrobble carrying the spool's `joint_session_id`
    /// — the N-rows-one-per-listener shape decided on 2026-08-02.
    ///
    /// **Whether it is forwarded on to Last.fm and ListenBrainz is the user's
    /// own setting**, `users.forward_spool_listens`, off by default. It shipped
    /// as never-forwarded, and the reason given was that pushing somebody
    /// else's play onto your permanent public record is not a decision this
    /// feature has standing to make for you. That reasoning was right about the
    /// *default* and wrong to stop there: you were in the room, you heard the
    /// record, and you pressed Accept on that particular play — which is a
    /// clearer statement of intent than most things this application forwards
    /// without asking. So it asks once, in Settings, rather than deciding
    /// forever. Ashwin's call, 2026-08-23.
    ///
    /// Returns `(settled, forwarded)` — the second is how many of those the
    /// flush loop will now pick up, which is the half the screen has to be able
    /// to say out loud.
    pub async fn settle_captures(
        &self,
        user_id: i64,
        ids: &[i64],
        accept: bool,
    ) -> Result<(i64, i64), sqlx::Error> {
        if ids.is_empty() {
            return Ok((0, 0));
        }
        // Read once for the batch rather than per capture: it is one person's
        // standing answer, and it cannot change halfway through a click.
        let forwarding = accept && self.forward_spool_listens(user_id).await?;
        let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");

        let sql = format!(
            "SELECT cap.id, cap.title, cap.artist, cap.album, cap.duration, cap.played_at,
                    sp.uuid, cap.company
               FROM spool_captures cap JOIN spools sp ON sp.id = cap.spool_id
              WHERE cap.user_id = ? AND cap.state = 'pending'
                AND cap.id IN ({placeholders})"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<
            _,
            (i64, String, String, Option<String>, Option<i64>, i64, String, Option<String>),
        >(&stmt)
        .bind(user_id);
        for id in ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;

        let mut settled = 0;
        let mut forwarded = 0;
        let now = Self::now_secs();
        for (id, title, artist, album, duration, played_at, spool_uuid, company) in rows {
            if accept {
                let mut play = Play::new(
                    title,
                    artist,
                    played_at as u64,
                    format!("spool-{spool_uuid}-{id}"),
                    "Shared Spool".into(),
                );
                play.album = album;
                play.duration = duration.map(|d| d as u64);
                // `imported` files it as already-delivered, so `flush_pending`
                // leaves it alone; the freshness gate is what keeps a
                // reconstructed evening from rewriting a public history even
                // when the switch is on.
                let relay = forwarding && spool_forward_is_live(played_at, now);
                if relay {
                    forwarded += 1;
                }
                self.save_scrobble(
                    user_id,
                    &play,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("Shared Spool"),
                    None,
                    ListenDisposition { imported: !relay, skipped: false },
                )
                .await?;
                // The same two stamps the other deck's own copy got. Company
                // is the snapshot taken when the play happened — which names
                // the host who played it and excludes the person accepting —
                // rather than a fresh reading of who is in the sitting now.
                sqlx::query(&crate::sql!(
                    "UPDATE scrobbles SET joint_session_id = ?, company = COALESCE(?, company)
                      WHERE user_id = ? AND source_id = ? AND source_name = 'Shared Spool'",
                ))
                .bind(&spool_uuid)
                .bind(&company)
                .bind(user_id)
                .bind(format!("spool-{spool_uuid}-{id}"))
                .execute(&self.pool)
                .await?;
            }
            sqlx::query(&crate::sql!(
                "UPDATE spool_captures SET state = ?, settled_at = ? WHERE id = ? AND user_id = ?",
            ))
            .bind(if accept { "accepted" } else { "declined" })
            .bind(Self::now_secs())
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
            settled += 1;
        }
        Ok((settled, forwarded))
    }
    /// Whether this account wants accepted Shared Spool plays relayed onward.
    pub async fn forward_spool_listens(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let on: Option<i64> = sqlx::query_scalar(&crate::sql!(
            "SELECT forward_spool_listens FROM users WHERE id = ?"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(on.unwrap_or(0) != 0)
    }
    /// Set it.
    pub async fn set_forward_spool_listens(
        &self,
        user_id: i64,
        on: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("UPDATE users SET forward_spool_listens = ? WHERE id = ?"))
            .bind(i64::from(on))
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Make a variant the chain itself, and the chain a variant.
    ///
    /// **A relabelling, not a change.** Which gear was fitted for any given
    /// listen is identical afterwards, and so is every piece of equipment's
    /// hours — `promoting_a_variant_moves_no_hours` asserts exactly that by
    /// comparing `equipment_seconds` either side.
    ///
    /// That is the whole difficulty, and it is not obvious. An override is
    /// `{position, equipment_id}` **relative to the base**, so a second variant
    /// that swaps position 2 means "the base, but with X at 2" — change the
    /// base and that sentence names different gear at position 1, silently,
    /// with hours landing on a box that was never fitted. So every variant is
    /// resolved to an absolute list of steps *first* and re-expressed against
    /// the new base afterwards.
    ///
    /// Four things move, in an order that matters:
    ///
    /// 1. The old base becomes a variant, so listens that were on the chain
    ///    itself keep meaning what they meant.
    /// 2. Every *other* variant is re-expressed.
    /// 3. Listens, side-plays and shelf records with **no** variant are pointed
    ///    at the new old-base variant, and those on the promoted one are
    ///    pointed at nothing. In that order — the reverse would catch the rows
    ///    it had just written.
    /// 4. The chain's components become the promoted variant's, and the
    ///    promoted variant row is deleted.
    ///
    /// **Refuses a variant that removes a step.** Removal shifts every later
    /// position, so promoting one would change what every other variant's
    /// overrides point at. Nothing in the UI creates one — the swap dialog
    /// always picks gear — so this costs nothing today and is a refusal rather
    /// than a silent wrong answer if that ever changes.
    pub async fn promote_variant(
        &self,
        user_id: i64,
        variant_id: i64,
        old_base_name: &str,
    ) -> Result<i64, sqlx::Error> {
        let variants = self.list_variants(user_id).await?;
        let Some(promoted) = variants.iter().find(|v| v.id == variant_id) else {
            return Err(sqlx::Error::RowNotFound);
        };
        let chain_id = promoted.chain_id;

        let chains = self.get_chains(user_id).await?;
        let Some(chain) = chains.iter().find(|c| c.id == Some(chain_id)) else {
            return Err(sqlx::Error::RowNotFound);
        };
        let base = chain.components.clone();

        if promoted.overrides.iter().any(|o| {
            o.equipment_id.is_none() && o.name.is_none() && (o.position as usize) < base.len()
        }) {
            return Err(sqlx::Error::Protocol(
                "that setup removes a step, which cannot become the chain: every other setup's \
                 positions would shift"
                    .into(),
            ));
        }

        let new_base = crate::models::resolve_variant(&base, &promoted.overrides);
        if new_base.is_empty() {
            return Err(sqlx::Error::Protocol("that setup has no steps".into()));
        }

        // Resolved *before* anything is written, against the base as it stands.
        let siblings: Vec<(i64, Vec<crate::models::ChainComponent>)> = variants
            .iter()
            .filter(|v| v.chain_id == chain_id && v.id != variant_id)
            .map(|v| (v.id, crate::models::resolve_variant(&base, &v.overrides)))
            .collect();

        let now = Self::now_secs();
        let mut tx = self.pool.begin().await?;

        // 1. The old base, as a variant that restores it.
        let old_overrides = crate::models::overrides_against(&new_base, &base);
        let old_json = serde_json::to_string(&old_overrides).unwrap_or_else(|_| "[]".into());
        sqlx::query(&crate::sql!(
            "INSERT INTO chain_variants (user_id, chain_id, name, overrides, created_at)
             VALUES (?, ?, ?, ?, ?)",
        ))
        .bind(user_id)
        .bind(chain_id)
        .bind(old_base_name)
        .bind(&old_json)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let old_variant_id: i64 = sqlx::query_scalar(&crate::sql!(
            "SELECT id FROM chain_variants WHERE user_id = ? AND chain_id = ? AND created_at = ?
              ORDER BY id DESC LIMIT 1",
        ))
        .bind(user_id)
        .bind(chain_id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        // 2. Every other variant, re-expressed so it names the gear it always
        //    named.
        for (id, resolved) in &siblings {
            let json =
                serde_json::to_string(&crate::models::overrides_against(&new_base, resolved))
                    .unwrap_or_else(|_| "[]".into());
            sqlx::query(&crate::sql!(
                "UPDATE chain_variants SET overrides = ? WHERE id = ? AND user_id = ?",
            ))
            .bind(json)
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }

        // 3. Repoint what was recorded. **Null first**, or the second statement
        //    would catch the rows the first just wrote.
        for table in ["scrobbles", "physical_plays", "physical_releases"] {
            sqlx::query(&crate::sql!(&format!(
                "UPDATE {table} SET chain_variant_id = ?
                  WHERE user_id = ? AND chain_id = ? AND chain_variant_id IS NULL"
            )))
            .bind(old_variant_id)
            .bind(user_id)
            .bind(chain_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(&crate::sql!(&format!(
                "UPDATE {table} SET chain_variant_id = NULL
                  WHERE user_id = ? AND chain_variant_id = ?"
            )))
            .bind(user_id)
            .bind(variant_id)
            .execute(&mut *tx)
            .await?;
        }

        // 4. The chain becomes what the variant described, and the variant row
        //    goes — it is the chain now.
        sqlx::query(&crate::sql!(
            "UPDATE signal_chains SET components = ? WHERE id = ? AND user_id = ?"
        ))
        .bind(serde_json::to_string(&new_base).unwrap_or_else(|_| "[]".into()))
        .bind(chain_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(&crate::sql!("DELETE FROM chain_variants WHERE id = ? AND user_id = ?"))
            .bind(variant_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(old_variant_id)
    }
}
