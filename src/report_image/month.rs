//! The month card, in three shapes.
//!
//! **This is a composition, not a rendering of the chapter.** `MonthChapter`
//! is 404 lines of DOM that only a browser can lay out, and usvg has no
//! flexbox, no wrapping and no way to ask how tall a paragraph came out — so
//! reproducing the page was never on the table. What is here instead is the
//! handful of facts a month is actually shared *for*, laid out for the shape
//! it is being shared into, which is what the design brief assumed when it said
//! story and square want different layouts.
//!
//! Every section is skipped rather than emptied when the period has nothing to
//! put in it. A heading over an absence reads as a rendering fault, and on an
//! early month — few genres, no chain, one album — that would be most of the
//! card.

use super::card::Card;
use super::fonts::{ellipsise, width, Face};
use super::svg::{Anchor, Doc};
use super::theme::{period_colour, Palette, Theme};

/// The shapes a share is asked for. Not one layout scaled: a square has room
/// for about half of what a story does, and drawing the story's sections into
/// it at a smaller size produces something nobody can read on a phone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    /// 1080×1920 — the story shape, and the fullest card.
    Story,
    /// 1080×1080 — a feed post. Headline, both top entities, the counts.
    Square,
    /// 1600×900 — a link preview or an embed, laid out in two columns.
    Wide,
}

impl Size {
    pub fn parse(s: Option<&str>) -> Option<Size> {
        match s.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("story") | None => Some(Size::Story),
            Some("square") => Some(Size::Square),
            Some("wide") => Some(Size::Wide),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Size::Story => "story",
            Size::Square => "square",
            Size::Wide => "wide",
        }
    }

    pub fn dims(self) -> (f32, f32) {
        match self {
            Size::Story => (1080.0, 1920.0),
            Size::Square => (1080.0, 1080.0),
            Size::Wide => (1600.0, 900.0),
        }
    }
}

/// The type scale and spacing for one shape.
struct M {
    w: f32,
    h: f32,
    pad: f32,
    gap: f32,
    radius: f32,
    eyebrow: f32,
    label: f32,
    huge: f32,
    sub: f32,
    lead: f32,
    row_label: f32,
    row_title: f32,
    row_sub: f32,
    tile: f32,
    foot: f32,
}

impl M {
    fn for_size(z: Size) -> M {
        let (w, h) = z.dims();
        match z {
            Size::Story => M {
                w,
                h,
                pad: 88.0,
                gap: 30.0,
                radius: 30.0,
                eyebrow: 25.0,
                label: 78.0,
                huge: 190.0,
                sub: 34.0,
                lead: 31.0,
                row_label: 22.0,
                row_title: 46.0,
                row_sub: 28.0,
                tile: 148.0,
                foot: 26.0,
            },
            Size::Square => M {
                w,
                h,
                pad: 76.0,
                gap: 24.0,
                radius: 26.0,
                eyebrow: 23.0,
                label: 60.0,
                huge: 128.0,
                sub: 30.0,
                lead: 28.0,
                row_label: 20.0,
                row_title: 38.0,
                row_sub: 25.0,
                tile: 104.0,
                foot: 24.0,
            },
            Size::Wide => M {
                w,
                h,
                pad: 78.0,
                gap: 22.0,
                radius: 24.0,
                eyebrow: 22.0,
                label: 56.0,
                huge: 132.0,
                sub: 29.0,
                lead: 27.0,
                row_label: 19.0,
                row_title: 34.0,
                row_sub: 23.0,
                tile: 104.0,
                foot: 23.0,
            },
        }
    }

    fn content(&self) -> f32 {
        self.w - self.pad * 2.0
    }

    /// The top of the footer strip. Sections stop above this; the rule is drawn
    /// on it. One definition, because a card whose last section is drawn *over*
    /// the footer looks like a rendering fault and there is no scrollbar to
    /// reveal it — see `no_section_is_drawn_over_the_footer`.
    fn footer_top(&self) -> f32 {
        self.h - self.pad - self.foot * 2.4
    }
}

/// How tall each optional section is, asked *before* it is drawn.
///
/// The story card holds more than the tallest month has room for — four genres,
/// three repeats and a crush overran the footer by 376px on the first attempt —
/// and hand-tuning the type scale until it fitted would have been undone by the
/// next section anybody added. So the stack asks instead: sections are drawn in
/// priority order and one that would not fit is left out entirely. A section
/// half-drawn against the bottom edge is the failure this avoids, and it cannot
/// be seen from the numbers.
impl M {
    fn entity_h(&self) -> f32 {
        self.tile + self.tile * 0.16 * 2.0
    }
    fn counts_h(&self) -> f32 {
        self.tile * 0.98
    }
    fn heading_h(&self) -> f32 {
        self.row_label * 1.9
    }
    /// The pitch of a genre row and of a repeat row. Named because both the
    /// drawer and the fit check need them, and two copies of a step is how a
    /// list that measured as fitting comes out one row too tall.
    fn genre_step(&self) -> f32 {
        self.row_sub * 2.9
    }
    fn repeat_step(&self) -> f32 {
        self.row_sub * 2.6
    }
    fn crush_h(&self) -> f32 {
        self.tile * 0.78
    }

    /// The most rows of `each` that fit in the space left, capped at `want`.
    fn rows_that_fit(&self, y: f32, want: usize, each: f32, heading: bool) -> usize {
        let head = if heading { self.heading_h() } else { 0.0 };
        let room = self.footer_top() - y - head;
        if room <= 0.0 {
            return 0;
        }
        want.min((room / each).floor().max(0.0) as usize)
    }
}

/// The baseline that puts text of size `px` optically centred in a band.
///
/// Cap height rather than the full em box: centring on the em leaves text
/// visibly high, because the box reserves room for descenders the string may
/// not have.
fn centred(y: f32, h: f32, px: f32) -> f32 {
    y + h / 2.0 + px * 0.36
}

/// The page's own grouping, borrowed rather than reimplemented — a share image
/// that wrote `4182` where the chapter wrote `4,182` would be its own small lie.
use crate::server::reports::thousands as nf;

/// `1` → `1st`. Used for the rank line, which is the card's one comparison —
/// and it is against the reader's **own** periods. There is no cross-user
/// figure here for the reason Reports refuses one: on a private household
/// instance it would quietly report how much everyone else plays.
fn ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The eyebrow, the period's name, and the rule under it.
fn header(d: &mut Doc, m: &M, c: &Card, p: &Palette, colour: &str, x: f32, top: f32) -> f32 {
    d.text_tracked(
        x,
        top + m.eyebrow,
        "TAPEDECK · LISTENING REPORT",
        Face::SemiBold,
        m.eyebrow,
        p.muted,
        Anchor::Start,
        m.eyebrow * 0.24,
        1.0,
    );
    let base = top + m.eyebrow + m.label * 1.15;
    d.text(x, base, &c.label, Face::Display, m.label, p.text, Anchor::Start);
    // Below the descenders, not against them: at label size a bar 0.30em under
    // the baseline reads as an underline of the first word rather than as a
    // rule under the whole line.
    let ry = base + m.label * 0.42;
    d.rounded(x, ry, m.label * 1.6, 7.0, 3.5, colour);
    ry + 7.0
}

/// The number the month is remembered by, and what it is a number of.
fn headline(d: &mut Doc, m: &M, c: &Card, p: &Palette, colour: &str, x: f32, top: f32) -> f32 {
    let base = top + m.huge * 0.80;
    d.text(x, base, &nf(c.plays), Face::Display, m.huge, colour, Anchor::Start);

    // Hours are a floor and the chapter says so; the share has no room for the
    // caveat, so it states the two figures and claims nothing about coverage.
    let mut line = format!("scrobbles · {:.1}h", c.hours);
    if c.avg_daily > 0 {
        line.push_str(&format!(" · {}/day", nf(c.avg_daily)));
    }
    // Clearance measured from the *display* size, not the sub's. At 190px the
    // comma in "4,182" descends about 40px below the baseline, and spacing the
    // next line by its own size put it straight through the descender.
    let sub = base + m.huge * 0.28 + m.sub;
    d.text(x, sub, &line, Face::Regular, m.sub, p.muted, Anchor::Start);

    let mut y = sub;
    if c.rank > 0 && c.rank_of > 1 {
        y = sub + m.lead * 1.6;
        let unit = if c.rank_of == 1 { "month" } else { "months" };
        let s = format!("Your {} busiest of {} {unit}", ordinal(c.rank), c.rank_of);
        d.text(x, y, &s, Face::SemiBold, m.lead, p.text, Anchor::Start);
    }
    y + m.lead * 0.4
}

/// A surface card: a lettered tile, a small label, a title and a subtitle.
///
/// The tile carries the initial rather than a picture — see `Card::initial`.
#[allow(clippy::too_many_arguments)]
fn entity_row(
    d: &mut Doc,
    m: &M,
    p: &Palette,
    colour: &str,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    title: &str,
    sub: &str,
    initial: &str,
) -> f32 {
    let inset = m.tile * 0.16;
    let h = m.entity_h();
    d.rounded(x, y, w, h, m.radius, p.surface);

    let tx = x + inset;
    d.rounded(tx, y + inset, m.tile, m.tile, m.radius * 0.8, colour);
    d.text(
        tx + m.tile / 2.0,
        centred(y + inset, m.tile, m.tile * 0.56),
        initial,
        Face::Display,
        m.tile * 0.56,
        p.bg,
        Anchor::Middle,
    );

    let cx = tx + m.tile + inset;
    let room = w - (cx - x) - inset;
    d.text_tracked(
        cx,
        y + inset + m.row_label,
        label,
        Face::SemiBold,
        m.row_label,
        p.muted_3,
        Anchor::Start,
        m.row_label * 0.22,
        1.0,
    );
    d.text(
        cx,
        y + inset + m.row_label + m.row_title * 1.05,
        &ellipsise(title, Face::Display, m.row_title, room),
        Face::Display,
        m.row_title,
        p.text,
        Anchor::Start,
    );
    d.text(
        cx,
        y + inset + m.row_label + m.row_title * 1.05 + m.row_sub * 1.5,
        &ellipsise(sub, Face::Regular, m.row_sub, room),
        Face::Regular,
        m.row_sub,
        p.muted,
        Anchor::Start,
    );
    y + h
}

/// Artists / albums / tracks, in one card with hairlines between.
fn counts(d: &mut Doc, m: &M, c: &Card, p: &Palette, x: f32, y: f32, w: f32) -> f32 {
    let h = m.counts_h();
    d.rounded(x, y, w, h, m.radius, p.surface);
    let cells = [("artists", c.artists), ("albums", c.albums), ("tracks", c.tracks)];
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

/// A small-caps section heading.
fn heading(d: &mut Doc, m: &M, p: &Palette, x: f32, y: f32, s: &str) -> f32 {
    d.text_tracked(
        x,
        y + m.row_label,
        s,
        Face::SemiBold,
        m.row_label,
        p.muted_3,
        Anchor::Start,
        m.row_label * 0.22,
        1.0,
    );
    y + m.heading_h()
}

/// The period's biggest genres, as bars.
///
/// A share of **tag mentions**, not of listens — one track carries several tags
/// — which is why the bars are unlabelled beyond their own percentage and the
/// card never sums them or calls them a breakdown of the month.
#[allow(clippy::too_many_arguments)]
fn genres(
    d: &mut Doc,
    m: &M,
    c: &Card,
    p: &Palette,
    colour: &str,
    x: f32,
    y: f32,
    w: f32,
    rows: usize,
) -> f32 {
    if c.genres.is_empty() {
        return y;
    }
    let mut y = heading(d, m, p, x, y, "TOP GENRES");
    let bar = m.row_sub * 0.5;
    let step = m.genre_step();
    for (name, pct) in c.genres.iter().take(rows) {
        let pct_s = format!("{pct}%");
        let pw = width(&pct_s, Face::SemiBold, m.row_sub);
        d.text(
            x,
            y + m.row_sub,
            &ellipsise(name, Face::Regular, m.row_sub, w - pw - m.gap),
            Face::Regular,
            m.row_sub,
            p.text,
            Anchor::Start,
        );
        d.text(x + w, y + m.row_sub, &pct_s, Face::SemiBold, m.row_sub, p.muted, Anchor::End);
        let ty = y + m.row_sub * 1.5;
        d.rounded(x, ty, w, bar, bar / 2.0, p.track);
        // Clamped, so a share the fold rounded to 101 cannot run past the card.
        let fill = w * (*pct as f32 / 100.0).clamp(0.0, 1.0);
        if fill > 0.5 {
            d.rounded(x, ty, fill.max(bar), bar, bar / 2.0, colour);
        }
        y += step;
    }
    y
}

/// The heaviest single day each track had. The chapter's own framing, kept
/// word for word, because "on repeat" without it reads as a period total.
#[allow(clippy::too_many_arguments)]
fn repeats(d: &mut Doc, m: &M, c: &Card, p: &Palette, x: f32, y: f32, w: f32, rows: usize) -> f32 {
    if c.on_repeat.is_empty() {
        return y;
    }
    let mut y = heading(d, m, p, x, y, "ON REPEAT · HEAVIEST SINGLE DAY");
    let step = m.repeat_step();
    for r in c.on_repeat.iter().take(rows) {
        let right = format!("{} in a day", r.plays);
        let rw = width(&right, Face::SemiBold, m.row_sub);
        let room = w - rw - m.gap;
        d.text(
            x,
            y + m.row_sub,
            &ellipsise(&r.title, Face::SemiBold, m.row_sub * 1.15, room),
            Face::SemiBold,
            m.row_sub * 1.15,
            p.text,
            Anchor::Start,
        );
        d.text(
            x,
            y + m.row_sub * 2.0,
            &ellipsise(&r.artist, Face::Regular, m.row_sub * 0.92, room),
            Face::Regular,
            m.row_sub * 0.92,
            p.muted,
            Anchor::Start,
        );
        d.text(x + w, y + m.row_sub, &right, Face::SemiBold, m.row_sub, p.muted, Anchor::End);
        y += step;
    }
    y
}

/// The most-played artist the reader had never played before.
///
/// Omitted where the report omitted it, which it does for the **oldest**
/// period: the history begins inside it, so every artist there is new by
/// construction and the crush would just be the top artist wearing a different
/// label. The card inherits that rather than deciding it again.
#[allow(clippy::too_many_arguments)]
fn crush(d: &mut Doc, m: &M, c: &Card, p: &Palette, colour: &str, x: f32, y: f32, w: f32) -> f32 {
    let Some(cr) = &c.crush else { return y };
    let h = m.crush_h();
    d.rounded(x, y, w, h, m.radius, p.surface);
    d.rule(x, y, 6.0, h, colour, 1.0);
    let inset = (h - m.row_label - m.row_title * 1.05) / 2.0;
    d.text_tracked(
        x + m.tile * 0.16,
        y + inset + m.row_label,
        "THE CRUSH · NEW TO YOU THIS MONTH",
        Face::SemiBold,
        m.row_label,
        p.muted_3,
        Anchor::Start,
        m.row_label * 0.22,
        1.0,
    );
    let pad = m.tile * 0.16;
    let room = w - pad * 2.0 - m.row_sub * 6.0;
    d.text(
        x + pad,
        y + inset + m.row_label + m.row_title * 1.05,
        &ellipsise(&cr.name, Face::Display, m.row_title, room),
        Face::Display,
        m.row_title,
        p.text,
        Anchor::Start,
    );
    d.text(
        x + w - pad,
        y + inset + m.row_label + m.row_title * 1.05,
        &format!("{} plays", nf(cr.plays)),
        Face::Regular,
        m.row_sub,
        p.muted,
        Anchor::End,
    );
    y + h
}

/// The strip along the bottom: what made this, and the period's own colour.
fn footer(d: &mut Doc, m: &M, c: &Card, p: &Palette, colour: &str) {
    let y = m.h - m.pad;
    d.rule(m.pad, m.footer_top(), m.content(), 1.0, p.border, p.border_opacity);

    let mut left = "tapedeck".to_string();
    if let Some(chain) = &c.top_chain {
        left = format!("tapedeck · mostly through {chain}");
    }
    d.text(
        m.pad,
        y,
        &ellipsise(&left, Face::Regular, m.foot, m.content() * 0.72),
        Face::Regular,
        m.foot,
        p.muted_3,
        Anchor::Start,
    );

    // The swatch is the period's *own* colour and is drawn only when there is
    // one. An un-enriched history has none — genres are what blend into it — and
    // the fallback fills the card with the app accent, so a dot there would be
    // presenting a default as a fact about the month. Found on a real instance
    // with nothing enriched, where it rendered as a lone dot beside no hex.
    if let Some(hex) = c.colour.as_deref() {
        let r = m.foot * 0.5;
        let cx = m.w - m.pad - r;
        d.rounded(cx - r, y - r * 1.6, r * 2.0, r * 2.0, r, colour);
        d.text(cx - r * 1.6, y, hex, Face::Regular, m.foot, p.muted_3, Anchor::End);
    }
}

/// What one pass of the composition produced.
pub struct Drawn {
    pub svg: String,
    /// Where the last section ended.
    ///
    /// Read only by `no_section_is_drawn_over_the_footer`, which is the point:
    /// a card has no scrollbar to reveal a section drawn past the bottom, so the
    /// layout reports where it stopped and the guard checks it. Reported rather
    /// than re-derived, because a second copy of the arithmetic would be free to
    /// disagree with the one that drew.
    #[allow(dead_code)]
    pub bottom: f32,
    /// Whether every string that actually reached the page is in the embedded
    /// faces.
    pub covered: bool,
}

/// Build the SVG for one month, and report what the pass produced.
pub fn render_measured(c: &Card, size: Size, theme: Theme) -> Drawn {
    let m = M::for_size(size);
    let p = theme.palette();
    let colour = period_colour(c.colour.as_deref(), &p);
    let mut d = Doc::new(m.w, m.h);

    // Ground, then a wash of the period's own colour behind the headline —
    // what makes two months look like two months rather than one template
    // filled in twice.
    d.rect(0.0, 0.0, m.w, m.h, p.bg);
    d.wash(m.h * 0.62, &colour, 0.16, 0.0);
    d.rect(0.0, 0.0, m.w, 12.0, &colour);

    let bottom = match size {
        Size::Story => {
            let x = m.pad;
            let w = m.content();
            let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad + 20.0);
            y = headline(&mut d, &m, c, &p, &colour, x, y + m.gap * 1.5);
            y += m.gap * 1.5;

            // Priority order, and each section is skipped whole rather than
            // clipped. The two entity rows and the counts are what a month is
            // shared *for*, so they go first and are not negotiable; everything
            // below them takes the room that is left.
            if let Some(a) = &c.top_artist {
                if y + m.entity_h() <= m.footer_top() {
                    y = entity_row(
                        &mut d,
                        &m,
                        &p,
                        &colour,
                        x,
                        y,
                        w,
                        "TOP ARTIST",
                        &a.name,
                        &format!("{} plays", nf(a.plays)),
                        &Card::initial(&a.name),
                    ) + m.gap;
                }
            }
            if let Some(al) = &c.top_album {
                if y + m.entity_h() <= m.footer_top() {
                    y = entity_row(
                        &mut d,
                        &m,
                        &p,
                        &colour,
                        x,
                        y,
                        w,
                        "TOP RECORD",
                        &al.title,
                        &format!("{} · {} plays", al.artist, nf(al.plays)),
                        &Card::initial(&al.title),
                    ) + m.gap;
                }
            }
            if y + m.counts_h() <= m.footer_top() {
                y = counts(&mut d, &m, c, &p, x, y, w) + m.gap * 1.5;
            }

            // The crush is reserved for *before* the lists rather than after
            // them: it is one row, it is the most shareable line on the card,
            // and leaving it to last meant it was always the thing squeezed out.
            let reserve = if c.crush.is_some() { m.crush_h() + m.gap * 1.2 } else { 0.0 };

            let g = m.rows_that_fit(y + reserve, 3, m.genre_step(), true).min(c.genres.len());
            if g > 0 {
                y = genres(&mut d, &m, c, &p, &colour, x, y, w, g) + m.gap * 1.3;
            }
            let r = m.rows_that_fit(y + reserve, 3, m.repeat_step(), true).min(c.on_repeat.len());
            if r > 0 {
                y = repeats(&mut d, &m, c, &p, x, y, w, r) + m.gap * 1.2;
            }
            if y + m.crush_h() <= m.footer_top() {
                y = crush(&mut d, &m, c, &p, &colour, x, y, w);
            }
            y
        }
        Size::Square => {
            let x = m.pad;
            let w = m.content();
            let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad);
            y = headline(&mut d, &m, c, &p, &colour, x, y + m.gap);
            y += m.gap * 1.2;

            if let Some(a) = &c.top_artist {
                y = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    x,
                    y,
                    w,
                    "TOP ARTIST",
                    &a.name,
                    &format!("{} plays", nf(a.plays)),
                    &Card::initial(&a.name),
                ) + m.gap;
            }
            if let Some(al) = &c.top_album {
                y = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    x,
                    y,
                    w,
                    "TOP RECORD",
                    &al.title,
                    &format!("{} · {} plays", al.artist, nf(al.plays)),
                    &Card::initial(&al.title),
                ) + m.gap;
            }
            counts(&mut d, &m, c, &p, x, y, w)
        }
        Size::Wide => {
            // Two columns: the numbers on the left, what made them on the right.
            let colw = (m.content() - m.gap * 2.0) / 2.0;
            let lx = m.pad;
            let rx = m.pad + colw + m.gap * 2.0;

            let mut y = header(&mut d, &m, c, &p, &colour, lx, m.pad);
            let left_y = headline(&mut d, &m, c, &p, &colour, lx, y + m.gap);

            y = m.pad + m.eyebrow;
            if let Some(a) = &c.top_artist {
                y = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    rx,
                    y,
                    colw,
                    "TOP ARTIST",
                    &a.name,
                    &format!("{} plays", nf(a.plays)),
                    &Card::initial(&a.name),
                ) + m.gap;
            }
            if let Some(al) = &c.top_album {
                y = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    rx,
                    y,
                    colw,
                    "TOP RECORD",
                    &al.title,
                    &format!("{} · {} plays", al.artist, nf(al.plays)),
                    &Card::initial(&al.title),
                ) + m.gap;
            }
            let right = counts(&mut d, &m, c, &p, rx, y, colw);
            let left = genres(&mut d, &m, c, &p, &colour, lx, left_y + m.gap * 2.2, colw, 2);
            left.max(right)
        }
    };

    footer(&mut d, &m, c, &p, &colour);
    Drawn { covered: d.fully_covered(), svg: d.finish(), bottom }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Card {
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
            top_artist: Some(super::super::card::Named {
                name: "Cœur de pirate".into(),
                plays: 612,
            }),
            top_album: Some(super::super::card::Titled {
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
                super::super::card::Titled {
                    title: "Comme des enfants".into(),
                    artist: "Cœur de pirate".into(),
                    plays: 9,
                },
                super::super::card::Titled {
                    title: "Ne m'appelle pas Fanny".into(),
                    artist: "Clara Luciani".into(),
                    plays: 7,
                },
                super::super::card::Titled {
                    title: "Place de la République".into(),
                    artist: "Cœur de pirate".into(),
                    plays: 6,
                },
            ],
            crush: Some(super::super::card::Named { name: "Clara Luciani".into(), plays: 63 }),
            top_chain: Some("Desk2 · HD 650".into()),
        }
    }

    #[test]
    fn an_unknown_shape_is_refused_rather_than_guessed() {
        assert_eq!(Size::parse(Some("story")), Some(Size::Story));
        assert_eq!(Size::parse(None), Some(Size::Story));
        assert_eq!(Size::parse(Some("portrait")), None);
    }

    /// Every shape declares the box its coordinates were computed in, and
    /// nothing is drawn outside it.
    ///
    /// There is no clipping and no scrollbar in a PNG: a section that overruns
    /// is simply gone, with the image looking deliberate. So the guard is on
    /// the *coordinates*, which is the only form that catches a section added
    /// later pushing the one below it off the bottom.
    #[test]
    fn nothing_a_card_draws_falls_outside_the_image() {
        let c = full();
        for size in [Size::Story, Size::Square, Size::Wide] {
            let (w, h) = size.dims();
            let svg = render_measured(&c, size, Theme::Dark).svg;
            assert!(svg.contains(&format!(r#"viewBox="0 0 {w} {h}""#)), "{size:?}");
            for cap in [" x=\"", " y=\""] {
                for part in svg.split(cap).skip(1) {
                    let v: f32 = part.split('"').next().unwrap().parse().unwrap();
                    let limit = if cap == " x=\"" { w } else { h };
                    assert!(v >= -1.0 && v <= limit, "{size:?}: {cap}{v} outside 0..{limit}");
                }
            }
        }
    }

    /// A period with nothing but listens draws a headline and stops.
    ///
    /// The alternative — headings with nothing under them — is what an early
    /// month would have looked like, and it reads as a broken renderer rather
    /// than as a quiet month.
    #[test]
    fn a_thin_month_leaves_sections_out_instead_of_drawing_empty_ones() {
        let c = Card { label: "March 2019".into(), plays: 12, ..Default::default() };
        let svg = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(svg.contains("March 2019"));
        assert!(!svg.contains("TOP ARTIST"), "no artist, so no heading");
        assert!(!svg.contains("TOP GENRES"), "no genres, so no heading");
        assert!(!svg.contains("ON REPEAT"), "nothing on repeat, so no heading");
    }

    /// The rank line compares the reader against their own periods and nobody
    /// else's — the rule Reports settled when it refused a cross-user
    /// percentile on a private household instance.
    #[test]
    fn the_only_comparison_on_the_card_is_against_your_own_months() {
        let svg = render_measured(&full(), Size::Story, Theme::Dark).svg;
        assert!(svg.contains("Your 2nd busiest of 14 months"));
        for word in ["listeners", "%ile", "percentile", "average"] {
            assert!(!svg.contains(word), "the card must not say {word}");
        }
    }

    /// The card must not grow into its own footer.
    ///
    /// There is no clipping and no scrollbar in a PNG, so a section that
    /// overruns is drawn *over* the footer rule and looks deliberate. This is
    /// the guard for the shape most at risk: a square has room for about half
    /// of what a story does, and the first version of it landed the counts card
    /// exactly on the rule. Found by looking at the image, kept as a test so the
    /// next section added does not have to be.
    #[test]
    fn no_section_is_drawn_over_the_footer() {
        for size in [Size::Story, Size::Square, Size::Wide] {
            let m = M::for_size(size);
            let drawn = render_measured(&full(), size, Theme::Dark);
            assert!(
                drawn.bottom <= m.footer_top(),
                "{size:?}: content ends at {} but the footer starts at {}",
                drawn.bottom,
                m.footer_top()
            );
        }
    }

    /// A month whose genres are not enriched yet has no colour of its own, and
    /// the card must not present the app's accent as one.
    #[test]
    fn a_month_with_no_colour_of_its_own_draws_no_swatch() {
        let mut c = full();
        c.colour = None;
        let svg = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(!svg.contains("#a18430"), "the period colour is gone");
        let back = render_measured(&full(), Size::Story, Theme::Dark).svg;
        assert!(back.contains("#a18430"), "and back when set");
    }

    #[test]
    fn ordinals_and_grouping_read_the_way_the_page_does() {
        assert_eq!((nf(4182), nf(902), nf(0)), ("4,182".into(), "902".into(), "0".into()));
        let got: Vec<_> = [1, 2, 3, 4, 11, 12, 13, 21, 22].iter().map(|n| ordinal(*n)).collect();
        assert_eq!(got, ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd"]);
    }
}
