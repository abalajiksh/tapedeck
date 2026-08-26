//! The palette the image is drawn in.
//!
//! A server-side render necessarily holds its own copy of the tokens — there is
//! no CSS and no `var()` to resolve, so the hex has to be somewhere. This is
//! that somewhere, and it is the *only* place: the rule against hardcoded hex
//! is about scattering them through components, and one named palette beside
//! the renderer is what keeps that rule true rather than breaking it.
//!
//! Values are lifted from `web/src/app.css`'s `:root` and
//! `:root[data-theme="dark"]` blocks. Both are carried because the export asks
//! which one to use — a share image lands on somebody else's feed rather than
//! inside the app, so it is a choice at export time and not an inherited
//! setting.

/// Which look the image is rendered in. Not derived from a user preference:
/// the export asks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    /// Parsed from the query string. An unknown or absent value is **dark**
    /// rather than an error: this is presentation, and a share that refuses to
    /// render over a misspelling is worse than one that renders in the default.
    pub fn parse(s: Option<&str>) -> Theme {
        match s.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("light") => Theme::Light,
            _ => Theme::Dark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Theme::Dark => Palette {
                bg: "#17120e",
                surface: "#221b15",
                track: "#3a2f24",
                text: "#f3e9d6",
                muted: "#ab9c85",
                muted_3: "#8a7d68",
                // `--td-border` is an alpha colour in CSS. SVG carries the
                // opacity separately, so the pair travels together.
                border: "#f4e7cf",
                border_opacity: 0.12,
                accent: "#c67139",
            },
            Theme::Light => Palette {
                bg: "#f5ead8",
                surface: "#fbf5ea",
                track: "#e4d3b3",
                text: "#201e1d",
                muted: "#7a7060",
                muted_3: "#9a8f7d",
                border: "#201e1d",
                border_opacity: 0.08,
                accent: "#c67139",
            },
        }
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: &'static str,
    pub surface: &'static str,
    pub track: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub muted_3: &'static str,
    pub border: &'static str,
    pub border_opacity: f32,
    pub accent: &'static str,
}

/// The period's own blended colour, if it is a colour we are willing to paint.
///
/// `colour` comes back from the report as a hex string computed by blending the
/// period's genres in linear light — it is the most personal thing on the card,
/// and it is also a **value from the database being written straight into an
/// SVG attribute**. Anything that is not exactly `#rrggbb` is refused and the
/// accent stands in, so a malformed row cannot inject markup into the document.
pub fn period_colour(raw: Option<&str>, p: &Palette) -> String {
    let ok = raw
        .map(str::trim)
        .filter(|s| s.len() == 7 && s.starts_with('#'))
        .filter(|s| s[1..].chars().all(|c| c.is_ascii_hexdigit()));
    ok.map(str::to_string).unwrap_or_else(|| p.accent.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_theme_is_the_default_rather_than_an_error() {
        assert_eq!(Theme::parse(Some("light")), Theme::Light);
        assert_eq!(Theme::parse(Some("LIGHT")), Theme::Light);
        assert_eq!(Theme::parse(Some("chartreuse")), Theme::Dark);
        assert_eq!(Theme::parse(None), Theme::Dark);
    }

    /// The period colour is data, and it lands in an SVG attribute.
    ///
    /// Only `#rrggbb` is painted. Everything else falls back to the accent —
    /// including the shapes somebody would reach for to close the attribute and
    /// write their own markup.
    #[test]
    fn only_a_real_hex_colour_reaches_the_document() {
        let p = Theme::Dark.palette();
        assert_eq!(period_colour(Some("#a18430"), &p), "#a18430");
        assert_eq!(period_colour(Some("  #A18430  "), &p), "#A18430");
        for bad in ["red", "#a1843", "#gggggg", "\" onload=\"x", "#a18430;", ""] {
            assert_eq!(period_colour(Some(bad), &p), p.accent, "{bad} must not be painted");
        }
        assert_eq!(period_colour(None, &p), p.accent);
    }
}
