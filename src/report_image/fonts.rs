//! The fonts the share image is drawn with, and how wide a string will be.
//!
//! **They are embedded rather than looked up.** `app.html` pulls Figtree and
//! Caprasimo from Google Fonts, so the *browser* has them and the server has
//! neither — and `fontdb::Database::load_system_fonts()` on a minimal Pi image
//! finds nothing at all, which renders every string as blank space with nothing
//! erroring. That is the silent class of failure this codebase keeps finding, so
//! the bytes ride in the binary: ~110 KB, OFL 1.1, licence beside them.
//!
//! Embedding also makes the render **deterministic** — the same period produces
//! the same PNG on a laptop, a Pi and a phone's server — which is most of the
//! reason the render is server-side at all.
//!
//! System fonts are loaded *behind* them as a fallback, and that is the only
//! thing standing between a Hangul or CJK title and a row of tofu: both faces
//! here are Latin (222 glyphs, no Hangul, no CJK, not even `ā`). See
//! [`covers`] — the renderer declares that blind spot rather than drawing
//! boxes, the same discipline a source declaring what it cannot see follows.

use std::sync::OnceLock;

use skrifa::instance::Size as SkSize;
use skrifa::metrics::GlyphMetrics;
use skrifa::{FontRef, MetadataProvider};

/// The body face — **one variable file, not three static instances.**
///
/// Google's static Figtree instances all declare the family `"Figtree Light"`
/// (name ID 1), so an SVG asking for `"Figtree"` matches nothing, usvg falls
/// back to whatever the host calls sans-serif, and the image renders in a face
/// nobody chose — measured against a different face's advances. Completely
/// silent on a machine that has fonts, and blank on one that does not. The
/// variable file declares the *typographic* family (name ID 16) as `"Figtree"`,
/// which is the name we want, and at 62 KB it is smaller than the three
/// statics were together.
///
/// Its default instance is **Light**, so the weight axis is not a nicety: with
/// it unapplied every string comes out thin and plausible. See
/// `measuring_a_heavier_weight_is_wider_than_measuring_regular`.
pub const FIGTREE: &[u8] = include_bytes!("../../assets/fonts/Figtree-Variable.ttf");
/// The display face — `.h-display` everywhere in the app.
pub const CAPRASIMO: &[u8] = include_bytes!("../../assets/fonts/Caprasimo-Regular.ttf");

/// The variable axis carrying weight, and the tag skrifa wants for it.
const WGHT: &str = "wght";

/// The family names the SVG asks for. Taken from the files themselves rather
/// than assumed — see `the_embedded_families_are_the_names_the_svg_asks_for`.
pub const BODY: &str = "Figtree";
pub const DISPLAY: &str = "Caprasimo";

/// Which face a string is measured and drawn in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Regular,
    SemiBold,
    Display,
}

impl Face {
    /// The `font-family` list the SVG carries.
    ///
    /// Display names **both** faces, and the fallback is load-bearing rather
    /// than defensive: Caprasimo has no `·` (U+00B7), which is the separator
    /// this app uses everywhere, so a record whose title contains one would
    /// come out with a hole in it. SVG resolves a family list per glyph, so
    /// naming Figtree behind it costs nothing and closes the gap for every
    /// other character Caprasimo happens to lack.
    pub fn family(self) -> &'static str {
        match self {
            Face::Display => "Caprasimo, Figtree",
            _ => BODY,
        }
    }

    /// The CSS weight the SVG carries, so usvg picks the same file we measured.
    pub fn weight(self) -> u16 {
        match self {
            Face::Regular | Face::Display => 400,
            Face::SemiBold => 600,
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Face::Display => CAPRASIMO,
            _ => FIGTREE,
        }
    }
}

/// The font database handed to usvg, built once.
///
/// Embedded faces are loaded **first** so they win the family lookup, then the
/// host's own fonts as a fallback for scripts these two do not carry. Loading
/// system fonts is best-effort: a host with none is the normal case on a
/// container image and must not fail the render.
pub fn db() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_font_data(FIGTREE.to_vec());
        db.load_font_data(CAPRASIMO.to_vec());
        db.load_system_fonts();
        db.set_sans_serif_family(BODY);
        db.set_serif_family(DISPLAY);
        db
    })
}

/// How wide `text` is at `px` in `face`, in user units.
///
/// Raw glyph advances, not shaped ones — no kerning and no ligatures. That is
/// deliberate rather than a shortcut: kerning is almost always *negative*, so
/// this overestimates, and every caller uses the number to decide whether a
/// string fits. Overestimating truncates a character early in the rare case;
/// underestimating runs text off the edge of the image, which is the failure
/// nobody can correct after the PNG is saved.
///
/// A character no font in the file covers measures as the fallback advance
/// rather than zero, so an uncoverable string still reserves space instead of
/// collapsing to nothing.
pub fn width(text: &str, face: Face, px: f32) -> f32 {
    let Ok(font) = FontRef::new(face.bytes()) else { return 0.0 };
    let charmap = font.charmap();
    // At the *requested* weight. The variable file's default instance is Light,
    // so measuring at the default would size every string against letters
    // narrower than the ones actually drawn — and text laid out that way runs
    // off the edge of an image nobody can scroll.
    let location = font.axes().location([(WGHT, face.weight() as f32)]);
    let metrics: GlyphMetrics = font.glyph_metrics(SkSize::new(px), &location);
    let fallback = px * 0.6;
    text.chars()
        .map(|c| match charmap.map(c) {
            Some(gid) => metrics.advance_width(gid).unwrap_or(fallback),
            None => fallback,
        })
        .sum()
}

/// Whether the embedded faces can draw every character of `text`.
///
/// `false` does **not** mean the image is broken — the system fonts loaded
/// behind them may well cover it. It means this server cannot *promise* to, and
/// the endpoint says so rather than letting a Korean title come out as boxes
/// with nothing anywhere admitting it.
pub fn covers(text: &str) -> bool {
    let Ok(body) = FontRef::new(FIGTREE) else { return false };
    let Ok(display) = FontRef::new(CAPRASIMO) else { return false };
    let (bc, dc) = (body.charmap(), display.charmap());
    // Either face is enough. Display text names both families in its
    // `font-family`, so a glyph Caprasimo lacks is drawn from Figtree rather
    // than lost — which is exactly the `·` case, and requiring both here would
    // report tofu for a separator this app puts on nearly every card.
    text.chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .all(|c| bc.map(c).is_some() || dc.map(c).is_some())
}

/// Shorten `text` so it fits `max` units, appending an ellipsis when it cannot.
///
/// Cuts on **characters**, never bytes — an artist name is routinely not ASCII
/// and slicing a multi-byte character panics rather than merely looking wrong.
pub fn ellipsise(text: &str, face: Face, px: f32, max: f32) -> String {
    if width(text, face, px) <= max {
        return text.to_string();
    }
    let ell = "…";
    let room = max - width(ell, face, px);
    if room <= 0.0 {
        return ell.to_string();
    }
    let mut out = String::new();
    let mut used = 0.0;
    for c in text.chars() {
        let w = width(&c.to_string(), face, px);
        if used + w > room {
            break;
        }
        out.push(c);
        used += w;
    }
    // A trailing space before an ellipsis reads as a typo.
    while out.ends_with(' ') {
        out.pop();
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SVG asks for a family by name, so the name has to be the one the
    /// files actually declare.
    ///
    /// This is not a formality — it failed. Google's *static* Figtree instances
    /// declare `"Figtree Light"`, so a document asking for `"Figtree"` matched
    /// nothing: usvg silently fell back to the host's sans-serif and every card
    /// rendered in a face nobody chose, laid out against advances measured from
    /// a different one. On a machine with fonts that looks deliberate; on a bare
    /// container it is blank. Determined by loading the files rather than by
    /// reasoning about naming records — the rule the `MediaStream` serde
    /// collision earned.
    #[test]
    fn the_embedded_families_are_the_names_the_svg_asks_for() {
        let db = db();
        for family in [BODY, DISPLAY] {
            let q =
                fontdb::Query { families: &[fontdb::Family::Name(family)], ..Default::default() };
            let id = db.query(&q).unwrap_or_else(|| panic!("nothing registered as {family:?}"));
            let declared = db
                .face(id)
                .map(|f| f.families.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>())
                .expect("face metadata");
            assert!(
                declared.iter().any(|n| n == family),
                "{family:?} resolved to a face declaring {declared:?}"
            );
        }
    }

    /// A weight the layout asks for is a weight the measurement uses.
    ///
    /// The variable file's default instance is **Light**, so an unapplied axis
    /// is not a missing feature — it is every string on the card measured
    /// against letters narrower than the ones drawn, which walks text off an
    /// edge nobody can scroll back from.
    #[test]
    fn measuring_a_heavier_weight_is_wider_than_measuring_regular() {
        let s = "Cœur de pirate";
        let (r, b) = (width(s, Face::Regular, 60.0), width(s, Face::SemiBold, 60.0));
        assert!(
            b > r,
            "semibold measured {b} against regular {r} — the weight axis is not applied"
        );
    }

    /// Wider text measures wider, and a bigger size measures bigger. The
    /// absolute numbers are the font's business; the ordering is ours, and it
    /// is what every truncation decision rests on.
    #[test]
    fn width_grows_with_the_string_and_with_the_size() {
        assert!(width("mm", Face::Regular, 40.0) > width("m", Face::Regular, 40.0));
        assert!(width("Burial", Face::Regular, 80.0) > width("Burial", Face::Regular, 40.0));
        assert!(width("", Face::Regular, 40.0) == 0.0);
    }

    /// Truncation cuts on characters, not bytes.
    ///
    /// Half this history is accented or non-ASCII, so a byte slice would panic
    /// on the names most likely to be too long in the first place.
    #[test]
    fn a_long_name_is_cut_between_characters_and_never_inside_one() {
        let name = "Cœur de pirate et un très très long nom d'artiste";
        let short = ellipsise(name, Face::Regular, 40.0, 300.0);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() < name.chars().count());
        assert!(width(&short, Face::Regular, 40.0) <= 300.0);
        // Nothing to do: a name that already fits comes back untouched.
        assert_eq!(ellipsise("Zaz", Face::Regular, 40.0, 300.0), "Zaz");
    }

    /// The blind spot is real and is asserted, so nobody assumes these faces
    /// cover a library they do not.
    #[test]
    fn the_embedded_faces_are_latin_and_say_so() {
        assert!(covers("Cœur de pirate"), "the top artist here must render");
        assert!(covers("Alizée · Why Don’t We"), "accents and a curly apostrophe");
        assert!(!covers("밤하늘의 저 별처럼"), "Hangul is not in a Latin font");
    }
}
