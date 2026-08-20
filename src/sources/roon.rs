//! Roon.
//!
//! Structurally a *server*, not a device: one Core owns N zones, so this is
//! closer to Plex — a server whose sessions you filter — than to a renderer you
//! discover and poll. It therefore sits outside Phase 10's milestones and gets
//! its own `user_sources` kind.
//!
//! # What Roon gives, and what it does not
//!
//! The complete documented `now_playing` schema is `seek_position`, `length`,
//! `image_key`, `one_line`, `two_line`, `three_line`. That is the whole of it,
//! and two consequences follow that are worth stating plainly before anyone
//! plans around this source:
//!
//! * **There is no signal path.** Roon computes the Lossless / High Quality /
//!   Enhanced verdict and every stage behind it, and keeps all of it behind the
//!   purple light in its own client. It is not in the extension API and has
//!   been requested on their forums for years. A Roon listen resolves its chain
//!   through the ladder's fourth rung — the device's default — like any other.
//! * **There are no MBIDs and no structured metadata**, only *rendered display
//!   strings*. So a Roon listen carries **worse** metadata than a Plex or
//!   Jellyfin one and pays for a MusicBrainz lookup it could have skipped,
//!   which is the opposite of what anyone expects from Roon.
//!
//! What it does give is `seek_position` and `length`, which are a real
//! playhead — so the scrobble threshold and the repeat gates work exactly as
//! they do for Plex's `view_offset`, and [`SessionTracker`] is shared verbatim.
//! And it gives coverage of **RAAT**, a transport nothing else can see.
//!
//! # Poll, with the subscription as a wakeup
//!
//! Roon's protocol is push (`subscribe_zones`), and Tapedeck's engine is a
//! poller. This source is the hybrid: **state is always read from
//! `get_zones()`** on the engine tick, and the subscription is used *only* to
//! ring a bell so the tick happens sooner.
//!
//! That split is deliberate and both halves matter:
//!
//! * `ZoneEvent::Seeked` carries [`roon_api::ZoneSeek`] — `zone_id` and a seek
//!   position with **no `now_playing`**. So the playhead, the one field every
//!   gate reads, arrives detached from the track it belongs to. A push-only
//!   source would have to hold its own zone map and merge deltas into it; a
//!   poll gets complete state every time and merges nothing.
//! * The subscription pump has no reconnect of its own — when the websocket
//!   drops, the receiver simply stops yielding, with no error. Here that
//!   degrades to *exactly the poll design*: latency, and nothing else. That is
//!   the whole reason the hybrid is safe where push alone would not be.
//!
//! [`roon_api::RoonClient::connect`] does spawn a reconnection manager, and a
//! re-pair arrives as a fresh [`RoonEvent::CorePaired`] — which is why
//! `drain_events` runs at the top of every poll rather than the `Core` being
//! held forever.
//!
//! # NOT TESTED AGAINST A REAL ROON CORE
//!
//! As of v0.60.0 nothing here has run against a Core. The field shapes come
//! from the `roon-api` crate's declared types (v0.5.3) and the vendored
//! `temp/node-roon-api-master` docs, which are the authoritative field lists —
//! but the mapping in [`RoonSource::to_play`] is *guesswork by construction*
//! and is the part most likely to be wrong. See its own note.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use roon_api::token::PersistedState;
use roon_api::{Core, PlayState, RoonClient, RoonClientBuilder, RoonEvent, StateStore, Zone};
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::models::Play;
use crate::sources::mediabrowser::SessionTracker;

/// Identifies this extension to the Core. The user approves it by this name in
/// Roon Settings → Extensions, so it must be recognisable and must not change
/// between versions — a new id reads as a different extension and asks for
/// approval again.
const EXTENSION_ID: &str = "org.tapedeck.scrobbler";

/// A Core reached over the LAN. Roon's own default.
const DEFAULT_PORT: u16 = 9330;

/// What Roon calls a track when it has nothing better.
const UNKNOWN: &str = "Unknown";

/// The token and paired-core id Roon hands back after the user approves the
/// extension, held in memory and mirrored into `user_sources.credentials`.
///
/// [`StateStore`] is a **synchronous** trait and Tapedeck's credential store is
/// async and encrypted, so it cannot be implemented directly against the
/// database. Instead this is seeded from the stored value at build time and
/// raises `dirty` when Roon writes a new one; the source drains that after
/// connecting and persists it properly. Losing the token is not fatal — it
/// means the user re-approves in Roon — so a missed write costs a prompt
/// rather than data.
#[derive(Default)]
pub struct PairingStore {
    state: Mutex<PersistedState>,
    dirty: AtomicBool,
}

impl PairingStore {
    pub fn seeded(json: Option<&str>) -> Self {
        let state =
            json.and_then(|j| serde_json::from_str::<PersistedState>(j).ok()).unwrap_or_default();
        Self { state: Mutex::new(state), dirty: AtomicBool::new(false) }
    }

    /// The state to persist, if Roon has written anything since the last call.
    /// Returns `None` when nothing changed, so a poll doesn't rewrite the row
    /// every tick.
    pub fn take_if_changed(&self) -> Option<String> {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return None;
        }
        let state = self.state.lock().ok()?;
        serde_json::to_string(&*state).ok()
    }

    pub fn is_paired(&self) -> bool {
        self.state.lock().map(|s| s.paired_core_id.is_some()).unwrap_or(false)
    }
}

impl StateStore for PairingStore {
    fn load_token(&self, core_id: &str) -> Option<String> {
        self.state.lock().ok()?.tokens.get(core_id).cloned()
    }

    fn save_token(&self, core_id: &str, token: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.tokens.insert(core_id.to_string(), token.to_string());
        self.dirty.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn load_paired_core_id(&self) -> Option<String> {
        self.state.lock().ok()?.paired_core_id.clone()
    }

    fn save_paired_core_id(&self, core_id: Option<&str>) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.paired_core_id = core_id.map(|s| s.to_string());
        self.dirty.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// The output a zone is playing through, in `devices` terms.
///
/// Keyed on `output_id`, which is stable per endpoint — `display_name` is a
/// label the user edits, and keying on it would re-register the endpoint as a
/// new device when renamed. The same rule Jellyfin's `DeviceId` follows.
#[derive(Debug, Clone)]
pub struct RoonOutput {
    pub machine_id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RoonTrack {
    pub play: Play,
    pub position: u64,
    pub zone_id: String,
    pub output: Option<RoonOutput>,
}

#[derive(Debug, Default)]
pub struct RoonZones {
    pub now_playing: Vec<RoonTrack>,
    pub ready_to_scrobble: Vec<RoonTrack>,
    /// Tracks replaced before they ever counted; `position` is how far in.
    pub skipped: Vec<RoonTrack>,
}

pub struct RoonSource {
    client: RoonClient,
    store: Arc<PairingStore>,
    events: tokio::sync::broadcast::Receiver<RoonEvent>,
    /// The live Core, replaced whenever a re-pair arrives.
    core: Option<Core>,
    host: String,
    port: u16,
    /// Zones whose listens belong to this user. Empty means every zone — see
    /// the note on `consume_zones`.
    zones_allow: Vec<String>,
    tracker: SessionTracker,
    /// Rung by the zone subscription so the engine can tick early.
    wake: Arc<Notify>,
    /// False when the subscription pump has exited, so the next poll knows to
    /// start a new one. Not an error state: polling is unaffected.
    subscribed: Arc<AtomicBool>,
    /// Raised by the pump alongside the engine's bell.
    ///
    /// Both are needed and they do different jobs: the `Notify` wakes the tick
    /// early, and this is what lets this source past its own `next_poll_at`
    /// once the tick is running. Without it the tick would wake and skip the
    /// very source that rang.
    dirty: Arc<AtomicBool>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

impl RoonSource {
    pub fn new(
        host: String,
        port: Option<u16>,
        stored_state: Option<&str>,
        zones_allow: Vec<String>,
        wake: Arc<Notify>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let store = Arc::new(PairingStore::seeded(stored_state));
        let client = RoonClientBuilder::new(
            EXTENSION_ID,
            "Tapedeck",
            env!("CARGO_PKG_VERSION"),
            "Tapedeck",
            "",
        )
        .token_store(StoreHandle(store.clone()))
        // Transport is the only service this needs, and requiring it means a
        // Core that cannot provide it fails at handshake rather than at the
        // first poll.
        .require_transport()
        .build()?;

        // Taken before the client is moved into the struct — and before
        // `connect`, so the first `CorePaired` is not missed.
        let events = client.events();

        Ok(Self {
            client,
            events,
            store,
            core: None,
            host,
            port: port.unwrap_or(DEFAULT_PORT),
            zones_allow,
            tracker: SessionTracker::new(),
            wake,
            subscribed: Arc::new(AtomicBool::new(false)),
            dirty: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Whether the user has approved the extension in Roon yet.
    ///
    /// This is the one setup step no other source has: Roon requires the
    /// extension to be enabled *by hand* in Settings → Extensions, so a source
    /// can be correctly configured and still not authorised. The UI needs to be
    /// able to say "now go and approve this in Roon" rather than reporting a
    /// failure.
    pub fn is_paired(&self) -> bool {
        self.store.is_paired()
    }

    /// The pairing state to write back, if Roon has issued a new token.
    pub fn take_pairing_state(&self) -> Option<String> {
        self.store.take_if_changed()
    }

    /// Connect and pair. The handshake is what triggers the approval prompt in
    /// Roon, so a first run legitimately fails here until the user approves.
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let core = match self.client.connect(&self.host, self.port).await {
            Ok(core) => core,
            // Distinguish "not approved yet" from "cannot reach the Core".
            // They need completely different things from the user, and the
            // first is the normal first-run state rather than an error.
            Err(e) if !self.is_paired() => {
                return Err(format!(
                    "Roon has not approved Tapedeck yet — open Roon → Settings →                      Extensions and enable Tapedeck, then this will connect by itself ({e})"
                )
                .into())
            }
            Err(e) => return Err(e.into()),
        };
        info!("🎵 Paired with Roon Core \"{}\"", core.display_name());
        self.adopt(core).await;
        Ok(())
    }

    /// Take a Core as current and start a subscription against it.
    async fn adopt(&mut self, core: Core) {
        self.core = Some(core);
        self.resubscribe().await;
    }

    /// Start the wakeup pump.
    ///
    /// It reads events purely to ring the bell — every payload is discarded,
    /// because state comes from `get_zones()`. That is what makes the partial
    /// `Seeked` delta a non-problem rather than a zone map to maintain.
    async fn resubscribe(&mut self) {
        let Some(core) = self.core.clone() else {
            return;
        };
        let rx = match core.transport().subscribe_zones().await {
            Ok(rx) => rx,
            Err(e) => {
                // Not fatal, and deliberately not an error: without the
                // subscription this source is simply the pure poll, which is a
                // working source with a slower deck.
                debug!("Roon zone subscription unavailable, polling only: {}", e);
                return;
            }
        };

        let wake = self.wake.clone();
        let alive = self.subscribed.clone();
        let dirty = self.dirty.clone();
        alive.store(true, Ordering::SeqCst);
        tokio::spawn(async move {
            let mut rx = rx;
            while rx.recv().await.is_some() {
                // Order matters: raise the flag *before* ringing, or the tick
                // can wake and find the source not yet due.
                dirty.store(true, Ordering::SeqCst);
                wake.notify_one();
            }
            // The pump has ended — the websocket dropped, or the Core went
            // away. Flagged so the next poll starts a new one; nothing else
            // changes, which is the point of the hybrid.
            alive.store(false, Ordering::SeqCst);
            debug!("Roon zone subscription ended; polling continues");
        });
    }

    /// Pick up a re-pair, so a Core that reconnected is the one polled.
    ///
    /// `RoonClient::connect` spawns a reconnection manager that emits a fresh
    /// `CorePaired` on success. Holding the original `Core` forever would mean
    /// polling a dead handle after the first blip.
    fn drain_events(&mut self) -> Option<Core> {
        let mut adopted = None;
        loop {
            match self.events.try_recv() {
                Ok(RoonEvent::CorePaired(core)) => adopted = Some(core),
                Ok(RoonEvent::CoreLost { core_id }) => {
                    warn!("Roon Core {} disconnected; reconnecting", core_id);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
                // Falling behind only loses intermediate events; the newest
                // pairing still arrives, and state comes from a poll anyway.
                Err(TryRecvError::Lagged(n)) => debug!("Roon events lagged by {}", n),
            }
        }
        adopted
    }

    /// Poll every zone and decide what is playing and what has crossed the
    /// threshold.
    pub async fn fetch_zones(&mut self) -> Result<RoonZones, Box<dyn std::error::Error>> {
        if let Some(core) = self.drain_events() {
            self.adopt(core).await;
        }
        let Some(core) = self.core.clone() else {
            return Err("not paired with a Roon Core".into());
        };
        if !core.is_alive() {
            return Err("the Roon Core connection is down".into());
        }
        if !self.subscribed.load(Ordering::SeqCst) {
            self.resubscribe().await;
        }

        let zones = core.transport().get_zones().await?;
        Ok(self.consume_zones(zones))
    }

    /// The pure half of `fetch_zones`, so the threshold and repeat behaviour
    /// can be tested without a Core.
    pub fn consume_zones(&mut self, zones: Vec<Zone>) -> RoonZones {
        let mut result = RoonZones::default();
        let mut live = Vec::new();

        for zone in zones {
            // A Core is a household server, so a zone filter is the only
            // isolation available — Roon's transport API has no user identity
            // at all. Empty means every zone, which is right for a single-user
            // install and is the same call `own_history_tracks` makes for a
            // Plex source with no allow-list.
            if !self.zones_allow.is_empty()
                && !self.zones_allow.iter().any(|z| {
                    z.eq_ignore_ascii_case(&zone.zone_id)
                        || z.eq_ignore_ascii_case(&zone.display_name)
                })
            {
                continue;
            }
            let Some(np) = zone.now_playing.as_ref() else {
                continue;
            };

            let position = np.seek_position.unwrap_or(0.0).max(0.0) as u64;
            let duration = np.length.filter(|l| *l > 0.0).map(|l| l as u64);
            let play = Self::to_play(&zone, np);

            let track = RoonTrack {
                position,
                zone_id: zone.zone_id.clone(),
                output: Some(zone.outputs.first().map_or_else(
                    || RoonOutput {
                        machine_id: format!("roon-zone-{}", zone.zone_id),
                        name: Some(zone.display_name.clone()),
                    },
                    |o| RoonOutput {
                        machine_id: o.output_id.clone(),
                        name: Some(o.display_name.clone()),
                    },
                )),
                play,
            };
            live.push(zone.zone_id.clone());

            // Loading is a track about to start, not one playing — treating it
            // as paused keeps it off the threshold while still showing on the
            // deck.
            let paused = !matches!(zone.state, PlayState::Playing);

            // Roon has no item id, so the tracker's identity key is the track
            // itself — see `source_key`.
            let key = Self::source_key(&track.play);
            let observed =
                self.tracker.observe(&zone.zone_id, &key, &track.play, position, duration, paused);

            // Replaced before it ever counted — a skip, on this same zone and
            // therefore this same endpoint.
            if let Some(gone) = observed.abandoned {
                let mut skipped = track.clone();
                skipped.play = gone.play;
                skipped.position = gone.listened_secs;
                skipped.play.source_id =
                    format!("{}-skip-{}", Self::source_key(&skipped.play), now_secs());
                result.skipped.push(skipped);
            }

            if observed.ready {
                let mut banked = track.clone();
                banked.play.source_id = format!("{}-scrobble-{}", key, now_secs());
                result.ready_to_scrobble.push(banked);
            }
            result.now_playing.push(track);
        }

        self.tracker.retain_live(&live);
        result
    }

    /// A stable identity for "this track in this zone".
    ///
    /// Every other source has a library item id. Roon's transport API has none,
    /// so identity has to come from the display strings — which means a queue
    /// of two identical titles by the same artist is one track to the repeat
    /// gate. That is the honest limit of what Roon exposes, not something to
    /// engineer around; the consequence is a missed scrobble on a
    /// back-to-back duplicate, which the rewind gate then catches anyway.
    fn source_key(play: &Play) -> String {
        format!("roon-{}-{}", play.artist.to_lowercase(), play.title.to_lowercase())
    }

    /// Map Roon's display strings onto a listen.
    ///
    /// **This is guesswork and cannot be anything else.** `three_line` is
    /// `{line1, line2, line3}` — title, artist, album *as rendered for a
    /// screen*, per the vendored `node-roon-api` docs. There is no schema
    /// saying which field holds what, no artist-credit field, no release id and
    /// no MBIDs. Two known consequences:
    ///
    /// * A compilation renders its album artist, so a Roon listen of one is
    ///   credited to whatever Roon chose to display — the `artist_credit`
    ///   recovery `report_rows` relies on has nothing to work with.
    /// * `two_line` collapses artist and album into one string, so a Core that
    ///   sends only that leaves the album unset rather than guessed at. An
    ///   album parsed out of a joined string would be wrong often enough to
    ///   corrupt the album pages.
    fn to_play(zone: &Zone, np: &roon_api::NowPlaying) -> Play {
        let (title, artist, album) = match np.three_line.as_ref() {
            Some(t) => (t.line1.clone(), t.line2.clone(), t.line3.clone()),
            // Deliberately no parsing of `two_line`/`one_line` into parts: they
            // are a single rendered string, and splitting on a dash would file
            // "Sunday Morning - Live" under an artist that does not exist.
            None => (np.one_line.line1.clone(), None, None),
        };

        let artist = artist.filter(|a| !a.trim().is_empty()).unwrap_or_else(|| UNKNOWN.to_string());

        let mut play = Play {
            source_id: String::new(),
            source_name: "roon".into(),
            title,
            artist,
            album: album.filter(|a| !a.trim().is_empty()),
            timestamp: now_secs(),
            duration: np.length.filter(|l| *l > 0.0).map(|l| l as u64),
            track_number: None,
            // Roon holds superb library metadata and exposes none of it here,
            // so every Roon listen pays for a MusicBrainz lookup.
            mbid_recording: None,
            mbid_release: None,
            mbid_artist: None,
            mbid_release_group: None,
            artists: None,
            caa_id: None,
            caa_release_mbid: None,
        };
        play.source_id = format!("{}-{}", Self::source_key(&play), zone.zone_id);
        play
    }
}

/// Newtype so an `Arc<PairingStore>` satisfies `StateStore`, which the builder
/// takes by value.
struct StoreHandle(Arc<PairingStore>);

impl StateStore for StoreHandle {
    fn load_token(&self, core_id: &str) -> Option<String> {
        self.0.load_token(core_id)
    }
    fn save_token(&self, core_id: &str, token: &str) -> Result<(), String> {
        self.0.save_token(core_id, token)
    }
    fn load_paired_core_id(&self) -> Option<String> {
        self.0.load_paired_core_id()
    }
    fn save_paired_core_id(&self, core_id: Option<&str>) -> Result<(), String> {
        self.0.save_paired_core_id(core_id)
    }
}

#[async_trait::async_trait]
impl crate::sources::MusicSource for RoonSource {
    fn name(&self) -> &str {
        "Roon"
    }

    fn set_scrobble_threshold(&mut self, threshold: crate::models::ScrobbleThreshold) {
        self.tracker.set_threshold(threshold);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    /// Roon's transport API exposes no play history — only what the zones are
    /// doing now. Same shape as Jellyfin and Emby.
    async fn fetch_new_plays(
        &mut self,
        _last_checked: u64,
    ) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        Ok(Vec::new())
    }

    fn take_wakeup(&mut self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roon_api::zone::{LineInfo, ThreeLineInfo};
    use roon_api::NowPlaying as RoonNowPlaying;

    /// A source with no live Core. Everything below exercises `consume_zones`,
    /// which is pure — the Core is only needed to *fetch* zones.
    fn source(zones_allow: Vec<String>) -> RoonSource {
        RoonSource::new("127.0.0.1".into(), None, None, zones_allow, Arc::new(Notify::new()))
            .expect("builds without a Core")
    }

    fn zone(id: &str, name: &str, state: PlayState, seek: f64, length: f64) -> Zone {
        Zone {
            zone_id: id.into(),
            display_name: name.into(),
            state,
            outputs: vec![roon_api::Output {
                output_id: format!("{id}-out"),
                display_name: name.into(),
                zone_id: id.into(),
                volume: None,
                source_controls: Vec::new(),
                can_group_with_output_ids: Vec::new(),
            }],
            now_playing: Some(RoonNowPlaying {
                seek_position: Some(seek),
                length: Some(length),
                image_key: None,
                one_line: LineInfo { line1: "Archangel".into() },
                two_line: None,
                three_line: Some(ThreeLineInfo {
                    line1: "Archangel".into(),
                    line2: Some("Burial".into()),
                    line3: Some("Untrue".into()),
                }),
            }),
            settings: None,
            seek_position: Some(seek),
            is_previous_allowed: true,
            is_next_allowed: true,
            is_pause_allowed: true,
            is_play_allowed: true,
            is_seek_allowed: true,
            queue_items_remaining: None,
            queue_time_remaining: None,
        }
    }

    /// `three_line` is title / artist / album. Getting this order wrong would
    /// file every Roon listen under a track name as the artist, and nothing
    /// would error — the listens would just be nonsense.
    #[test]
    fn three_line_maps_to_title_artist_album() {
        let mut src = source(vec![]);
        let out = src.consume_zones(vec![zone("z1", "Study", PlayState::Playing, 10.0, 240.0)]);
        let play = &out.now_playing[0].play;
        assert_eq!(play.title, "Archangel");
        assert_eq!(play.artist, "Burial");
        assert_eq!(play.album.as_deref(), Some("Untrue"));
        assert_eq!(play.source_name, "roon");
        assert_eq!(play.duration, Some(240));
        // Roon exposes none of these, and claiming otherwise would be worse
        // than leaving them unset.
        assert!(play.mbid_recording.is_none());
        assert!(play.mbid_artist.is_none());
    }

    /// Without `three_line` there is only a single rendered string, and it must
    /// be taken as the title rather than split. Splitting "Sunday Morning -
    /// Live" on the dash would invent an artist that does not exist.
    #[test]
    fn a_single_line_is_a_title_and_never_split() {
        let mut src = source(vec![]);
        let mut z = zone("z1", "Study", PlayState::Playing, 10.0, 240.0);
        let np = z.now_playing.as_mut().unwrap();
        np.three_line = None;
        np.one_line = LineInfo { line1: "Sunday Morning - Live".into() };

        let out = src.consume_zones(vec![z]);
        let play = &out.now_playing[0].play;
        assert_eq!(play.title, "Sunday Morning - Live");
        assert_eq!(play.artist, "Unknown");
        assert_eq!(play.album, None);
    }

    /// The shared threshold, reached through Roon's own units — `seek_position`
    /// and `length` are seconds as `f64`, not .NET ticks or milliseconds.
    #[test]
    fn a_zone_past_half_banks_once() {
        let mut src = source(vec![]);
        assert_eq!(
            src.consume_zones(vec![zone("z1", "Study", PlayState::Playing, 30.0, 240.0)])
                .ready_to_scrobble
                .len(),
            0
        );
        let out = src.consume_zones(vec![zone("z1", "Study", PlayState::Playing, 121.0, 240.0)]);
        assert_eq!(out.ready_to_scrobble.len(), 1);
        // The banked copy needs a per-pass id, or a repeat is dropped forever.
        assert!(out.ready_to_scrobble[0].play.source_id.contains("-scrobble-"));
        assert!(!out.now_playing[0].play.source_id.contains("-scrobble-"));
        // …and only once.
        assert_eq!(
            src.consume_zones(vec![zone("z1", "Study", PlayState::Playing, 130.0, 240.0)])
                .ready_to_scrobble
                .len(),
            0
        );
    }

    /// Only `Playing` advances. `Loading` is a track about to start, and
    /// counting it would bank a listen for something nobody heard.
    #[test]
    fn a_paused_or_loading_zone_never_crosses() {
        for state in [PlayState::Paused, PlayState::Loading, PlayState::Stopped] {
            let mut src = source(vec![]);
            let out = src.consume_zones(vec![zone("z1", "Study", state, 200.0, 240.0)]);
            assert_eq!(out.ready_to_scrobble.len(), 0, "{state:?} must not scrobble");
            // …but it is still on the deck, which is what the registry wants.
            assert_eq!(out.now_playing.len(), 1);
        }
    }

    /// A Core is a household server and Roon's transport API carries no user
    /// identity at all, so the zone filter is the only isolation there is.
    #[test]
    fn a_zone_outside_the_allow_list_is_ignored() {
        let mut src = source(vec!["Study".into()]);
        let out = src.consume_zones(vec![
            zone("z1", "Study", PlayState::Playing, 121.0, 240.0),
            zone("z2", "Kitchen", PlayState::Playing, 121.0, 240.0),
        ]);
        assert_eq!(out.now_playing.len(), 1);
        assert_eq!(out.now_playing[0].zone_id, "z1");
        assert_eq!(out.ready_to_scrobble.len(), 1);
    }

    /// An empty allow-list means every zone — right for a single-user install,
    /// and the same call a Plex source with no allow-list makes.
    #[test]
    fn no_allow_list_takes_every_zone() {
        let mut src = source(vec![]);
        let out = src.consume_zones(vec![
            zone("z1", "Study", PlayState::Playing, 10.0, 240.0),
            zone("z2", "Kitchen", PlayState::Playing, 10.0, 240.0),
        ]);
        assert_eq!(out.now_playing.len(), 2);
    }

    /// The endpoint is keyed on `output_id`, not the display name — renaming a
    /// zone in Roon must not re-register it as a new device, which would split
    /// its chain hours across two rows.
    #[test]
    fn the_output_id_is_the_device_key() {
        let mut src = source(vec![]);
        let out = src.consume_zones(vec![zone("z1", "Study", PlayState::Playing, 10.0, 240.0)]);
        let dev = out.now_playing[0].output.as_ref().expect("has an output");
        assert_eq!(dev.machine_id, "z1-out");

        let mut src = source(vec![]);
        let mut renamed = zone("z1", "The Study", PlayState::Playing, 10.0, 240.0);
        renamed.outputs[0].display_name = "The Study".into();
        let out = src.consume_zones(vec![renamed]);
        assert_eq!(out.now_playing[0].output.as_ref().unwrap().machine_id, "z1-out");
    }

    /// The pairing token round-trips through the store, and is only offered for
    /// persistence when Roon actually wrote a new one — otherwise every poll
    /// would rewrite the credentials row.
    #[test]
    fn the_pairing_token_persists_only_when_it_changes() {
        let store = PairingStore::seeded(None);
        assert!(store.take_if_changed().is_none(), "nothing written yet");
        assert!(!store.is_paired());

        store.save_token("core-1", "tok").unwrap();
        store.save_paired_core_id(Some("core-1")).unwrap();
        assert!(store.is_paired());

        let json = store.take_if_changed().expect("a new token must be persisted");
        assert!(store.take_if_changed().is_none(), "and only once");

        // …and it comes back, so an approved extension survives a restart.
        let reloaded = PairingStore::seeded(Some(&json));
        assert_eq!(reloaded.load_token("core-1").as_deref(), Some("tok"));
        assert_eq!(reloaded.load_paired_core_id().as_deref(), Some("core-1"));
        assert!(reloaded.is_paired());
    }
}
