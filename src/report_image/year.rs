//! The year card.
//!
//! `YearChapter` is the one place the design breaks the chapter shape — a bento
//! of six columns rather than a column of sections — and its reasoning is that
//! a year has no "scrobbles by week" to headline: what it has is **a shape over
//! twelve months and a handful of superlatives**.
//!
//! A story image is a single tall column, so the bento does not transfer as a
//! grid. What transfers is the *priority*, and it is the bento's own: the two
//! largest tiles are the twelve-month shape and the genre bands under it, and
//! everything else on that page is a small tile. So this card leads with those
//! two charts and puts the superlatives beneath, which is the same page read
//! top to bottom.
//!
//! **The discovery figures are the year's, not the month's.** New artists and
//! sides off the shelf are things a month can barely move and a year states
//! plainly, and they are the one row here that has no equivalent on the other
//! two cards.

use super::card::Card;
use super::layout::*;
use super::svg::Doc;
use super::theme::{period_colour, Theme};

/// Artists heard for the first time, sides off the shelf, and who it was heard
/// with — the year's own row of figures.
///
/// **Company is counted and never given as a share.** `scrobbles.company` is
/// null on everything nobody has tagged, so a percentage would report how the
/// listening was *recorded* while appearing to report how it happened — the
/// fidelity-card failure in a new place. The card states the marked count and
/// what it is a count of, and claims nothing about the rest.
fn discovery_lines(c: &Card) -> Vec<String> {
    let mut lines = Vec::new();
    let mut found = Vec::new();
    if c.new_artists > 0 {
        found.push(format!("{} new artists", nf(c.new_artists)));
    }
    if c.sides > 0 {
        found.push(format!("{} sides off the shelf", nf(c.sides)));
    }
    if !found.is_empty() {
        lines.push(format!("You discovered {}", found.join(" · ")));
    }
    if let Some((marked, of)) = c.company {
        lines.push(format!("{} of {} listens marked as heard with somebody", nf(marked), nf(of)));
    }
    lines
}

fn facts_rows(
    d: &mut Doc,
    m: &M,
    p: &super::theme::Palette,
    x: f32,
    y: f32,
    w: f32,
    lines: &[String],
) -> f32 {
    if lines.is_empty() {
        return y;
    }
    facts(d, m, p, x, y, w, lines)
}

/// Build the SVG for one year, and report what the pass produced.
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
            y = headline(&mut d, &m, c, &p, &colour, Unit::Year, x, y + m.gap * 1.5);
            y += m.gap * 1.5;

            // The bento's two big tiles, in its own order.
            if y + m.bars_h() <= m.floor() {
                y = bars(&mut d, &m, &p, &colour, Unit::Year, c, x, y, w) + m.gap * 1.1;
            }
            if y + m.bands_h() <= m.floor() {
                y = bands(&mut d, &m, &p, &colour, c, x, y, w) + m.gap * 1.1;
            }

            // The crush is reserved before the entity rows rather than after
            // them — the month's rule, and it matters more here: a year's crush
            // is the single most shareable line the card has.
            // **Only the crush is reserved.** The discovery lines are the least
            // shareable thing on the card and the record of the year is among
            // the most, so reserving both put the record out — which is the
            // wrong way round, and visible only by rendering it.
            let reserve = if c.crush.is_some() { m.crush_h() + m.gap * 1.2 } else { 0.0 };

            if let Some(a) = &c.top_artist {
                if y + m.entity_h() + reserve <= m.floor() {
                    y = entity_row(
                        &mut d,
                        &m,
                        &p,
                        &colour,
                        x,
                        y,
                        w,
                        "ARTIST OF THE YEAR",
                        &a.name,
                        &format!("{} plays", nf(a.plays)),
                        &Card::initial(&a.name),
                    ) + m.gap;
                }
            }
            if let Some(al) = &c.top_album {
                if y + m.entity_h() + reserve <= m.floor() {
                    y = entity_row(
                        &mut d,
                        &m,
                        &p,
                        &colour,
                        x,
                        y,
                        w,
                        "RECORD OF THE YEAR",
                        &al.title,
                        &format!("{} · {} plays", al.artist, nf(al.plays)),
                        &Card::initial(&al.title),
                    ) + m.gap;
                }
            }
            // As many of the discovery lines as fit **beside** the crush, not
            // instead of it: they are a list, so the tail can be dropped where
            // an entity row could only be dropped whole.
            let facts = discovery_lines(c);
            let n = m.rows_that_fit(y + reserve, facts.len(), m.facts_line(), false);
            if n > 0 {
                y = facts_rows(&mut d, &m, &p, x, y, w, &facts[..n]) + m.gap * 1.1;
            } else if facts.is_empty() && y + m.counts_h() + reserve <= m.floor() {
                // **Nothing discovered is a real state**, and on a live instance
                // a common one: a year whose artists all first appeared earlier
                // has none, and an un-enriched history has neither a shelf
                // figure nor genre bands. With all of those absent the card
                // stopped after the record and left a third of itself empty,
                // which reads as a render that failed rather than as a quiet
                // year. The counts are the one thing every history can always
                // answer, and they take the slot only when there is room —
                // a full year ends on the crush, which is no hole at all.
                y = counts(&mut d, &m, c, &p, x, y, w) + m.gap * 1.2;
            }
            if y + m.crush_h() <= m.floor() {
                y = crush(&mut d, &m, c, &p, &colour, Unit::Year, x, y, w);
            }
            y
        }
        Size::Square => {
            let x = m.pad;
            let w = m.content();
            let mut y = header(&mut d, &m, c, &p, &colour, x, m.pad);
            y = headline(&mut d, &m, c, &p, &colour, Unit::Year, x, y + m.gap);
            y += m.gap * 1.2;

            if y + m.bars_h() <= m.floor() {
                y = bars(&mut d, &m, &p, &colour, Unit::Year, c, x, y, w) + m.gap * 1.2;
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
                        "ARTIST OF THE YEAR",
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
            let colw = (m.content() - m.gap * 2.0) / 2.0;
            let lx = m.pad;
            let rx = m.pad + colw + m.gap * 2.0;

            let y = header(&mut d, &m, c, &p, &colour, lx, m.pad);
            let mut left = headline(&mut d, &m, c, &p, &colour, Unit::Year, lx, y + m.gap);
            left += m.gap * 1.6;
            if left + m.bars_h() <= m.floor() {
                left = bars(&mut d, &m, &p, &colour, Unit::Year, c, lx, left, colw);
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
                    "ARTIST OF THE YEAR",
                    &a.name,
                    &format!("{} plays", nf(a.plays)),
                    &Card::initial(&a.name),
                ) + m.gap;
            }
            if let Some(al) = &c.top_album {
                right = entity_row(
                    &mut d,
                    &m,
                    &p,
                    &colour,
                    rx,
                    right,
                    colw,
                    "RECORD OF THE YEAR",
                    &al.title,
                    &format!("{} · {} plays", al.artist, nf(al.plays)),
                    &Card::initial(&al.title),
                ) + m.gap;
            }
            right = facts_rows(&mut d, &m, &p, rx, right + m.gap * 0.4, colw, &discovery_lines(c));
            // The left column ends well above the rule once the chart is drawn,
            // and the crush is the best line either card has — so it fills the
            // space rather than being dropped for want of a slot.
            if c.crush.is_some() {
                let cy = left + m.gap * 0.9;
                if cy + m.crush_h() <= m.floor() {
                    left = crush(&mut d, &m, c, &p, &colour, Unit::Year, lx, cy, colw);
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

    fn a_year() -> Card {
        Card::sample(Unit::Year)
    }

    /// The year's two charts are what the bento gives its largest tiles, and
    /// they are what this card leads with.
    #[test]
    fn a_year_leads_with_its_shape_and_its_bands() {
        let svg = render_measured(&a_year(), Size::Story, Theme::Dark).svg;
        assert!(svg.contains("SCROBBLES BY MONTH"));
        assert!(svg.contains("MONTH BY MONTH"), "the genre bands");
        assert!(svg.contains("dance-pop"), "and the key that makes them readable");
    }

    /// A month with nothing tagged draws **no band**, rather than a full-height
    /// column of "other" — which on the page read as a genre actually played.
    #[test]
    fn a_month_with_no_tags_draws_no_band_rather_than_a_full_column_of_other() {
        let mut c = a_year();
        let mut shares: Vec<Vec<f64>> = (0..12).map(|_| vec![40.0, 22.0, 14.0, 24.0]).collect();
        shares[5] = vec![0.0, 0.0, 0.0, 0.0];
        c.genre_bands = Some((vec!["dance-pop".into()], shares));
        let with_gap = render_measured(&c, Size::Story, Theme::Dark).svg;

        let mut full = a_year();
        full.genre_bands = Some((
            vec!["dance-pop".into()],
            (0..12).map(|_| vec![40.0, 22.0, 14.0, 24.0]).collect(),
        ));
        let without = render_measured(&full, Size::Story, Theme::Dark).svg;

        assert!(
            with_gap.len() < without.len(),
            "the untagged month contributes no rectangles at all"
        );
    }

    /// Counted, never a share — the rule `scrobbles.company` carries
    /// everywhere. A bare percentage would be a fact about what has been
    /// *marked*, wearing the clothes of a fact about the listening.
    ///
    /// Asserted on the **wording** rather than on a rendered card, because
    /// which lines survive is the fit rule's business and varies with the
    /// shape: on a story there is room for one, on a wide card for both. The
    /// rule being guarded is that the sentence carries its own denominator, and
    /// that is true wherever it lands.
    #[test]
    fn company_is_a_count_of_something_and_says_what() {
        let lines = discovery_lines(&a_year());
        let company = lines.iter().find(|l| l.contains("heard with somebody")).expect("the line");
        assert!(company.contains("418 of 21,390"), "both numbers, or neither: {company}");
        assert!(!company.contains('%'), "never a share: {company}");

        // And it really is drawn where there is room for it.
        let svg = render_measured(&a_year(), Size::Wide, Theme::Dark).svg;
        assert!(svg.contains("418 of 21,390"));
    }

    /// The discovery lines are what yields, and the crush is what does not.
    ///
    /// **This is the priority, stated as a test.** A full year has more to say
    /// than a story card has room for, and the order it gives things up in is a
    /// decision rather than an accident: the crush and the two superlatives are
    /// what a year is shared for, and "you discovered 214 new artists" — good as
    /// it is — is the line that goes. Where there is no crush the room comes
    /// straight back.
    #[test]
    fn the_discovery_lines_give_way_to_the_crush_and_not_the_other_way_round() {
        let full = render_measured(&a_year(), Size::Story, Theme::Dark).svg;
        assert!(full.contains("THE CRUSH"), "the crush is reserved before the lines");
        assert!(full.contains("RECORD OF THE YEAR"), "and so are the superlatives");

        let mut c = a_year();
        c.crush = None;
        let roomier = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(roomier.contains("You discovered"), "with no crush, the lines land");
    }

    /// The rank line and the crush name the year, not the month.
    #[test]
    fn a_year_calls_itself_a_year() {
        let svg = render_measured(&a_year(), Size::Story, Theme::Dark).svg;
        assert!(svg.contains("Your 1st busiest of 4 years"));
        assert!(svg.contains("NEW TO YOU THIS YEAR"));
        assert!(!svg.contains("THIS MONTH"));
    }

    /// The earliest year has no crush — every artist in it is new by
    /// construction — and the card inherits that absence rather than drawing a
    /// heading over it.
    #[test]
    fn the_earliest_year_draws_no_crush() {
        let mut c = a_year();
        c.crush = None;
        let svg = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(!svg.contains("THE CRUSH"));
    }

    /// A year that discovered nothing draws its counts instead of a hole.
    ///
    /// Found on a live instance, not in a test: a year whose artists all first
    /// appeared earlier has no discovery line and an un-enriched history has no
    /// genre bands either, and the card stopped after the record with a third of
    /// itself empty. Sections are left out rather than emptied — but *nothing at
    /// all* below the fold reads as a render that failed.
    #[test]
    fn a_year_that_discovered_nothing_still_fills_its_card() {
        // The live state, reproduced: an un-enriched history has no genre
        // bands, a year whose artists all first appeared earlier has no crush
        // and no new artists, and nothing has come off a shelf. Removing only
        // the discovery figures is *not* this case — on a full year the counts
        // have nowhere to go either, and the crush ends the card perfectly
        // well.
        let mut c = a_year();
        c.new_artists = 0;
        c.sides = 0;
        c.company = None;
        c.crush = None;
        c.genre_bands = None;
        assert!(discovery_lines(&c).is_empty());
        let svg = render_measured(&c, Size::Story, Theme::Dark).svg;
        assert!(svg.contains("artists") && svg.contains("albums") && svg.contains("tracks"));
    }

    /// A year with no shelf and nobody in the room says neither, rather than
    /// writing a zero — which reads as a measurement of something.
    ///
    /// On the lines themselves, because which of them survive the fit is the
    /// composition's business and this is about what may be *said*.
    #[test]
    fn a_year_with_no_shelf_says_nothing_about_a_shelf() {
        let mut c = a_year();
        c.sides = 0;
        c.company = None;
        let lines = discovery_lines(&c).join(" | ");
        assert!(lines.contains("214 new artists"));
        assert!(!lines.contains("sides off the shelf"));
        assert!(!lines.contains("marked as heard"));

        // And a year with nothing to report says nothing at all.
        let mut bare = a_year();
        bare.new_artists = 0;
        bare.sides = 0;
        bare.company = None;
        assert!(discovery_lines(&bare).is_empty());
    }
}
