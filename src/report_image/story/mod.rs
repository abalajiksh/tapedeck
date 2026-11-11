//! A report period as a short animated story.
//!
//! The same facts as the share card, told one at a time: a handful of scenes,
//! each a stack of layers that fade, rise and count into place. It is played
//! two ways from **one description**:
//!
//! * the browser inlines each layer's SVG and moves it by evaluating the
//!   tracks (`web/src/lib/story.ts`);
//! * the server evaluates the same tracks at each frame's timestamp, writes
//!   the whole frame as one SVG and rasterises it for the video
//!   (`crate::report_video`).
//!
//! So there is still one thing that *draws* — every layer comes out of the
//! `svg::Doc` writer the cards use, with the embedded fonts and the measured
//! text — and two small evaluators that only move what was drawn.
//!
//! A frame is redrawn from SVG rather than composited from layers rasterised
//! once. That was measured on the Pi 4B (2026-11-10): the year card parses and
//! rasterises in 62 ms, while compositing eight pre-drawn tiles with
//! `tiny-skia` took 93 ms. Flat shapes and text are cheaper to draw again than
//! bitmaps are to move.

mod scenes;
pub mod timeline;

use std::fmt::Write;

use serde::Serialize;

use super::card::Card;
use super::layout::Unit;
use super::theme::Theme;
pub use timeline::{Count, Tracks};

/// The story shape. One size on purpose: a story is a phone held upright.
pub const WIDTH: u32 = 1080;
pub const HEIGHT: u32 = 1920;
/// Frames per second of the video. The browser plays at the display's rate.
pub const FPS: u32 = 30;
/// The token a counting layer carries where its number goes.
pub const COUNT_TOKEN: &str = "{{n}}";

/// One thing on screen: a piece of SVG and how it moves.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = StoryLayer)]
pub struct Layer {
    /// SVG elements in story coordinates (no `<svg>` root). Text is already
    /// fitted and escaped; draw it as it is.
    pub svg: String,
    /// The point scales are taken about, in story pixels.
    pub origin: [f32; 2],
    pub tracks: Tracks,
    /// Present when `svg` carries the `{{n}}` token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<Count>,
}

/// A few seconds about one fact.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = StoryScene)]
pub struct Scene {
    /// What the scene is about: `intro`, `volume`, `shape`, `artist`,
    /// `record`, `discovery` or `outro`. A period with nothing to say about
    /// one of these has no such scene.
    pub id: String,
    pub duration_ms: u32,
    /// How long the scene takes to fade into the next one, at its end. Zero
    /// for the last scene, which is held.
    pub fade_out_ms: u32,
    /// Bottom first.
    pub layers: Vec<Layer>,
}

/// A whole story: a ground that never moves and the scenes played over it.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Story {
    pub width: u32,
    pub height: u32,
    /// The video's frame rate. A player may draw more often than this.
    pub fps: u32,
    pub unit: String,
    /// The period's own name: "2026", "August 2026", "Week of 17 August".
    pub label: String,
    pub theme: String,
    /// SVG drawn under every scene and never animated.
    pub background: String,
    pub scenes: Vec<Scene>,
    /// False when a string that is drawn contains a character the embedded
    /// fonts cannot draw. The video may then show boxes for it; a browser,
    /// which has its own fonts, usually will not.
    pub fully_covered: bool,
}

impl Story {
    /// Build the story of one period.
    pub fn build(c: &Card, unit: Unit, theme: Theme) -> Story {
        scenes::build(c, unit, theme)
    }

    pub fn duration_ms(&self) -> u32 {
        self.scenes.iter().map(|s| s.duration_ms).sum()
    }

    pub fn frames(&self) -> u32 {
        (self.duration_ms() as u64 * self.fps as u64).div_ceil(1000) as u32
    }

    /// Which scene a moment falls in, and how far into it. Past the end is
    /// the last scene's last moment, so a rounding frame never draws nothing.
    pub fn locate(&self, ms: u32) -> (usize, u32) {
        let mut left = ms;
        for (i, s) in self.scenes.iter().enumerate() {
            if left < s.duration_ms {
                return (i, left);
            }
            left -= s.duration_ms;
        }
        let last = self.scenes.len().saturating_sub(1);
        (last, self.scenes.get(last).map(|s| s.duration_ms).unwrap_or(0))
    }

    /// The complete SVG of one frame.
    ///
    /// Deterministic to the byte for a given `(scene, t)`, and identical for
    /// two moments in which nothing moved — which is how the encoder knows a
    /// frame is a repeat without rasterising it.
    pub fn frame_svg(&self, scene: usize, t: u32) -> String {
        let mut out = String::with_capacity(self.background.len() + 8 * 1024);
        let _ = write!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#,
            w = self.width,
            h = self.height
        );
        out.push_str(&self.background);
        let Some(s) = self.scenes.get(scene) else {
            out.push_str("</svg>");
            return out;
        };

        let env = timeline::envelope(t, s.duration_ms, s.fade_out_ms);
        if env > 0.001 {
            if env < 0.999 {
                let _ = write!(out, r#"<g opacity="{env:.3}">"#);
            } else {
                out.push_str("<g>");
            }
            for layer in &s.layers {
                let st = layer.tracks.at(t);
                // An invisible layer is left out rather than drawn at zero:
                // usvg would still lay its text out.
                if st.opacity <= 0.001 {
                    continue;
                }
                out.push_str("<g");
                if let Some(tf) = timeline::transform(&st, layer.origin) {
                    let _ = write!(out, r#" transform="{tf}""#);
                }
                if st.opacity < 0.999 {
                    let _ = write!(out, r#" opacity="{:.3}""#, st.opacity);
                }
                out.push('>');
                match &layer.count {
                    Some(c) => {
                        out.push_str(&layer.svg.replace(COUNT_TOKEN, &super::layout::nf(c.at(t))))
                    }
                    None => out.push_str(&layer.svg),
                }
                out.push_str("</g>");
            }
            out.push_str("</g>");
        }
        out.push_str("</svg>");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> usvg::Options<'static> {
        let mut opt = usvg::Options {
            fontdb: std::sync::Arc::new(super::super::fonts::db().clone()),
            ..Default::default()
        };
        opt.font_family = super::super::fonts::BODY.to_string();
        opt
    }

    fn stories() -> Vec<(Unit, Story)> {
        [Unit::Week, Unit::Month, Unit::Year]
            .into_iter()
            .map(|u| (u, Story::build(&Card::sample(u), u, Theme::Dark)))
            .collect()
    }

    /// Every frame of every story is a document usvg accepts.
    ///
    /// A layer's fragment is concatenated into the frame, so one unescaped
    /// name or one unclosed group fails the *whole* frame — and in a video
    /// that is a job that dies halfway. Sampled across each scene, including
    /// its first and last millisecond and the middle of its fade.
    #[test]
    fn every_scene_parses_at_every_stage_of_its_life() {
        let opt = options();
        for (unit, story) in stories() {
            assert!(story.scenes.len() >= 3, "{unit:?} tells almost nothing");
            for (i, s) in story.scenes.iter().enumerate() {
                let fade = s.duration_ms.saturating_sub(s.fade_out_ms / 2);
                for t in [0, 1, 300, 900, s.duration_ms / 2, fade, s.duration_ms] {
                    let svg = story.frame_svg(i, t);
                    usvg::Tree::from_str(&svg, &opt)
                        .unwrap_or_else(|e| panic!("{unit:?}/{} at {t} ms: {e}", s.id));
                }
            }
        }
    }

    /// A name with markup in it cannot break a frame.
    #[test]
    fn a_name_with_markup_in_it_cannot_break_a_frame() {
        let mut c = Card::sample(Unit::Year);
        c.top_artist =
            Some(super::super::card::Named { name: "Simon & <Garfunkel>".into(), plays: 12 });
        c.top_chain = Some("\"Desk\" <2>".into());
        let story = Story::build(&c, Unit::Year, Theme::Light);
        let opt = options();
        for (i, s) in story.scenes.iter().enumerate() {
            usvg::Tree::from_str(&story.frame_svg(i, s.duration_ms / 2), &opt)
                .unwrap_or_else(|e| panic!("{}: {e}", s.id));
        }
    }

    /// A count ends on the figure the report gave, and the token never
    /// reaches a frame.
    #[test]
    fn a_count_lands_on_the_real_figure_and_the_token_is_never_drawn() {
        for (unit, story) in stories() {
            for (i, s) in story.scenes.iter().enumerate() {
                for t in [0, s.duration_ms / 3, s.duration_ms] {
                    assert!(
                        !story.frame_svg(i, t).contains(COUNT_TOKEN),
                        "{unit:?}/{} at {t} ms draws the raw token",
                        s.id
                    );
                }
                for l in &s.layers {
                    match &l.count {
                        Some(c) => {
                            assert_eq!(l.svg.matches(COUNT_TOKEN).count(), 1, "{}", s.id);
                            assert!(
                                c.end_ms <= s.duration_ms,
                                "{} still counting at its end",
                                s.id
                            );
                        }
                        None => assert!(!l.svg.contains(COUNT_TOKEN), "{}", s.id),
                    }
                }
            }
        }
        let year = Story::build(&Card::sample(Unit::Year), Unit::Year, Theme::Dark);
        let vol = year.scenes.iter().position(|s| s.id == "volume").expect("a volume scene");
        assert!(year.frame_svg(vol, year.scenes[vol].duration_ms - 400).contains("21,390"));
    }

    /// Every entrance has finished before the scene starts to leave, so the
    /// moment before the fade shows the whole scene at rest.
    #[test]
    fn every_layer_has_arrived_before_its_scene_fades() {
        for (unit, story) in stories() {
            for s in &story.scenes {
                let settle = s.duration_ms - s.fade_out_ms;
                for (k, l) in s.layers.iter().enumerate() {
                    let tr = &l.tracks;
                    let last = [&tr.opacity, &tr.dx, &tr.dy, &tr.scale, &tr.scale_x, &tr.scale_y]
                        .iter()
                        .filter_map(|keys| keys.last().map(|k| k.t))
                        .max()
                        .unwrap_or(0);
                    assert!(
                        last + 600 <= settle,
                        "{unit:?}/{} layer {k} arrives at {last} ms of {settle}",
                        s.id
                    );
                    assert_eq!(tr.at(settle).opacity, 1.0, "{unit:?}/{} layer {k}", s.id);
                }
            }
        }
    }

    /// A held moment produces the same bytes twice — the encoder's cue to
    /// repeat a frame instead of drawing it — and a moving one does not.
    #[test]
    fn a_held_moment_is_the_same_frame_and_a_moving_one_is_not() {
        let story = Story::build(&Card::sample(Unit::Year), Unit::Year, Theme::Dark);
        let s = &story.scenes[0];
        let rest = s.duration_ms - s.fade_out_ms - 100;
        assert_eq!(story.frame_svg(0, rest), story.frame_svg(0, rest + 33));
        assert_ne!(story.frame_svg(0, 200), story.frame_svg(0, 233));
    }

    /// A period with nothing in it but a count still tells a story, and one
    /// with no top artist has no scene about one rather than an empty scene.
    #[test]
    fn a_thin_period_drops_scenes_rather_than_drawing_empty_ones() {
        let c = Card { label: "March 2019".into(), plays: 3, ..Default::default() };
        let story = Story::build(&c, Unit::Month, Theme::Dark);
        let ids: Vec<&str> = story.scenes.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["intro", "volume", "outro"]);
        assert_eq!(story.scenes.last().unwrap().fade_out_ms, 0, "the last scene is held");
        let (scene, t) = story.locate(story.duration_ms() + 5000);
        assert_eq!(scene, story.scenes.len() - 1);
        assert_eq!(t, story.scenes[scene].duration_ms);
    }

    /// Writes one PNG per scene (at rest) to `TAPEDECK_CARD_DIR`, because
    /// nothing above can tell that a scene is ugly.
    /// `TAPEDECK_CARD_DIR=/tmp/story cargo test -- --ignored render_the_story`
    #[test]
    #[ignore]
    fn render_the_story_to_look_at() {
        let dir = std::env::var("TAPEDECK_CARD_DIR").expect("set TAPEDECK_CARD_DIR");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let opt = options();
        for unit in [Unit::Week, Unit::Month, Unit::Year] {
            for theme in [Theme::Dark, Theme::Light] {
                let story = Story::build(&Card::sample(unit), unit, theme);
                for (i, s) in story.scenes.iter().enumerate() {
                    let svg = story.frame_svg(i, s.duration_ms - s.fade_out_ms - 1);
                    let tree = usvg::Tree::from_str(&svg, &opt).expect("svg");
                    let mut px = tiny_skia::Pixmap::new(WIDTH, HEIGHT).expect("pixmap");
                    resvg::render(&tree, tiny_skia::Transform::identity(), &mut px.as_mut());
                    let path = format!(
                        "{dir}/story-{}-{}-{i}-{}.png",
                        unit.as_str(),
                        theme.as_str(),
                        s.id
                    );
                    px.save_png(&path).expect("write");
                    println!("{path}");
                }
            }
        }
    }
}
