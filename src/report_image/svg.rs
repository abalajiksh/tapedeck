//! A very small SVG writer.
//!
//! Not a general one — just enough to place text, rectangles and paths at
//! computed coordinates. There is no layout here on purpose: usvg has no
//! flexbox, no text wrapping and no way to ask how tall a paragraph came out,
//! so every position is worked out in Rust before anything is written. That is
//! also why the share image is its own composition rather than a rendering of
//! the chapter — `MonthChapter.svelte` is 404 lines of DOM that only a browser
//! can lay out.

use std::fmt::Write;

use super::fonts::{self, Face};

/// Escapes text for an XML text node or a quoted attribute.
///
/// Every string on this card is user data — artist names, album titles, a
/// chain's name, a person's name in `company`. `AC/DC & Friends` is an ordinary
/// name and `<` is legal in one, so this is not a formality: an unescaped
/// ampersand makes the document unparseable and usvg returns an error for the
/// whole image rather than one bad line.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // A control character is not renderable and can terminate parsing.
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// Where a string sits relative to its `x`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

impl Anchor {
    fn as_str(self) -> &'static str {
        match self {
            Anchor::Start => "start",
            Anchor::Middle => "middle",
            Anchor::End => "end",
        }
    }
}

/// An SVG document under construction.
pub struct Doc {
    defs: String,
    body: String,
    next_id: u32,
    covered: bool,
    pub width: f32,
    pub height: f32,
}

impl Doc {
    pub fn new(width: f32, height: f32) -> Doc {
        Doc {
            defs: String::new(),
            body: String::with_capacity(8 * 1024),
            next_id: 0,
            covered: true,
            width,
            height,
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: &str) -> &mut Self {
        let _ = write!(
            self.body,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="{}"/>"#,
            esc(fill)
        );
        self
    }

    pub fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: &str) -> &mut Self {
        let _ = write!(
            self.body,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="{r:.2}" ry="{r:.2}" fill="{}"/>"#,
            esc(fill)
        );
        self
    }

    /// A rounded rect at less than full opacity.
    ///
    /// Separate from `rounded` rather than an `Option` on it, because every
    /// existing caller is opaque and threading a parameter through them to say
    /// so would make the common case read as the exception. Used by the genre
    /// bands, where the tints are opacities of the period's own colour rather
    /// than three hues invented here.
    #[allow(clippy::too_many_arguments)]
    pub fn rounded_opacity(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        fill: &str,
        op: f32,
    ) -> &mut Self {
        let _ = write!(
            self.body,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="{r:.2}" ry="{r:.2}" fill="{}" fill-opacity="{op:.3}"/>"#,
            esc(fill)
        );
        self
    }

    /// A hairline, drawn as a filled rect rather than a stroked line: a stroke
    /// straddles its coordinate, so a 1px rule at an integer y lands half in
    /// each row of pixels and renders as two grey rows instead of one.
    pub fn rule(&mut self, x: f32, y: f32, w: f32, h: f32, fill: &str, op: f32) -> &mut Self {
        let _ = write!(
            self.body,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="{}" fill-opacity="{op:.3}"/>"#,
            esc(fill)
        );
        self
    }

    /// One line of text. `y` is the **baseline**, which is what SVG means by it
    /// — callers add the cap height themselves rather than guessing.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        x: f32,
        y: f32,
        s: &str,
        face: Face,
        px: f32,
        fill: &str,
        anchor: Anchor,
    ) -> &mut Self {
        self.text_tracked(x, y, s, face, px, fill, anchor, 0.0, 1.0)
    }

    /// As [`Doc::text`], with letter-spacing and opacity — the two things the
    /// app's own small-caps labels and muted lines need.
    #[allow(clippy::too_many_arguments)]
    pub fn text_tracked(
        &mut self,
        x: f32,
        y: f32,
        s: &str,
        face: Face,
        px: f32,
        fill: &str,
        anchor: Anchor,
        tracking: f32,
        opacity: f32,
    ) -> &mut Self {
        if s.is_empty() {
            return self;
        }
        // Checked here rather than over the card's whole vocabulary, because
        // this is the only place that knows what was *actually* drawn. A square
        // holds no repeats list, so asking the card would report a Korean title
        // it never draws and warn about tofu that is not in the picture.
        self.covered &= fonts::covers(s);
        let _ = write!(
            self.body,
            r#"<text x="{x:.2}" y="{y:.2}" font-family="{}" font-weight="{}" font-size="{px:.2}" fill="{}" fill-opacity="{opacity:.3}" text-anchor="{}" letter-spacing="{tracking:.2}" xml:space="preserve">{}</text>"#,
            esc(face.family()),
            face.weight(),
            esc(fill),
            anchor.as_str(),
            esc(s),
        );
        self
    }

    /// A vertical wash of one colour fading to nothing.
    ///
    /// A *band* was tried first and rejected on the rendered image: a flat rect
    /// over the top third leaves a hard horizontal seam straight across the
    /// card — and, at story height, straight through the middle of the top
    /// artist row. A gradient has no edge to land in the wrong place, so the
    /// wash cannot collide with a section added later.
    pub fn wash(&mut self, h: f32, fill: &str, from: f32, to: f32) -> &mut Self {
        let id = format!("w{}", self.next_id);
        self.next_id += 1;
        let _ = write!(
            self.defs,
            r#"<linearGradient id="{id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{c}" stop-opacity="{from:.3}"/><stop offset="1" stop-color="{c}" stop-opacity="{to:.3}"/></linearGradient>"#,
            c = esc(fill)
        );
        let _ = write!(
            self.body,
            r#"<rect x="0" y="0" width="{:.2}" height="{h:.2}" fill="url(#{id})"/>"#,
            self.width
        );
        self
    }

    /// Whether the embedded faces could draw every string that reached the
    /// document. See `Rendered::fully_covered` for what the answer means.
    pub fn fully_covered(&self) -> bool {
        self.covered
    }

    /// The finished document.
    ///
    /// `width`/`height` are written in `px` **and** as a `viewBox`, because the
    /// two answer different questions: the box is the coordinate system every
    /// position above was computed in, and the size is what a renderer with no
    /// opinion of its own falls back to.
    pub fn finish(self) -> String {
        let defs = if self.defs.is_empty() {
            String::new()
        } else {
            format!("<defs>{}</defs>", self.defs)
        };
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{defs}{}</svg>"#,
            self.body,
            w = self.width,
            h = self.height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ampersand in an artist name must not take the whole image down.
    ///
    /// `Simon & Garfunkel` is in this history and is a name rather than a
    /// credit — the sanitiser leaves it alone deliberately — so it reaches the
    /// document exactly as stored. Unescaped it makes the SVG unparseable, and
    /// usvg fails the *document*, so one band would cost the entire card.
    #[test]
    fn a_name_with_markup_in_it_cannot_break_the_document() {
        assert_eq!(esc("Simon & Garfunkel"), "Simon &amp; Garfunkel");
        assert_eq!(esc("<script>"), "&lt;script&gt;");
        assert_eq!(esc(r#"a"b'c"#), "a&quot;b&apos;c");
        // Accents and curly quotes are ordinary characters and stay as they are.
        assert_eq!(esc("Cœur de pirate — Why Don’t We"), "Cœur de pirate — Why Don’t We");
    }

    #[test]
    fn a_document_carries_both_a_size_and_the_box_its_coordinates_are_in() {
        let mut d = Doc::new(1080.0, 1920.0);
        d.rect(0.0, 0.0, 1080.0, 1920.0, "#17120e");
        d.text(40.0, 100.0, "Burial", Face::Display, 72.0, "#f3e9d6", Anchor::Start);
        let out = d.finish();
        assert!(out.starts_with("<svg xmlns="));
        assert!(out.contains(r#"viewBox="0 0 1080 1920""#));
        assert!(out.contains("Caprasimo"));
        assert!(out.ends_with("</svg>"));
    }

    /// An empty string writes nothing at all, rather than an empty `<text>`
    /// element that usvg still has to lay out.
    /// Coverage is about the strings that reached the page.
    #[test]
    fn coverage_follows_what_was_written_and_not_what_was_offered() {
        let mut d = Doc::new(100.0, 100.0);
        d.text(0.0, 0.0, "Cœur de pirate", Face::Regular, 12.0, "#fff", Anchor::Start);
        assert!(d.fully_covered());
        d.text(0.0, 20.0, "밤하늘의 저 별처럼", Face::Regular, 12.0, "#fff", Anchor::Start);
        assert!(!d.fully_covered(), "a Hangul title that was drawn is reported");
    }

    #[test]
    fn an_empty_string_writes_no_element() {
        let mut d = Doc::new(10.0, 10.0);
        d.text(0.0, 0.0, "", Face::Regular, 12.0, "#fff", Anchor::Start);
        assert!(!d.finish().contains("<text"));
    }
}
