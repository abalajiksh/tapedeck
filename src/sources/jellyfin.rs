//! Jellyfin, which is [`MediaBrowserSource`] with [`Flavour::Jellyfin`].
//!
//! The client lives in `mediabrowser.rs`, shared with Emby — the two are one
//! protocol and Jellyfin is a 2018 fork of Emby 3.5.2. What lives here is the
//! part that must *not* be shared: tests pinned to what a **Jellyfin** server
//! actually sends.
//!
//! Two structural facts about this source, both still true:
//!
//! * **Auth is an API key**, sent as `X-Emby-Token` — Jellyfin kept the header
//!   name from Emby. Jellyfin lets a user mint one per client, so it is
//!   per-user in the same way a Plex token is.
//! * **There is no play history to backfill.** Jellyfin doesn't record listens
//!   natively — that needs the Playback Reporting plugin, which not everyone
//!   installs. So this source only ever sees what's playing now, unlike
//!   Navidrome where `/api/scrobble/` hands over years of it.

#[cfg(test)]
mod tests {
    use crate::sources::egress::Egress;
    use crate::sources::mediabrowser::*;

    /// `/Users` is the fallback that makes a dashboard API key usable at all —
    /// `/Users/Me` answers 400 for one, because a server-scoped key belongs to
    /// no user.
    ///
    /// **Pinned against a real 10.11.11 response.** These names are PascalCase
    /// and a mismatch would be silent, leaving the picker empty and the source
    /// unconfigurable with no error to explain it.
    #[test]
    fn the_users_list_parses_a_real_response() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Policy {
            #[serde(default)]
            is_disabled: bool,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct User {
            id: String,
            name: String,
            #[serde(default)]
            policy: Option<Policy>,
        }

        let body = r#"[
            {
                "Name": "abalaji",
                "ServerId": "f61075a77ace44edb6a28a67eb9cfef0",
                "Id": "2ed208b8f7314587915b25f0602d8df8",
                "HasPassword": true,
                "Policy": { "IsAdministrator": true, "IsHidden": true, "IsDisabled": false }
            },
            {
                "Name": "retired",
                "Id": "0000",
                "Policy": { "IsAdministrator": false, "IsDisabled": true }
            }
        ]"#;

        let users: Vec<User> = serde_json::from_str(body).expect("parses");
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "abalaji");
        assert_eq!(users[0].id, "2ed208b8f7314587915b25f0602d8df8");

        // A disabled account can't play anything, so offering it would only be
        // a way to configure a source that never scrobbles.
        let live: Vec<&User> =
            users.iter().filter(|u| !u.policy.as_ref().is_some_and(|p| p.is_disabled)).collect();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].name, "abalaji");
    }

    /// A `/Sessions` element carrying the transcode and client fields.
    ///
    /// **Built from the Jellyfin 12.0.0 OpenAPI schema, not captured from a
    /// live server** — unlike `the_users_list_parses_a_real_response` above,
    /// which is a real 10.11.11 body. It pins the PascalCase spellings and the
    /// shapes the schema declares, which is the half that fails silently; it
    /// cannot prove Jellyfin actually populates these on a session, and that is
    /// worth confirming against a real server before relying on the transcode
    /// data.
    ///
    /// Emby has its own copy of this in `emby.rs` rather than sharing it. The
    /// spellings below are Jellyfin's; asserting them here would say nothing
    /// about what Emby sends.
    const SESSION_TRANSCODING: &str = r#"{
        "Id": "sess-1",
        "UserName": "abalaji",
        "Client": "Finamp",
        "DeviceName": "Pixel 8",
        "DeviceId": "0f1e2d3c4b5a",
        "DeviceType": "Android",
        "ApplicationVersion": "0.9.2",
        "PlayState": {
            "PositionTicks": 1230000000,
            "IsPaused": false,
            "PlayMethod": "Transcode"
        },
        "TranscodingInfo": {
            "AudioCodec": "opus",
            "Container": "ogg",
            "Bitrate": 128000,
            "AudioChannels": 2,
            "IsAudioDirect": false,
            "TranscodeReasons": ["ContainerNotSupported", "AudioCodecNotSupported"]
        },
        "NowPlayingItem": {
            "Id": "item-1",
            "Name": "Xtal",
            "Type": "Audio",
            "Album": "SAW 85-92",
            "Artists": ["Aphex Twin"],
            "Container": "flac",
            "RunTimeTicks": 2930000000,
            "IndexNumber": 1,
            "ProviderIds": { "MusicBrainzTrack": "mb-track" },
            "MediaStreams": [
                { "Type": "Audio", "Codec": "flac", "BitRate": 900000,
                  "SampleRate": 44100, "BitDepth": 16, "Channels": 2 }
            ]
        }
    }"#;

    #[test]
    fn a_session_parses_its_client_and_transcode_fields() {
        let s: MbSession = serde_json::from_str(SESSION_TRANSCODING).expect("parses");
        let player = MediaBrowserSource::to_player(&s).expect("has a device id");
        // DeviceId is the key, not DeviceName — the name is user-editable and
        // would re-register the client as a new device when changed.
        assert_eq!(player.machine_id, "0f1e2d3c4b5a");
        assert_eq!(player.name.as_deref(), Some("Pixel 8"));
        assert_eq!(player.product.as_deref(), Some("Finamp"));
        assert_eq!(player.platform.as_deref(), Some("Android"));

        let item = s.now_playing_item.as_ref().unwrap();
        let q = MediaBrowserSource::extract_quality(
            item,
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .expect("has streams");

        // The source half is the file on disk and must not be overwritten by
        // what was delivered.
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.container.as_deref(), Some("flac"));
        assert_eq!(q.is_lossless, Some(true));
        assert_eq!(q.sample_rate, Some(44100));

        // The delivery half.
        assert_eq!(q.is_transcoded, Some(true));
        assert_eq!(q.delivery_codec.as_deref(), Some("opus"));
        assert_eq!(q.delivery_bitrate, Some(128), "bits per second → kbps");
        assert_eq!(
            q.transcode_reason.as_deref(),
            Some("ContainerNotSupported, AudioCodecNotSupported")
        );
    }

    /// Jellyfin 10.8 sent `TranscodeReasons` as a comma-joined string; 12.x
    /// sends an array. Reading only one shape leaves `transcode_reason` empty
    /// on the other — which looks exactly like a direct play, so nothing errors
    /// and the data is quietly wrong.
    #[test]
    fn transcode_reasons_parse_as_a_string_or_an_array() {
        let as_array: MbSession = serde_json::from_str(SESSION_TRANSCODING).unwrap();
        let as_string: MbSession = serde_json::from_str(&SESSION_TRANSCODING.replace(
            r#"["ContainerNotSupported", "AudioCodecNotSupported"]"#,
            r#""ContainerNotSupported, AudioCodecNotSupported""#,
        ))
        .expect("the 10.8 spelling parses too");

        let reasons =
            |s: &MbSession| s.transcoding_info.as_ref().unwrap().transcode_reasons.clone().unwrap();
        assert_eq!(reasons(&as_array), reasons(&as_string));
        assert_eq!(reasons(&as_array).len(), 2);
    }

    /// A direct play must not claim a delivery codec. `compute_score` docks
    /// points whenever `delivery_codec` is set, so reporting one for an
    /// untouched file would quietly penalise the best listens on the system.
    #[test]
    fn a_direct_play_reports_no_delivery_downgrade() {
        let body = SESSION_TRANSCODING
            .replace(r#""PlayMethod": "Transcode""#, r#""PlayMethod": "DirectPlay""#);
        let body = body[..body.find(r#""TranscodingInfo""#).unwrap()].to_string()
            + &body[body.find(r#""NowPlayingItem""#).unwrap()..];
        let s: MbSession = serde_json::from_str(&body).expect("parses without TranscodingInfo");

        let q = MediaBrowserSource::extract_quality(
            s.now_playing_item.as_ref().unwrap(),
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .unwrap();
        assert_eq!(q.is_transcoded, Some(false));
        assert_eq!(q.delivery_codec, None);
        assert_eq!(q.transcode_reason, None);
    }

    /// A video transcode can leave the audio untouched. Calling that a
    /// transcoded listen would be wrong, and `IsAudioDirect` is the only field
    /// that says so.
    #[test]
    fn a_passthrough_audio_stream_is_not_a_transcoded_listen() {
        let s: MbSession = serde_json::from_str(
            &SESSION_TRANSCODING.replace(r#""IsAudioDirect": false"#, r#""IsAudioDirect": true"#),
        )
        .unwrap();
        let q = MediaBrowserSource::extract_quality(
            s.now_playing_item.as_ref().unwrap(),
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .unwrap();
        assert_eq!(q.is_transcoded, Some(false));
        assert_eq!(q.delivery_codec, None);
    }

    /// Build a `/Sessions` body for one track at a given playhead, always the
    /// same session and the same item — which is exactly what repeat looks like
    /// from here.
    fn session_at(position_secs: u64) -> String {
        format!(
            r#"[{{
                "Id": "sess-1", "UserName": "abalaji", "DeviceId": "dev-1",
                "PlayState": {{ "PositionTicks": {}, "IsPaused": false, "PlayMethod": "DirectPlay" }},
                "NowPlayingItem": {{
                    "Id": "item-1", "Name": "Combustible", "Type": "Audio",
                    "Album": "Blonde", "Artists": ["Cœur de pirate"],
                    "RunTimeTicks": {}, "IndexNumber": 1
                }}
            }}]"#,
            position_secs * TICKS_PER_SECOND,
            200 * TICKS_PER_SECOND
        )
    }

    /// The tracker's gates are pinned in `mediabrowser.rs`; this walks them
    /// end-to-end through a real Jellyfin session body, so a change to the
    /// parsing that broke the state machine's inputs would still be caught.
    #[test]
    fn a_looped_track_scrobbles_once_per_pass_through_the_wire_format() {
        let mut src = MediaBrowserSource::jellyfin(
            "http://jf:8096".into(),
            "k".into(),
            "abalaji".into(),
            Egress::Open,
        );
        let mut banked = 0usize;

        for pass in 0..3 {
            for secs in [3u64, 120, 195] {
                let sessions: Vec<MbSession> = serde_json::from_str(&session_at(secs)).unwrap();
                banked += src.consume_sessions(sessions).ready_to_scrobble.len();
            }
            assert_eq!(banked, pass + 1, "pass {pass} should have banked exactly one");
        }
    }

    /// Another account's playback on a shared server is not this user's, and
    /// the filter is the whole of the isolation for this source.
    #[test]
    fn another_accounts_session_is_ignored() {
        let mut src = MediaBrowserSource::jellyfin(
            "http://jf:8096".into(),
            "k".into(),
            "abalaji".into(),
            Egress::Open,
        );
        let body =
            session_at(195).replace(r#""UserName": "abalaji""#, r#""UserName": "housemate""#);
        let sessions: Vec<MbSession> = serde_json::from_str(&body).unwrap();
        let out = src.consume_sessions(sessions);
        assert!(out.ready_to_scrobble.is_empty());
        assert!(out.now_playing.is_empty());
    }

    #[test]
    fn converts_dotnet_ticks_to_seconds() {
        // 293s expressed in 100-nanosecond units; reading these as milliseconds
        // would make every track absurdly long and break the scrobble
        // threshold.
        let sessions: Vec<MbSession> = serde_json::from_str(&session_at(0)).unwrap();
        let item = sessions[0].now_playing_item.as_ref().unwrap();
        let p = MediaBrowserSource::to_play(Flavour::Jellyfin, item, &Some("Living Room".into()));
        assert_eq!(p.duration, Some(200));
    }

    #[test]
    fn carries_library_mbids_and_track_number() {
        let s: MbSession = serde_json::from_str(SESSION_TRANSCODING).unwrap();
        let item = s.now_playing_item.as_ref().unwrap();
        let p = MediaBrowserSource::to_play(Flavour::Jellyfin, item, &None);
        assert_eq!(p.mbid_recording.as_deref(), Some("mb-track"));
        assert_eq!(p.track_number, Some(1));
        assert_eq!(p.artist, "Aphex Twin");
        assert_eq!(p.source_name, "jellyfin");
    }

    /// `Start` is .NET ticks, and reading it as milliseconds is silent.
    ///
    /// Pinned against the 12.0.0 schema's own units: 10,000,000 ticks to the
    /// second. Read as ms, 27,500,000 becomes 7.6 *hours* instead of 2.75
    /// seconds — or, the way it actually goes wrong, every cue lands in the
    /// first instant, the reader never leaves line one, and nothing errors.
    #[test]
    fn jellyfin_lyric_cues_are_ticks_not_milliseconds() {
        let body = r#"{
            "Metadata": { "IsSynced": true, "Offset": 0 },
            "Lyrics": [
                { "Text": "first line",  "Start": 10000000 },
                { "Text": "second line", "Start": 27500000 },
                { "Text": "past a minute", "Start": 615000000 }
            ]
        }"#;
        let dto: crate::sources::mediabrowser::LyricDto = serde_json::from_str(body).unwrap();
        let (plain, synced) = crate::sources::mediabrowser::lyric_dto_to_pair(&dto).unwrap();

        let synced = synced.expect("cues present, so the lyric is timed");
        assert!(synced.starts_with("[00:01.00] first line"), "got {synced}");
        assert!(synced.contains("[00:02.75] second line"), "got {synced}");
        // 615,000,000 ticks = 61.5s = 1:01.50 — the minute rollover, which an
        // implementation that forgot to divide would put at [00:61.50].
        assert!(synced.contains("[01:01.50] past a minute"), "got {synced}");

        assert_eq!(plain.unwrap(), "first line\nsecond line\npast a minute");
    }

    /// A signed offset really moves the cues, and never past zero.
    #[test]
    fn a_negative_lyric_offset_pulls_the_words_earlier() {
        let body = r#"{
            "Metadata": { "Offset": -5000000 },
            "Lyrics": [
                { "Text": "early", "Start": 10000000 },
                { "Text": "clamped", "Start": 1000000 }
            ]
        }"#;
        let dto: crate::sources::mediabrowser::LyricDto = serde_json::from_str(body).unwrap();
        let (_, synced) = crate::sources::mediabrowser::lyric_dto_to_pair(&dto).unwrap();
        let synced = synced.unwrap();
        assert!(synced.contains("[00:00.50] early"), "1.0s - 0.5s = 0.5s; got {synced}");
        // Clamped rather than negative: a cue before the start of the track is
        // not a time, and `[-00:00.40]` parses as nothing downstream.
        assert!(synced.contains("[00:00.00] clamped"), "got {synced}");
    }

    /// A lyric with words and no cues is plain, not a timed lyric with none.
    ///
    /// `IsSynced` is the server's claim about the file; the cues are the
    /// evidence. Trusting the claim yields a "timed" lyric the reader cannot
    /// follow, which looks like the deck being broken.
    #[test]
    fn a_lyric_with_no_cues_is_plain_whatever_the_metadata_claims() {
        let body = r#"{
            "Metadata": { "IsSynced": true },
            "Lyrics": [ { "Text": "just words" }, { "Text": "no cues" } ]
        }"#;
        let dto: crate::sources::mediabrowser::LyricDto = serde_json::from_str(body).unwrap();
        let (plain, synced) = crate::sources::mediabrowser::lyric_dto_to_pair(&dto).unwrap();
        assert!(synced.is_none(), "no cues, so not timed");
        assert_eq!(plain.unwrap(), "just words\nno cues");
    }

    /// An empty lyric document is no lyrics, not empty ones.
    #[test]
    fn an_empty_lyric_document_is_no_lyrics() {
        let dto: crate::sources::mediabrowser::LyricDto =
            serde_json::from_str(r#"{"Lyrics": []}"#).unwrap();
        assert!(crate::sources::mediabrowser::lyric_dto_to_pair(&dto).is_none());
    }
}
