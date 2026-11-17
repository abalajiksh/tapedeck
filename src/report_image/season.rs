//! A season — Christmas, Halloween, Easter — as a card.
//!
//! A fifth composition, for the reason the sitting card gives for being the
//! fourth: a season is not a period of the report kind. It has no rank and no
//! per-day rate; what it has is a **curve** — the share of each day's
//! listening that was the season's, from before it started to its last day —
//! and the day that curve lifted. That is Glenn McDonald's country chart drawn
//! for one listener, and it is the thing worth handing to somebody.
//!
//! Same type scale, palette, fit rule and rasteriser as the others.

use super::layout::*;
use super::svg::{Anchor, Doc};
use super::theme::{period_colour, Palette, Theme};
use crate::report_image::fonts::{ellipsise, Face};

#[derive(Debug, Default, Clone)]
pub struct SeasonCard {
    /// "CHRISTMAS".
    pub eyebrow: String,
    /// "Christmas 2026".
    pub title: String,
    /// "1 September – 6 January".
    pub span: String,
    pub plays: i64,
    pub seconds: i64,
    /// The season's share of everything played in it, in percent.
    pub share_pct: Option<i64>,
    /// "Your season opened on 14 November".
    pub opened: Option<String>,
    pub top_artist: Option<(String, i64)>,
    /// `(title, artist, plays)`, most played first.
    pub songs: Vec<(String, String, i64)>,
    /// Daily share, 0–1, first day to last.
    pub curve: Vec<f64>,
}

impl SeasonCard {
    #[cfg(test)]
    pub fn sample() -> SeasonCard {
        SeasonCard {
            eyebrow: "CHRISTMAS".into(),
            title: "Christmas 2026".into(),
            span: "1 September – 6 January".into(),
            plays: 412,
            seconds: 91_200,
            share_pct: Some(18),
            opened: Some("Your season opened on 14 November".into()),
            top_artist: Some(("Michael Bublé".into(), 61)),
            songs: vec![
                ("Last Christmas".into(), "Wham!".into(), 24),
                ("All I Want for Christmas Is You".into(), "Mariah Carey".into(), 19),
                ("Blue Christmas".into(), "Chris Isaak".into(), 14),
                ("White Christmas".into(), "Bing Crosby".into(), 11),
                ("Jingle Bell Rock".into(), "Bobby Helms".into(), 9),
            ],
            curve: (0..128).map(|i| ((i as f64 - 70.0) / 40.0).clamp(0.0, 1.0) * 0.4).collect(),
        }
    }
}

pub(super) fn render_measured(c: &SeasonCard, size: Size, theme: Theme) -> Drawn {
    let m = M::for_size(size);
    let p = theme.palette();
    let colour = period_colour(None, &p);
    let mut d = Doc::new(m.w, m.h);

    d.rect(0.0, 0.0, m.w, m.h, p.bg);
    d.wash(m.h * 0.62, &colour, 0.16, 0.0);
    d.rect(0.0, 0.0, m.w, 12.0, &colour);

    let x = m.pad;
    let w = m.content();

    // Header: eyebrow, the season's name, its span.
    let top = m.pad + 20.0;
    d.text_tracked(
        x,
        top + m.eyebrow,
        &format!("TAPEDECK · {}", c.eyebrow),
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
    d.rounded(x, ry, m.label * 1.6, 7.0, 3.5, &colour);
    let mut y = ry + 7.0 + m.sub * 1.5;
    d.text(x, y, &c.span, Face::Regular, m.sub, p.muted, Anchor::Start);

    // The number, and what share of the season it was.
    let big = y + m.gap * 1.2 + m.huge * 0.86;
    d.text(x, big, &nf(c.plays), Face::Display, m.huge, &colour, Anchor::Start);
    let mut line = if c.plays == 1 { "play".to_string() } else { "plays".to_string() };
    let hours = c.seconds / 3600;
    if hours > 0 {
        line.push_str(&format!(" · {}h", nf(hours)));
    }
    if let Some(pct) = c.share_pct {
        line.push_str(&format!(" · {pct}% of your listening"));
    }
    y = big + m.huge * 0.28 + m.sub;
    d.text(x, y, &line, Face::Regular, m.sub, p.muted, Anchor::Start);
    if let Some(opened) = &c.opened {
        y += m.lead * 1.4;
        d.text(x, y, opened, Face::SemiBold, m.lead, p.text, Anchor::Start);
    }

    // A wide card is two columns, like the others: the figures on the left,
    // the curve and what was on repeat on the right.
    let (x, w) = if size == Size::Wide {
        y = top;
        (m.pad + m.content() * 0.53, m.content() * 0.47)
    } else {
        (x, w)
    };

    // The curve.
    let curve_h = m.row_title * 2.6;
    if !c.curve.is_empty() && y + m.gap + curve_h <= m.floor() {
        y = curve(&mut d, &m, &p, &colour, &c.curve, x, y + m.gap, w, curve_h) + m.gap;
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
                &initial(name),
            ) + m.gap;
        }
    }

    let row = m.row_sub * 1.45;
    // `facts` leaves a little air under its last line; that counts too.
    let room = ((m.floor() - y - m.heading_h() - m.row_sub * 0.4) / row).floor().max(0.0) as usize;
    if room >= 2 && !c.songs.is_empty() {
        y = heading(&mut d, &m, &p, x, y, "ON REPEAT");
        let lines: Vec<String> = c
            .songs
            .iter()
            .take(room)
            .map(|(t, a, n)| {
                ellipsise(&format!("{t} — {a} · {}", nf(*n)), Face::Regular, m.row_sub, w)
            })
            .collect();
        y = facts(&mut d, &m, &p, x, y, w, &lines);
    }

    let fy = m.h - m.pad;
    d.rule(m.pad, m.footer_top(), m.content(), 1.0, p.border, p.border_opacity);
    d.text(m.pad, fy, "tapedeck", Face::Regular, m.foot, p.muted_3, Anchor::Start);
    d.rounded(m.w - m.pad - m.foot, fy - m.foot * 0.75, m.foot, m.foot, m.foot * 0.3, &colour);

    Drawn { covered: d.fully_covered(), svg: d.finish(), bottom: y }
}

fn initial(s: &str) -> String {
    s.trim()
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "·".into())
}

/// One bar per day, height by that day's share, on a panel.
#[allow(clippy::too_many_arguments)]
fn curve(
    d: &mut Doc,
    m: &M,
    p: &Palette,
    colour: &str,
    days: &[f64],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> f32 {
    d.rounded(x, y, w, h, m.radius, p.surface);
    let inset = m.radius * 0.8;
    let (cx, cw, ch) = (x + inset, w - inset * 2.0, h - inset * 2.0);
    let peak = days.iter().cloned().fold(0.0_f64, f64::max).max(0.01);
    let step = cw / days.len() as f32;
    for (i, v) in days.iter().enumerate() {
        let bh = ((v / peak) as f32 * ch).max(if *v > 0.0 { 2.0 } else { 0.0 });
        if bh > 0.0 {
            d.rect(cx + step * i as f32, y + inset + ch - bh, (step * 0.8).max(1.0), bh, colour);
        }
    }
    y + h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_fits_above_the_footer() {
        for size in [Size::Story, Size::Square, Size::Wide] {
            let drawn = render_measured(&SeasonCard::sample(), size, Theme::parse(None));
            assert!(drawn.bottom <= M::for_size(size).floor() + 1.0, "{size:?}");
            assert!(drawn.svg.contains("Christmas 2026"));
        }
    }
}
