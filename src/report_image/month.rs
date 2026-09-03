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
//!
//! **A month draws no by-week bars, and that is the design's own judgement.**
//! `sub_bucket_count(Month)` is five, not thirty, and five columns sit directly
//! under a ranked genre list that says the same thing better. The week and the
//! year both draw theirs, because seven days and twelve months are shapes.

use super::card::Card;
use super::layout::*;
use super::svg::Doc;
use super::theme::{period_colour, Theme};

/// Build the SVG for one month, and report what the pass produced.
pub(super) fn render_measured(c: &Card, size: Size, theme: Theme) -> Drawn {
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
            y = headline(&mut d, &m, c, &p, &colour, Unit::Month, x, y + m.gap * 1.5);
            y += m.gap * 1.5;

            // Priority order, and each section is skipped whole rather than
            // clipped. The two entity rows and the counts are what a month is
            // shared *for*, so they go first and are not negotiable; everything
            // below them takes the room that is left.
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
            if let Some(al) = &c.top_album {
                if y + m.entity_h() <= m.floor() {
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
            if y + m.counts_h() <= m.floor() {
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
            if y + m.crush_h() <= m.floor() {
                y = crush(&mut d, &m, c, &p, &colour, Unit::Month, x, y, w);
            }
            y
        }
        Size::Square => {
            let x = m.pad;
            let w = m.content();
            let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad);
            y = headline(&mut d, &m, c, &p, &colour, Unit::Month, x, y + m.gap);
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
            let left_y = headline(&mut d, &m, c, &p, &colour, Unit::Month, lx, y + m.gap);

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
        Card::sample(Unit::Month)
    }

    #[test]
    fn an_unknown_shape_is_refused_rather_than_guessed() {
        assert_eq!(Size::parse(Some("story")), Some(Size::Story));
        assert_eq!(Size::parse(None), Some(Size::Story));
        assert_eq!(Size::parse(Some("portrait")), None);
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
