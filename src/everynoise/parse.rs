//! Parsing for Every Noise at Once pages.
//!
//! Pure functions over HTML text — no network, no database — so the shapes can
//! be pinned by tests against real captured bytes.
//!
//! The *data is the CSS*, which is unusual enough to state plainly. A real
//! entry from `engenremap.html`:
//!
//! ```text
//! <div id=item1 preview_url="https://p.scdn.co/mp3-preview/17fa16…"
//!      class="genre scanme" scan=true
//!      style="color: #ad8907; top: 4997px; left: 783px; font-size: 160%"
//!      role=button tabindex=0 onKeyDown="kb(event);"
//!      onclick="playx(&quot;1V6gIisPpYqgFeWbMLI0bA&quot;, &quot;pop&quot;, this);"
//!      title="e.g. Demi Lovato &quot;Heart Attack&quot;">pop<a class=navlink
//!      href="engenremap-pop.html" …>&raquo;</a> </div>
//! ```
//!
//! `left`/`top` are the two axes of the manifold, `color`'s three channels
//! encode audio character, and `font-size` is prominence — six numbers per
//! genre with no audio analysis of our own.
//!
//! **Four things about this markup that a reasonable parser gets wrong**, all
//! found by fetching the live page rather than trusting a description of it:
//!
//! * **Attribute values are often unquoted** — `id=item1`, `class=navlink`,
//!   `scan=true`, `tabindex=0`. Matching `id="item` finds nothing at all.
//! * **`onclick` quotes its arguments as `&quot;`**, so the preview track id is
//!   not delimited by a quote character.
//! * **Artist profiles are `artistprofile.html?id=…`, not `.cgi`.** The
//!   reference scraper in `temp/` matches `.cgi` and therefore silently
//!   collects no Spotify ids whatsoever.
//! * **Names carry HTML entities** — 33 genres are spelled `r&amp;b` and
//!   friends. Storing that literally means every "r&b" listen fails to join.
//!
//! **Hand-rolled rather than pulling in an HTML parser or `regex`.** Every
//! Noise has been frozen since December 2023, so the markup cannot drift out
//! from under us — which removes the usual reason to want a real parser.
//! `regex` is only a build-dependency in this tree today (via
//! `chrono-tz-build`), so using it here would be a new runtime dependency for
//! one frozen page shape.

/// A genre as placed on the main map.
#[derive(Debug, Clone, PartialEq)]
pub struct Genre {
    pub name: String,
    pub slug: String,
    /// Horizontal axis — dense/atmospheric ↔ spiky/bouncy.
    pub x: f64,
    /// Vertical axis — organic ↔ mechanical/electric.
    pub y: f64,
    /// `#rrggbb`; the channels encode audio character.
    pub color: String,
    /// Percent. Prominence, not a typographic choice.
    pub font_size: f64,
    pub preview_track_id: Option<String>,
    pub example_title: Option<String>,
}

/// An artist as placed on a genre's page.
#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    pub name: String,
    /// Every Noise keys artists by Spotify id. Tapedeck has no Spotify
    /// integration, so this is stored for later rather than joined on.
    pub spotify_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub color: String,
    pub font_size: f64,
}

/// A neighbouring genre, in one of the two directions the site precomputes.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbour {
    pub slug: String,
    pub name: String,
    pub relation: Relation,
    /// Rank within the list, which is the ordering the site presents.
    pub position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The ~20 most similar genres.
    Nearby,
    /// The ~20 maximally opposite ones. Worth keeping: "furthest from" is a
    /// signal you cannot recover from 2D coordinates alone.
    Opposite,
}

impl Relation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Relation::Nearby => "nearby",
            Relation::Opposite => "opposite",
        }
    }
}

#[derive(Debug, Default)]
pub struct GenrePage {
    pub artists: Vec<Artist>,
    pub neighbours: Vec<Neighbour>,
}

// ── Scanning helpers ──

/// The value of `attr=…` in one tag's text, quoted **or bare**.
///
/// Bare values are the norm on this site (`id=item1`, `class=navlink`), which
/// is the single detail that stops a quotes-only parser from finding anything.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(rel) = tag[from..].find(name) {
        let at = from + rel;
        // Must start a word, or `class` would also match inside `onclick`.
        if at > 0 && !tag.as_bytes()[at - 1].is_ascii_whitespace() {
            from = at + name.len();
            continue;
        }
        let after = &tag[at + name.len()..];
        let Some(rest) = after.trim_start().strip_prefix('=') else {
            from = at + name.len();
            continue;
        };
        let rest = rest.trim_start();
        return match rest.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let body = &rest[q.len_utf8()..];
                body.find(q).map(|end| &body[..end])
            }
            Some(_) => {
                let end =
                    rest.find(|c: char| c.is_ascii_whitespace() || c == '>').unwrap_or(rest.len());
                Some(&rest[..end])
            }
            None => None,
        };
    }
    None
}

/// A numeric CSS property: `css("top: 4997px", "top")` → `4997.0`.
fn css(style: &str, prop: &str) -> Option<f64> {
    let mut from = 0;
    while let Some(rel) = style[from..].find(prop) {
        let at = from + rel;
        // `font-size` contains `size`, and `left` must not match inside some
        // other property name either.
        let starts_word = at == 0 || !style.as_bytes()[at - 1].is_ascii_alphanumeric();
        if !starts_word {
            from = at + prop.len();
            continue;
        }
        let Some(rest) = style[at + prop.len()..].trim_start().strip_prefix(':') else {
            from = at + prop.len();
            continue;
        };
        let rest = rest.trim_start();
        let end =
            rest.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-').unwrap_or(rest.len());
        return rest[..end].parse().ok();
    }
    None
}

fn css_color(style: &str) -> String {
    let Some(at) = style.find('#') else { return String::new() };
    let hex: String = style[at..].chars().take(7).collect();
    if hex.len() == 7 && hex[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        hex
    } else {
        String::new()
    }
}

/// Decode the handful of entities this site actually emits.
///
/// `&amp;` matters most: 33 genres are spelled `r&amp;b`, and storing that
/// literally means no "r&b" tag ever joins to a position.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&raquo;", "")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        // Last, or an already-decoded `&amp;lt;` would double-decode.
        .replace("&amp;", "&")
}

/// The first `playx(…)` argument — the preview track's Spotify id.
///
/// The delimiter is `&quot;` in the live markup rather than a quote character.
fn playx_track(tag: &str) -> Option<String> {
    let at = tag.find("playx(")?;
    let rest = tag[at + "playx(".len()..].trim_start();
    let (delim, body): (&str, &str) = if let Some(b) = rest.strip_prefix("&quot;") {
        ("&quot;", b)
    } else if let Some(b) = rest.strip_prefix('"') {
        ("\"", b)
    } else if let Some(b) = rest.strip_prefix('\'') {
        ("'", b)
    } else {
        return None;
    };
    let end = body.find(delim)?;
    let id = &body[..end];
    (!id.is_empty()).then(|| id.to_string())
}

/// The text directly inside the div, ignoring the nested navlink anchor.
fn text_of(block: &str) -> String {
    let Some(gt) = block.find('>') else { return String::new() };
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in block[gt + 1..].chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    decode_entities(out.trim())
}

/// The `href` of the nested `<a class=navlink>`.
fn navlink_href(block: &str) -> Option<&str> {
    let at = block.find("navlink")?;
    let tag_start = block[..at].rfind('<')?;
    let tag_end = block[tag_start..].find('>')? + tag_start;
    attr(&block[tag_start..tag_end], "href")
}

/// `…engenremap-<slug>.html…` → `<slug>`. Tolerates the `#tunnel` suffix the
/// neighbour links carry.
fn genre_slug(href: &str) -> Option<&str> {
    let at = href.find("engenremap-")?;
    let rest = &href[at + "engenremap-".len()..];
    rest.find(".html").map(|end| &rest[..end])
}

/// `artistprofile.html?id=<id>` → `<id>`.
///
/// The live site serves `.html`; the reference scraper looks for `.cgi` and so
/// finds nothing. Both are accepted here rather than betting on either.
fn artist_spotify_id(href: &str) -> Option<String> {
    let at = href.find("artistprofile")?;
    let rest = &href[at..];
    let q = rest.find("id=")?;
    let rest = &rest[q + "id=".len()..];
    let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_string())
}

/// A `<div …>` opening tag's text, for attribute lookups.
fn open_tag(block: &str) -> &str {
    match block.find('>') {
        Some(end) => &block[..end],
        None => block,
    }
}

/// Every `<div …>…</div>` whose `id` is `prefix` followed by digits.
///
/// These items are flat — the only child is the navlink anchor — so scanning to
/// the next `</div>` is exact rather than approximate. That is the property
/// that makes hand-rolling safe here.
fn blocks<'a>(html: &'a str, prefix: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = html[from..].find("<div") {
        let start = from + rel;
        from = start + 4;
        let Some(tag_end) = html[start..].find('>').map(|e| e + start) else { break };
        let Some(id) = attr(&html[start..tag_end], "id") else { continue };
        // `item` must not also match `nearbyitem` or `mirroritem`: those are
        // different regions with different meanings, and folding them together
        // would file neighbouring genres as if they were artists.
        let Some(n) = id.strip_prefix(prefix) else { continue };
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Some(end) = html[start..].find("</div>") {
            out.push(&html[start..start + end]);
            from = start + end;
        }
    }
    out
}

fn has_class(tag: &str, want: &str) -> bool {
    attr(tag, "class").is_some_and(|c| c.split_whitespace().any(|w| w == want))
}

// ── Public parsers ──

/// Parse `engenremap.html` — the entire manifold in one page.
pub fn genre_map(html: &str) -> Vec<Genre> {
    let mut out = Vec::new();
    for block in blocks(html, "item") {
        let tag = open_tag(block);
        if !has_class(tag, "genre") {
            continue;
        }
        let name = text_of(block);
        if name.is_empty() {
            continue;
        }
        let style = attr(tag, "style").unwrap_or("");
        let slug = navlink_href(block)
            .and_then(genre_slug)
            .map(str::to_string)
            .unwrap_or_else(|| slugify(&name));

        out.push(Genre {
            name,
            slug,
            x: css(style, "left").unwrap_or(0.0),
            y: css(style, "top").unwrap_or(0.0),
            color: css_color(style),
            font_size: css(style, "font-size").unwrap_or(100.0),
            preview_track_id: playx_track(tag),
            example_title: attr(tag, "title").map(decode_entities),
        });
    }
    out
}

/// Parse `engenremap-<slug>.html` — the artists placed in one genre, plus the
/// precomputed neighbour graph.
pub fn genre_page(html: &str, slug: &str) -> GenrePage {
    let mut page = GenrePage::default();

    for block in blocks(html, "item") {
        let tag = open_tag(block);
        if !has_class(tag, "genre") {
            continue;
        }
        let name = text_of(block);
        if name.is_empty() {
            continue;
        }
        let style = attr(tag, "style").unwrap_or("");
        page.artists.push(Artist {
            name,
            spotify_id: navlink_href(block).and_then(artist_spotify_id),
            x: css(style, "left").unwrap_or(0.0),
            y: css(style, "top").unwrap_or(0.0),
            color: css_color(style),
            font_size: css(style, "font-size").unwrap_or(100.0),
        });
    }

    for (prefix, relation) in [("nearbyitem", Relation::Nearby), ("mirroritem", Relation::Opposite)]
    {
        let mut position = 0i64;
        for block in blocks(html, prefix) {
            let Some(href) = navlink_href(block) else { continue };
            // The first nearby entry is the genre itself, linking to its own
            // one-dimensional view. Keeping it gives every genre a self-edge.
            if href.contains("everynoise1d-") {
                continue;
            }
            let Some(found) = genre_slug(href) else { continue };
            if found == slug {
                continue;
            }
            page.neighbours.push(Neighbour {
                slug: found.to_string(),
                name: text_of(block),
                relation,
                position,
            });
            position += 1;
        }
    }

    page
}

/// Fallback when a genre has no navlink to take a slug from.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every string below is **captured verbatim from the live site**
    // (2026-08-05), not reconstructed from a description of it. The first
    // version of this parser was written against the documented 2023 snapshot
    // in `temp/everynoise-scraper` and matched nothing at all: ids are bare,
    // `onclick` quotes with `&quot;`, and artist links are `.html` not `.cgi`.

    const MAP_ITEM: &str = r#"<div id=item1 preview_url="https://p.scdn.co/mp3-preview/17fa164b977074dd1b1869bfb10b08cfd513e53b" class="genre scanme" scan=true style="color: #ad8907; top: 4997px; left: 783px; font-size: 160%" role=button tabindex=0 onKeyDown="kb(event);" onclick="playx(&quot;1V6gIisPpYqgFeWbMLI0bA&quot;, &quot;pop&quot;, this);" title="e.g. Demi Lovato &quot;Heart Attack&quot;">pop<a class=navlink href="engenremap-pop.html" role=button tabindex=0 onKeyDown="kb(event);" onclick="event.stopPropagation();" >&raquo;</a> </div>"#;

    const ART_ITEM: &str = r#"<div id=item1 preview_url="https://p.scdn.co/mp3-preview/783fbf" class="genre scanme" scan=true style="color: #a87d0d; top: 243px; left: 771px; font-size: 160%" role=button tabindex=0 onKeyDown="kb(event);" onclick="playx(&quot;2LlQb7Uoj1kKyGhlkBf9aC&quot;, &quot;Michael Jackson&quot;, this);" title="e.g. Michael Jackson &quot;Thriller&quot;">Michael Jackson<a class=navlink href="artistprofile.html?id=3fMbdgg4jU18AjLCKBhRSm" role=button tabindex=0 onKeyDown="kb(event);" onclick="event.stopPropagation();" target=artistprofile>&raquo;</a> </div>"#;

    const NEAR1: &str = r#"<div id=nearbyitem1 class="genre" scan=true style="color: #698716; top: 262px; left: 111px; font-size: 240%" onclick="playx(&quot;2cK7SJ2O3CqDRHeixvFKgh&quot;, &quot;soul&quot;, this);" title="e.g. Curtis Mayfield &quot;Move On Up&quot;">soul<a class=navlink href="everynoise1d-soul.html" role=button >&raquo;</a> </div>"#;

    const NEAR2: &str = r#"<div id=nearbyitem2 class="genre" scan=true style="color: #9d7713; top: 109px; left: 195px; font-size: 127%" onclick="playx(&quot;2UCgWpa2r4ytWueNItYbWE&quot;, &quot;funk&quot;, this);" title="e.g. Dazz Band &quot;Let It Whip&quot;">funk<a class=navlink href="engenremap-funk.html#tunnel" role=button >&raquo;</a> </div>"#;

    const MIRROR1: &str = r#"<div id=mirroritem1 class="genre" scan=true style="color: #debdb0; top: 199px; left: 106px; font-size: 160%" onclick="playx(&quot;4PzCufqO0vE0nLPVcX1Jcc&quot;, &quot;post-punk colombiano&quot;, this);" title="e.g. Hipsum &quot;Sue&#241;os&quot;">post-punk colombiano<a class=navlink href="engenremap-postpunkcolombiano.html#tunnel" role=button >&raquo;</a> </div>"#;

    /// The single most important test here: the previous parser looked for
    /// `id="item` and this markup writes `id=item1`, so it found nothing on a
    /// page holding 6,291 genres — a 200 response, a clean parse, zero rows.
    #[test]
    fn bare_unquoted_attributes_are_read() {
        let g = genre_map(MAP_ITEM);
        assert_eq!(g.len(), 1, "bare `id=item1` must be found");
        assert_eq!(g[0].name, "pop");
        assert_eq!(g[0].slug, "pop");
        // `left` is x and `top` is y — swapping them mirrors the whole map,
        // which would look plausible and be wrong.
        assert_eq!(g[0].x, 783.0);
        assert_eq!(g[0].y, 4997.0);
        assert_eq!(g[0].color, "#ad8907");
        assert_eq!(g[0].font_size, 160.0);
    }

    /// `onclick` delimits its arguments with `&quot;`, not a quote character.
    #[test]
    fn the_preview_track_survives_entity_quoting() {
        let g = genre_map(MAP_ITEM);
        assert_eq!(g[0].preview_track_id.as_deref(), Some("1V6gIisPpYqgFeWbMLI0bA"));
        assert_eq!(g[0].example_title.as_deref(), Some(r#"e.g. Demi Lovato "Heart Attack""#));
    }

    /// 33 genres are spelled `r&amp;b`. Stored literally, no "r&b" tag from
    /// MusicBrainz would ever join to a position — and nothing would error.
    #[test]
    fn entity_encoded_names_are_decoded() {
        let html = MAP_ITEM.replace(">pop<", ">r&amp;b<");
        let g = genre_map(&html);
        assert_eq!(g[0].name, "r&b");
    }

    /// The live site serves `artistprofile.html?id=…`. The reference scraper
    /// matches `.cgi` and therefore collects no Spotify ids at all.
    #[test]
    fn artist_links_are_html_not_cgi() {
        let page = genre_page(ART_ITEM, "soul");
        assert_eq!(page.artists.len(), 1);
        assert_eq!(page.artists[0].name, "Michael Jackson");
        assert_eq!(page.artists[0].spotify_id.as_deref(), Some("3fMbdgg4jU18AjLCKBhRSm"));
    }

    #[test]
    fn the_self_referential_first_neighbour_is_dropped() {
        let html = format!("{ART_ITEM}{NEAR1}{NEAR2}{MIRROR1}");
        let page = genre_page(&html, "soul");
        let nearby: Vec<_> =
            page.neighbours.iter().filter(|n| n.relation == Relation::Nearby).collect();
        assert_eq!(
            nearby.len(),
            1,
            "nearbyitem1 links to everynoise1d-soul and is the genre itself"
        );
        assert_eq!(nearby[0].slug, "funk");
        assert!(page.neighbours.iter().all(|n| n.slug != "soul"));
    }

    /// "Furthest from" is not recoverable from 2D coordinates, so the mirror
    /// region is kept as its own relation.
    #[test]
    fn opposite_genres_are_kept_apart_from_nearby_ones() {
        let html = format!("{ART_ITEM}{NEAR1}{NEAR2}{MIRROR1}");
        let page = genre_page(&html, "soul");
        let opp: Vec<_> =
            page.neighbours.iter().filter(|n| n.relation == Relation::Opposite).collect();
        assert_eq!(opp.len(), 1);
        assert_eq!(opp[0].slug, "postpunkcolombiano");
        assert_eq!(opp[0].name, "post-punk colombiano");
    }

    /// `id=item…` must not also match `id=nearbyitem…` / `id=mirroritem…`, or
    /// the neighbour panels' genres get filed as artists in this genre — a
    /// silent, entirely plausible-looking corruption.
    #[test]
    fn neighbour_panels_are_not_mistaken_for_artists() {
        let html = format!("{ART_ITEM}{NEAR1}{NEAR2}{MIRROR1}");
        let page = genre_page(&html, "soul");
        assert_eq!(page.artists.len(), 1, "only the real artist item is an artist");
        assert_eq!(page.artists[0].name, "Michael Jackson");
    }

    #[test]
    fn a_page_that_is_not_what_we_expected_yields_nothing_rather_than_guesses() {
        assert!(genre_map("<html><body><p>404 Not Found</p></body></html>").is_empty());
        let page = genre_page("", "soul");
        assert!(page.artists.is_empty() && page.neighbours.is_empty());
    }

    /// `font-size` contains `size`, and a naive substring search for `top`
    /// would also hit it inside other words. Fractional values are real here.
    #[test]
    fn css_properties_match_on_whole_names() {
        let style = "color: #abcdef; top: 120.5px; left: 42px; font-size: 92%";
        assert_eq!(css(style, "top"), Some(120.5));
        assert_eq!(css(style, "left"), Some(42.0));
        assert_eq!(css(style, "font-size"), Some(92.0));
    }
}
