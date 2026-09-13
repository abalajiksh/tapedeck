//! Star ratings from a media server, and the threshold that turns one into a
//! love.
//!
//! Plex, Navidrome, Jellyfin and Emby all let you rate a track, and until now
//! Tapedeck read none of it — a library with years of deliberate five-star
//! marks on it arrived here as an undifferentiated history. This is the reading
//! half; [`crate::db`] stores it and the threshold below decides what becomes a
//! [`crate::db::Database::set_love`].
//!
//! ## Two scales, and keeping them apart is the whole design
//!
//! **The source's scale is a fact and the display scale is a preference.** A
//! rating means nothing without knowing what it was out of, and the three
//! servers disagree:
//!
//! | server | field | out of | what it is |
//! |---|---|---|---|
//! | Plex | `userRating` | **10** | five stars at half-star granularity — the UI draws 7 as 3½ |
//! | Navidrome / Subsonic | `userRating` | **5** | five stars, whole ones only |
//! | Jellyfin / Emby | `UserData.Rating` | **10** | a legacy MediaBrowser double; `IsFavorite` is what those servers actually use |
//! | Roon | — | — | **nothing.** See the note on it below |
//!
//! So the *raw number the server gave* is stored verbatim alongside *what it
//! was out of* ([`SourceScale`]), and everything downstream compares
//! [`Rating::fraction`] — a 0..=1 position on whatever scale it came from. That
//! is what lets one threshold mean the same thing across a Plex library rated
//! out of ten and a Navidrome one rated out of five.
//!
//! [`DisplayScale`] is the other half and is purely presentation: how *you*
//! want to read those numbers, and the units the threshold is typed in. Five
//! stars, five with halves, ten, or ten with halves.
//!
//! **The comparison is always on the fraction and never on the displayed
//! value**, which matters because a display scale can be coarser than the
//! source's. A Plex 7/10 shown on a whole-five-star display rounds to 4 stars —
//! and it must not then clear a "4 stars" threshold it does not actually meet.
//! `the_threshold_compares_the_fraction_and_never_the_rounded_display` pins it.
//!
//! ## Roon has no ratings, and that is not an omission here
//!
//! Checked against the `roon-api` crate's own types and the vendored
//! `node-roon-api` reference: there is no rating, favourite, star or thumb
//! field anywhere in the extension API. It is the same finding the Roon source
//! already records about signal path and MBIDs — Roon keeps that inside its own
//! client. A Roon source therefore reports that it has nothing to offer rather
//! than quietly returning zero rows, which is the rule every source in this
//! codebase follows about declaring its own blind spots.

use serde::{Deserialize, Serialize};

/// What a source's rating number is out of.
///
/// A bare `max` and nothing else, because all three servers rate in whole
/// units — the granularity a *person* sees is a property of that server's UI
/// (Plex draws its 0–10 as half stars) and not of the number on the wire.
/// Reconstructing "half stars" from the number would be inventing a fact the
/// payload does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SourceScale {
    pub max: f32,
}

impl SourceScale {
    pub const fn new(max: f32) -> Self {
        Self { max }
    }
    /// Plex's `userRating`, confirmed `minimum: 0, maximum: 10` in its own
    /// OpenAPI description of a `metadata` item.
    pub const PLEX: Self = Self::new(10.0);
    /// Subsonic's `userRating` is 1–5, and `setRating` takes 0–5.
    pub const SUBSONIC: Self = Self::new(5.0);
    /// `UserItemDataDto.Rating`, a nullable double inherited from MediaBrowser.
    ///
    /// Ten, not five. Worth knowing before trusting it: Jellyfin's own
    /// `POST /UserItems/{itemId}/Rating` takes **`likes` (a boolean) and no
    /// number at all**, so on a modern Jellyfin this field is usually empty and
    /// `IsFavorite` carries the signal. It is read because Emby still writes
    /// one and an old library may hold them.
    pub const MEDIABROWSER: Self = Self::new(10.0);
}

/// How the person reading wants ratings shown, and the units their threshold is
/// typed in.
///
/// Four, because these are the four ways anybody actually counts stars. It is a
/// closed set rather than an arbitrary `(max, step)` pair on purpose: a
/// threshold of "7 out of 13" is not a thing a person means, and every extra
/// degree of freedom here is another way for the number on the screen to
/// disagree with the number being compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayScale {
    /// ★★★★★ — five stars, whole ones.
    #[default]
    Five,
    /// ★★★★½ — five stars, halves allowed.
    FiveHalf,
    /// Ten stars, whole ones.
    Ten,
    /// Ten stars, halves allowed.
    TenHalf,
}

impl DisplayScale {
    /// The stored spelling. Short and stable — it goes in the preferences blob
    /// and comes back out of it.
    pub fn id(self) -> &'static str {
        match self {
            Self::Five => "5",
            Self::FiveHalf => "5h",
            Self::Ten => "10",
            Self::TenHalf => "10h",
        }
    }

    /// Parse the stored spelling. **An unrecognised value is the default, not
    /// an error** — this is read on paths with nobody to report to, and a
    /// settings blob that somehow holds nonsense should show five stars rather
    /// than fail. Same call `ScrobbleThreshold::from_prefs` makes by clamping.
    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "5h" | "5.5" | "fivehalf" => Self::FiveHalf,
            "10" | "ten" => Self::Ten,
            "10h" | "tenhalf" => Self::TenHalf,
            _ => Self::Five,
        }
    }

    /// Highest value expressible.
    pub fn max(self) -> f32 {
        match self {
            Self::Five | Self::FiveHalf => 5.0,
            Self::Ten | Self::TenHalf => 10.0,
        }
    }

    /// Smallest increment expressible.
    pub fn step(self) -> f32 {
        match self {
            Self::Five | Self::Ten => 1.0,
            Self::FiveHalf | Self::TenHalf => 0.5,
        }
    }

    /// What a fraction reads as on this scale, snapped to the nearest step.
    ///
    /// **Presentation only.** A scale coarser than the source's cannot express
    /// every rating it will be asked to show — a Plex 7/10 is 3.5 stars and a
    /// whole-five display rounds it to 4 — so this is a lens over the number
    /// and never the number itself. Every comparison uses the fraction.
    pub fn show(self, fraction: f32) -> f32 {
        let step = self.step();
        let raw = fraction.clamp(0.0, 1.0) * self.max();
        ((raw / step).round() * step * 100.0).round() / 100.0
    }

    /// The fraction a threshold typed on this scale means.
    ///
    /// Clamped rather than rejected: this decides what lands on somebody's
    /// Loved page, and a zero would love their entire library.
    pub fn fraction_of(self, value: f32) -> f32 {
        (value / self.max()).clamp(0.0, 1.0)
    }

    /// Every value a person can pick on this scale, for a slider or a picker.
    /// Starts at one step — zero is "not rated", not a rating.
    pub fn choices(self) -> Vec<f32> {
        let step = self.step();
        let n = (self.max() / step).round() as i32;
        (1..=n).map(|i| (i as f32 * step * 10.0).round() / 10.0).collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Five => "5 stars",
            Self::FiveHalf => "5 stars, half stars allowed",
            Self::Ten => "10 stars",
            Self::TenHalf => "10 stars, half stars allowed",
        }
    }
}

/// One rating, as the server gave it.
///
/// `value` is the number on the wire and `scale` is what that number was out
/// of. Both are kept: a bare 7 means nothing, and normalising at the door would
/// throw away the only thing that makes it answerable later if the reading
/// turns out to be wrong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rating {
    pub value: f32,
    pub scale: SourceScale,
}

impl Rating {
    pub fn new(value: f32, scale: SourceScale) -> Self {
        Self { value, scale }
    }

    /// Where this sits on its own scale, 0..=1. The one comparable number, and
    /// the only thing the threshold is ever applied to.
    pub fn fraction(&self) -> f32 {
        if self.scale.max <= 0.0 {
            return 0.0;
        }
        (self.value / self.scale.max).clamp(0.0, 1.0)
    }
}

/// The rule that turns ratings into loves.
///
/// Two parts, and they are separate because they answer different questions. A
/// **rating** is a measurement on a scale and needs a cut-off. A **favourite**
/// (Subsonic's `starred`, Jellyfin's `IsFavorite`) is already the same
/// statement a love is — somebody pressed a heart — so there is no threshold to
/// apply to it and applying one would be inventing a scale it does not have.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoveRule {
    /// At or above this fraction of a source's own maximum, a rating is a love.
    pub at_least: f32,
    /// Whether a starred or favourited item is a love regardless of its rating.
    pub include_starred: bool,
}

impl LoveRule {
    /// Four out of five. High enough that it means something, low enough that a
    /// library rated in whole stars is not reduced to its fives — and it is the
    /// same cut in both directions, since 8/10 is the same fraction.
    pub const DEFAULT_FRACTION: f32 = 0.8;

    pub fn new(at_least: f32, include_starred: bool) -> Self {
        Self { at_least: at_least.clamp(0.01, 1.0), include_starred }
    }

    /// Read the user's stored preferences.
    ///
    /// The threshold is stored **in the display scale's own units**, because
    /// that is what they typed and storing the derived fraction instead would
    /// make "4 stars" un-recoverable the moment they switched to a ten-star
    /// display. Converted here, once.
    pub fn from_prefs(prefs: &serde_json::Value) -> Self {
        let scale = display_scale_from_prefs(prefs);
        let at_least = prefs
            .get("rating_love_threshold")
            .and_then(|v| v.as_f64())
            .map(|v| scale.fraction_of(v as f32))
            .unwrap_or(Self::DEFAULT_FRACTION);
        let include_starred =
            prefs.get("rating_love_starred").and_then(|v| v.as_bool()).unwrap_or(true);
        Self::new(at_least, include_starred)
    }

    /// Does this rating clear the bar?
    ///
    /// `>=`, so a threshold of five stars includes five stars. The obvious
    /// reading, and the one a person means when they say "four and up".
    pub fn loves(&self, rating: Option<Rating>, starred: bool) -> bool {
        if starred && self.include_starred {
            return true;
        }
        match rating {
            // A rating of zero is how these servers spell "not rated", so it is
            // not a nought-star verdict and must not clear a floor-level
            // threshold.
            Some(r) if r.value > 0.0 => r.fraction() + f32::EPSILON >= self.at_least,
            _ => false,
        }
    }
}

impl Default for LoveRule {
    fn default() -> Self {
        Self { at_least: Self::DEFAULT_FRACTION, include_starred: true }
    }
}

/// The display scale out of a preferences blob, defaulting to five stars.
pub fn display_scale_from_prefs(prefs: &serde_json::Value) -> DisplayScale {
    prefs
        .get("rating_scale")
        .and_then(|v| v.as_str())
        .map(DisplayScale::from_id)
        .unwrap_or_default()
}

/// The native scale of a source kind, or `None` where that source has no
/// ratings to give.
///
/// `None` is an answer rather than a failure and the caller must say so: Roon
/// genuinely publishes nothing, and reporting that is more useful than a source
/// that silently returns no rows and looks broken.
pub fn scale_for_source(kind: &str) -> Option<SourceScale> {
    match kind {
        "plex" => Some(SourceScale::PLEX),
        "navidrome" => Some(SourceScale::SUBSONIC),
        "jellyfin" | "emby" => Some(SourceScale::MEDIABROWSER),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rating_is_a_fraction_of_its_own_scale_and_not_of_somebody_elses() {
        // The whole reason the source's scale is stored: these are the same
        // verdict, said by two servers that count differently.
        let plex = Rating::new(10.0, SourceScale::PLEX);
        let navidrome = Rating::new(5.0, SourceScale::SUBSONIC);
        assert_eq!(plex.fraction(), 1.0);
        assert_eq!(navidrome.fraction(), 1.0);

        // And four-out-of-five is eight-out-of-ten.
        assert_eq!(Rating::new(8.0, SourceScale::PLEX).fraction(), 0.8);
        assert_eq!(Rating::new(4.0, SourceScale::SUBSONIC).fraction(), 0.8);
    }

    #[test]
    fn the_threshold_compares_the_fraction_and_never_the_rounded_display() {
        // A Plex 7/10 is three and a half stars. On a *whole* five-star display
        // it can only be drawn as 4 — and it must not therefore clear a
        // four-star bar it does not meet. This is the load-bearing rule: the
        // display is a lens, the fraction is the measurement.
        let seven = Rating::new(7.0, SourceScale::PLEX);
        assert_eq!(DisplayScale::Five.show(seven.fraction()), 4.0, "it reads as four stars");

        let four_stars = LoveRule::new(DisplayScale::Five.fraction_of(4.0), false);
        assert!(!four_stars.loves(Some(seven), false), "and is still not a four-star rating");

        // Eight is, on either display.
        let eight = Rating::new(8.0, SourceScale::PLEX);
        assert!(four_stars.loves(Some(eight), false));
    }

    #[test]
    fn one_threshold_means_the_same_thing_on_every_server() {
        let rule = LoveRule::new(DisplayScale::Five.fraction_of(4.0), false);
        // Plex counts to ten, Navidrome to five, Jellyfin to ten. Four stars.
        assert!(rule.loves(Some(Rating::new(8.0, SourceScale::PLEX)), false));
        assert!(rule.loves(Some(Rating::new(4.0, SourceScale::SUBSONIC)), false));
        assert!(rule.loves(Some(Rating::new(8.0, SourceScale::MEDIABROWSER)), false));
        // And three stars is not, on any of them.
        assert!(!rule.loves(Some(Rating::new(6.0, SourceScale::PLEX)), false));
        assert!(!rule.loves(Some(Rating::new(3.0, SourceScale::SUBSONIC)), false));
        assert!(!rule.loves(Some(Rating::new(6.0, SourceScale::MEDIABROWSER)), false));
    }

    #[test]
    fn a_full_marks_threshold_still_includes_full_marks() {
        // `>=` on floats, where 5.0 / 5.0 is exactly 1.0 but 10.0 / 10.0 need
        // not be on every target. A five-star threshold that excluded five
        // stars would be the bug nobody would think to look for.
        let rule = LoveRule::new(1.0, false);
        assert!(rule.loves(Some(Rating::new(5.0, SourceScale::SUBSONIC)), false));
        assert!(rule.loves(Some(Rating::new(10.0, SourceScale::PLEX)), false));
        assert!(!rule.loves(Some(Rating::new(9.0, SourceScale::PLEX)), false));
    }

    #[test]
    fn an_unrated_item_is_not_a_nought_star_verdict() {
        // These servers spell "never rated" as a zero or an absent field, so a
        // low threshold must not sweep the entire library onto the Loved page.
        let generous = LoveRule::new(0.01, false);
        assert!(!generous.loves(None, false));
        assert!(!generous.loves(Some(Rating::new(0.0, SourceScale::PLEX)), false));
        assert!(generous.loves(Some(Rating::new(1.0, SourceScale::PLEX)), false));
    }

    #[test]
    fn a_favourite_needs_no_threshold_and_obeys_its_own_switch() {
        // Somebody pressing a heart in Jellyfin has already said the thing a
        // love says. There is no scale to apply a cut-off to.
        let strict = LoveRule::new(1.0, true);
        assert!(strict.loves(None, true));
        assert!(strict.loves(Some(Rating::new(1.0, SourceScale::SUBSONIC)), true));

        let ratings_only = LoveRule::new(1.0, false);
        assert!(!ratings_only.loves(None, true));
    }

    #[test]
    fn a_display_scale_survives_the_round_trip_and_nonsense_reads_as_the_default() {
        for s in
            [DisplayScale::Five, DisplayScale::FiveHalf, DisplayScale::Ten, DisplayScale::TenHalf]
        {
            assert_eq!(DisplayScale::from_id(s.id()), s, "{} did not round-trip", s.id());
        }
        // Not an error: read on paths with nobody to report to.
        assert_eq!(DisplayScale::from_id("seventeen"), DisplayScale::Five);
        assert_eq!(DisplayScale::from_id(""), DisplayScale::Five);
    }

    #[test]
    fn every_choice_on_a_scale_is_expressible_on_it() {
        // A picker built from `choices` must not offer a value that `show`
        // would then move. Both halves come from `step`, so this is really a
        // guard that they keep coming from the same place.
        for s in
            [DisplayScale::Five, DisplayScale::FiveHalf, DisplayScale::Ten, DisplayScale::TenHalf]
        {
            for v in s.choices() {
                let round_tripped = s.show(s.fraction_of(v));
                assert!(
                    (round_tripped - v).abs() < 0.001,
                    "{}: {v} came back as {round_tripped}",
                    s.id()
                );
            }
            assert_eq!(*s.choices().last().unwrap(), s.max());
            assert_eq!(s.choices()[0], s.step());
        }
        assert_eq!(DisplayScale::Five.choices().len(), 5);
        assert_eq!(DisplayScale::FiveHalf.choices().len(), 10);
        assert_eq!(DisplayScale::Ten.choices().len(), 10);
        assert_eq!(DisplayScale::TenHalf.choices().len(), 20);
    }

    #[test]
    fn the_threshold_is_stored_in_the_units_it_was_typed_in() {
        // Storing the derived fraction instead would make "4 stars" impossible
        // to show back to somebody who then switched to a ten-star display.
        let prefs = serde_json::json!({ "rating_scale": "5", "rating_love_threshold": 4.0 });
        assert!((LoveRule::from_prefs(&prefs).at_least - 0.8).abs() < 0.001);

        let prefs = serde_json::json!({ "rating_scale": "10", "rating_love_threshold": 4.0 });
        assert!(
            (LoveRule::from_prefs(&prefs).at_least - 0.4).abs() < 0.001,
            "four out of ten is not four out of five"
        );
    }

    #[test]
    fn roon_has_no_ratings_and_says_so() {
        // Checked against the roon-api crate's own types and the vendored
        // node-roon-api reference: no rating, favourite, star or thumb field
        // exists in the extension API. A source that cannot answer must report
        // that rather than returning nothing and looking broken.
        assert_eq!(scale_for_source("roon"), None);
        assert_eq!(scale_for_source("plex"), Some(SourceScale::PLEX));
        assert_eq!(scale_for_source("navidrome"), Some(SourceScale::SUBSONIC));
        assert_eq!(scale_for_source("jellyfin"), Some(SourceScale::MEDIABROWSER));
        assert_eq!(scale_for_source("emby"), Some(SourceScale::MEDIABROWSER));
    }
}
