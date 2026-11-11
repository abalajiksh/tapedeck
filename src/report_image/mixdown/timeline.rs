//! The arithmetic of a mixdown: where a layer is at a given millisecond.
//!
//! **This file has a twin**, `web/src/lib/mixdown.ts`. The browser plays a mixdown
//! by evaluating the same tracks the encoder does, so the two must agree on
//! every rule here: what a track is before its first key and after its last,
//! the easing, the order the transform is composed in, how a count rounds.
//! Change one, change the other — `the_browser_twin_follows_the_same_rules`
//! pins the numbers the TypeScript is written against.

use serde::Serialize;

/// One point on a track: at `t` milliseconds into the scene the value is `v`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, utoipa::ToSchema)]
#[schema(as = MixdownKey)]
pub struct Key {
    pub t: u32,
    pub v: f32,
}

/// How one layer moves. Every track is optional; an empty one means the
/// property stays at rest (opacity 1, no offset, scale 1).
///
/// Before a track's first key the value is the first key's, after its last
/// the last key's, and between two keys it eases out (cubic) from one to the
/// other.
#[derive(Clone, Debug, Default, PartialEq, Serialize, utoipa::ToSchema)]
#[schema(as = MixdownTracks)]
pub struct Tracks {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub opacity: Vec<Key>,
    /// Horizontal offset in mixdown pixels.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dx: Vec<Key>,
    /// Vertical offset in mixdown pixels; positive is down.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dy: Vec<Key>,
    /// Uniform scale about the layer's `origin`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scale: Vec<Key>,
    /// Horizontal scale about the `origin`, multiplied with `scale`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scale_x: Vec<Key>,
    /// Vertical scale about the `origin`, multiplied with `scale`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scale_y: Vec<Key>,
}

/// A number that counts up while its layer is on screen.
///
/// The layer's `svg` carries the literal token `{{n}}` exactly once, and it is
/// replaced by the value at that moment, written with thousands separators.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, utoipa::ToSchema)]
#[schema(as = MixdownCount)]
pub struct Count {
    pub from: i64,
    pub to: i64,
    pub start_ms: u32,
    pub end_ms: u32,
}

/// Where a layer is at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub opacity: f32,
    pub dx: f32,
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
}

/// Ease-out cubic: fast away from the start, settling into the end.
pub fn ease(p: f32) -> f32 {
    let q = 1.0 - p.clamp(0.0, 1.0);
    1.0 - q * q * q
}

fn at(keys: &[Key], t: u32, rest: f32) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return rest;
    };
    if t <= first.t {
        return first.v;
    }
    if t >= last.t {
        return last.v;
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t >= a.t && t < b.t {
            let span = (b.t - a.t).max(1) as f32;
            return a.v + (b.v - a.v) * ease((t - a.t) as f32 / span);
        }
    }
    last.v
}

fn key(t: u32, v: f32) -> Key {
    Key { t, v }
}

impl Tracks {
    pub fn at(&self, t: u32) -> State {
        let s = at(&self.scale, t, 1.0);
        State {
            opacity: at(&self.opacity, t, 1.0).clamp(0.0, 1.0),
            dx: at(&self.dx, t, 0.0),
            dy: at(&self.dy, t, 0.0),
            sx: s * at(&self.scale_x, t, 1.0),
            sy: s * at(&self.scale_y, t, 1.0),
        }
    }

    /// Fade in while rising into place: the entrance most text makes.
    pub fn rise(start: u32) -> Tracks {
        Tracks {
            opacity: vec![key(start, 0.0), key(start + 420, 1.0)],
            dy: vec![key(start, 44.0), key(start + 560, 0.0)],
            ..Default::default()
        }
    }

    /// Fade in while growing to full size about the origin.
    pub fn pop(start: u32) -> Tracks {
        Tracks {
            opacity: vec![key(start, 0.0), key(start + 320, 1.0)],
            scale: vec![key(start, 0.82), key(start + 520, 1.0)],
            ..Default::default()
        }
    }

    /// Grow upwards from the origin, the way a bar stands up from a baseline.
    pub fn stand(start: u32) -> Tracks {
        Tracks { scale_y: vec![key(start, 0.0), key(start + 640, 1.0)], ..Default::default() }
    }

    /// Draw out to the right from the origin, the way a rule is ruled.
    pub fn rule(start: u32) -> Tracks {
        Tracks { scale_x: vec![key(start, 0.0), key(start + 520, 1.0)], ..Default::default() }
    }

    /// Nothing more than a fade.
    pub fn fade(start: u32) -> Tracks {
        Tracks { opacity: vec![key(start, 0.0), key(start + 420, 1.0)], ..Default::default() }
    }
}

impl Count {
    pub fn at(&self, t: u32) -> i64 {
        if t <= self.start_ms {
            return self.from;
        }
        if t >= self.end_ms {
            return self.to;
        }
        let p = (t - self.start_ms) as f32 / (self.end_ms - self.start_ms).max(1) as f32;
        self.from + ((self.to - self.from) as f64 * ease(p) as f64).round() as i64
    }
}

/// The SVG `transform` for a state, or `None` when the layer is at rest.
///
/// Composed as: move the origin to zero, scale, move it back, then offset. So
/// a scale never drifts the layer and an offset is never scaled.
pub fn transform(s: &State, origin: [f32; 2]) -> Option<String> {
    let [ox, oy] = origin;
    let moved = s.dx.abs() > 0.005 || s.dy.abs() > 0.005;
    let scaled = (s.sx - 1.0).abs() > 0.0005 || (s.sy - 1.0).abs() > 0.0005;
    match (moved, scaled) {
        (false, false) => None,
        (true, false) => Some(format!("translate({:.2} {:.2})", s.dx, s.dy)),
        (_, true) => Some(format!(
            "translate({:.2} {:.2}) scale({:.4} {:.4}) translate({:.2} {:.2})",
            s.dx + ox,
            s.dy + oy,
            s.sx,
            s.sy,
            -ox,
            -oy
        )),
    }
}

/// How visible the whole scene is at `t`: full, except for the fade that
/// hands over to the next scene. The last scene has `fade_out_ms = 0` and is
/// simply held.
pub fn envelope(t: u32, duration_ms: u32, fade_out_ms: u32) -> f32 {
    if fade_out_ms == 0 || t + fade_out_ms <= duration_ms {
        return 1.0;
    }
    (duration_ms.saturating_sub(t) as f32 / fade_out_ms as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track holds its first value until it starts and its last once it has
    /// finished. This is what lets a layer sit invisible until its turn
    /// without a key at zero.
    #[test]
    fn a_track_holds_before_its_first_key_and_after_its_last() {
        let t = Tracks::rise(500);
        assert_eq!(t.at(0).opacity, 0.0);
        assert_eq!(t.at(0).dy, 44.0);
        assert_eq!(t.at(5000).opacity, 1.0);
        assert_eq!(t.at(5000).dy, 0.0);
        let rest = Tracks::default().at(123);
        assert_eq!(rest, State { opacity: 1.0, dx: 0.0, dy: 0.0, sx: 1.0, sy: 1.0 });
    }

    /// The exact numbers `web/src/lib/mixdown.ts` is written against. If these
    /// change, the browser and the video stop showing the same thing.
    #[test]
    fn the_browser_twin_follows_the_same_rules() {
        assert_eq!(ease(0.5), 0.875);
        let t = Tracks { dx: vec![key(0, 0.0), key(1000, 100.0)], ..Default::default() };
        assert_eq!(t.at(500).dx, 87.5);
        let c = Count { from: 0, to: 21_390, start_ms: 200, end_ms: 1200 };
        assert_eq!(c.at(0), 0);
        assert_eq!(c.at(700), 18_716);
        assert_eq!(c.at(1200), 21_390);
        assert_eq!(c.at(9000), 21_390, "a count ends on the real figure, never near it");
        assert_eq!(envelope(0, 4000, 240), 1.0);
        assert_eq!(envelope(3880, 4000, 240), 0.5);
        assert_eq!(envelope(4000, 4000, 240), 0.0);
        assert_eq!(envelope(4000, 4000, 0), 1.0);
    }

    #[test]
    fn a_layer_at_rest_writes_no_transform() {
        let rest = Tracks::default().at(0);
        assert_eq!(transform(&rest, [10.0, 20.0]), None);
        let moved = State { dy: 12.0, ..rest };
        assert_eq!(transform(&moved, [10.0, 20.0]).as_deref(), Some("translate(0.00 12.00)"));
        let scaled = State { sy: 0.5, ..rest };
        assert_eq!(
            transform(&scaled, [10.0, 20.0]).as_deref(),
            Some("translate(10.00 20.00) scale(1.0000 0.5000) translate(-10.00 -20.00)")
        );
    }
}
