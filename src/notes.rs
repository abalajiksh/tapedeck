//! Liner notes: Markdown with shortcodes that point at other music.
//!
//! Rendering happens **server-side**. A note is stored as the Markdown the user
//! typed and turned into HTML on read, so sanitisation lives in exactly one
//! place instead of in every client that ever shows a note, and shortcode
//! resolution — which needs the database — happens in the same pass.
//!
//! ## Shortcodes
//!
//! `[[track:Xtal]]`, `[[album:Selected Ambient Works 85-92]]`,
//! `[[artist:Aphex Twin]]`, and the positional `[[track:3]]` meaning "track 3 of
//! the record this note is about". Square brackets because they collide with
//! nothing in Markdown — `[text](url)` needs the parenthesised half to be a
//! link, so a bare `[[…]]` is inert.
//!
//! **An unresolved shortcode is never an error.** It renders as the target text
//! with a dotted underline. Most of any library is un-enriched and half of what
//! is worth annotating was never in MusicBrainz to begin with; a note that
//! refuses to render because a reference didn't match would fail on exactly the
//! records this feature exists for.
//!
//! This module is deliberately pure: it finds shortcodes and renders Markdown,
//! and the caller supplies the resolutions. Nothing here touches the database.

use std::collections::HashMap;
use std::ops::Range;

/// What a shortcode points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Track,
    Album,
    Artist,
}

impl RefKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "track" | "recording" | "song" => Some(Self::Track),
            "album" | "release" => Some(Self::Album),
            "artist" => Some(Self::Artist),
            _ => None,
        }
    }

    /// The `entities.kind` this maps to.
    pub fn entity_kind(&self) -> &'static str {
        match self {
            Self::Track => "recording",
            Self::Album => "release",
            Self::Artist => "artist",
        }
    }
}

/// One `[[kind:target]]` occurrence in the source text.
#[derive(Debug, Clone)]
pub struct Shortcode {
    pub kind: RefKind,
    /// The text after the colon, trimmed. `"3"` for a positional track ref.
    pub target: String,
    span: Range<usize>,
}

impl Shortcode {
    /// `[[track:3]]` — a position on the record being annotated rather than a
    /// name. Only meaningful for tracks.
    pub fn position(&self) -> Option<i64> {
        if self.kind != RefKind::Track {
            return None;
        }
        self.target.parse::<i64>().ok().filter(|n| *n > 0)
    }

    /// Key for the resolution map handed back to [`render`].
    pub fn key(&self) -> (RefKind, String) {
        (self.kind, self.target.to_lowercase())
    }
}

/// Where a resolved shortcode points, and what to call it.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub entity_id: i64,
    /// Display text — for a positional ref this is the track's actual title,
    /// which is the whole point of writing `[[track:3]]`.
    pub label: String,
}

/// Every shortcode in the text, in order of appearance.
///
/// Unclosed or unknown-kind brackets are skipped rather than reported: they're
/// almost always someone writing about `[[` for its own sake, and a note is
/// prose first.
pub fn scan(src: &str) -> Vec<Shortcode> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] != b'[' || bytes[i + 1] != b'[' {
            i += 1;
            continue;
        }
        let Some(rel_end) = src[i + 2..].find("]]") else { break };
        let inner_end = i + 2 + rel_end;
        let inner = &src[i + 2..inner_end];

        // A newline inside means the brackets were never a shortcode — don't
        // let one stray `[[` swallow a paragraph.
        if let Some((kind, target)) = inner.split_once(':') {
            if !inner.contains('\n') {
                if let Some(kind) = RefKind::parse(kind) {
                    let target = target.trim();
                    if !target.is_empty() {
                        out.push(Shortcode {
                            kind,
                            target: target.to_string(),
                            span: i..inner_end + 2,
                        });
                    }
                }
            }
        }
        i = inner_end + 2;
    }
    out
}

/// Replace shortcodes with Markdown links (resolved) or an inline span
/// (unresolved), leaving the rest of the text untouched.
fn substitute(src: &str, resolved: &HashMap<(RefKind, String), Resolved>) -> String {
    let codes = scan(src);
    if codes.is_empty() {
        return src.to_string();
    }

    let mut out = String::with_capacity(src.len() + codes.len() * 24);
    let mut cursor = 0;
    for code in codes {
        out.push_str(&src[cursor..code.span.start]);
        match resolved.get(&code.key()) {
            Some(r) => {
                // Markdown link syntax, so the label still gets escaped by the
                // renderer rather than injected as HTML.
                out.push('[');
                out.push_str(&escape_link_text(&r.label));
                out.push_str("](/notes?entity=");
                out.push_str(&r.entity_id.to_string());
                out.push(')');
            }
            None => {
                out.push_str("<span class=\"td-unresolved\" title=\"No match yet — write a note or love it and this links up\">");
                out.push_str(&escape_html(&code.target));
                out.push_str("</span>");
            }
        }
        cursor = code.span.end;
    }
    out.push_str(&src[cursor..]);
    out
}

fn escape_link_text(s: &str) -> String {
    s.replace('\\', r"\\").replace('[', r"\[").replace(']', r"\]")
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Markdown → sanitised HTML, with shortcodes substituted first.
///
/// The output is safe to drop into the page with `{@html}`: ammonia strips
/// scripts, event handlers and anything else that isn't in its allow-list, and
/// this is the only place a note becomes HTML.
pub fn render(src: &str, resolved: &HashMap<(RefKind, String), Resolved>) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let substituted = substitute(src, resolved);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let mut unsafe_html = String::new();
    html::push_html(&mut unsafe_html, Parser::new_ext(&substituted, options));

    ammonia::Builder::default()
        // The dotted-underline marker for an unresolved reference is the one
        // class the renderer itself emits, so it has to survive sanitising.
        .add_allowed_classes("span", &["td-unresolved"])
        .link_rel(Some("noopener nofollow"))
        .clean(&unsafe_html)
        .to_string()
}

/// First line or so of a note, for list views. Shortcodes become their bare
/// target text — an excerpt is a label, not a place to click through from.
pub fn excerpt(src: &str, max_chars: usize) -> String {
    let mut flat = String::new();
    let codes = scan(src);
    let mut cursor = 0;
    for code in &codes {
        flat.push_str(&src[cursor..code.span.start]);
        flat.push_str(&code.target);
        cursor = code.span.end;
    }
    flat.push_str(&src[cursor..]);

    let flat = flat
        .lines()
        // Skip Markdown structure that reads as noise out of context.
        .map(|l| l.trim_start_matches(['#', '>', '-', '*', ' ']).trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head.to_string()).unwrap_or(cut);
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(pairs: &[(RefKind, &str, i64, &str)]) -> HashMap<(RefKind, String), Resolved> {
        pairs.iter()
            .map(|(k, t, id, label)| {
                ((*k, t.to_lowercase()), Resolved { entity_id: *id, label: label.to_string() })
            })
            .collect()
    }

    #[test]
    fn finds_each_shortcode_kind() {
        let codes = scan("[[track:Xtal]] from [[album:SAW 85-92]] by [[artist:Aphex Twin]]");
        let kinds: Vec<_> = codes.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![RefKind::Track, RefKind::Album, RefKind::Artist]);
        assert_eq!(codes[1].target, "SAW 85-92");
    }

    #[test]
    fn positional_track_refs_are_recognised() {
        let codes = scan("side two opens with [[track:3]]");
        assert_eq!(codes[0].position(), Some(3));
        // A named track is not a position, and neither is a nonsense one.
        assert_eq!(scan("[[track:Xtal]]")[0].position(), None);
        assert_eq!(scan("[[track:0]]")[0].position(), None);
        assert_eq!(scan("[[album:3]]")[0].position(), None);
    }

    #[test]
    fn prose_that_merely_contains_brackets_is_left_alone() {
        assert!(scan("the sleeve reads [[ nothing ]]").is_empty());
        assert!(scan("unclosed [[track:Xtal").is_empty());
        assert!(scan("[[unknownkind:thing]]").is_empty());
        // A stray opener must not swallow the rest of the note.
        assert!(scan("[[\nstill writing").is_empty());
    }

    #[test]
    fn resolved_refs_become_links_and_unresolved_ones_stay_readable() {
        let html = render(
            "Best on [[track:Xtal]], worse on [[track:Ageispolis]].",
            &resolved(&[(RefKind::Track, "xtal", 7, "Xtal")]),
        );
        assert!(html.contains(r#"href="/notes?entity=7""#), "{html}");
        assert!(html.contains(">Xtal</a>"), "{html}");
        // The unresolved one is still readable prose, not an error or a link.
        assert!(html.contains("td-unresolved"), "{html}");
        assert!(html.contains("Ageispolis"), "{html}");
        assert!(!html.contains(r#"href="/notes?entity=0""#), "{html}");
    }

    #[test]
    fn positional_ref_renders_the_real_title() {
        // Resolution keys on the raw target ("3"), but the label is the title —
        // that substitution is the entire reason to write [[track:3]].
        let html = render(
            "[[track:3]] is the one",
            &resolved(&[(RefKind::Track, "3", 12, "Ageispolis")]),
        );
        assert!(html.contains(">Ageispolis</a>"), "{html}");
        assert!(!html.contains(">3</a>"), "{html}");
    }

    #[test]
    fn markdown_renders_but_scripts_do_not_survive() {
        let html = render(
            "# Heading\n\n*emphasis* and <script>alert(1)</script> and \
             <img src=x onerror=alert(1)>",
            &HashMap::new(),
        );
        assert!(html.contains("<h1>Heading</h1>"), "{html}");
        assert!(html.contains("<em>emphasis</em>"), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("onerror"), "{html}");
    }

    #[test]
    fn a_label_with_brackets_cannot_break_out_of_its_link() {
        let html = render(
            "[[track:weird]]",
            &resolved(&[(RefKind::Track, "weird", 3, "Track [with] brackets")]),
        );
        assert!(html.contains("Track [with] brackets</a>"), "{html}");
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
    }

    #[test]
    fn excerpts_flatten_markup_and_shortcodes() {
        let e = excerpt("## On repeat\n\nThe best is [[track:Xtal]], really", 80);
        assert_eq!(e, "On repeat The best is Xtal, really");
        assert!(excerpt(&"long ".repeat(60), 40).ends_with('…'));
    }
}
