//! The built-in mixdown: which scenes a period gets and what each one draws.
//!
//! The rules are the cards' rules. A scene with nothing to say is left out
//! rather than drawn empty; every figure is the report's own; a count that is
//! a count *of* something says of what; and hours are a floor, because a
//! listen whose source reported no length adds nothing to them.
//!
//! Each unit gets the scenes its chapter can support — a week has no
//! discovery count, and only a year calls its artists "new to you this year".

use super::super::card::Card;
use super::super::fonts::{ellipsise, width, Face};
use super::super::layout::{self, nf, ordinal, Unit, M};
use super::super::svg::{Anchor, Doc};
use super::super::theme::{period_colour, Palette, Theme};
use super::{href_ok, Art, Count, Layer, Mixdown, Scene, Tracks, COUNT_TOKEN, FPS, HEIGHT, WIDTH};

/// How long a scene takes to hand over to the next.
const FADE_MS: u32 = 260;

struct B<'a> {
    m: M,
    p: Palette,
    colour: String,
    unit: Unit,
    c: &'a Card,
    covered: bool,
    art: &'a Art,
    /// The pictures actually drawn, with the largest side each is drawn at.
    used: Vec<(String, u32)>,
}

/// The largest size up to `max` at which `text` fits in `room`, never below
/// 42% of it — past that the caller's ellipsis takes over.
fn fit(text: &str, face: Face, max: f32, room: f32) -> f32 {
    let w = width(text, face, max);
    if w <= room {
        max
    } else {
        // A hair under the exact ratio: glyph advances do not scale to the
        // last fraction of a pixel, and a string sized to fit exactly would be
        // ellipsised by the caller for want of a rounding error.
        (max * room / w * 0.985).max(max * 0.42)
    }
}

impl B<'_> {
    fn layer(
        &mut self,
        origin: [f32; 2],
        tracks: Tracks,
        draw: impl FnOnce(&mut Doc, &Self),
    ) -> Layer {
        let mut d = Doc::new(self.m.w, self.m.h);
        draw(&mut d, self);
        self.covered &= d.fully_covered();
        Layer { svg: d.into_fragment(), origin, tracks, count: None }
    }

    /// A picture this mixdown may draw, noted with the size it is drawn at. An
    /// address of a shape a mixdown does not load is dropped here, so it is
    /// neither drawn nor fetched.
    fn picture(&mut self, href: Option<&String>, side: f32) -> Option<String> {
        let href = href.filter(|h| href_ok(h))?.clone();
        let px = side.ceil() as u32;
        match self.used.iter_mut().find(|(h, _)| *h == href) {
            Some(seen) => seen.1 = seen.1.max(px),
            None => self.used.push((href.clone(), px)),
        }
        Some(href)
    }

    /// A small tracked heading, the cards' eyebrow.
    fn eyebrow(&mut self, y: f32, s: &str, anchor: Anchor, start: u32) -> Layer {
        let x = if anchor == Anchor::Middle { self.m.w / 2.0 } else { self.m.pad };
        self.layer([x, y], Tracks::rise(start), |d, b| {
            let px = b.m.eyebrow;
            d.text_tracked(x, y, s, Face::SemiBold, px, b.p.muted, anchor, px * 0.24, 1.0);
        })
    }

    /// One line of body text.
    #[allow(clippy::too_many_arguments)]
    fn line(
        &mut self,
        y: f32,
        s: &str,
        face: Face,
        px: f32,
        strong: bool,
        anchor: Anchor,
        start: u32,
    ) -> Layer {
        let x = if anchor == Anchor::Middle { self.m.w / 2.0 } else { self.m.pad };
        self.layer([x, y], Tracks::rise(start), |d, b| {
            let fill = if strong { b.p.text } else { b.p.muted };
            let s = ellipsise(s, face, px, b.m.content());
            d.text(x, y, &s, face, px, fill, anchor);
        })
    }

    fn unit_upper(&self) -> &'static str {
        match self.unit {
            Unit::Week => "WEEK",
            Unit::Month => "MONTH",
            Unit::Year => "YEAR",
        }
    }

    /// What one bar of the shape chart is.
    fn bucket(&self) -> &'static str {
        match self.unit {
            Unit::Week => "day",
            Unit::Month => "week",
            Unit::Year => "month",
        }
    }

    fn intro(&mut self) -> Scene {
        let (pad, content) = (self.m.pad, self.m.content());
        let mut layers = Vec::new();
        let top = 640.0;
        let head = format!("TAPEDECK · YOUR {}", self.unit_upper());
        layers.push(self.eyebrow(top, &head, Anchor::Start, 150));

        let label = self.c.label.clone();
        let px = fit(&label, Face::Display, self.m.huge, content);
        let base = top + 50.0 + px * 0.86;
        layers.push(self.layer([pad, base], Tracks::rise(380), |d, b| {
            let s = ellipsise(&label, Face::Display, px, content);
            d.text(pad, base, &s, Face::Display, px, b.p.text, Anchor::Start);
        }));

        let ry = base + px * 0.24 + 18.0;
        layers.push(self.layer([pad, ry], Tracks::rule(760), |d, b| {
            d.rounded(pad, ry, 300.0, 10.0, 5.0, &b.colour);
        }));

        let sub = format!("{} scrobbles. Here is how it went.", nf(self.c.plays));
        layers.push(self.line(
            ry + 96.0,
            &sub,
            Face::Regular,
            self.m.sub * 1.15,
            false,
            Anchor::Start,
            1050,
        ));
        scene("intro", 3400, layers)
    }

    fn volume(&mut self) -> Scene {
        let (pad, content) = (self.m.pad, self.m.content());
        let c = self.c;
        let mut layers = Vec::new();
        let top = 580.0;
        layers.push(self.eyebrow(top, "YOU PRESSED PLAY", Anchor::Start, 100));

        // Sized for the final figure, which is the widest the count gets.
        let px = fit(&nf(c.plays), Face::Display, self.m.huge * 1.25, content);
        let base = top + 44.0 + px * 0.86;
        let mut n = self.layer([pad, base], Tracks::rise(250), |d, b| {
            d.text(pad, base, COUNT_TOKEN, Face::Display, px, &b.colour, Anchor::Start);
        });
        n.count = Some(Count { from: 0, to: c.plays, start_ms: 300, end_ms: 1700 });
        layers.push(n);

        let mut y = base + px * 0.22 + self.m.sub * 1.4;
        let word = if c.plays == 1 { "scrobble" } else { "scrobbles" };
        layers.push(self.line(y, word, Face::Regular, self.m.sub * 1.3, false, Anchor::Start, 500));

        let step = self.m.lead * 2.1;
        let mut start = 1900;
        y += step * 1.5;
        // A floor, and said as one: a listen with no reported length adds
        // nothing to the hours, so the true figure can only be higher.
        if c.hours >= 1.0 {
            let s = format!("At least {} hours of music", nf(c.hours.floor() as i64));
            layers.push(self.line(
                y,
                &s,
                Face::SemiBold,
                self.m.lead * 1.35,
                true,
                Anchor::Start,
                start,
            ));
            y += step;
            start += 400;
        }
        if c.avg_daily > 0 {
            let s = format!(
                "{} a day, counting every day of the {}",
                nf(c.avg_daily),
                self.unit.as_str()
            );
            layers.push(self.line(
                y,
                &s,
                Face::Regular,
                self.m.lead * 1.2,
                false,
                Anchor::Start,
                start,
            ));
            y += step;
            start += 400;
        }
        if c.rank > 0 && c.rank_of > 1 {
            let s = format!(
                "Your {} busiest of {} {}",
                ordinal(c.rank),
                c.rank_of,
                self.unit.noun(c.rank_of)
            );
            layers.push(self.line(
                y,
                &s,
                Face::Regular,
                self.m.lead * 1.2,
                false,
                Anchor::Start,
                start,
            ));
        }
        scene("volume", 4600, layers)
    }

    fn shape(&mut self) -> Option<Scene> {
        let c = self.c;
        if c.sub.len() < 2 {
            return None;
        }
        let (pad, w) = (self.m.pad, self.m.content());
        let mut layers = Vec::new();
        let top = 540.0;
        layers.push(self.eyebrow(top, self.unit.shape_heading(), Anchor::Start, 100));

        let band = 560.0;
        let base = top + 70.0 + band;
        let n = c.sub.len() as f32;
        let gap = (self.m.gap * 0.5).min(w / n * 0.34);
        let bw = (w - gap * (n - 1.0)) / n;
        let peak = c.sub.iter().copied().max().unwrap_or(0).max(1);
        let peak_at = c.sub.iter().position(|v| *v == peak);

        for (i, v) in c.sub.iter().enumerate() {
            let bx = pad + (bw + gap) * i as f32;
            let h = (band * (*v as f32 / peak as f32)).max(4.0);
            let r = (bw * 0.24).min(14.0).min(h / 2.0);
            let loudest = Some(i) == peak_at;
            let start = 350 + 70 * i as u32;
            layers.push(self.layer([bx, base], Tracks::stand(start), |d, b| {
                // The loudest bucket in full colour, the rest held back, so
                // the line under the chart has something to point at.
                let op = if loudest { 1.0 } else { 0.42 };
                d.rounded_opacity(bx, base - h, bw, h, r, &b.colour, op);
            }));
        }

        let lpx = self.m.row_sub * 0.9;
        if !c.sub_labels.is_empty() && bw >= width("00", Face::Regular, lpx) {
            layers.push(self.layer([pad, base], Tracks::fade(300), |d, b| {
                for (i, label) in c.sub_labels.iter().enumerate().take(c.sub.len()) {
                    let cx = pad + (bw + gap) * i as f32 + bw / 2.0;
                    d.text(
                        cx,
                        base + lpx * 1.6,
                        label,
                        Face::Regular,
                        lpx,
                        b.p.muted_3,
                        Anchor::Middle,
                    );
                }
            }));
        }

        let after = 350 + 70 * c.sub.len() as u32 + 640;
        let s = format!("Your busiest {}: {} scrobbles", self.bucket(), nf(peak));
        layers.push(self.line(
            base + 190.0,
            &s,
            Face::SemiBold,
            self.m.lead * 1.35,
            true,
            Anchor::Start,
            after,
        ));
        Some(scene("shape", after + 2900, layers))
    }

    fn artist(&mut self) -> Option<Scene> {
        let a = self.c.top_artist.clone()?;
        let (cx, content) = (self.m.w / 2.0, self.m.content());
        let mut layers = Vec::new();
        let top = 470.0;
        let head = format!("ARTIST OF THE {}", self.unit_upper());
        layers.push(self.eyebrow(top, &head, Anchor::Middle, 100));

        let side = 380.0;
        let (tx, ty) = (cx - side / 2.0, top + 70.0);
        let portrait = self.picture(self.art.artist.as_ref(), side);
        layers.push(self.layer([cx, ty + side / 2.0], Tracks::pop(300), |d, b| {
            // The lettered tile first and the portrait over it: if the
            // picture never arrives, what is left is the design, not a hole.
            let r = b.m.radius * 1.4;
            d.rounded(tx, ty, side, side, r, &b.colour);
            let px = side * 0.56;
            d.text(
                cx,
                layout::centred(ty, side, px),
                &Card::initial(&a.name),
                Face::Display,
                px,
                b.p.bg,
                Anchor::Middle,
            );
            if let Some(href) = &portrait {
                d.image(tx, ty, side, side, r, href, "art-artist");
            }
        }));

        let px = fit(&a.name, Face::Display, 112.0, content);
        let ny = ty + side + 60.0 + px;
        layers.push(self.layer([cx, ny], Tracks::rise(700), |d, b| {
            let s = ellipsise(&a.name, Face::Display, px, content);
            d.text(cx, ny, &s, Face::Display, px, b.p.text, Anchor::Middle);
        }));

        let spx = self.m.sub * 1.25;
        let word = if a.plays == 1 { "scrobble" } else { "scrobbles" };
        let mut n = self.layer([cx, ny + spx * 2.1], Tracks::rise(1000), |d, b| {
            let s = format!("{COUNT_TOKEN} {word}");
            d.text(cx, ny + spx * 2.1, &s, Face::Regular, spx, b.p.muted, Anchor::Middle);
        });
        n.count = Some(Count { from: 0, to: a.plays, start_ms: 1050, end_ms: 2200 });
        layers.push(n);
        Some(scene("artist", 4600, layers))
    }

    fn record(&mut self) -> Option<Scene> {
        let c = self.c;
        if c.top_album.is_none() && c.top_track.is_none() {
            return None;
        }
        let (pad, w) = (self.m.pad, self.m.content());
        let mut layers = Vec::new();
        let top = 600.0;
        layers.push(self.eyebrow(top, "WHAT YOU KEPT COMING BACK TO", Anchor::Start, 100));

        let mut y = top + 60.0;
        let mut start = 300;
        let unit = self.unit_upper();
        let rows = [
            ("RECORD", c.top_album.as_ref(), self.art.album.as_ref(), "art-record"),
            ("TRACK", c.top_track.as_ref(), self.art.track.as_ref(), "art-track"),
        ];
        // Where `entity_row` puts its tile.
        let (tile, inset) = (self.m.tile, self.m.tile * 0.16);
        for (kind, item, cover, clip) in rows {
            let Some(t) = item else { continue };
            let cover = self.picture(cover, tile);
            let label = format!("{kind} OF THE {unit}");
            let sub = if t.artist.is_empty() {
                format!("{} plays", nf(t.plays))
            } else {
                format!("{} · {} plays", t.artist, nf(t.plays))
            };
            let row_y = y;
            layers.push(self.layer([pad, row_y], Tracks::rise(start), |d, b| {
                layout::entity_row(
                    d,
                    &b.m,
                    &b.p,
                    &b.colour,
                    pad,
                    row_y,
                    w,
                    &label,
                    &t.title,
                    &sub,
                    &Card::initial(&t.title),
                );
                if let Some(href) = &cover {
                    let r = b.m.radius * 0.8;
                    d.image(pad + inset, row_y + inset, tile, tile, r, href, clip);
                }
            }));
            y += self.m.entity_h() + self.m.gap * 1.6;
            start += 500;
        }
        Some(scene("record", 4400, layers))
    }

    fn discovery(&mut self) -> Option<Scene> {
        let c = self.c;
        // "New to you" as a headline count is a year's claim; a week or a
        // month has the crush and nothing else to say here.
        let found = if self.unit == Unit::Year { c.new_artists } else { 0 };
        if found <= 0 && c.crush.is_none() {
            return None;
        }
        let (pad, w) = (self.m.pad, self.m.content());
        let mut layers = Vec::new();
        let mut y = 600.0;
        let mut start = 100;

        if found > 0 {
            layers.push(self.eyebrow(y, "NEW TO YOU", Anchor::Start, start));
            let px = fit(&nf(found), Face::Display, self.m.huge * 1.1, w);
            let base = y + 44.0 + px * 0.86;
            let mut n = self.layer([pad, base], Tracks::rise(250), |d, b| {
                d.text(pad, base, COUNT_TOKEN, Face::Display, px, &b.colour, Anchor::Start);
            });
            n.count = Some(Count { from: 0, to: found, start_ms: 300, end_ms: 1500 });
            layers.push(n);
            let word = if found == 1 { "artist" } else { "artists" };
            let s = format!("{word} you had never played before");
            y = base + px * 0.22 + self.m.sub * 1.5;
            layers.push(self.line(
                y,
                &s,
                Face::Regular,
                self.m.sub * 1.2,
                false,
                Anchor::Start,
                600,
            ));
            y += self.m.gap * 4.0;
            start = 1700;
        }

        if c.crush.is_some() {
            let row_y = y;
            let unit = self.unit;
            layers.push(self.layer([pad, row_y], Tracks::rise(start + 200), |d, b| {
                layout::crush(d, &b.m, b.c, &b.p, &b.colour, unit, pad, row_y, w);
            }));
        }
        Some(scene("discovery", 4800, layers))
    }

    fn outro(&mut self) -> Scene {
        let c = self.c;
        let (pad, w) = (self.m.pad, self.m.content());
        let mut layers = Vec::new();
        let top = 560.0;

        let label = c.label.clone();
        let px = fit(&label, Face::Display, self.m.label * 1.3, w);
        layers.push(self.layer([pad, top], Tracks::rise(100), |d, b| {
            let s = ellipsise(&label, Face::Display, px, w);
            d.text(pad, top, &s, Face::Display, px, b.p.text, Anchor::Start);
        }));
        let ry = top + px * 0.34;
        layers.push(self.layer([pad, ry], Tracks::rule(400), |d, b| {
            d.rounded(pad, ry, 200.0, 8.0, 4.0, &b.colour);
        }));

        let mut y = ry + self.m.gap * 2.4;
        let mut start = 600;
        if c.artists > 0 || c.albums > 0 || c.tracks > 0 {
            let row_y = y;
            layers.push(self.layer([pad, row_y], Tracks::rise(start), |d, b| {
                layout::counts(d, &b.m, b.c, &b.p, pad, row_y, w);
            }));
            y += self.m.counts_h() + self.m.gap * 2.6;
            start += 400;
        } else {
            y += self.m.gap;
        }

        let mut facts: Vec<String> = Vec::new();
        if c.sides > 0 {
            let word = if c.sides == 1 { "side" } else { "sides" };
            facts.push(format!("{} {word} played off the shelf", nf(c.sides)));
        }
        // Counted, never a share, and never without what it is a count of:
        // `company` is null on everything nobody tagged.
        if let Some((marked, of)) = c.company {
            if marked > 0 && of > 0 {
                facts.push(format!(
                    "{} of {} scrobbles marked as heard with somebody",
                    nf(marked),
                    nf(of)
                ));
            }
        }
        let fpx = self.m.lead * 1.15;
        for f in facts {
            layers.push(self.line(y, &f, Face::Regular, fpx, false, Anchor::Start, start));
            y += fpx * 1.9;
            start += 350;
        }

        let s = format!("That was {}.", c.label);
        let cpx = fit(&s, Face::Display, self.m.row_title * 1.25, w);
        layers.push(self.line(
            y + self.m.gap * 2.0 + cpx,
            &s,
            Face::Display,
            cpx,
            true,
            Anchor::Start,
            start + 200,
        ));
        scene("outro", start + 200 + 560 + 3200, layers)
    }
}

fn scene(id: &str, duration_ms: u32, layers: Vec<Layer>) -> Scene {
    Scene { id: id.to_string(), duration_ms, fade_out_ms: FADE_MS, layers }
}

pub(super) fn build(c: &Card, unit: Unit, theme: Theme, art: &Art) -> Mixdown {
    let p = theme.palette();
    let colour = period_colour(c.colour.as_deref(), &p);
    let mut b = B {
        m: M::for_size(super::super::layout::Size::Story),
        p,
        colour,
        unit,
        c,
        covered: true,
        art,
        used: Vec::new(),
    };

    // The ground: the card's own, so a mixdown and the card it ends on are
    // plainly the same object.
    let mut d = Doc::new(b.m.w, b.m.h);
    d.rect(0.0, 0.0, b.m.w, b.m.h, b.p.bg);
    d.wash(b.m.h * 0.62, &b.colour, 0.16, 0.0);
    d.rect(0.0, 0.0, b.m.w, 12.0, &b.colour);
    layout::footer(&mut d, &b.m, c, &b.p, &b.colour);
    b.covered &= d.fully_covered();
    let background = d.into_fragment();

    let mut scenes = vec![b.intro(), b.volume()];
    scenes.extend(b.shape());
    scenes.extend(b.artist());
    scenes.extend(b.record());
    scenes.extend(b.discovery());
    scenes.push(b.outro());
    if let Some(last) = scenes.last_mut() {
        last.fade_out_ms = 0;
    }

    Mixdown {
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        unit: unit.as_str().to_string(),
        label: c.label.clone(),
        theme: theme.as_str().to_string(),
        background,
        scenes,
        fully_covered: b.covered,
        art: b.used,
    }
}
