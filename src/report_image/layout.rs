//! The pieces every chapter card is built from, and the box they are drawn in.
//!
//! Three units share one type scale, one palette, one footer and one set of
//! primitives; what differs is the **composition** — which sections a card
//! draws and in what order — and that lives in `week`, `month` and `year`.
//! Reports itself is built the same way, three chapter components over one set
//! of parts, and for the same reason: a week has no clock and no genre split,
//! and a year is a bento rather than a column.
//!
//! **The fit rule is here rather than in any one composition.** A PNG has no
//! clipping and no scrollbar, so a section that overruns is drawn straight over
//! the footer and looks deliberate. Sections are therefore asked how tall they
//! are *before* they are drawn, and one that would not fit is left out whole —
//! see `rows_that_fit` and `no_section_is_drawn_over_the_footer`.

use super::card::Card;
use super::fonts::{ellipsise, width, Face};
use super::svg::{Anchor, Doc};
use super::theme::Palette;

/// Which bucket the card is about.
///
/// Carried through the drawing rather than baked into each composition,
/// because three sentences on a shared card name the period — the rank line,
/// the crush heading and the by-sub chart's own label — and three copies of
/// that noun is how a year's card comes to say "month".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Week,
    Month,
    Year,
}

impl Unit {
    pub fn parse(s: &str) -> Option<Unit> {
        match s.trim().to_ascii_lowercase().as_str() {
            "week" => Some(Unit::Week),
            "month" => Some(Unit::Month),
            "year" => Some(Unit::Year),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Year => "year",
        }
    }

    /// Singular and plural, for the rank line — "your 2nd busiest of 14 months".
    pub(super) fn noun(self, n: i64) -> &'static str {
        match (self, n) {
            (Unit::Week, 1) => "week",
            (Unit::Week, _) => "weeks",
            (Unit::Month, 1) => "month",
            (Unit::Month, _) => "months",
            (Unit::Year, 1) => "year",
            (Unit::Year, _) => "years",
        }
    }

    /// What the crush heading says. The chapters word it per unit and so does
    /// this — "new to you this year" is a much bigger claim than the weekly one
    /// and the card should not make the wrong one.
    pub(super) fn crush_heading(self) -> &'static str {
        match self {
            Unit::Week => "THE CRUSH · NEW TO YOU THIS WEEK",
            Unit::Month => "THE CRUSH · NEW TO YOU THIS MONTH",
            Unit::Year => "THE CRUSH · NEW TO YOU THIS YEAR",
        }
    }

    /// The heading over the by-sub-bucket bars.
    pub(super) fn shape_heading(self) -> &'static str {
        match self {
            Unit::Week => "SCROBBLES BY DAY",
            Unit::Month => "SCROBBLES BY WEEK",
            Unit::Year => "SCROBBLES BY MONTH",
        }
    }
}

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
pub(super) struct M {
    pub(super) w: f32,
    pub(super) h: f32,
    pub(super) pad: f32,
    pub(super) gap: f32,
    pub(super) radius: f32,
    pub(super) eyebrow: f32,
    pub(super) label: f32,
    pub(super) huge: f32,
    pub(super) sub: f32,
    pub(super) lead: f32,
    pub(super) row_label: f32,
    pub(super) row_title: f32,
    pub(super) row_sub: f32,
    pub(super) tile: f32,
    pub(super) foot: f32,
}

impl M {
    pub(super) fn for_size(z: Size) -> M {
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

    pub(super) fn content(&self) -> f32 {
        self.w - self.pad * 2.0
    }

    /// The top of the footer strip. Sections stop above this; the rule is drawn
    /// on it. One definition, because a card whose last section is drawn *over*
    /// the footer looks like a rendering fault and there is no scrollbar to
    /// reveal it — see `no_section_is_drawn_over_the_footer`.
    pub(super) fn footer_top(&self) -> f32 {
        self.h - self.pad - self.foot * 2.4
    }

    /// The lowest a section may **end**, which is a gap above the footer rule
    /// rather than on it.
    ///
    /// `footer_top` is where the rule is drawn and is what the guard checks;
    /// this is what the compositions fit against. The difference is the whole
    /// of it: a card whose last section ends exactly on the rule passes every
    /// assertion and looks jammed, which was true of the fullest year until
    /// somebody rendered one and looked at it.
    pub(super) fn floor(&self) -> f32 {
        self.footer_top() - self.gap
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
    pub(super) fn entity_h(&self) -> f32 {
        self.tile + self.tile * 0.16 * 2.0
    }
    pub(super) fn counts_h(&self) -> f32 {
        self.tile * 0.98
    }
    pub(super) fn heading_h(&self) -> f32 {
        self.row_label * 1.9
    }
    /// The pitch of a genre row and of a repeat row. Named because both the
    /// drawer and the fit check need them, and two copies of a step is how a
    /// list that measured as fitting comes out one row too tall.
    pub(super) fn genre_step(&self) -> f32 {
        self.row_sub * 2.9
    }
    pub(super) fn repeat_step(&self) -> f32 {
        self.row_sub * 2.6
    }
    pub(super) fn crush_h(&self) -> f32 {
        self.tile * 0.78
    }
    /// The plot height of the by-sub-bucket bars, without the heading or the
    /// labels under them.
    pub(super) fn bars_band(&self) -> f32 {
        self.tile * 0.82
    }
    pub(super) fn bands_band(&self) -> f32 {
        self.tile * 0.48
    }
    /// The whole of the bars section, heading and labels included — what a
    /// composition has to reserve before deciding it fits.
    pub(super) fn bars_h(&self) -> f32 {
        self.heading_h() + self.bars_band() + self.row_sub * 1.75
    }
    pub(super) fn bands_h(&self) -> f32 {
        self.heading_h() + self.bands_band() + self.row_sub * 2.1
    }
    /// The pitch of one `facts` line. Named because the drawer and the fit
    /// check both need the step, and two copies of a step is how a list that
    /// measured as fitting comes out a row too tall.
    pub(super) fn facts_line(&self) -> f32 {
        self.row_sub * 1.45
    }

    /// The most rows of `each` that fit in the space left, capped at `want`.
    pub(super) fn rows_that_fit(&self, y: f32, want: usize, each: f32, heading: bool) -> usize {
        let head = if heading { self.heading_h() } else { 0.0 };
        let room = self.floor() - y - head;
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
pub(super) fn centred(y: f32, h: f32, px: f32) -> f32 {
    y + h / 2.0 + px * 0.36
}

/// The page's own grouping, borrowed rather than reimplemented — a share image
/// that wrote `4182` where the chapter wrote `4,182` would be its own small lie.
pub(super) use crate::server::reports::thousands as nf;

/// `1` → `1st`. Used for the rank line, which is the card's one comparison —
/// and it is against the reader's **own** periods. There is no cross-user
/// figure here for the reason Reports refuses one: on a private household
/// instance it would quietly report how much everyone else plays.
pub(super) fn ordinal(n: i64) -> String {
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
pub(super) fn header(
    d: &mut Doc,
    m: &M,
    c: &Card,
    p: &Palette,
    colour: &str,
    x: f32,
    top: f32,
) -> f32 {
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
#[allow(clippy::too_many_arguments)]
pub(super) fn headline(
    d: &mut Doc,
    m: &M,
    c: &Card,
    p: &Palette,
    colour: &str,
    unit: Unit,
    x: f32,
    top: f32,
) -> f32 {
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
        let noun = unit.noun(c.rank_of);
        let s = format!("Your {} busiest of {} {noun}", ordinal(c.rank), c.rank_of);
        d.text(x, y, &s, Face::SemiBold, m.lead, p.text, Anchor::Start);
    }
    y + m.lead * 0.4
}

/// A surface card: a lettered tile, a small label, a title and a subtitle.
///
/// The tile carries the initial rather than a picture — see `Card::initial`.
#[allow(clippy::too_many_arguments)]
pub(super) fn entity_row(
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
pub(super) fn counts(d: &mut Doc, m: &M, c: &Card, p: &Palette, x: f32, y: f32, w: f32) -> f32 {
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
pub(super) fn heading(d: &mut Doc, m: &M, p: &Palette, x: f32, y: f32, s: &str) -> f32 {
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
pub(super) fn genres(
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
pub(super) fn repeats(
    d: &mut Doc,
    m: &M,
    c: &Card,
    p: &Palette,
    x: f32,
    y: f32,
    w: f32,
    rows: usize,
) -> f32 {
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
pub(super) fn crush(
    d: &mut Doc,
    m: &M,
    c: &Card,
    p: &Palette,
    colour: &str,
    unit: Unit,
    x: f32,
    y: f32,
    w: f32,
) -> f32 {
    let Some(cr) = &c.crush else { return y };
    let h = m.crush_h();
    d.rounded(x, y, w, h, m.radius, p.surface);
    d.rule(x, y, 6.0, h, colour, 1.0);
    let inset = (h - m.row_label - m.row_title * 1.05) / 2.0;
    d.text_tracked(
        x + m.tile * 0.16,
        y + inset + m.row_label,
        unit.crush_heading(),
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

/// The period's own shape: one bar per sub-bucket, labelled underneath.
///
/// **This is the chart a week and a year are actually shared for**, and it is
/// why they are compositions rather than a month card with a different label.
/// A week's seven days and a year's twelve months both read at a glance; a
/// month's five weeks say much less, which is why the month card leaves it out.
///
/// Heights are a share of the **period's own busiest bucket**, so the tallest
/// bar always touches the top of the band. That is a statement about shape and
/// never about volume, and nothing here claims otherwise — the headline
/// immediately above carries the count.
///
/// A bucket with nothing in it draws a **hairline** rather than nothing: a gap
/// in the row reads as a rendering fault, where a floor reads as a quiet day.
#[allow(clippy::too_many_arguments)]
pub(super) fn bars(
    d: &mut Doc,
    m: &M,
    p: &Palette,
    colour: &str,
    unit: Unit,
    c: &Card,
    x: f32,
    y: f32,
    w: f32,
) -> f32 {
    if c.sub.len() < 2 {
        return y;
    }
    let y = heading(d, m, p, x, y, unit.shape_heading());
    let band = m.bars_band();
    let n = c.sub.len() as f32;
    let gap = (m.gap * 0.28).min(w / n * 0.34);
    let bw = (w - gap * (n - 1.0)) / n;
    let peak = c.sub.iter().copied().max().unwrap_or(0).max(1) as f32;
    let floor = 3.0;

    for (i, v) in c.sub.iter().enumerate() {
        let bx = x + (bw + gap) * i as f32;
        let h = (band * (*v as f32 / peak)).max(floor);
        // Capped in absolute terms as well as relative: on a wide card the
        // columns are broad enough that a proportional radius turns a short bar
        // into a floating pill rather than something standing on a baseline.
        let r = (bw * 0.24).min(14.0).min(h / 2.0);
        d.rounded(bx, y + band - h, bw, h, r, colour);
    }

    // Labels only where they will not collide. Twelve months at story width is
    // comfortable; the check is here because a unit with more buckets would
    // otherwise overprint rather than drop them.
    let ly = y + band + m.row_sub * 1.25;
    if !c.sub_labels.is_empty() && bw >= width("00", Face::Regular, m.row_sub * 0.82) {
        for (i, label) in c.sub_labels.iter().enumerate().take(c.sub.len()) {
            let bx = x + (bw + gap) * i as f32 + bw / 2.0;
            d.text(bx, ly, label, Face::Regular, m.row_sub * 0.82, p.muted_3, Anchor::Middle);
        }
        return ly + m.row_sub * 0.5;
    }
    y + band + m.gap * 0.5
}

/// The year's signature: what was played, month by month, as stacked bands.
///
/// The three biggest genres of the whole year are tracked **by name** across
/// every bucket with the rest folded into `other` — the rule the page's own
/// `GenreBands` follows, and it is load-bearing: a stacked chart whose series
/// are renamed partway along is unreadable, and the shape implies a shift that
/// is only a relabelling.
///
/// **A bucket with no tags at all draws nothing**, rather than a full-height
/// band of `other`. That was found on the page — a month with no listening
/// rendered as a solid column that read as a genre actually played.
///
/// A share of *tag mentions*, not of listens, which is why no total is printed
/// and the card never calls it a breakdown of the year.
#[allow(clippy::too_many_arguments)]
pub(super) fn bands(
    d: &mut Doc,
    m: &M,
    p: &Palette,
    colour: &str,
    c: &Card,
    x: f32,
    y: f32,
    w: f32,
) -> f32 {
    let (names, shares) = match &c.genre_bands {
        Some((n, s)) if !n.is_empty() && s.len() >= 2 => (n, s),
        _ => return y,
    };
    let y = heading(d, m, p, x, y, "MONTH BY MONTH");
    let band = m.bands_band();
    let n = shares.len() as f32;
    let gap = (m.gap * 0.22).min(w / n * 0.3);
    let bw = (w - gap * (n - 1.0)) / n;

    for (i, col) in shares.iter().enumerate() {
        let bx = x + (bw + gap) * i as f32;
        let total: f32 = col.iter().sum::<f64>() as f32;
        // Nothing tagged in this bucket: draw nothing. See the note above.
        if total <= 0.0 {
            continue;
        }
        let mut top = y;
        for (k, share) in col.iter().enumerate() {
            let h = band * (*share as f32 / total.max(1.0));
            if h <= 0.3 {
                continue;
            }
            let (fill, op) = band_tint(k, colour, p);
            d.rounded_opacity(bx, top, bw, h, 0.0, fill, op);
            top += h;
        }
    }

    // The key, so the bands are readable as something rather than as stripes.
    let mut ky = y + band + m.row_sub * 1.5;
    let mut kx = x;
    for (k, name) in names.iter().enumerate().take(3) {
        let sw = m.row_sub * 0.72;
        let label = ellipsise(name, Face::Regular, m.row_sub * 0.86, w * 0.3);
        let lw = width(&label, Face::Regular, m.row_sub * 0.86) + sw + m.gap * 0.9;
        // Wrap rather than run off the edge: three genre names can be long.
        if kx > x && kx + lw > x + w {
            kx = x;
            ky += m.row_sub * 1.5;
        }
        let (fill, op) = band_tint(k, colour, p);
        d.rounded_opacity(kx, ky - sw * 0.85, sw, sw, sw * 0.3, fill, op);
        d.text(
            kx + sw + m.gap * 0.22,
            ky,
            &label,
            Face::Regular,
            m.row_sub * 0.86,
            p.muted,
            Anchor::Start,
        );
        kx += lw;
    }
    ky + m.row_sub * 0.6
}

/// One tint per band.
///
/// **Not four opacities of one hue**, which is what this was first written as
/// and it does not work: on a dark ground the two faint bands sink into the
/// background and the column reads as a solid block with a smudge under it.
/// Found by looking at the picture, which is the only thing that could have
/// found it.
///
/// So the period's own colour carries the first two and the palette's neutrals
/// carry the rest, which puts a real difference in *hue* between adjacent bands
/// rather than only in weight. No third colour is invented: the period colour
/// is the card's identity and a palette picked here would be a second scheme
/// with no relationship to it.
fn band_tint<'a>(k: usize, colour: &'a str, p: &Palette) -> (&'a str, f32) {
    match k {
        0 => (colour, 1.0),
        1 => (colour, 0.58),
        // The weight has to keep falling as well as the hue changing, or the
        // third band reads as louder than the second — which it did in dark
        // mode, where the neutrals are light.
        2 => (p.muted, 0.5),
        // `other` is a residue and should read as one.
        _ => (p.muted_3, 0.28),
    }
}

/// A short list of plain facts, one per line.
///
/// **Deliberately lines rather than a card of big figures.** The bento gives
/// the year's discovery numbers a tile because a six-column grid has room for
/// one; a story image is a single column 1920 tall, and the same three facts as
/// a card cost 145px that the two charts above them have a better claim to.
/// Measured rather than guessed: with them as a card the fit rule dropped
/// either the record of the year or the crush, and both of those are more
/// shareable than a large "63".
///
/// A line whose text will not fit is **dropped**, not ellipsised — half a
/// figure is worse than no figure, and these are already the least important
/// thing on the card.
pub(super) fn facts(
    d: &mut Doc,
    m: &M,
    p: &Palette,
    x: f32,
    y: f32,
    w: f32,
    lines: &[String],
) -> f32 {
    let mut y = y;
    for line in lines {
        if width(line, Face::Regular, m.row_sub) > w {
            continue;
        }
        y += m.row_sub * 1.45;
        d.text(x, y, line, Face::Regular, m.row_sub, p.muted, Anchor::Start);
    }
    y + m.row_sub * 0.4
}

/// The strip along the bottom: what made this, and the period's own colour.
pub(super) fn footer(d: &mut Doc, m: &M, c: &Card, p: &Palette, colour: &str) {
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
pub(super) struct Drawn {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_image::card::Card;
    use crate::report_image::theme::Theme;

    /// One pass of every composition, so the two guards below are about the
    /// *rule* rather than about the month that happened to be written first.
    fn every_card() -> Vec<(Unit, Size, Drawn)> {
        let mut out = Vec::new();
        for unit in [Unit::Week, Unit::Month, Unit::Year] {
            let c = Card::sample(unit);
            for size in [Size::Story, Size::Square, Size::Wide] {
                let drawn = match unit {
                    Unit::Week => crate::report_image::week::render_measured(&c, size, Theme::Dark),
                    Unit::Month => {
                        crate::report_image::month::render_measured(&c, size, Theme::Dark)
                    }
                    Unit::Year => crate::report_image::year::render_measured(&c, size, Theme::Dark),
                };
                out.push((unit, size, drawn));
            }
        }
        out
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
        for (unit, size, drawn) in every_card() {
            let (w, h) = size.dims();
            assert!(drawn.svg.contains(&format!(r#"viewBox="0 0 {w} {h}""#)), "{unit:?}/{size:?}");
            for cap in [" x=\"", " y=\""] {
                for part in drawn.svg.split(cap).skip(1) {
                    let v: f32 = part.split('"').next().unwrap().parse().unwrap();
                    let limit = if cap == " x=\"" { w } else { h };
                    assert!(
                        v >= -1.0 && v <= limit,
                        "{unit:?}/{size:?}: {cap}{v} outside 0..{limit}"
                    );
                }
            }
        }
    }

    /// No card may grow into its own footer.
    ///
    /// There is no clipping and no scrollbar in a PNG, so a section that
    /// overruns is drawn *over* the footer rule and looks deliberate. The
    /// square is the shape most at risk — it has room for about half of what a
    /// story does — and the year is the fullest stack, two charts and two
    /// entity rows deep. Found by looking at the image, kept as a test so the
    /// next section added does not have to be.
    #[test]
    fn no_section_is_drawn_over_the_footer() {
        for (unit, size, drawn) in every_card() {
            let m = M::for_size(size);
            assert!(
                drawn.bottom <= m.footer_top(),
                "{unit:?}/{size:?}: content ends at {} but the footer starts at {}",
                drawn.bottom,
                m.footer_top()
            );
        }
    }

    /// And neither may a sitting, which is the fourth composition and the one
    /// with the most variable content — a guest list, a deck list and two
    /// superlatives, any of which an evening may or may not have.
    #[test]
    fn a_sitting_is_not_drawn_over_its_footer_either() {
        use crate::report_image::sitting;
        use crate::report_image::SittingCard;

        // The full card, and then the awkward ones: an evening with a long
        // guest list and every section present is the tallest stack it can
        // produce, and one with nothing but plays is the shortest.
        let mut crowded = SittingCard::sample();
        crowded.with = (0..6).map(|i| format!("Person{i}")).collect();
        crowded.decks = (0..6).map(|i| (format!("Deck{i}"), 20 - i)).collect();

        let mut bare = SittingCard::sample();
        bare.top_artist = None;
        bare.top_album = None;
        bare.decks = vec![("Ash".into(), 3)];
        bare.with = vec![];

        for c in [SittingCard::sample(), crowded, bare] {
            for size in [Size::Story, Size::Square, Size::Wide] {
                for theme in [Theme::Dark, Theme::Light] {
                    let drawn = sitting::render_measured(&c, size, theme);
                    let m = M::for_size(size);
                    assert!(
                        drawn.bottom <= m.footer_top(),
                        "sitting/{size:?}: content ends at {} but the footer starts at {}",
                        drawn.bottom,
                        m.footer_top()
                    );
                }
            }
        }
    }

    /// The rank line names the bucket it is ranking, and singular is not a
    /// special case somebody has to remember.
    #[test]
    fn a_unit_names_itself_in_the_singular_and_the_plural() {
        assert_eq!((Unit::Week.noun(1), Unit::Week.noun(9)), ("week", "weeks"));
        assert_eq!((Unit::Month.noun(1), Unit::Month.noun(9)), ("month", "months"));
        assert_eq!((Unit::Year.noun(1), Unit::Year.noun(9)), ("year", "years"));
    }
}
