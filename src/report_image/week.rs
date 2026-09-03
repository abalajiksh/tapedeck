//! The week card — the compact sibling, as the chapter is.
//!
//! **What a week is not given is the point.** `WeekChapter` draws one feature
//! card rather than two, no clock and no genre breakdown, and the design's
//! reasoning is sound: seven days make a thin hour-of-day distribution and a
//! thinner genre split, and drawing them anyway dresses up noise. The share
//! image inherits that judgement rather than deciding it again — so there is no
//! `genres` call here, and its absence is deliberate rather than unfinished.
//!
//! What it *does* get and the month does not is the **by-day bars**. Seven
//! columns are a shape you read at a glance, which is most of what a week has
//! to say, and they are the reason this is a composition rather than a month
//! card with a different label.
//!
//! It leads with a **track** where the month leads with a record, following the
//! chapter: seven days often do not contain a record played end to end.

use super::card::Card;
use super::layout::*;
use super::svg::Doc;
use super::theme::{period_colour, Theme};

/// Build the SVG for one week, and report what the pass produced.
pub(super) fn render_measured(c: &Card, size: Size, theme: Theme) -> Drawn {
    let m = M::for_size(size);
    let p = theme.palette();
    let colour = period_colour(c.colour.as_deref(), &p);
    let mut d = Doc::new(m.w, m.h);

    d.rect(0.0, 0.0, m.w, m.h, p.bg);
    d.wash(m.h * 0.62, &colour, 0.16, 0.0);
    d.rect(0.0, 0.0, m.w, 12.0, &colour);

    let bottom = match size {
        Size::Story => {
            let x = m.pad;
            let w = m.content();
            let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad + 20.0);
            y = headline(&mut d, &m, c, &p, &colour, Unit::Week, x, y + m.gap * 1.5);
            y += m.gap * 1.5;

            // The bars first, and reserved rather than fitted after the fact:
            // they are the week's own picture of itself, and a week card that
            // dropped them to make room for a third entity row would be a month
            // card wearing the wrong label.
            if y + m.bars_h() <= m.floor() {
                y = bars(&mut d, &m, &p, &colour, Unit::Week, c, x, y, w) + m.gap * 1.4;
            }

            if let Some(a) = &c.top_artist {
                if y + m.entity_h() <= m.floor() {
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
            if let Some(t) = &c.top_track {
                if y + m.entity_h() <= m.floor() {
                    y = entity_row(
                        &mut d,
                        &m,
                        &p,
                        &colour,
                        x,
                        y,
                        w,
                        "TRACK OF THE WEEK",
                        &t.title,
                        &format!("{} · {} plays", t.artist, nf(t.plays)),
                        &Card::initial(&t.title),
                    ) + m.gap;
                }
            }
            if y + m.counts_h() <= m.floor() {
                y = counts(&mut d, &m, c, &p, x, y, w) + m.gap * 1.4;
            }
            // Same reservation the month makes: one row, the most shareable
            // line on the card, and last in the stack is where it gets squeezed
            // out — so it is checked on its own rather than after a list.
            if c.crush.is_some() && y + m.crush_h() <= m.floor() {
                y = crush(&mut d, &m, c, &p, &colour, Unit::Week, x, y, w);
            }
            y
        }
        Size::Square => {
            let x = m.pad;
            let w = m.content();
            let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad);
            y = headline(&mut d, &m, c, &p, &colour, Unit::Week, x, y + m.gap);
            y += m.gap * 1.2;

            if y + m.bars_h() <= m.floor() {
                y = bars(&mut d, &m, &p, &colour, Unit::Week, c, x, y, w) + m.gap * 1.2;
            }
            if let Some(a) = &c.top_artist {
                if y + m.entity_h() <= m.floor() {
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
            if y + m.counts_h() <= m.floor() {
                y = counts(&mut d, &m, c, &p, x, y, w);
            }
            y
        }
        Size::Wide => {
            // The numbers and the shape on the left, what made them on the
            // right — the month's arrangement, with the bars taking the slot
            // its genre list has.
            let colw = (m.content() - m.gap * 2.0) / 2.0;
            let lx = m.pad;
            let rx = m.pad + colw + m.gap * 2.0;

            let y = header(&mut d, &m, c, &p, &colour, lx, m.pad);
            let mut left = headline(&mut d, &m, c, &p, &colour, Unit::Week, lx, y + m.gap);
            left += m.gap * 1.6;
            if left + m.bars_h() <= m.floor() {
                left = bars(&mut d, &m, &p, &colour, Unit::Week, c, lx, left, colw);
            }

            let mut right = m.pad + m.eyebrow;
            if let Some(a) = &c.top_artist {
                right = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    rx,
                    right,
                    colw,
                    "TOP ARTIST",
                    &a.name,
                    &format!("{} plays", nf(a.plays)),
                    &Card::initial(&a.name),
                ) + m.gap;
            }
            if let Some(t) = &c.top_track {
                right = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    rx,
                    right,
                    colw,
                    "TRACK OF THE WEEK",
                    &t.title,
                    &format!("{} · {} plays", t.artist, nf(t.plays)),
                    &Card::initial(&t.title),
                ) + m.gap;
            }
            right = counts(&mut d, &m, c, &p, rx, right, colw);
            // The left column ends well above the rule once the chart is drawn,
            // and the crush is the best line either card has — so it fills the
            // space rather than being dropped for want of a slot.
            if c.crush.is_some() {
                let cy = left + m.gap * 0.9;
                if cy + m.crush_h() <= m.floor() {
                    left = crush(&mut d, &m, c, &p, &colour, Unit::Week, lx, cy, colw);
                }
            }
            left.max(right)
        }
    };

    footer(&mut d, &m, c, &p, &colour);
    Drawn { covered: d.fully_covered(), svg: d.finish(), bottom }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_week() -> Card {
        Card::sample(Unit::Week)
    }

    /// The design's own rule, carried into the image: seven days do not support
    /// an hour-of-day distribution or a genre split, so the week card draws
    /// neither. It is an omission on purpose, and a test is what stops somebody
    /// "completing" the card by adding them.
    #[test]
    fn a_week_draws_no_genre_split_however_many_genres_it_has() {
        let mut c = a_week();
        c.genres = vec![("dance-pop".into(), 41), ("chanson".into(), 22)];
        let svg = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(!svg.contains("TOP GENRES"), "a week's genre split is noise, by design");
        assert!(svg.contains("SCROBBLES BY DAY"), "and its own shape is what it has instead");
    }

    /// The rank line and the crush both name the period, and naming it wrongly
    /// is the failure a shared unit invites — a week card reading "new to you
    /// this month" is a claim about four times the span.
    #[test]
    fn a_week_calls_itself_a_week() {
        let svg = render_measured(&a_week(), Size::Story, Theme::Dark).svg;
        assert!(svg.contains("Your 3rd busiest of 61 weeks"));
        assert!(svg.contains("NEW TO YOU THIS WEEK"));
        assert!(!svg.contains("THIS MONTH"), "the month's wording must not leak in");
    }

    /// A quiet day is a floor, not a gap. A missing bar in a row of seven reads
    /// as a rendering fault where a hairline reads as a Thursday off.
    #[test]
    fn a_day_with_nothing_in_it_still_draws_something() {
        let svg = render_measured(&a_week(), Size::Story, Theme::Dark).svg;
        let bars = svg.matches(r#"rx="#).count();
        assert!(bars >= 7, "seven days, seven marks — got {bars} rounded shapes in total");
    }

    /// A week with nothing but listens draws a headline and stops, rather than
    /// stacking headings over absences.
    #[test]
    fn a_thin_week_leaves_its_sections_out() {
        let c = Card { label: "Week of 3 March".into(), plays: 4, ..Default::default() };
        let svg = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(svg.contains("Week of 3 March"));
        assert!(!svg.contains("SCROBBLES BY DAY"), "no sub-buckets, so no chart");
        assert!(!svg.contains("TOP ARTIST"));
        assert!(!svg.contains("THE CRUSH"));
    }
}
