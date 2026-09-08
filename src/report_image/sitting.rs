//! An evening, as a card.
//!
//! **A sitting is the one thing in this application that has an ending**, and
//! until now ending one simply stopped it. This is the ending: what the evening
//! came to, in a shape you can hand to the people who were in it.
//!
//! It is a fourth composition rather than a fourth `Unit`, and that is the same
//! call the other three make about each other. A sitting has no period, so no
//! rank, no per-day rate and no bucket chart; what it has instead is **who was
//! in the room**, which none of the report cards have and which is the whole
//! subject here. Reusing their `Card` would have meant a struct where two
//! thirds of the fields were permanently zero and the one that mattered had
//! nowhere to go.
//!
//! What it does share is the type scale, the palette and the fit rule, so a
//! sitting card and a month card read as the same family — which they should,
//! since both are somebody's listening handed to somebody else.

use super::layout::*;
use super::svg::{Anchor, Doc};
use super::theme::{period_colour, Theme};
use crate::report_image::fonts::{ellipsise, Face};

/// What an evening came to. Built by the handler from `SpoolDetail`.
#[derive(Debug, Default, Clone)]
pub struct SittingCard {
    pub title: String,
    /// "Tuesday, 8 September 2026".
    pub date: String,
    /// "10:14 PM – 10:24 PM · 10 min", or that it is still running.
    pub span: String,
    /// Everybody else who was there, decks and guests alike — the reader is
    /// already left out by the time it reaches here.
    pub with: Vec<String>,
    pub plays: i64,
    pub seconds: i64,
    /// How many of those plays reported a length. **Travels with `seconds` or
    /// neither does**: a source that reported none contributes nothing, so the
    /// running time is a floor, and a card that stated it as a total would be
    /// claiming coverage it does not have.
    pub duration_known: i64,
    pub artists: i64,
    pub albums: i64,
    pub top_artist: Option<(String, i64)>,
    pub top_album: Option<(String, String, i64)>,
    /// Whose deck played how many, biggest first.
    pub decks: Vec<(String, i64)>,
    /// A few of the records, in the order they went on.
    ///
    /// **The most evocative thing a card of an evening can carry**, and the
    /// reason it is a *sequence* rather than a ranking: a sitting is a run of
    /// records somebody put on one after another, and the top three by play
    /// count says something about a month rather than about a night. Truncated
    /// by the composition, which knows how much room is left.
    pub played: Vec<(String, String)>,
    /// Plays the sharing switch kept off the timeline.
    pub withheld: i64,
}

impl SittingCard {
    /// "with Noor, Tobias and Marie", or that you were on your own.
    ///
    /// Capped, because a card is not a guest list: past four the names stop
    /// being readable at any size that would fit them, and "and 3 others" is
    /// the honest version of a line that would otherwise be a paragraph.
    pub(super) fn company(&self) -> String {
        match self.with.len() {
            0 => "On your own".into(),
            1 => format!("with {}", self.with[0]),
            2 => format!("with {} and {}", self.with[0], self.with[1]),
            3 => format!("with {}, {} and {}", self.with[0], self.with[1], self.with[2]),
            n => format!("with {}, {} and {} others", self.with[0], self.with[1], n - 2),
        }
    }

    /// The running time as words, and never as a bare zero.
    ///
    /// Nothing reported a length is a different statement from "no time
    /// passed", and only one of them is ever true of an evening somebody sat
    /// through. `None` here means the card says nothing rather than saying
    /// `0h` — the em-dash discipline the quality card already follows.
    pub(super) fn running(&self) -> Option<String> {
        if self.duration_known == 0 || self.seconds <= 0 {
            return None;
        }
        let m = (self.seconds as f64 / 60.0).round() as i64;
        Some(if m < 60 {
            format!("{m} min")
        } else {
            let (h, rest) = (m / 60, m % 60);
            if rest == 0 {
                format!("{h}h")
            } else {
                format!("{h}h {rest}m")
            }
        })
    }

    pub(super) fn initial(s: &str) -> String {
        s.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "·".into())
    }

    /// A sitting to look at while working on the layout.
    pub fn sample() -> SittingCard {
        SittingCard {
            title: "The long one".into(),
            date: "Tuesday, 8 September 2026".into(),
            span: "20:14 – 23:48 · 3h 34m".into(),
            with: vec!["Noor".into(), "Tobias".into(), "Marie".into()],
            plays: 31,
            seconds: 11_400,
            duration_known: 29,
            artists: 14,
            albums: 9,
            top_artist: Some(("Massive Attack".into(), 7)),
            top_album: Some(("Mezzanine".into(), "Massive Attack".into(), 5)),
            decks: vec![("Ash".into(), 18), ("Noor".into(), 9), ("Tobias".into(), 4)],
            played: vec![
                ("Mezzanine".into(), "Massive Attack".into()),
                ("Untrue".into(), "Burial".into()),
                ("Selected Ambient Works 85-92".into(), "Aphex Twin".into()),
                ("Blonde".into(), "Frank Ocean".into()),
                ("In Rainbows".into(), "Radiohead".into()),
                ("Spirit of Eden".into(), "Talk Talk".into()),
            ],
            withheld: 2,
        }
    }
}

/// Build the SVG for one sitting, and report what the pass produced.
pub(super) fn render_measured(c: &SittingCard, size: Size, theme: Theme) -> Drawn {
    let m = M::for_size(size);
    let p = theme.palette();
    // A sitting has no blended genre colour — that is a fact about a period's
    // listening and an evening is not a period. The accent stands in, through
    // the same validator, so there is one path to a colour on a card.
    let colour = period_colour(None, &p);
    let mut d = Doc::new(m.w, m.h);

    d.rect(0.0, 0.0, m.w, m.h, p.bg);
    d.wash(m.h * 0.62, &colour, 0.16, 0.0);
    d.rect(0.0, 0.0, m.w, 12.0, &colour);

    let x = m.pad;
    let w = m.content();
    let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad + 20.0);
    y = lead(&mut d, &m, c, &p, &colour, x, y + m.gap * 1.4);

    // The counts, then the two superlatives, then whose deck. Priority order,
    // and each one drawn only if it fits — a card has no scrollbar, so a
    // section that would run past the footer is left out whole rather than
    // drawn over it.
    y += m.gap * 1.3;
    if y + m.counts_h() <= m.floor() {
        y = counts(&mut d, &m, c, &p, x, y, w) + m.gap * 1.2;
    }

    if let Some((name, plays)) = &c.top_artist {
        if y + m.entity_h() <= m.floor() {
            y = entity_row(
                &mut d,
                &m,
                &p,
                &colour,
                x,
                y,
                w,
                "MOST PLAYED",
                name,
                &format!("{} {}", nf(*plays), if *plays == 1 { "play" } else { "plays" }),
                &SittingCard::initial(name),
            ) + m.gap;
        }
    }
    if let Some((album, artist, plays)) = &c.top_album {
        if y + m.entity_h() <= m.floor() {
            y = entity_row(
                &mut d,
                &m,
                &p,
                &colour,
                x,
                y,
                w,
                "THE RECORD",
                album,
                &format!("{artist} · {} plays", nf(*plays)),
                &SittingCard::initial(album),
            ) + m.gap;
        }
    }

    // Whose deck, which is the only figure that says a sitting was several
    // people's listening in one sequence. One deck is not a distribution, so
    // it is not drawn as one.
    if c.decks.len() > 1 {
        let need = m.heading_h() + m.row_sub * 1.45 * c.decks.len().min(4) as f32;
        if y + need <= m.floor() {
            y = heading(&mut d, &m, &p, x, y, "WHOSE DECK");
            let lines: Vec<String> =
                c.decks.iter().take(4).map(|(who, n)| format!("{who} — {}", nf(*n))).collect();
            y = facts(&mut d, &m, &p, x, y, w, &lines) + m.gap;
        }
    }

    // **What actually went on, in order**, filling whatever is left. It is
    // last because it is the section that can give ground: the figures above
    // are what an evening *was*, and this is what it sounded like, so a card
    // with no room drops the records rather than the superlatives.
    //
    // Sized to the space rather than to a constant — a story card has a third
    // of itself spare after the figures and a wide one has almost none, and a
    // fixed count would either leave the tall one looking like a failed render
    // or overrun the short one.
    let row = m.row_sub * 1.45;
    let room = ((m.floor() - y - m.heading_h()) / row).floor().max(0.0) as usize;
    if room >= 2 && !c.played.is_empty() {
        y = heading(&mut d, &m, &p, x, y, "WHAT PLAYED");
        // One line held back for "and N more" whenever there are more than fit,
        // because a list that simply stops implies the evening did.
        let shown = if c.played.len() > room { room.saturating_sub(1) } else { c.played.len() };
        // **Ellipsised here rather than left to `facts`**, which drops a line
        // it cannot fit. Dropping one is right for a fact nobody promised —
        // and wrong for a list with a count under it, because the record
        // disappears *and* "and N more" then undercounts by the ones that
        // vanished. A long title is shortened; it is never silently absent.
        let mut lines: Vec<String> = c
            .played
            .iter()
            .take(shown)
            .map(|(t, a)| ellipsise(&format!("{t} — {a}"), Face::Regular, m.row_sub, w))
            .collect();
        if c.played.len() > shown {
            lines.push(format!("and {} more", nf((c.played.len() - shown) as i64)));
        }
        y = facts(&mut d, &m, &p, x, y, w, &lines);
    }

    footer(&mut d, &m, c, &p, &colour);
    Drawn { covered: d.fully_covered(), svg: d.finish(), bottom: y }
}

/// The eyebrow, the sitting's name, and the evening it was.
fn header(
    d: &mut Doc,
    m: &M,
    c: &SittingCard,
    p: &super::theme::Palette,
    colour: &str,
    x: f32,
    top: f32,
) -> f32 {
    d.text_tracked(
        x,
        top + m.eyebrow,
        "TAPEDECK · SHARED SPOOL",
        Face::SemiBold,
        m.eyebrow,
        p.muted,
        Anchor::Start,
        m.eyebrow * 0.24,
        1.0,
    );
    let base = top + m.eyebrow + m.label * 1.15;
    d.text(x, base, &c.title, Face::Display, m.label, p.text, Anchor::Start);
    let ry = base + m.label * 0.42;
    d.rounded(x, ry, m.label * 1.6, 7.0, 3.5, colour);

    let mut y = ry + 7.0 + m.sub * 1.5;
    d.text(x, y, &c.date, Face::Regular, m.sub, p.muted, Anchor::Start);
    if !c.span.is_empty() {
        y += m.lead * 1.35;
        d.text(x, y, &c.span, Face::Regular, m.lead, p.muted_3, Anchor::Start);
    }
    y
}

/// **The people, then the number.**
///
/// The other cards lead with a count because a period is a quantity of
/// listening. A sitting is not: it is who you were in a room with, which is the
/// rule the running banner already follows and the reason that one leads with
/// an avatar stack rather than a title.
fn lead(
    d: &mut Doc,
    m: &M,
    c: &SittingCard,
    p: &super::theme::Palette,
    colour: &str,
    x: f32,
    top: f32,
) -> f32 {
    let y = top + m.row_title;
    d.text(x, y, &c.company(), Face::SemiBold, m.row_title, p.text, Anchor::Start);

    let base = y + m.huge * 0.86;
    d.text(x, base, &nf(c.plays), Face::Display, m.huge, colour, Anchor::Start);

    // "records" is what somebody says about an evening. The running time is a
    // floor and only joins the line when anything reported one at all.
    let mut line = if c.plays == 1 { "record".to_string() } else { "records".to_string() };
    if let Some(t) = c.running() {
        line.push_str(&format!(" · {t}"));
    }
    let sub = base + m.huge * 0.28 + m.sub;
    d.text(x, sub, &line, Face::Regular, m.sub, p.muted, Anchor::Start);
    sub
}

/// Artists, records, and how many decks were in it.
fn counts(
    d: &mut Doc,
    m: &M,
    c: &SittingCard,
    p: &super::theme::Palette,
    x: f32,
    y: f32,
    w: f32,
) -> f32 {
    let h = m.counts_h();
    d.rounded(x, y, w, h, m.radius, p.surface);
    let cells = [("artists", c.artists), ("records", c.albums), ("decks", c.decks.len() as i64)];
    let cw = w / cells.len() as f32;
    for (i, (label, n)) in cells.iter().enumerate() {
        let cx = x + cw * i as f32 + cw / 2.0;
        if i > 0 {
            d.rule(x + cw * i as f32, y + h * 0.22, 1.0, h * 0.56, p.border, p.border_opacity);
        }
        d.text(
            cx,
            y + h * 0.46,
            &nf(*n),
            Face::Display,
            m.row_title * 0.95,
            p.text,
            Anchor::Middle,
        );
        d.text(cx, y + h * 0.76, label, Face::Regular, m.row_sub, p.muted, Anchor::Middle);
    }
    y + h
}

/// The strip along the bottom — and where the card admits what it is not
/// showing.
///
/// `withheld` is the rule `handed_sitting` and the sitting page both carry: a
/// picture of an evening with invisible holes in it misrepresents the evening,
/// and the reader could never tell. On a card that is going to somebody else it
/// matters more rather than less.
fn footer(d: &mut Doc, m: &M, c: &SittingCard, p: &super::theme::Palette, colour: &str) {
    let y = m.h - m.pad;
    d.rule(m.pad, m.footer_top(), m.content(), 1.0, p.border, p.border_opacity);

    let left = if c.withheld > 0 {
        format!(
            "tapedeck · {} {} not shown",
            nf(c.withheld),
            if c.withheld == 1 { "play" } else { "plays" }
        )
    } else {
        "tapedeck".to_string()
    };
    d.text(m.pad, y, &left, Face::Regular, m.foot, p.muted_3, Anchor::Start);
    d.rounded(m.w - m.pad - m.foot, y - m.foot * 0.75, m.foot, m.foot, m.foot * 0.3, colour);
}
