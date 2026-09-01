//! Backfills that fill in metadata the ingest path doesn't have: lyrics
//! language, and release year for the by-decade breakdown. Both are bounded per
//! call so the MusicBrainz rate limit is respected, and both are driven by the
//! engine's periodic tick.
//!
//! Language backfill: classify listens that don't have a language yet.
//!
//! Script detection is free and applied to everything; the MusicBrainz
//! work-language lookup is bounded per call (`mb_budget`) so we respect the API
//! rate limit. Shared by the engine's periodic tick and the manual "enrich now"
//! endpoint.

use crate::db::Database;
use crate::language::{script_language, DEFAULT_LANGUAGE};
use crate::metadata::ArtworkProvider;
use crate::models::Play;
use crate::musicbrainz::MusicBrainzClient;
use tracing::debug;

/// Classify up to `batch` un-languaged listens, using at most `mb_budget`
/// MusicBrainz lookups. Returns how many were classified this call. Rows that
/// need MusicBrainz but exceed the budget are left for the next call.
pub async fn backfill_languages(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_language(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, album, mbid) in rows {
        // Tier 1: script detection (instant, non-Latin).
        if let Some(lang) = script_language(&title, album.as_deref()) {
            if db.set_scrobble_language(id, lang, "script").await.is_ok() {
                done += 1;
            }
            continue;
        }
        // Tier 2: MusicBrainz work-language (bounded), else default.
        match mbid.filter(|m| !m.is_empty()) {
            Some(m) if budget > 0 => {
                budget -= 1;
                let lang = mb
                    .fetch_recording_language(&m)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string());
                if db.set_scrobble_language(id, &lang, "work").await.is_ok() {
                    done += 1;
                }
            }
            Some(_) => { /* has an MBID but budget spent — leave for the next pass */ }
            None => {
                if db.set_scrobble_language(id, DEFAULT_LANGUAGE, "default").await.is_ok() {
                    done += 1;
                }
            }
        }
    }
    done
}

/// Ask LRCLIB about tracks whose language is still unresolved, and take the
/// answer from the script of the lyric.
///
/// **The tier between MusicBrainz's work language and the shrug.** `script_language`
/// settles any title written in a non-Latin script and gives up on Latin text,
/// so a *transliterated* title falls all the way through to `DEFAULT_LANGUAGE`
/// — 45% of a real listening window, drawn on the page next to real categories.
/// The lyric is where that title's own script went.
///
/// Three things this returns that nothing else could:
///
/// * **A language for a romanised title.** Measured against the live API:
///   "Heize — Star", romanised in both fields, comes back 504 Hangul characters
///   to 69 Latin.
/// * **`Instrumental` as a real answer.** A track with no words has no
///   language, which the vocabulary could not previously say — every listen was
///   either classified or shrugged at.
/// * **Nothing at all, honestly.** A Latin lyric leaves the row exactly as it
///   was rather than asserting English, because telling French from English is
///   a different problem needing a different tool.
///
/// Budgeted per track rather than per listen: a lyric is a property of the
/// song, so a track played fifty times is one request. A 429 **stops the pass**
/// — the docs say a client that ignores `Retry-After` may be banned, and that
/// would cost every Tapedeck instance the feature rather than only this one.
/// Fetch lyrics for the tracks whose language is still a shrug.
///
/// **Two tiers, and the user's own library goes first.** They tag their own
/// albums, so their words are the right pressing with their own corrections in
/// them — the vision rule that the user's own library beats any provider, in
/// the one place where the provider is a free service being asked nicely.
/// A library hit costs no LRCLIB request and no politeness budget, so a
/// well-tagged library makes this pass nearly free.
///
/// **The one exception is timing** (Ashwin's call, 2026-08-31): a tagged lyric
/// is often plain text where LRCLIB has a timed one, and only a timed lyric can
/// follow the deck in the reader. So a library lyric that is *already timed*
/// settles it outright; a plain one still asks LRCLIB, and loses only if LRCLIB
/// has cues. A person's own correction beats both, which `remember_library`
/// and `remember` both enforce by refusing a row holding `edited`.
pub async fn backfill_lyrics(
    db: &Database,
    lyrics: &crate::lyrics::LyricsClient,
    budget: i64,
) -> usize {
    // More candidates than budget, because most of the head of this queue is
    // already cached — a miss is remembered, and re-reading it costs no request.
    let candidates = match db.tracks_wanting_language(budget * 20).await {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // Where each of them sits in the user's own servers: one bounded scan for
    // the whole batch rather than a lookup per track.
    let keys: Vec<String> = candidates.iter().map(|c| c.key.clone()).collect();
    let refs = db.library_refs_for_tracks(&keys).await.unwrap_or_default();
    let mut servers = LibraryClients::default();

    let mut spent = 0i64;
    let mut done = 0usize;
    for cand in candidates {
        let crate::db::LanguageCandidate { key, title, artist, album, duration, .. } = cand;

        // Already answered — for or against — costs nothing to re-read.
        if let Some(f) = lyrics.cached(&artist, &title).await {
            if let Some(lang) = language_from(&f) {
                if db.set_track_language(&artist, &title, &lang, "lyrics").await.is_ok() {
                    done += 1;
                }
            }
            continue;
        }

        // Both tiers, in `resolve_lyrics` — the one copy of the ordering rule.
        // `spent >= budget` withholds only the *LRCLIB* half: their own library
        // costs nothing and is asked either way.
        let out = resolve_lyrics(
            db,
            lyrics,
            &mut servers,
            refs.get(&key),
            &artist,
            &title,
            album.as_deref(),
            duration,
            spent < budget,
        )
        .await;
        if out.asked {
            spent += 1;
        }
        if out.language_set {
            done += 1;
        }
        match out.resolved {
            Resolved::RateLimited(secs) => {
                // Stop, rather than sleep through it holding the budget open.
                // The next tick is a minute away.
                tracing::warn!(
                    "🎤 LRCLIB asked us to wait {secs}s — pausing lyrics until the next pass"
                );
                break;
            }
            // Nothing from the library and no budget left to ask LRCLIB: every
            // candidate after this one is in the same position, so stop.
            Resolved::Nothing | Resolved::Unreachable if spent >= budget => break,
            _ => {}
        }
    }
    done
}

/// What one track's lyric lookup concluded.
///
/// **The last two are the distinction this whole enum exists for.** *Asked, and
/// there is nothing* is a fact about the track; *could not ask* is a fact about
/// the request, and reporting the second as the first tells somebody their song
/// has no words when nobody ever managed to look. The same line
/// `/physical/:id/durations` draws, and the reason `unavailable` is a field
/// rather than a 5xx.
pub enum Resolved {
    /// Settled — this is what is now stored.
    Words(crate::lyrics::Found),
    /// LRCLIB asked us to wait. The backfill stops the pass rather than
    /// sleeping through it; a handler reports it and asks the caller to retry.
    RateLimited(u64),
    /// LRCLIB could not be reached at all. Nothing was learned and nothing was
    /// cached — in particular no miss, which would retire the track for a month
    /// on the strength of a network blip.
    Unreachable,
    /// Asked, and nothing came back.
    Nothing,
}

/// The result of one lookup, and what it cost.
pub struct LyricOutcome {
    pub resolved: Resolved,
    /// Whether an **LRCLIB** request was actually made — the thing a budget
    /// paces. A library hit costs nothing and is not counted here.
    pub asked: bool,
    /// Whether a language was concluded and written to the listens.
    pub language_set: bool,
}

/// Look one track up across both tiers, and store whatever comes back.
///
/// **This is the only copy of the ordering rule**, which is why it is a function
/// rather than a shape repeated per caller: the backfill, the reader's page
/// load and the *Look for the words* button all resolve a track the same way,
/// and three copies of "a plain library lyric still asks LRCLIB, and loses only
/// to cues" would drift. It has already been subtle enough to be worth a
/// paragraph of its own — see [`backfill_lyrics`].
///
/// `ask_lrclib` withholds the second tier only. Their own library has no rate
/// limit and no shared budget, so it is asked whatever the answer.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_lyrics(
    db: &Database,
    lyrics: &crate::lyrics::LyricsClient,
    servers: &mut LibraryClients,
    lib_ref: Option<&crate::db::LibraryRef>,
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration: Option<i64>,
    ask_lrclib: bool,
) -> LyricOutcome {
    use crate::lyrics::Found;

    // Settling: a lookup that concluded a language writes it to the listens.
    // Done here rather than at each call site because every caller did it
    // identically, and one that forgot would leave a track with words and no
    // language — which looks exactly like a lyric that was never fetched.
    let settle = |found: Found, asked: bool| async move {
        let language_set = match language_from(&found) {
            Some(lang) => db.set_track_language(artist, title, &lang, "lyrics").await.is_ok(),
            None => false,
        };
        LyricOutcome { resolved: Resolved::Words(found), asked, language_set }
    };

    // Tier one: their own library. No budget, no rate limit.
    let mut from_library: Option<Found> = None;
    let mut label = "Library";
    if let Some(r) = lib_ref {
        if let Some((plain, synced)) = servers.fetch(db, r).await {
            let timed = synced.is_some();
            label = library_label(&r.source_name);
            match lyrics.remember_library(artist, title, plain, synced, label).await {
                Ok(f) => {
                    // A timed lyric of their own settles it: nothing LRCLIB
                    // could return would be preferred, so it is not asked.
                    if timed {
                        return settle(f, false).await;
                    }
                    from_library = Some(f);
                }
                Err(e) => tracing::debug!("🎤 could not store library lyrics: {e}"),
            }
        }
    }

    if !ask_lrclib {
        // Their plain words are already stored; stopping here loses nothing but
        // the chance of finding cues, which a later pass can still do.
        return match from_library {
            Some(f) => settle(f, false).await,
            None => LyricOutcome { resolved: Resolved::Nothing, asked: false, language_set: false },
        };
    }

    // Tier two: LRCLIB, which is the rate-limited one and therefore the one the
    // budget is for.
    let found = match lyrics.fetch(artist, title, album, duration).await {
        Ok(f) => f,
        Err(crate::lyrics::LyricsError::RateLimited(secs)) => {
            return LyricOutcome {
                resolved: Resolved::RateLimited(secs),
                asked: true,
                language_set: false,
            };
        }
        Err(e) => {
            tracing::debug!("🎤 lyrics lookup failed for {artist} — {title}: {e}");
            // Their plain words still stand; `fetch` wrote nothing.
            return match from_library {
                Some(f) => settle(f, true).await,
                None => LyricOutcome {
                    resolved: Resolved::Unreachable,
                    asked: true,
                    language_set: false,
                },
            };
        }
    };

    // **The whole of the ordering rule, in one condition.** LRCLIB only
    // displaces their own words when it brings cues they did not have.
    let resolved = match (&from_library, &found) {
        (Some(_), Found::Lyrics { synced: Some(_), .. }) => found,
        (Some(lib), _) => {
            // Nothing better came back. Restore their words, which `fetch` has
            // just written over with a worse answer.
            let (plain, synced) = match lib {
                Found::Lyrics { plain, synced, .. } => (plain.clone(), synced.clone()),
                _ => (None, None),
            };
            let _ = lyrics.remember_library(artist, title, plain, synced, label).await;
            lib.clone()
        }
        (None, _) => found,
    };
    settle(resolved, true).await
}

/// Fetch one track's lyrics from the user's own library, if it is in one.
///
/// The on-demand half of tier one: the backfill fills the cache in the
/// background, and this covers a track somebody opens the reader on before the
/// trickle has reached it. Their own server has none of the constraints that
/// make LRCLIB a trickle — no rate limit, no politeness budget, no shared
/// service to be considerate of — so asking it on a page load is fair where
/// asking LRCLIB would not be. **That is the whole reason `ask_lrclib` is
/// false here**, and the line between this and [`lyrics_now`], which a person
/// asks for by pressing a button.
///
/// Returns whether anything was stored, so the caller can re-read the cache.
pub async fn lyrics_from_library_now(
    db: &Database,
    lyrics: &crate::lyrics::LyricsClient,
    artist: &str,
    title: &str,
) -> bool {
    let Ok(Some(r)) = db.library_ref_for_track(title, artist).await else {
        return false;
    };
    let mut servers = LibraryClients::default();
    let out =
        resolve_lyrics(db, lyrics, &mut servers, Some(&r), artist, title, None, None, false).await;
    matches!(out.resolved, Resolved::Words(_))
}

/// Look for one track's words **now**, LRCLIB included, because somebody asked.
///
/// The backfill's pace — eight tracks a minute, most-played first — is right for
/// a queue running on its own and wrong for the one track a person is looking
/// at, which may be hours down it. This is the button behind that.
///
/// **A deliberate press is not a page load, and that is the whole argument for
/// asking LRCLIB here.** The reader's own load path stays library-only for
/// exactly that reason: a refresh, a bot or a second tab must not be able to
/// drive requests at a free service. What bounds this one is structural rather
/// than a policy nobody enforces — [`crate::lyrics::LyricsClient::pace`] spaces
/// every outbound call across callers, so a held-down button spends requests at
/// the rate the docs sanction rather than as fast as it can be clicked.
///
/// It **bypasses the miss tombstone**, which is what "force" means: LRCLIB fills
/// its own gaps, so "nobody had this a fortnight ago" is exactly the answer a
/// person is entitled to re-ask. It does **not** displace words already held —
/// there is nothing to force there, and *Pick a different match* is the control
/// for that, which already exists and does not spend a fetch on the answer it
/// already has.
pub async fn lyrics_now(
    db: &Database,
    lyrics: &crate::lyrics::LyricsClient,
    artist: &str,
    title: &str,
    album: Option<&str>,
    duration: Option<i64>,
) -> LyricOutcome {
    let lib = db.library_ref_for_track(title, artist).await.ok().flatten();
    let mut servers = LibraryClients::default();
    resolve_lyrics(db, lyrics, &mut servers, lib.as_ref(), artist, title, album, duration, true)
        .await
}

/// The language a lookup concluded, or `None` when it concluded nothing.
///
/// No language is not a failure and not a reason to write anything: the row
/// keeps whatever it had, and the MusicBrainz tier or the default still gets
/// its turn.
fn language_from(found: &crate::lyrics::Found) -> Option<String> {
    match found {
        crate::lyrics::Found::Instrumental => Some(crate::language::INSTRUMENTAL.to_string()),
        crate::lyrics::Found::Lyrics { language, .. } => language.clone(),
        crate::lyrics::Found::Missing => None,
    }
}

/// What to call a server's lyrics in the reader. Their name, not "Library" —
/// "your Plex" is a different statement from "LRCLIB" and worth making.
fn library_label(source_name: &str) -> &'static str {
    match source_name.to_ascii_lowercase().as_str() {
        "plex" => "Plex",
        "jellyfin" => "Jellyfin",
        "emby" => "Emby",
        _ => "Library",
    }
}

/// Media-server clients built once per pass and reused.
///
/// Several tracks in a batch usually come off the *same* server, and building a
/// client means reading and decrypting its credentials. A source that has gone
/// away is remembered as absent so it is not looked up again on every track.
#[derive(Default)]
pub struct LibraryClients {
    seen: std::collections::HashMap<i64, Option<(String, String, String)>>,
}

impl LibraryClients {
    /// `(kind, url, credential)` for a source, or `None` if it cannot be used.
    async fn creds(
        &mut self,
        db: &Database,
        source_ref: i64,
        user_id: i64,
    ) -> Option<&(String, String, String)> {
        if !self.seen.contains_key(&source_ref) {
            let got = match db.get_source(user_id, source_ref).await {
                Ok(Some(src)) if src.enabled => {
                    let kind = src.kind.to_ascii_lowercase();
                    let cred = match kind.as_str() {
                        "jellyfin" | "emby" => src.credential("api_key"),
                        _ => src.credential("token"),
                    };
                    cred.map(|c| (kind, src.url.clone(), c))
                }
                _ => None,
            };
            self.seen.insert(source_ref, got);
        }
        self.seen.get(&source_ref).and_then(|o| o.as_ref())
    }

    async fn fetch(
        &mut self,
        db: &Database,
        r: &crate::db::LibraryRef,
    ) -> Option<(Option<String>, Option<String>)> {
        let item = crate::server::artwork::remote_item_id(&r.source_name, &r.source_id)?;
        let (kind, url, cred) = self.creds(db, r.source_ref, r.user_id).await?.clone();
        let got = match kind.as_str() {
            "jellyfin" => {
                crate::sources::mediabrowser::MediaBrowserSource::jellyfin(url, cred, String::new())
                    .fetch_lyrics(&item)
                    .await
            }
            "emby" => {
                crate::sources::mediabrowser::MediaBrowserSource::emby(url, cred, String::new())
                    .fetch_lyrics(&item)
                    .await
            }
            "plex" => {
                crate::sources::plex::PlexSource::with_filters(
                    url,
                    cred,
                    crate::sources::plex::PlexFilters::default(),
                )
                .fetch_lyrics(&item)
                .await
            }
            _ => return None,
        };
        match got {
            Ok(v) => v,
            Err(e) => {
                // A server that is off is an ordinary state, not an incident.
                tracing::debug!("🎤 library lyrics unavailable from {}: {e}", r.source_name);
                None
            }
        }
    }
}

/// Resolve release years for up to `batch` listens, using at most `mb_budget`
/// MusicBrainz lookups. `fetch_metadata` checks its own SQLite cache first, so
/// a listen whose metadata was already fetched costs nothing.
///
/// A listen with no known release date is stored as year `0` rather than left
/// NULL — otherwise every pass would retry the same unresolvable rows forever
/// and never reach the rest. The decade query filters those out.
pub async fn backfill_release_years(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_release_year(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album) in rows {
        if budget <= 0 {
            break;
        }
        budget -= 1;

        let year = match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
            Ok(meta) => meta.release_date.as_deref().and_then(parse_year).unwrap_or(0),
            Err(_) => 0,
        };
        if db.set_scrobble_release_year(id, year).await.is_ok() {
            done += 1;
        }
    }
    done
}

/// Fill in the ISRC for listens that have none.
///
/// Keyed on the **recording MBID**, so this only reaches listens MusicBrainz has
/// already identified — 90% of a real history, and the ones a playlist push is
/// most likely to want. A listen with no recording MBID is left alone rather
/// than name-searched: the other backfills already resolve those, and this pass
/// will pick them up once they have one.
///
/// One request per *recording*, not per listen, and cached — so a track played
/// fifty times costs one lookup and one write that covers all fifty rows.
///
/// `''` is written when MusicBrainz has no ISRC for the recording, which is a
/// real answer and a common one. Without that tombstone the row would be
/// re-queued forever against a rate-limited service.
///
/// One resolution updates **every** listen of the same track, because a song
/// played fifty times is fifty rows sharing one recording and one ISRC.
/// Give imported listens a recording MBID.
///
/// **The gap this closes.** A recording MBID is written in one place —
/// `pipeline::flush_pending` — and that only ever sees `status = 'pending'`
/// listens on their way to a sink. An imported listen is `'imported'`, so it
/// never got one, and nothing else wrote the column. On a 92%-imported history
/// that left most of the listening with no MBID *permanently* — and every other
/// backfill that keys off a recording (ISRC, work-language, release year) is
/// therefore excluded from it. This is the one that unblocks the others, which
/// is why it should run before them rather than beside them.
///
/// **A wrong MBID is worse than none, so a match is checked.** `fetch_metadata`
/// searches by name with `limit=1` against a ranking MusicBrainz does not
/// promise is stable — the trap already recorded for recording-level tags. An
/// unchecked result would write a confident wrong id onto every play of the
/// track and then poison the ISRC and language lookups downstream, none of
/// which would error. So the answer has to *agree* with the question:
/// `artist_matches` on the artist, and the same fold on the title after the
/// trailing qualifier is stripped, since a search for "Xtal" legitimately
/// answers "Xtal (Remastered)".
///
/// A miss costs nothing twice: `fetch_metadata` keeps its own negative cache,
/// and `tracks_needing_recording_mbid` reads it so a batch is not spent on
/// tracks that have already been asked about.
pub async fn backfill_recording_mbids(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    // Over-fetch, then drop the ones already asked about. That filter cannot
    // be a SQL join: `musicbrainz_cache` lives in the *client's* database
    // (`SQLITE_DB_PATH`), not the one holding the listens, so the two sets can
    // only meet in Rust. Without it every batch would be spent re-asking the
    // tracks MusicBrainz has already said it does not have, and the ones it
    // could answer would never come up.
    let rows = match db.tracks_needing_recording_mbid(batch * 4).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if rows.is_empty() {
        return 0;
    }
    let asked = mb.negative_track_keys().await;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|(t, a, _)| !asked.contains(&(t.trim().to_lowercase(), a.trim().to_lowercase())))
        .take(batch as usize)
        .collect();

    let mut done = 0usize;
    let mut budget = mb_budget;

    for (title, artist, album) in rows {
        if budget == 0 {
            break;
        }
        budget -= 1;
        // A carrier for the lookup, not a listen: nothing here is ever stored
        // as a play. `enrich_play` reads the title and artist and writes the
        // ids back onto it.
        let mut play =
            Play::new(title.clone(), artist.clone(), 0, String::new(), "mbid-backfill".to_string());
        if !crate::engine::enrichment::enrich_play(mb, &mut play, album.as_deref()).await {
            continue;
        }
        let Some(found) = play.mbid_recording.as_deref() else { continue };
        if found.trim().is_empty() {
            continue;
        }
        if !answers_the_question(&title, &artist, &play) {
            debug!(
                "MBID backfill: refused {} — {} (MusicBrainz answered a different track)",
                artist, title
            );
            continue;
        }
        match db.set_recording_mbid_for_track(&title, &artist, &play).await {
            Ok(n) if n > 0 => done += 1,
            _ => {}
        }
    }
    done
}

/// Does the answer describe the track that was asked about?
///
/// The gate is on the *names MusicBrainz returned*, which on a fresh lookup are
/// its own canonical spellings rather than the query. `artist_matches` is the
/// codebase's single answer to "same performer?"; the title is folded with
/// `normalise_artist`, which is the same question asked of a different field —
/// the reuse `playlists::pick` already makes — and the trailing qualifier is
/// stripped because a search for "Xtal" answering "Xtal (Remastered 2008)" is
/// the *right* recording under a longer name.
fn answers_the_question(title: &str, artist: &str, play: &Play) -> bool {
    if !crate::db::artist_matches(&play.artist, artist) {
        return false;
    }
    let fold = |t: &str| crate::db::normalise_artist(crate::playlists::without_qualifier(t));
    let (want, got) = (fold(title), fold(&play.title));
    !want.is_empty() && (want == got || got.starts_with(&want) || want.starts_with(&got))
}

pub async fn backfill_isrc(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_isrc(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (mbid, title, artist) in rows {
        if budget <= 0 {
            break;
        }
        budget -= 1;

        let isrc = match mb.fetch_recording_isrcs(&mbid).await {
            // The first is as good as any: several ISRCs on one recording are
            // the same performance issued in different territories, and each
            // one identifies it.
            Ok(list) => list.first().cloned().unwrap_or_default(),
            // A failed request is not an answer. Leaving the row NULL means it
            // is asked again next pass, where writing the tombstone would
            // retire it permanently on the strength of a network blip.
            Err(_) => continue,
        };
        if let Ok(n) = db.set_scrobble_isrc(&title, &artist, &isrc).await {
            if n > 0 {
                done += 1;
            }
        }
    }
    done
}

/// Fill in genres for listens that have none.
///
/// Costs nothing new: `fetch_metadata` has always returned genres and the cache
/// has always stored them — they were simply never written onto the listen, so
/// a history that had been fully enriched already has every answer sitting in
/// `mb_cache` and this pass resolves it without a single API call.
///
/// A listen MusicBrainz has no tags for is recorded as `''` rather than left
/// NULL, so it isn't re-asked forever — the same tombstone `artwork_url` and
/// `caa_id = -1` use.
pub async fn backfill_genres(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_genres(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album) in rows {
        if budget <= 0 {
            break;
        }

        // The recording's own tags are the precise answer when they exist, and
        // cost nothing on an enriched history — this is the same cached lookup
        // the language and release-year passes already made.
        let mut genres = match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
            Ok(meta) => meta.genres,
            Err(_) => Vec::new(),
        };

        // Most recordings have none. Fall back to the artist's tags, which are
        // both better populated and stable — see `fetch_artist_tags`. Cached
        // per artist, so a library of one artist's records costs one lookup,
        // not one per listen, and only a real API call spends budget.
        if genres.is_empty() {
            genres = match db.artist_genres(&artist).await {
                Ok(Some(cached)) => cached,
                _ => {
                    budget -= 1;
                    let fetched = mb.fetch_artist_tags(&artist).await.unwrap_or_default();
                    // Written even when empty, so an artist MusicBrainz has
                    // nothing for isn't re-asked on every pass.
                    let _ = db.set_artist_genres(&artist, &fetched).await;
                    fetched
                }
            };
        }

        if db.set_scrobble_genres(id, &genres).await.is_ok() {
            done += 1;
        }
    }
    done
}

/// MusicBrainz dates are `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
fn parse_year(date: &str) -> Option<i32> {
    let y: i32 = date.get(..4)?.parse().ok()?;
    (1000..=2999).contains(&y).then_some(y)
}

/// Fill in cover art for listens that have none, cheapest provider first.
///
/// Deezer is tried before MusicBrainz because it needs no auth and imposes no
/// one-per-second limit, so a large backfill finishes in minutes rather than
/// hours — and every hit is one fewer request aimed at a free public service.
/// MusicBrainz/Cover Art Archive remains the fallback for everything Deezer
/// doesn't carry, which is most of the obscure end of a collection.
///
/// A listen with nothing anywhere is tombstoned as `caa_id = -1` so later passes
/// move on instead of re-asking forever.
pub async fn backfill_artwork(
    db: &Database,
    mb: &MusicBrainzClient,
    providers: &[Box<dyn ArtworkProvider>],
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_artwork(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album, caa_id, caa_mbid, source_name) in rows {
        // 0a. Came from the user's own media server — the proxy can serve the
        //     cover of the exact file that was played, with no lookup at all and
        //     nothing asked of a third party. Always the best answer available.
        // ponytail: written without checking the image resolves, and
        // `artwork_url` is this queue — so a library item with genuinely no
        // cover is stuck here and never reaches Deezer or MusicBrainz. Verify by
        // fetching once (or have the proxy's 404 path re-queue the row) if that
        // turns out to be common; media servers almost always have album art.
        if matches!(source_name.as_str(), "navidrome" | "Plex" | "plex" | "jellyfin" | "Jellyfin") {
            let url = format!("/api/v1/art/{id}");
            if db.set_scrobble_artwork(id, &url, None, None).await.is_ok() {
                done += 1;
                continue;
            }
        }

        // 0b. Already have a Cover Art Archive id from earlier enrichment — build
        //    the URL from it rather than asking anyone anything.
        if let (Some(cid), Some(mbid)) = (caa_id.filter(|c| *c > 0), caa_mbid.as_deref()) {
            let url = caa_url(mbid, cid);
            if db.set_scrobble_artwork(id, &url, None, None).await.is_ok() {
                done += 1;
            }
            continue;
        }

        // 1. Providers that cost MusicBrainz nothing (Deezer today; the user's
        //    own library once Phase 4 finishes).
        if let Some(found) =
            crate::metadata::resolve(providers, &artist, &title, album.as_deref()).await
        {
            // Providers hand back an artist portrait alongside the cover; it has
            // nowhere to live on a listen, so it goes to the artist entity.
            if let Some(ref portrait) = found.artist_image_url {
                let _ = db.upsert_artist(&artist, Some(portrait), None).await;
            }
            if let Some(url) = found.image_url {
                if db.set_scrobble_artwork(id, &url, None, None).await.is_ok() {
                    done += 1;
                    continue;
                }
            }
        }

        // 2. MusicBrainz + Cover Art Archive, bounded by the rate limit.
        if budget <= 0 {
            break;
        }
        budget -= 1;

        let (found_id, found_mbid) =
            match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
                Ok(meta) => (meta.caa_id, meta.caa_release_mbid),
                Err(_) => (None, None),
            };
        // Empty string tombstones "looked everywhere, nothing there".
        let url = match (found_id.filter(|c| *c > 0), found_mbid.as_deref()) {
            (Some(cid), Some(mbid)) => caa_url(mbid, cid),
            _ => String::new(),
        };
        if db.set_scrobble_artwork(id, &url, found_id, found_mbid.as_deref()).await.is_ok() {
            done += 1;
        }
    }
    done
}

fn caa_url(release_mbid: &str, caa_id: i64) -> String {
    format!("https://coverartarchive.org/release/{release_mbid}/{caa_id}-250.jpg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(title: &str, artist: &str) -> Play {
        let mut p = Play::new(title.into(), artist.into(), 0, String::new(), "t".into());
        p.mbid_recording = Some("some-mbid".into());
        p
    }

    /// The gate on what the MBID backfill is allowed to write.
    ///
    /// `fetch_metadata` searches by name with `limit=1` against a ranking
    /// MusicBrainz does not promise is stable. Accepting whatever comes back
    /// would write a confident wrong id onto **every play** of a track and then
    /// poison the ISRC and work-language lookups that key off it — none of
    /// which would error. The answer has to agree with the question.
    #[test]
    fn a_backfilled_mbid_has_to_answer_the_question_that_was_asked() {
        // The ordinary case.
        assert!(answers_the_question("Xtal", "Aphex Twin", &answered("Xtal", "Aphex Twin")));

        // A longer name for the same recording is the *right* answer: a search
        // for "Xtal" legitimately comes back with the remaster.
        assert!(answers_the_question(
            "Xtal",
            "Aphex Twin",
            &answered("Xtal (Remastered 2008)", "Aphex Twin")
        ));

        // Spelling and punctuation fold, the way they do everywhere else.
        assert!(answers_the_question(
            "Coeur de pirate",
            "Coeur de pirate",
            &answered("Cœur de pirate", "Cœur de pirate")
        ));

        // A different song by the right artist is the failure that matters —
        // it looks entirely plausible and is wrong on every play of the track.
        assert!(!answers_the_question("Xtal", "Aphex Twin", &answered("Ageispolis", "Aphex Twin")));

        // And a different artist, which is what a fuzzy search returns when it
        // has nothing: the trap `find_artist` already documents.
        assert!(!answers_the_question("Xtal", "Aphex Twin", &answered("Xtal", "Arkells")));

        // Nothing to compare against is a refusal, not a pass.
        assert!(!answers_the_question("", "Aphex Twin", &answered("Xtal", "Aphex Twin")));
    }
}
