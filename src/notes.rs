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
    /// A piece of the writer's own equipment. Not an entity — gear lives in its
    /// own table and is per-user, so it resolves and links differently. "Heard
    /// this on the [[gear:Bifrost 2]] for the first time" is the point.
    Gear,
    /// A numbered line of a lyric — `[[line:12]]` or `[[line:A:3:12]]`.
    ///
    /// **Stores the index, never the words**, so correcting a fetched lyric
    /// later corrects every quote with it rather than orphaning them. That is
    /// the first of the four rules the design states, and it is the same call
    /// this module already makes for entities.
    ///
    /// Two forms, and the short one exists for the reason `[[track:3]]` does:
    /// a reference is read *against the thing the note is about*. Inside a note
    /// on a track, `[[line:12]]` is line 12 of that track. Inside a note on a
    /// record, the coordinate needs the side and the track too, which is what
    /// the design's `A:3:12` is.
    ///
    /// Not an entity, and never creates one — the same refusal `Gear` makes.
    /// A line is a position in somebody else's text; there is nothing to
    /// conjure a row for, and it cannot be a love target. It *can* be a note
    /// target — that is a pin — but through `NoteTarget::Line` rather than
    /// through anything here.
    Line,
    /// The same coordinate, asked for as a **block** — `[[quote:12]]`.
    ///
    /// The design's second rendering, "for when the note is about the line":
    /// the words set as a pull-quote with their coordinate under them and a way
    /// into the reader, rather than a phrase inside a sentence.
    ///
    /// A separate kind rather than a modifier on `line` because the choice is
    /// about *how the line sits in the sentence*, which is a thing the writer
    /// decides per reference — and a distinct word is easier to read back in
    /// six months than a punctuation flag. The stored coordinate is identical,
    /// so switching between them is a one-word edit and nothing is re-resolved.
    LineBlock,
}

impl RefKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "track" | "recording" | "song" => Some(Self::Track),
            "album" | "release" => Some(Self::Album),
            "artist" => Some(Self::Artist),
            "gear" | "equipment" => Some(Self::Gear),
            "line" | "lyric" => Some(Self::Line),
            "quote" | "blockline" => Some(Self::LineBlock),
            _ => None,
        }
    }

    /// The `entities.kind` this maps to, for the three that are entities.
    pub fn entity_kind(&self) -> Option<&'static str> {
        match self {
            Self::Track => Some("recording"),
            Self::Album => Some("release"),
            Self::Artist => Some("artist"),
            Self::Gear => None,
            Self::Line | Self::LineBlock => None,
        }
    }

    /// Whether this kind is a coordinate into a lyric, in either rendering.
    pub fn is_line(&self) -> bool {
        matches!(self, Self::Line | Self::LineBlock)
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

/// How a resolved reference is set into the prose.
///
/// The design's three renderings, minus the one that is not a shortcode at all:
/// a **pin** is a note whose target *is* the line, so it is a `NoteTarget`
/// rather than something written into a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Render {
    /// A plain link. Every entity and every piece of gear.
    Link,
    /// The default for a line: the words in quotation marks, followed by a
    /// small chip that opens the reader at that line.
    LineInline {
        /// What the chip says — `12`, or `A3·12` for a coordinate on a record.
        chip: String,
    },
    /// The line as a pull-quote, for when the note is *about* it.
    LineBlock {
        /// The line under the quote — "line 12 · 1:04".
        meta: String,
    },
}

/// Where a resolved shortcode points, and what to call it.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Display text — for a positional ref this is the track's actual title,
    /// which is the whole point of writing `[[track:3]]`.
    pub label: String,
    /// The URL to link to. Supplied by the caller rather than built here: gear
    /// and entities live on different routes, and this module stays free of
    /// any knowledge of how the app is laid out.
    pub href: String,
    /// Which of the three shapes to draw. Defaulted to [`Render::Link`] by
    /// [`Resolved::link`], which is what every non-line caller wants.
    pub render: Render,
}

impl Resolved {
    /// A plain link — an entity or a piece of gear.
    pub fn link(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self { label: label.into(), href: href.into(), render: Render::Link }
    }
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
        let Some(rel_end) = src[i + 2..].find("]]") else {
            break;
        };
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
            Some(r) => match &r.render {
                Render::Link => {
                    // Markdown link syntax, so the label still gets escaped by
                    // the renderer rather than injected as HTML.
                    out.push('[');
                    out.push_str(&escape_link_text(&r.label));
                    out.push_str("](");
                    out.push_str(&escape_link_dest(&r.href));
                    out.push(')');
                }
                // The two line forms are HTML rather than Markdown, because a
                // link cannot carry a chip beside it or a caption under it.
                // Everything interpolated is escaped here, and ammonia then
                // sanitises the whole document anyway — the classes below are
                // the ones `render` allow-lists, and a form that emitted any
                // other would silently lose it.
                Render::LineInline { chip } => {
                    out.push_str("<span class=\"td-line\">");
                    out.push_str("<span class=\"td-line-words\">\u{201c}");
                    out.push_str(&escape_html(&r.label));
                    out.push_str("\u{201d}</span>");
                    out.push_str("<a class=\"td-line-chip\" href=\"");
                    out.push_str(&escape_attr(&r.href));
                    out.push_str("\">");
                    out.push_str(&escape_html(chip));
                    out.push_str("</a></span>");
                }
                Render::LineBlock { meta } => {
                    out.push_str("<figure class=\"td-line-block\"><blockquote>");
                    out.push_str(&escape_html(&r.label));
                    out.push_str("</blockquote><figcaption>");
                    out.push_str(&escape_html(meta));
                    out.push_str(" <a class=\"td-line-chip\" href=\"");
                    out.push_str(&escape_attr(&r.href));
                    out.push_str("\">Reader</a></figcaption></figure>");
                }
            },
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

/// Hrefs are built by the caller, but a stray space or paren would end the
/// Markdown destination early and spill the rest into the document. Angle
/// brackets are Markdown's own escape for exactly this.
fn escape_link_dest(s: &str) -> String {
    if s.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", s.replace('<', "%3C").replace('>', "%3E"))
    } else {
        s.to_string()
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Same, plus the quote that would end an attribute early.
///
/// The href is built by the caller from a track name a stranger's tagger wrote,
/// so it is not trusted here even though ammonia sanitises afterwards — a
/// double quote in a title would otherwise break out of the attribute and the
/// only thing standing between that and a live tag would be the sanitiser.
fn escape_attr(s: &str) -> String {
    escape_html(s).replace('"', "&quot;")
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
        // The classes the renderer itself emits, which therefore have to
        // survive sanitising. Nothing else in a note carries a class: a body is
        // the user's Markdown, and ammonia strips every attribute it is not
        // told about — so a rendering that grows a new class has to be added
        // here or it is *silently* lost, which looks like a styling bug.
        .add_allowed_classes("span", &["td-unresolved", "td-line", "td-line-words"])
        .add_allowed_classes("a", &["td-line-chip"])
        .add_allowed_classes("figure", &["td-line-block"])
        .add_tags(&["figure", "figcaption"])
        .link_rel(Some("noopener nofollow"))
        .clean(&unsafe_html)
        .to_string()
}

/// The note as Markdown for a wiki — Obsidian, Logseq, anything reading a
/// folder of files.
///
/// The body is the user's own text and is left alone. The one thing that has to
/// change is the shortcodes, and the reason is a genuine collision: `[[…]]` is
/// Tapedeck's reference syntax *and* the wikilink syntax of every tool this
/// export is aimed at. Dropped in as they stand, `[[track:Xtal]]` becomes a link
/// to a page called "track:Xtal" in a vault that will never have one.
///
/// So a **resolved** shortcode becomes `[[Label]]` — a real wikilink, pointing
/// at the note file this export gives that entity, or at a page the writer can
/// go on to create. That is the whole reason to export to a wiki rather than
/// download a text file.
///
/// An **unresolved** one is left exactly as it was typed. It is what the writer
/// wrote, and the same judgement the HTML renderer makes: a reference that
/// matches nothing is not an error and must not be rewritten into something
/// else. It lands in the vault as a dangling link named after the shortcode,
/// which is honest about what it is.
pub fn export_markdown(src: &str, resolved: &HashMap<(RefKind, String), Resolved>) -> String {
    let codes = scan(src);
    if codes.is_empty() {
        return src.to_string();
    }

    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    for code in codes {
        out.push_str(&src[cursor..code.span.start]);
        // **A lyric line is never written out.** The words are the provider's
        // text and an export is a folder of files that leaves this instance, so
        // the reference is left exactly as typed — which is also what the
        // renderer does with anything it cannot resolve, and what the writer
        // wrote. Refused *here*, in the function that produces the file, rather
        // than only in the caller that builds the resolutions: a second caller
        // would otherwise have to know, and the cost of it not knowing is
        // somebody else's copyrighted text in a vault.
        match resolved.get(&code.key()).filter(|_| !code.kind.is_line()) {
            // Through `export_filename`, not the raw label: a wikilink resolves
            // by *filename*, so the two must be produced by the same rule or
            // every link in the export dangles. Deriving both from one function
            // is what makes that impossible to get wrong later.
            Some(r) => {
                out.push_str("[[");
                out.push_str(&export_filename(&r.label));
                out.push_str("]]");
            }
            None => out.push_str(&src[code.span.clone()]),
        }
        cursor = code.span.end;
    }
    out.push_str(&src[cursor..]);
    out
}

/// A note's filename inside the export, without extension.
///
/// Wikilinks resolve by filename, so this has to agree with the label
/// [`export_markdown`] writes — that is why both go through the same character
/// rules. Slashes especially: `AC/DC` would otherwise be a directory.
pub fn export_filename(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| match c {
            // Reserved on Windows or meaningful to a wikilink. Replaced rather
            // than dropped so "AC/DC" stays two syllables.
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '[' | ']' | '#' | '^' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    // Collapse the runs those substitutions leave behind.
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // Long enough for "Album — Artist", short of every filesystem's limit.
    let cut: String = cleaned.chars().take(120).collect();
    let cut = cut.trim_end_matches('.').trim().to_string();
    if cut.is_empty() {
        "Untitled".into()
    } else {
        cut
    }
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
        pairs
            .iter()
            .map(|(k, t, id, label)| {
                let href = match k {
                    RefKind::Gear => format!("/chains?gear={id}"),
                    _ => format!("/notes?entity={id}"),
                };
                ((*k, t.to_lowercase()), Resolved::link(*label, href))
            })
            .collect()
    }

    /// A line reference is a fifth kind, and it parses both forms.
    ///
    /// The short one is read against whatever the note is about — the rule
    /// `[[track:3]]` already follows — which is what makes a line reference
    /// usable for the majority of listening, none of which came off a record.
    #[test]
    fn a_line_reference_parses_in_both_of_its_forms() {
        let codes = scan("the turn is [[line:12]], and [[line:A:3:12]] is the one everyone quotes");
        assert_eq!(codes.len(), 2);
        assert!(codes.iter().all(|c| c.kind == RefKind::Line));
        assert_eq!(codes[0].target, "12");
        assert_eq!(codes[1].target, "A:3:12");
        assert_eq!(RefKind::parse("lyric"), Some(RefKind::Line), "spelled either way");
    }

    /// A line is a position in somebody else's text, so it is not an entity and
    /// never creates one — the same refusal gear makes, and what stops it being
    /// a note or a love target.
    #[test]
    fn a_line_is_not_an_entity_and_conjures_nothing() {
        assert_eq!(RefKind::Line.entity_kind(), None);
        assert_eq!(RefKind::Gear.entity_kind(), None);
        assert_eq!(RefKind::Track.entity_kind(), Some("recording"));
    }

    /// An unresolved line renders as readable prose, never an error.
    ///
    /// A lyric nobody holds is the normal case for most of a library, and those
    /// are exactly the records worth writing about — the same reasoning that
    /// makes an unresolved `[[track:…]]` a dotted underline rather than a
    /// failure. The design says the same: "degrades to plain line A3·12".
    #[test]
    fn a_line_nobody_holds_the_words_to_stays_prose() {
        let html = render("the turn is [[line:A:3:12]]", &HashMap::new());
        assert!(html.contains("td-unresolved"), "{html}");
        assert!(html.contains("A:3:12"), "the coordinate is still readable: {html}");
        assert!(!html.contains("<a "), "and it is not a link to nowhere");
    }

    /// The three renderings, and only two of them are shortcodes.
    ///
    /// A **pin** is a note whose target *is* the line, so it is a `NoteTarget`
    /// rather than something written into a sentence — which is also why it is
    /// the only form that can federate. What this pins is the other two:
    /// inline carries the words plus a chip, block sets them as a pull-quote
    /// with the coordinate under them.
    #[test]
    fn a_line_reads_inline_by_default_and_as_a_block_when_asked() {
        let mut r = HashMap::new();
        r.insert(
            (RefKind::Line, "12".to_string()),
            Resolved {
                label: "longer than the map lets on".into(),
                href: "/lyrics?artist=X&title=Y&line=12".into(),
                render: Render::LineInline { chip: "A3\u{b7}12".into() },
            },
        );
        r.insert(
            (RefKind::LineBlock, "13".to_string()),
            Resolved {
                label: "I keep the sea in a coffee tin".into(),
                href: "/lyrics?artist=X&title=Y&line=13".into(),
                render: Render::LineBlock { meta: "line 13 \u{b7} 1:11".into() },
            },
        );

        let html = render("the turn is [[line:12]].\n\n[[quote:13]]", &r);

        // Inline: the words in quotation marks, and a chip that opens the
        // reader — a phrase inside a sentence rather than a bare link.
        assert!(html.contains("td-line-words"), "{html}");
        assert!(html.contains("longer than the map lets on"), "{html}");
        assert!(html.contains("A3\u{b7}12"), "the chip names the coordinate: {html}");

        // Block: a pull-quote with the coordinate under it.
        assert!(html.contains("td-line-block"), "{html}");
        assert!(html.contains("<blockquote>"), "{html}");
        assert!(html.contains("line 13 \u{b7} 1:11"), "{html}");

        // And the classes survived ammonia. This is the half that fails
        // silently: a rendering that grows a class nobody allow-listed is
        // stripped, the markup still parses, and it merely looks unstyled.
        assert!(html.contains("class=\"td-line-chip\""), "{html}");
    }

    /// `quote` and `line` are the same coordinate asked for differently.
    ///
    /// Switching between them has to be a one-word edit that re-resolves
    /// nothing — which is why the target is untouched and only the kind moves.
    #[test]
    fn the_block_form_is_the_same_coordinate_under_another_word() {
        let codes = scan("[[quote:A:3:12]] and [[blockline:7]]");
        assert_eq!(codes.len(), 2);
        assert!(codes.iter().all(|c| c.kind == RefKind::LineBlock));
        assert_eq!(codes[0].target, "A:3:12", "the stored coordinate is identical");
        assert!(RefKind::Line.is_line() && RefKind::LineBlock.is_line());
        assert!(!RefKind::Track.is_line());
        assert_eq!(RefKind::LineBlock.entity_kind(), None, "still not an entity");
    }

    /// **A quoted line never leaves this instance in an export.**
    ///
    /// The fourth of the design's rules, and it holds here by the reference
    /// being left exactly as typed: the words are the provider's text, and an
    /// export is a folder of files that goes into somebody's vault. A dangling
    /// `[[line:12]]` is honest about what it is; the words would not be.
    #[test]
    fn an_export_carries_the_reference_and_never_the_words() {
        // Even handed a resolution — which is what a caller that forgot the
        // rule would do — the markdown export must not paste the words in.
        let mut r = HashMap::new();
        r.insert(
            (RefKind::Line, "12".to_string()),
            Resolved {
                label: "longer than the map lets on".into(),
                href: String::new(),
                render: Render::LineInline { chip: "12".into() },
            },
        );
        let md = export_markdown("the turn is [[line:12]]", &HashMap::new());
        assert_eq!(md, "the turn is [[line:12]]", "left exactly as typed");
        assert!(!export_markdown("the turn is [[line:12]]", &r).contains("longer than the map"));
    }

    #[test]
    fn finds_each_shortcode_kind() {
        let codes = scan(
            "[[track:Xtal]] from [[album:SAW 85-92]] by [[artist:Aphex Twin]] on [[gear:HD 650]]",
        );
        let kinds: Vec<_> = codes.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![RefKind::Track, RefKind::Album, RefKind::Artist, RefKind::Gear]);
        assert_eq!(codes[1].target, "SAW 85-92");
    }

    /// Gear links somewhere else entirely, which is the reason `Resolved`
    /// carries an href instead of an entity id.
    #[test]
    fn gear_refs_link_to_the_gear_page() {
        let html = render(
            "Heard it on the [[gear:Bifrost 2]], and on [[gear:something I don't own]].",
            &resolved(&[(RefKind::Gear, "bifrost 2", 4, "Bifrost 2")]),
        );
        assert!(html.contains(r#"href="/chains?gear=4""#), "{html}");
        // Gear you haven't catalogued is never invented — it reads as prose.
        assert!(html.contains("td-unresolved"), "{html}");
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
        let html =
            render("[[track:3]] is the one", &resolved(&[(RefKind::Track, "3", 12, "Ageispolis")]));
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

    // ── Export ──

    #[test]
    fn a_resolved_ref_exports_as_a_wikilink_and_an_unresolved_one_as_itself() {
        let map = resolved(&[(RefKind::Track, "3", 9, "Xtal — Aphex Twin")]);
        let out = export_markdown("Best is [[track:3]], not [[track:Ptolemy]].", &map);
        assert_eq!(out, "Best is [[Xtal — Aphex Twin]], not [[track:Ptolemy]].");
    }

    #[test]
    fn the_wikilink_is_the_filename_the_export_would_write() {
        // The whole export hangs on these two agreeing: a wikilink resolves by
        // filename, so a label sanitised one way and a file named another means
        // every link in the vault dangles.
        let map = resolved(&[(RefKind::Artist, "AC/DC", 4, "AC/DC")]);
        let out = export_markdown("Saw [[artist:AC/DC]] once.", &map);
        assert_eq!(out, format!("Saw [[{}]] once.", export_filename("AC/DC")));
        assert!(!out.contains('/'), "a slash in a link is a directory in a vault");
    }

    #[test]
    fn a_filename_never_escapes_its_folder_or_ends_up_empty() {
        assert_eq!(export_filename("AC/DC"), "AC DC");
        assert_eq!(export_filename("../../etc/passwd"), ".. .. etc passwd");
        assert_eq!(export_filename("Untitled: \"Live\""), "Untitled Live");
        assert_eq!(export_filename("   "), "Untitled");
        assert_eq!(export_filename("///"), "Untitled");
        // Trailing dots are legal here and refused by Windows.
        assert_eq!(export_filename("Whatever..."), "Whatever");
        assert!(export_filename(&"x".repeat(400)).chars().count() <= 120);
    }

    #[test]
    fn a_body_with_no_shortcodes_is_exported_verbatim() {
        let src = "Bought this on a whim in 2011.\n\n- side A is the one\n";
        assert_eq!(export_markdown(src, &HashMap::new()), src);
    }

    #[test]
    fn a_gear_ref_exports_as_a_link_too() {
        let map = resolved(&[(RefKind::Gear, "Bifrost 2", 2, "Schiit Bifrost 2")]);
        assert_eq!(
            export_markdown("Heard on the [[gear:Bifrost 2]].", &map),
            "Heard on the [[Schiit Bifrost 2]]."
        );
    }
}
