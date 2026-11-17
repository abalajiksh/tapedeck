use super::*;
use crate::musicbrainz::{
    parse_group_facts, parse_recording_facts, parse_release_facts, parse_work_facts,
};

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/src/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn listen(title: &str, artist: &str, album: Option<&str>) -> ListenView {
    ListenView::new(title, artist, None, album, None)
}

fn soundtrack(title: &str, tracks: i64, sole: i64, secondary: &[&str]) -> ReleaseFacts {
    ReleaseFacts {
        title: title.into(),
        group_title: title.into(),
        primary_type: Some("Album".into()),
        secondary_types: secondary.iter().map(|s| s.to_string()).collect(),
        track_count: tracks,
        sole_tracks: sole,
        ..Default::default()
    }
}

fn work(title: &str, langs: &[&str], lyricist: bool) -> WorkRef {
    WorkRef {
        mbid: format!("w-{title}"),
        title: title.into(),
        languages: langs.iter().map(|s| s.to_string()).collect(),
        lyricist,
        ..Default::default()
    }
}

fn recording(title: &str, w: WorkRef) -> RecordingFacts {
    RecordingFacts { title: title.into(), works: vec![w], ..Default::default() }
}

#[test]
fn hahns_mendelssohn_is_classical_with_its_concerto_its_movement_and_its_players() {
    let rec = parse_recording_facts(&fixture("mb_recording_hahn_mendelssohn.json"));
    let piece = parse_work_facts(&fixture("mb_work_mendelssohn_concerto.json"));
    let l = listen(
        "Violin Concerto in E minor, Op. 64: I. Allegro molto appassionato",
        "Hilary Hahn",
        None,
    );
    let o = classify(&l, Some(&rec), Some(&piece), None, None);
    assert_ne!(o.facets & CLASSICAL, 0);
    assert_eq!(o.piece_key, "w:c230858f-8b2b-3e77-aa3e-777743980263");
    assert_eq!(o.part_count, Some(3));
    assert!(o.leaf_key.is_some());
    assert_eq!(o.composer_name.as_deref(), Some("Felix Mendelssohn"));
    let hahn = o.performers.iter().find(|p| p.name == "Hilary Hahn").unwrap();
    assert_eq!((hahn.role.as_str(), hahn.instrument.as_deref()), ("soloist", Some("violin")));
    assert!(o.performers.iter().any(|p| p.role == "conductor"));
    assert!(o.performers.iter().any(|p| p.role == "orchestra"));
    assert_eq!(o.facets & SCORE, 0);
}

#[test]
fn last_christmas_is_christmas_and_groups_with_its_covers() {
    let rec = parse_recording_facts(&fixture("mb_recording_facts_last_christmas.json"));
    let w = parse_work_facts(&fixture("mb_work_last_christmas.json"));
    let o = classify(&listen("Last Christmas", "Wham!", None), Some(&rec), Some(&w), None, None);
    assert_ne!(o.facets & CHRISTMAS, 0);
    assert_eq!(o.facets & CLASSICAL, 0, "a Song is not a classical form");
    assert_eq!(
        o.piece_key, "w:8e438d29-bc0b-3cfe-8c47-17e14113a3c3",
        "every cover shares the work"
    );
}

#[test]
fn tron_legacy_cues_are_score_and_the_remix_album_is_not() {
    let rel = parse_release_facts(&fixture("mb_release_tron_legacy.json"));
    let cue = recording("The Grid", work("The Grid", &["zxx"], false));
    let o = classify(
        &listen("The Grid", "Daft Punk", Some("TRON: Legacy")),
        Some(&cue),
        None,
        Some(&rel),
        None,
    );
    assert_ne!(o.facets & SCORE, 0, "{:?}", o.evidence);

    let remix = soundtrack("TRON: Legacy Reconfigured", 15, 15, &["Remix", "Soundtrack"]);
    let o = classify(
        &listen("Derezzed (The Glitch Mob remix)", "Daft Punk", None),
        Some(&cue),
        None,
        Some(&remix),
        None,
    );
    assert_eq!(o.facets & (SCORE | SCORE_SUGGESTED), 0, "club music stays in Top Charts");
}

#[test]
fn dont_you_forget_about_me_is_a_song_even_on_the_breakfast_club() {
    let rec = recording(
        "Don't You (Forget About Me)",
        work("Don't You (Forget About Me)", &["eng"], true),
    );
    let rel = soundtrack("The Breakfast Club", 10, 0, &["Soundtrack"]);
    let o = classify(
        &listen("Don't You (Forget About Me)", "Simple Minds", Some("The Breakfast Club")),
        Some(&rec),
        None,
        Some(&rel),
        None,
    );
    assert_eq!(o.facets & (SCORE | SCORE_SUGGESTED), 0);
    // Names only, from a scrobbler that sent the soundtrack's long title.
    let o = classify(
        &listen(
            "Don't You (Forget About Me)",
            "Simple Minds",
            Some("The Breakfast Club (Original Motion Picture Soundtrack)"),
        ),
        None,
        None,
        None,
        None,
    );
    assert_eq!(o.facets & (SCORE | SCORE_SUGGESTED), 0);
}

#[test]
fn purple_rain_is_songs_and_a_score_album_without_data_is_only_suggested() {
    let song = recording("When Doves Cry", work("When Doves Cry", &["eng"], true));
    let rain = soundtrack("Purple Rain", 9, 9, &["Soundtrack"]);
    let o = classify(
        &listen("When Doves Cry", "Prince", Some("Purple Rain")),
        Some(&song),
        None,
        Some(&rain),
        None,
    );
    assert_eq!(o.facets & (SCORE | SCORE_SUGGESTED), 0);

    // A Morricone album: composer alone on every track, nothing known about the works.
    let unknown = RecordingFacts { title: "L'estasi dell'oro".into(), ..Default::default() };
    let gbu = soundtrack("Il buono, il brutto, il cattivo", 12, 12, &["Soundtrack"]);
    let o = classify(
        &listen("L'estasi dell'oro", "Ennio Morricone", None),
        Some(&unknown),
        None,
        Some(&gbu),
        None,
    );
    assert_eq!(o.facets & SCORE, 0, "not without positive evidence");
    assert_ne!(o.facets & SCORE_SUGGESTED, 0, "but offered for confirmation");
}

#[test]
fn a_tamil_songs_album_never_loses_its_songs() {
    // Singers credited on most tracks; this one is the composer singing alone.
    let rel = soundtrack("Sillunu Oru Kaadhal", 8, 2, &["Soundtrack"]);
    let unknown = RecordingFacts { title: "New York Nagaram".into(), ..Default::default() };
    let o = classify(
        &listen("New York Nagaram", "A.R. Rahman", Some("Sillunu Oru Kaadhal")),
        Some(&unknown),
        None,
        Some(&rel),
        None,
    );
    assert_eq!(o.facets & (SCORE | SCORE_SUGGESTED), 0);
    // A background score album says so in its title.
    let bgm = soundtrack("Vikram (Original Background Score)", 20, 20, &["Soundtrack"]);
    let o = classify(
        &listen("Vikram Theme", "Anirudh Ravichander", None),
        Some(&unknown),
        None,
        Some(&bgm),
        None,
    );
    assert_ne!(o.facets & SCORE, 0);
}

#[test]
fn name_only_listens_find_their_composer() {
    // Pano Scrobbler's comma-joined credit.
    let l = listen(
        "Swan Lake",
        "London Symphony Orchestra, Pyotr Ilyich Tchaikovsky",
        Some("Classical Chillout"),
    );
    let o = classify(&l, None, None, None, None);
    assert_ne!(o.facets & CLASSICAL, 0);
    assert_eq!(o.composer_name.as_deref(), Some("Pyotr Ilyich Tchaikovsky"));
    assert_eq!(o.composer_mbid.as_deref(), Some("9ddd7abc-9e1b-471d-8031-583bc6bc8be9"));

    let o = classify(
        &listen("Etude Op. 25 No. 11 (Winter Wind)", "Chopin", None),
        None,
        None,
        None,
        None,
    );
    assert_eq!(o.composer_name.as_deref(), Some("Frédéric Chopin"));

    let o = classify(
        &listen(
            "Johann Sebastian Bach: Toccata and Fugue in d-minor (arranged for brass ensemble) V",
            "Christian Lindberg",
            None,
        ),
        None,
        None,
        None,
        None,
    );
    assert_ne!(o.facets & CLASSICAL, 0);
    assert_eq!(o.composer_name.as_deref(), Some("Johann Sebastian Bach"));

    let o = classify(&listen("Youth Gone Wild", "Sebastian Bach", None), None, None, None, None);
    assert_eq!(o.facets & CLASSICAL, 0, "Skid Row");
}

#[test]
fn spellings_of_one_movement_share_a_piece() {
    let a = classify(
        &listen(
            "Violin Partita No. 2 in D Minor, BWV 1004: V. Ciaccona",
            "Johann Sebastian Bach",
            None,
        ),
        None,
        None,
        None,
        None,
    );
    let b = classify(
        &listen("Partita for Violin No.2 in D minor, BWV 1004: Chaconne", "J.S. Bach", None),
        None,
        None,
        None,
        None,
    );
    assert_eq!(a.piece_key, b.piece_key);
    assert_eq!(a.movement.as_deref(), Some("V. Ciaccona"));
}

#[test]
fn ambient_is_the_albums_top_genre_never_one_vote() {
    let saw = parse_group_facts(&fixture("mb_release_group_saw2.json"));
    let o = classify(
        &listen("#3", "Aphex Twin", None),
        None,
        None,
        Some(&soundtrack("Selected Ambient Works Volume II", 24, 24, &[])),
        Some(&saw),
    );
    assert_ne!(o.facets & AMBIENT, 0);
    let dream_pop =
        GroupFacts { genres: vec![("dream pop".into(), 4), ("ambient".into(), 2)], tags: vec![] };
    let o = classify(
        &listen("Apocalypse", "Cigarettes After Sex", None),
        None,
        None,
        None,
        Some(&dream_pop),
    );
    assert_eq!(o.facets & AMBIENT, 0);
}

#[test]
fn seasons_from_words_but_an_album_only_for_christmas() {
    let o = classify(
        &listen("Blue Christmas", "Chris Isaak", Some("Christmas")),
        None,
        None,
        None,
        None,
    );
    assert_ne!(o.facets & CHRISTMAS, 0);
    let o = classify(
        &listen("Winter Wonderland", "Chris Isaak", Some("Christmas")),
        None,
        None,
        None,
        None,
    );
    assert_ne!(o.facets & CHRISTMAS, 0, "a track on a Christmas album");
    let o = classify(
        &listen("Because the Night", "Patti Smith Group", Some("Easter")),
        None,
        None,
        None,
        None,
    );
    assert_eq!(o.facets & EASTER, 0, "Patti Smith's Easter is not an Easter album");
    let o = classify(&listen("Holiday", "Madonna", None), None, None, None, None);
    assert_eq!(o.facets & (CHRISTMAS | HALLOWEEN | EASTER), 0);
}

#[test]
fn a_choir_on_a_score_cue_does_not_make_it_a_song() {
    let mut rec = recording(
        "The Breaking of the Fellowship",
        work("The Breaking of the Fellowship", &["zxx"], false),
    );
    rec.performers.push(Performer {
        name: "London Voices".into(),
        mbid: "x".into(),
        rel: "vocal".into(),
        attribute: None,
        artist_type: Some("Choir".into()),
        country: None,
    });
    let rel =
        soundtrack("The Lord of the Rings: The Fellowship of the Ring", 18, 17, &["Soundtrack"]);
    let o = classify(
        &listen("The Breaking of the Fellowship", "Howard Shore", None),
        Some(&rec),
        None,
        Some(&rel),
        None,
    );
    assert_ne!(o.facets & SCORE, 0);
}

#[test]
fn roles_from_names() {
    assert_eq!(role_from_name("London Symphony Orchestra"), Some("orchestra"));
    assert_eq!(role_from_name("Berliner Philharmoniker"), Some("orchestra"));
    assert_eq!(role_from_name("Emerson String Quartet"), Some("ensemble"));
    assert_eq!(role_from_name("Hilary Hahn"), None);
}

#[test]
fn overrides_apply_album_first_then_work() {
    use crate::db::FacetOverride;
    let o = |kind: &str, key: &str, facet, value| FacetOverride {
        target_kind: kind.into(),
        target_key: key.into(),
        facet,
        value,
    };
    let album = "christmas\u{1f}chris isaak";
    let overrides = vec![
        o("album", album, CHRISTMAS, true),
        o("work", "t:pretty woman|chris isaak", CHRISTMAS, false),
        o("album", "il buono\u{1f}ennio morricone", SCORE, true),
    ];
    assert_eq!(
        apply_overrides(0, "t:blue christmas|chris isaak", Some(album), &[], &overrides),
        CHRISTMAS
    );
    assert_eq!(
        apply_overrides(0, "t:pretty woman|chris isaak", Some(album), &[], &overrides),
        0,
        "work wins"
    );
    assert_eq!(
        apply_overrides(
            SCORE_SUGGESTED,
            "w:x",
            Some("il buono\u{1f}ennio morricone"),
            &[],
            &overrides
        ),
        SCORE_SUGGESTED | SCORE
    );
}

#[test]
fn an_artist_override_moves_everything_by_them_and_a_piece_can_still_opt_out() {
    use crate::db::FacetOverride;
    let o = |kind: &str, key: &str, facet, value| FacetOverride {
        target_kind: kind.into(),
        target_key: key.into(),
        facet,
        value,
    };
    let overrides = vec![
        o("artist", "gibran alcocer", CLASSICAL, true),
        o("work", "t:idea 22 anya nami remix|gibran alcocer", CLASSICAL, false),
    ];
    let gibran = vec!["gibran alcocer".to_string()];
    assert_eq!(apply_overrides(0, "t:idea 10|gibranalcocer", None, &gibran, &overrides), CLASSICAL);
    assert_eq!(
        apply_overrides(0, "t:idea 22 anya nami remix|gibran alcocer", None, &gibran, &overrides),
        0
    );
    assert_eq!(
        apply_overrides(0, "t:idea 10|someone", None, &["someone".to_string()], &overrides),
        0
    );
}

/// The period fallback for an undated piece — the composer's birth year plus
/// 35 — reproduces the era classes of the Cross-Era dataset (Weiß, Mauch,
/// Dixon & Müller, *Musicae Scientiae* 2019) for every one of its composers on
/// our list. Their "Modern" is the twentieth century, which we split in two;
/// their transitional add-on class (Beethoven, Schubert, Debussy, Ravel,
/// Mahler) is left out, being transitional by definition.
#[test]
fn the_period_fallback_agrees_with_cross_era() {
    let cross_era: &[(&str, &[&str])] = &[
        (
            "Baroque",
            &[
                "Johann Sebastian Bach",
                "George Frideric Handel",
                "Antonio Vivaldi",
                "Arcangelo Corelli",
            ],
        ),
        ("Classical", &["Joseph Haydn", "Wolfgang Amadeus Mozart", "Muzio Clementi"]),
        (
            "Romantic",
            &[
                "Johannes Brahms",
                "Franz Liszt",
                "Robert Schumann",
                "Frédéric Chopin",
                "Pyotr Ilyich Tchaikovsky",
            ],
        ),
        (
            "20th century",
            &[
                "Igor Stravinsky",
                "Arnold Schoenberg",
                "Béla Bartók",
                "Dmitri Shostakovich",
                "Anton Webern",
            ],
        ),
    ];
    for (era, names) in cross_era {
        for name in *names {
            let c = canon::by_name(name).unwrap_or_else(|| panic!("{name} on the list"));
            let (year, dated) = placed_year(None, Some(c.born)).unwrap();
            assert!(!dated);
            let ours = period(year);
            let agrees = match *era {
                "20th century" => ours == "Early 20th century" || ours == "Modern",
                e => ours == e,
            };
            assert!(agrees, "{name}: {ours}, Cross-Era says {era}");
        }
    }
}
