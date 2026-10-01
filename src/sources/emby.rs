//! Emby, which is [`MediaBrowserSource`] with [`Flavour::Emby`].
//!
//! Jellyfin is a 2018 fork of Emby 3.5.2, so the existing Jellyfin source is
//! most of this — and that closeness is exactly the trap. Emby has developed
//! independently for seven years; the overlap is an inheritance, not a
//! guarantee.
//!
//! # What is confirmed
//!
//! From `temp/audiomuseai-serverconnect/emby.py`, a working third-party
//! provider, plus Emby's published API docs:
//!
//! * **Everything lives under `/emby/`** — `/emby/Sessions`, `/emby/Users`,
//!   `/emby/Library/VirtualFolders`. Jellyfin serves the same routes at the
//!   root. This is the single largest part of the diff and is handled by
//!   [`Flavour::base`].
//! * **Auth is `X-Emby-Token: <api key>`** — the same header Jellyfin sends,
//!   because Jellyfin kept the name. So the credential shape, the "mint a key
//!   in the dashboard" flow, and the "an API key belongs to nobody so resolve
//!   the account against `/Users`" problem are all already solved.
//! * **Playlist creation takes query parameters with a comma-joined `Ids`**,
//!   not Jellyfin's JSON body with an array. Jellyfin marks those parameters
//!   deprecated; Emby never did. Sending Jellyfin's shape to Emby creates an
//!   *empty playlist* rather than failing, which is why these are two call
//!   shapes and not one with a flag.
//! * **Item queries are user-scoped** — `/emby/Users/<id>/Items` where Jellyfin
//!   accepts a bare `/Items`.
//!
//! # What is NOT confirmed — read this before trusting the data
//!
//! **No part of this source has been run against a live Emby server** (as of
//! v0.60.0). The reference provider is an analysis tool: it never touches
//! `/Sessions` at all, so it says nothing about the shape of a playing session,
//! which is the only thing a scrobbler cares about.
//!
//! Every field below is assumed to carry Jellyfin's spelling. Each one fails
//! *silently* on a mismatch — no error, no log, just a value that is always
//! `None`:
//!
//! | Field | If Emby spells it differently |
//! |---|---|
//! | `SessionInfo.DeviceId` | no device, so no signal chain — the bug v0.22.0 fixed for Plex and v0.30.0 for Jellyfin |
//! | `PlayState.PlayMethod` | every listen reads as a direct play |
//! | `TranscodingInfo` | transcodes invisible; `delivery_codec` never set |
//! | `TranscodeReasons` | empty reason, which looks exactly like a direct play |
//! | `ProviderIds.MusicBrainz*` | every listen pays for a MusicBrainz lookup it could have skipped |
//! | `MediaStreams[].BitDepth` / `SampleRate` | quality score computed from less than it could be |
//!
//! The tests below pin the *shape this code expects*. They cannot prove Emby
//! sends it. **Capture a real `/emby/Sessions` body and pin a test to it**, the
//! way `jellyfin.rs` pins `the_users_list_parses_a_real_response` to a real
//! 10.11.11 response — that is the only thing that closes this.
//!
//! # Still open
//!
//! Emby records no history this source can read, so like Jellyfin it sees only
//! now-playing — the same "plays that finished between polls" gap, and no
//! backfill. Emby's own Playback Reporting equivalent is unexplored.

#[cfg(test)]
mod tests {
    use crate::sources::egress::Egress;
    use crate::sources::mediabrowser::*;

    /// Every route is prefixed. This is the difference most likely to be got
    /// wrong by copying the Jellyfin source, and unlike the field spellings it
    /// fails loudly — a 404 on everything — so it is cheap to pin and cheap to
    /// diagnose.
    #[test]
    fn every_route_is_namespaced_under_emby() {
        let s = MediaBrowserSource::emby(
            "http://emby:8096".into(),
            "k".into(),
            "abalaji".into(),
            Egress::Open,
        );
        for (path, expected) in [
            ("/Sessions", "http://emby:8096/emby/Sessions"),
            ("/Users", "http://emby:8096/emby/Users"),
            ("/System/Info", "http://emby:8096/emby/System/Info"),
            ("/Playlists", "http://emby:8096/emby/Playlists"),
        ] {
            assert_eq!(s.endpoint(path), expected);
        }
    }

    /// A listen must record that it came off Emby specifically. The same item
    /// id can exist on a Jellyfin server too — someone migrating between them
    /// keeps their library — and `Database::exists` dedups on
    /// `(user_id, source_id, source_name)`, so a shared name would drop real
    /// listens as duplicates.
    #[test]
    fn an_emby_listen_is_stamped_emby() {
        assert_eq!(Flavour::Emby.source_name(), "emby");
        assert_eq!(Flavour::Emby.label(), "Emby");
        assert_eq!(Flavour::Emby.base(), "/emby");
    }

    /// A `/emby/Sessions` element.
    ///
    /// **This is NOT captured from a live server.** It is the Jellyfin shape
    /// with Emby's spellings assumed to match, and it exists to pin what this
    /// code expects rather than to prove Emby sends it. See the module docs for
    /// the list of fields that go silently `None` if the assumption is wrong.
    ///
    /// Note `TranscodeReasons` as a comma-joined string rather than an array:
    /// Emby is the older lineage and 10.8-era Jellyfin — forked from it — used
    /// the string form, so this is the *more* likely of the two shapes here.
    /// Both are accepted either way.
    const ASSUMED_SESSION: &str = r#"{
        "Id": "sess-emby-1",
        "UserName": "abalaji",
        "Client": "Emby for Android",
        "DeviceName": "Pixel 8",
        "DeviceId": "emby-dev-0f1e2d",
        "DeviceType": "Android",
        "PlayState": {
            "PositionTicks": 1230000000,
            "IsPaused": false,
            "PlayMethod": "Transcode"
        },
        "TranscodingInfo": {
            "AudioCodec": "aac",
            "Bitrate": 192000,
            "IsAudioDirect": false,
            "TranscodeReasons": "ContainerNotSupported"
        },
        "NowPlayingItem": {
            "Id": "emby-item-1",
            "Name": "Archangel",
            "Type": "Audio",
            "Album": "Untrue",
            "Artists": ["Burial"],
            "Container": "flac",
            "RunTimeTicks": 2400000000,
            "IndexNumber": 3,
            "ProviderIds": { "MusicBrainzTrack": "mb-archangel" },
            "MediaStreams": [
                { "Type": "Audio", "Codec": "flac", "BitRate": 900000,
                  "SampleRate": 44100, "BitDepth": 16, "Channels": 2 }
            ]
        }
    }"#;

    /// The client identity, which is what gives an Emby listen a device and
    /// therefore a signal chain.
    ///
    /// Unverified — `DeviceId` is top of the list of fields that would leave
    /// every Emby listen with no chain at all, exactly as happened to Plex
    /// before v0.22.0 and Jellyfin before v0.30.0.
    #[test]
    fn a_session_yields_a_device_under_the_assumed_spelling() {
        let s: MbSession = serde_json::from_str(ASSUMED_SESSION).expect("parses");
        let player = MediaBrowserSource::to_player(&s).expect("has a device id");
        assert_eq!(player.machine_id, "emby-dev-0f1e2d");
        assert_eq!(player.name.as_deref(), Some("Pixel 8"));
        assert_eq!(player.product.as_deref(), Some("Emby for Android"));
    }

    /// Emby is the older lineage, so the comma-joined `TranscodeReasons` string
    /// is the likelier shape here. Reading only the array form would leave the
    /// reason empty, which looks exactly like a direct play.
    #[test]
    fn the_older_string_form_of_transcode_reasons_is_accepted() {
        let s: MbSession = serde_json::from_str(ASSUMED_SESSION).unwrap();
        let q = MediaBrowserSource::extract_quality(
            s.now_playing_item.as_ref().unwrap(),
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .expect("has streams");

        assert_eq!(q.transcode_reason.as_deref(), Some("ContainerNotSupported"));
        assert_eq!(q.is_transcoded, Some(true));
        assert_eq!(q.delivery_codec.as_deref(), Some("aac"));
        assert_eq!(q.delivery_bitrate, Some(192), "bits per second → kbps");
        // The source half is still the file on disk.
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.is_lossless, Some(true));
    }

    /// The account filter is the whole of the isolation on a shared Emby, the
    /// same as it is for Jellyfin.
    #[test]
    fn another_accounts_session_is_ignored() {
        let mut src = MediaBrowserSource::emby(
            "http://emby:8096".into(),
            "k".into(),
            "abalaji".into(),
            Egress::Open,
        );
        let body = format!(
            "[{}]",
            ASSUMED_SESSION.replace(r#""UserName": "abalaji""#, r#""UserName": "housemate""#)
        );
        let sessions: Vec<MbSession> = serde_json::from_str(&body).unwrap();
        let out = src.consume_sessions(sessions);
        assert!(out.ready_to_scrobble.is_empty());
        assert!(out.now_playing.is_empty());
    }

    /// End-to-end through the assumed wire format: the shared state machine's
    /// gates are pinned in `mediabrowser.rs`, and this checks the Emby parsing
    /// feeds them correctly.
    #[test]
    fn a_track_past_the_threshold_is_banked_once() {
        let mut src = MediaBrowserSource::emby(
            "http://emby:8096".into(),
            "k".into(),
            "abalaji".into(),
            Egress::Open,
        );

        // 60s of a 240s track — short of the halfway gate at 120s, and short of
        // the four-minute one.
        let early = ASSUMED_SESSION
            .replace(r#""PositionTicks": 1230000000"#, r#""PositionTicks": 600000000"#);
        let sessions: Vec<MbSession> = serde_json::from_str(&format!("[{early}]")).unwrap();
        assert_eq!(src.consume_sessions(sessions).ready_to_scrobble.len(), 0);

        // 123s — past half.
        let sessions: Vec<MbSession> =
            serde_json::from_str(&format!("[{ASSUMED_SESSION}]")).unwrap();
        let out = src.consume_sessions(sessions);
        assert_eq!(out.ready_to_scrobble.len(), 1);

        // …and only once. A second poll at the same place must not bank again.
        let sessions: Vec<MbSession> =
            serde_json::from_str(&format!("[{ASSUMED_SESSION}]")).unwrap();
        assert_eq!(src.consume_sessions(sessions).ready_to_scrobble.len(), 0);

        let play = &out.ready_to_scrobble[0].play;
        assert_eq!(play.source_name, "emby");
        assert!(
            play.source_id.starts_with("emby-emby-item-1-scrobble-"),
            "the banked copy needs a per-pass id or a repeat is dropped forever: {}",
            play.source_id
        );
        assert_eq!(play.artist, "Burial");
        assert_eq!(play.title, "Archangel");
        assert_eq!(play.duration, Some(240));
        assert_eq!(play.track_number, Some(3));
        assert_eq!(play.mbid_recording.as_deref(), Some("mb-archangel"));

        // And the deck's copy keeps the stable id.
        assert_eq!(out.now_playing.len(), 1);
        assert!(!out.now_playing[0].play.source_id.contains("scrobble"));
    }
}
