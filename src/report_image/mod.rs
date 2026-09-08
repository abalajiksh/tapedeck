//! A report chapter as an image, rendered on the server.
//!
//! **Why the server rather than a canvas in the browser.** A share image is a
//! deliverable, not a view: an Android or iOS client should ask for one rather
//! than reimplementing a layout, a type scale and a palette that would then be
//! free to drift from the web's. Rendering it here also makes it
//! *deterministic* — the fonts ride in the binary, no artwork is fetched, and
//! the same period produces the same bytes on a laptop, a Pi or a phone.
//!
//! The stack is `resvg` + `usvg` + `tiny-skia` + `fontdb`: pure Rust, no C
//! toolchain, no `-sys` crate anywhere in the subtree. That constraint has
//! decided crate choices twice in this tree and it decided this one.
//!
//! **No network call happens inside the render**, which is the single largest
//! thing that keeps it cheap and safe. usvg does not fetch remote `href`s, so
//! artwork would have to be fetched and inlined as data URIs — and the report
//! payload carries none anyway. Reports already settled that the giant initial
//! is the *design* rather than a placeholder for a photo that never arrives, so
//! the card uses that and the period's own blended colour, which is the most
//! personal thing on it.

pub mod card;
pub mod fonts;
pub mod layout;
pub mod month;
pub mod sitting;
pub mod svg;
pub mod theme;
pub mod week;
pub mod year;

pub use card::Card;
pub use layout::{Size, Unit};
pub use sitting::SittingCard;
pub use theme::Theme;

/// What a render produced, and what it could not promise about it.
pub struct Rendered {
    pub png: Vec<u8>,
    pub svg: String,
    /// False when a string **that was actually drawn** contains a character the
    /// embedded faces cannot draw — Hangul, CJK, Cyrillic.
    ///
    /// It is not a failure and the image is still returned: the host's own
    /// fonts are loaded behind ours and may well cover it. What it means is
    /// that this server cannot *promise* the image is free of tofu, and saying
    /// so is the discipline a source declaring its blind spots already follows.
    /// The alternative is a Korean title coming out as boxes with nothing
    /// anywhere admitting it.
    pub fully_covered: bool,
}

#[derive(Debug)]
pub enum RenderError {
    /// The document we generated is not valid SVG. This is our bug, never the
    /// caller's, so it is reported as one.
    Svg(String),
    /// The pixmap could not be allocated — a size so large the buffer overflows.
    Pixmap,
    Encode(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Svg(e) => write!(f, "the generated document is not valid SVG: {e}"),
            RenderError::Pixmap => write!(f, "could not allocate the image"),
            RenderError::Encode(e) => write!(f, "could not encode the PNG: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Draw a card and rasterise it.
///
/// **Synchronous and CPU-bound on purpose** — the caller runs it on
/// `spawn_blocking`. A 1080×1920 pixmap is ~8 MB of RGBA and rasterising it in
/// an async handler pins a tokio worker, one of four on a Pi, which is the
/// mistake `report_rows`' fold already had to be moved off.
pub fn render(c: &Card, unit: Unit, size: Size, theme: Theme) -> Result<Rendered, RenderError> {
    // Three units, three compositions — never one card with a different label.
    // A week has no clock and no genre split, a year is a bento over twelve
    // months, and a month is neither; the chapters on the page are three
    // components for exactly this reason.
    let drawn = match unit {
        Unit::Week => week::render_measured(c, size, theme),
        Unit::Month => month::render_measured(c, size, theme),
        Unit::Year => year::render_measured(c, size, theme),
    };

    rasterise(drawn, size)
}

/// Draw an evening and rasterise it.
///
/// A separate entry rather than a fourth `Unit`, because a sitting is not a
/// period — see `sitting.rs`. Everything below the composition is shared, which
/// is the half worth sharing: one font database, one rasteriser, one set of
/// failure modes.
pub fn render_sitting(c: &SittingCard, size: Size, theme: Theme) -> Result<Rendered, RenderError> {
    let drawn = sitting::render_measured(c, size, theme);
    rasterise(drawn, size)
}

/// Turn a drawn document into a PNG. Shared so the two entries above cannot
/// come to disagree about fonts or pixmap size.
fn rasterise(drawn: layout::Drawn, size: Size) -> Result<Rendered, RenderError> {
    // The default font family is whatever the host calls sans-serif. Ours is
    // embedded and is the only one whose metrics the layouts were measured
    // against.
    let mut opt =
        usvg::Options { fontdb: std::sync::Arc::new(fonts::db().clone()), ..Default::default() };
    opt.font_family = fonts::BODY.to_string();

    let tree =
        usvg::Tree::from_str(&drawn.svg, &opt).map_err(|e| RenderError::Svg(e.to_string()))?;

    let (w, h) = size.dims();
    let mut pixmap = tiny_skia::Pixmap::new(w as u32, h as u32).ok_or(RenderError::Pixmap)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());

    let png = pixmap.encode_png().map_err(|e| RenderError::Encode(e.to_string()))?;
    Ok(Rendered { png, svg: drawn.svg, fully_covered: drawn.covered })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_month() -> Card {
        Card::sample(Unit::Month)
    }

    /// The whole point, end to end: every shape and both themes produce a real
    /// PNG of the size that was asked for.
    ///
    /// Asserted on the **decoded header** rather than on the byte length,
    /// because a document usvg failed to lay out still encodes perfectly well —
    /// as a blank pixmap of exactly the right size, which is the failure this
    /// would otherwise wave through.
    #[test]
    fn every_shape_and_theme_rasterises_to_a_png_of_the_right_size() {
        for (unit, card) in [Unit::Week, Unit::Month, Unit::Year].map(|u| (u, Card::sample(u))) {
            for size in [Size::Story, Size::Square, Size::Wide] {
                for theme in [Theme::Dark, Theme::Light] {
                    let r = render(&card, unit, size, theme).expect("render");
                    let (w, h) = size.dims();
                    assert_eq!(
                        &r.png[..8],
                        b"\x89PNG\r\n\x1a\n",
                        "{size:?}/{theme:?} is not a PNG"
                    );
                    // IHDR width and height, big-endian, at a fixed offset.
                    let px = u32::from_be_bytes(r.png[16..20].try_into().unwrap());
                    let py = u32::from_be_bytes(r.png[20..24].try_into().unwrap());
                    assert_eq!((px, py), (w as u32, h as u32), "{unit:?}/{size:?}");
                    assert!(r.fully_covered, "a Latin card must be fully covered");
                }
            }
        }
    }

    /// A blank image is what a silently-failed layout looks like, so the test
    /// that matters is that something was actually drawn.
    ///
    /// If the font families were misnamed, or usvg dropped every `<text>`, the
    /// PNG would still be the right size and still decode — it would just be
    /// one flat colour. Counting distinct pixels is the cheap way to tell those
    /// apart, and it fails if text ever stops rendering.
    #[test]
    fn the_card_is_actually_drawn_and_not_a_flat_rectangle() {
        let r = render(&a_month(), Unit::Month, Size::Square, Theme::Dark).expect("render");
        let pix = tiny_skia::Pixmap::decode_png(&r.png).expect("decode");
        let mut seen = std::collections::HashSet::new();
        for p in pix.pixels() {
            seen.insert((p.red(), p.green(), p.blue()));
            if seen.len() > 32 {
                break;
            }
        }
        assert!(seen.len() > 32, "only {} distinct colours — nothing was drawn", seen.len());
    }

    /// The weight the layout asked for is the weight that got drawn.
    ///
    /// The variable file's default instance is Light. If usvg ignored the
    /// `font-weight` on a `<text>`, every card would render thin — legible,
    /// plausible, and not what was measured. Comparing ink between two weights
    /// is the only check that can tell those apart, since both produce a
    /// perfectly valid PNG of the right size.
    #[test]
    fn the_weight_the_layout_asked_for_is_the_weight_that_got_drawn() {
        let ink = |face: fonts::Face| {
            let mut d = svg::Doc::new(600.0, 120.0);
            d.rect(0.0, 0.0, 600.0, 120.0, "#ffffff");
            d.text(20.0, 90.0, "Cœur de pirate", face, 64.0, "#000000", svg::Anchor::Start);
            let opt = usvg::Options {
                fontdb: std::sync::Arc::new(fonts::db().clone()),
                font_family: fonts::BODY.to_string(),
                ..Default::default()
            };
            let tree = usvg::Tree::from_str(&d.finish(), &opt).expect("svg");
            let mut px = tiny_skia::Pixmap::new(600, 120).unwrap();
            resvg::render(&tree, tiny_skia::Transform::identity(), &mut px.as_mut());
            px.pixels().iter().filter(|p| p.red() < 128).count()
        };
        let (regular, heavy) = (ink(fonts::Face::Regular), ink(fonts::Face::SemiBold));
        assert!(regular > 0, "nothing was drawn at all");
        assert!(
            heavy > regular,
            "semibold laid down {heavy} dark pixels against regular's {regular} — \
             the weight axis is not reaching the raster"
        );
    }

    /// Writes every shape and theme to `TAPEDECK_CARD_DIR` so they can be
    /// looked at.
    ///
    /// Ignored by default, like `tidal_live_search`. Nothing here can tell you
    /// a card is *ugly* — every assertion above passes on a layout with two
    /// panels overlapping — and this feature is one where the only real check
    /// is opening the picture. Run with:
    /// `TAPEDECK_CARD_DIR=/tmp/cards cargo test -- --ignored render_the_cards`
    /// All three units, because three compositions is three times as much that
    /// can look wrong while every assertion passes.
    #[test]
    #[ignore]
    fn render_the_cards_to_look_at() {
        let dir = std::env::var("TAPEDECK_CARD_DIR").expect("set TAPEDECK_CARD_DIR");
        std::fs::create_dir_all(&dir).expect("mkdir");
        for unit in [Unit::Week, Unit::Month, Unit::Year] {
            let card = Card::sample(unit);
            for size in [Size::Story, Size::Square, Size::Wide] {
                for theme in [Theme::Dark, Theme::Light] {
                    let r = render(&card, unit, size, theme).expect("render");
                    let path =
                        format!("{dir}/{}-{}-{}.png", unit.as_str(), size.as_str(), theme.as_str());
                    std::fs::write(&path, &r.png).expect("write");
                    println!("{path} ({} bytes)", r.png.len());
                }
            }
        }
        let sitting = SittingCard::sample();
        for size in [Size::Story, Size::Square, Size::Wide] {
            for theme in [Theme::Dark, Theme::Light] {
                let r = render_sitting(&sitting, size, theme).expect("render");
                let path = format!("{dir}/sitting-{}-{}.png", size.as_str(), theme.as_str());
                std::fs::write(&path, &r.png).expect("write");
                println!("{path} ({} bytes)", r.png.len());
            }
        }
    }

    /// A sitting rasterises at every shape and both themes, and is actually
    /// drawn rather than a flat rectangle.
    ///
    /// The two guards the report cards already carry, applied to the fourth
    /// composition — a document usvg failed to lay out still encodes perfectly
    /// well, as a blank pixmap of exactly the right size.
    #[test]
    fn a_sitting_rasterises_at_every_shape_and_is_actually_drawn() {
        let c = SittingCard::sample();
        for size in [Size::Story, Size::Square, Size::Wide] {
            for theme in [Theme::Dark, Theme::Light] {
                let r = render_sitting(&c, size, theme).expect("render");
                let (w, h) = size.dims();
                assert_eq!(&r.png[..8], b"\x89PNG\r\n\x1a\n", "{size:?}/{theme:?}");
                let px = u32::from_be_bytes(r.png[16..20].try_into().unwrap());
                let py = u32::from_be_bytes(r.png[20..24].try_into().unwrap());
                assert_eq!((px, py), (w as u32, h as u32), "{size:?}");
                assert!(r.fully_covered, "a Latin card must be fully covered");
            }
        }

        let r = render_sitting(&c, Size::Square, Theme::Dark).expect("render");
        let pix = tiny_skia::Pixmap::decode_png(&r.png).expect("decode");
        let mut seen = std::collections::HashSet::new();
        for p in pix.pixels() {
            seen.insert((p.red(), p.green(), p.blue()));
            if seen.len() > 32 {
                break;
            }
        }
        assert!(seen.len() > 32, "only {} distinct colours — nothing was drawn", seen.len());
    }

    /// An evening that reported no running time says nothing about one.
    ///
    /// Nothing reported a length and "no time passed" are different statements,
    /// and only one of them is ever true of an evening somebody sat through —
    /// the em-dash discipline the quality card already follows, which is why
    /// `running()` returns `None` rather than a zero to format.
    #[test]
    fn an_evening_with_no_running_time_does_not_claim_one() {
        let mut c = SittingCard::sample();
        c.seconds = 0;
        c.duration_known = 0;
        assert_eq!(c.running(), None);
        assert!(render_sitting(&c, Size::Story, Theme::Dark).is_ok());

        // And a sitting of one play is a record rather than records.
        let mut one = SittingCard::sample();
        one.plays = 1;
        assert!(render_sitting(&one, Size::Square, Theme::Light).is_ok());
    }

    /// A record too long to fit is shortened, never silently dropped.
    ///
    /// `facts` skips a line it cannot fit, which is right for a fact nobody
    /// promised and wrong under a count: the record would disappear *and* the
    /// "and N more" beneath it would undercount by exactly the ones that
    /// vanished, so the card would quietly claim a shorter evening.
    #[test]
    fn a_long_record_title_is_shortened_rather_than_dropped() {
        let mut c = SittingCard::sample();
        c.played = vec![
            (
                "The Rise and Fall of a Midwest Princess (Deluxe Edition)".into(),
                "Somebody With a Very Long Name Indeed".into(),
            ),
            ("Untrue".into(), "Burial".into()),
        ];
        let drawn = sitting::render_measured(&c, Size::Story, Theme::Dark);
        assert!(
            drawn.svg.contains('…'),
            "the long title should be ellipsised into the card, not left out of it"
        );
        assert!(drawn.svg.contains("Untrue"), "and the rest of the list still drew");
    }

    /// A guest list is capped rather than allowed to become a paragraph.
    #[test]
    fn the_company_line_is_a_sentence_and_not_a_roster() {
        let names = |n: usize| {
            let mut c = SittingCard::sample();
            c.with = (0..n).map(|i| format!("Person{i}")).collect();
            c.company()
        };
        assert_eq!(names(0), "On your own");
        assert_eq!(names(1), "with Person0");
        assert_eq!(names(2), "with Person0 and Person1");
        assert_eq!(names(3), "with Person0, Person1 and Person2");
        assert_eq!(names(6), "with Person0, Person1 and 4 others");
    }

    /// A Hangul title is rendered rather than refused, and the result says the
    /// embedded faces could not promise it.
    #[test]
    fn a_title_the_embedded_faces_cannot_draw_is_reported_and_not_refused() {
        let mut c = a_month();
        c.top_album = Some(card::Titled {
            title: "밤하늘의 저 별처럼".into(),
            artist: "헤이즈".into(),
            plays: 12,
        });
        let r = render(&c, Unit::Month, Size::Square, Theme::Dark).expect("render");
        assert!(!r.png.is_empty(), "the image is still produced");
        assert!(!r.fully_covered, "and the caller is told it may contain tofu");
    }
}
