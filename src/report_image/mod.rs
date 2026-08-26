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
pub mod month;
pub mod svg;
pub mod theme;

pub use card::Card;
pub use month::Size;
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
pub fn render(c: &Card, size: Size, theme: Theme) -> Result<Rendered, RenderError> {
    let drawn = month::render_measured(c, size, theme);

    let mut opt =
        usvg::Options { fontdb: std::sync::Arc::new(fonts::db().clone()), ..Default::default() };
    // The default is whatever face the host calls sans-serif. Ours is embedded
    // and is the only one whose metrics the layout above was measured against.
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
        Card {
            label: "August 2026".into(),
            plays: 4182,
            hours: 214.6,
            avg_daily: 135,
            rank: 2,
            rank_of: 14,
            artists: 214,
            albums: 318,
            tracks: 902,
            colour: Some("#a18430".into()),
            top_artist: Some(card::Named { name: "Cœur de pirate".into(), plays: 612 }),
            top_album: Some(card::Titled {
                title: "Blonde".into(),
                artist: "Cœur de pirate".into(),
                plays: 96,
            }),
            genres: vec![
                ("dance-pop".into(), 41),
                ("chanson".into(), 22),
                ("french pop".into(), 14),
                ("indie poptimism".into(), 8),
            ],
            on_repeat: vec![
                card::Titled {
                    title: "Comme des enfants".into(),
                    artist: "Cœur de pirate".into(),
                    plays: 9,
                },
                card::Titled {
                    title: "Ne m'appelle pas Fanny".into(),
                    artist: "Clara Luciani".into(),
                    plays: 7,
                },
            ],
            crush: Some(card::Named { name: "Clara Luciani".into(), plays: 63 }),
            top_chain: Some("Desk2 · HD 650".into()),
        }
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
        for size in [Size::Story, Size::Square, Size::Wide] {
            for theme in [Theme::Dark, Theme::Light] {
                let r = render(&a_month(), size, theme).expect("render");
                let (w, h) = size.dims();
                assert_eq!(&r.png[..8], b"\x89PNG\r\n\x1a\n", "{size:?}/{theme:?} is not a PNG");
                // IHDR width and height, big-endian, at a fixed offset.
                let px = u32::from_be_bytes(r.png[16..20].try_into().unwrap());
                let py = u32::from_be_bytes(r.png[20..24].try_into().unwrap());
                assert_eq!((px, py), (w as u32, h as u32), "{size:?}");
                assert!(r.fully_covered, "a Latin card must be fully covered");
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
        let r = render(&a_month(), Size::Square, Theme::Dark).expect("render");
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
    #[test]
    #[ignore]
    fn render_the_cards_to_look_at() {
        let dir = std::env::var("TAPEDECK_CARD_DIR").expect("set TAPEDECK_CARD_DIR");
        std::fs::create_dir_all(&dir).expect("mkdir");
        for size in [Size::Story, Size::Square, Size::Wide] {
            for theme in [Theme::Dark, Theme::Light] {
                let r = render(&a_month(), size, theme).expect("render");
                let path = format!("{dir}/{}-{}.png", size.as_str(), theme.as_str());
                std::fs::write(&path, &r.png).expect("write");
                println!("{path} ({} bytes)", r.png.len());
            }
        }
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
        let r = render(&c, Size::Square, Theme::Dark).expect("render");
        assert!(!r.png.is_empty(), "the image is still produced");
        assert!(!r.fully_covered, "and the caller is told it may contain tofu");
    }
}
