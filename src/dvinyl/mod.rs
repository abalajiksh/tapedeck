//! Reading a DVinyl collection export.
//!
//! DVinyl (github.com/Kyonew/DVinyl, MIT) is a self-hosted catalogue for
//! physical media. It has no API a scrobbler could sync against — its pages are
//! server-rendered behind a login cookie — so the way a collection leaves it is
//! the per-collection backup: **Settings → Backup → Export collection**, as a
//! zip (`backup.json` plus `images/`) or as the bare JSON.
//!
//! This module only *reads* that file. Nothing here touches the database; the
//! handler in `server::physical` decides what already sits on the shelf and
//! writes the rest. Pinned against a real 3.2.1 export in `fixtures/` — the
//! shape is documented by DVinyl's code alone, and a field-name guess compiles
//! fine and reads as a silent `None`.
//
// What comes across, and what deliberately does not:
//
// * Music items only. DVinyl also holds books, films, games, LEGO and board
//   games; those are counted and left, because this is a journal of music.
// * `media_type: digital` is a file, not an object on a shelf, and is left too.
// * The wishlist goes to the Crate, not the shelf — it is music you do not
//   have yet, which is exactly the line the Crate exists to hold.
// * Genres and styles are not carried: here genres attach to *artists* and
//   come from the genre map, and a second source per record would make two
//   screens disagree about what an artist is.
// * Condition grading, the format descriptor and copies go into the note
//   rather than into columns. They are the user's own words about the copy,
//   which is what the note is for; the shelf deliberately has no grading field.
// * Lists, the shelf furniture and market value are DVinyl's own views of the
//   collection and have no counterpart here.

use crate::metadata::release_lookup::{discogs_tracks, DiscogsTrackRow, PhysicalTrack};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;

/// The biggest `backup.json` read out of an archive. DVinyl caps its own at
/// 50 MB when restoring, so anything larger did not come from it.
const MAX_JSON_BYTES: u64 = 50 * 1024 * 1024;
/// DVinyl stores item images as JPEGs of at most 5 MB, and refuses larger ones
/// in its own archives.
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// What a DVinyl export holds that the shelf can use.
#[derive(Debug, Default)]
pub struct Export {
    /// The collection's name in DVinyl ("Vinyl" by default).
    pub collection: String,
    /// The DVinyl version that wrote the file.
    pub version: Option<String>,
    pub pressings: Vec<Pressing>,
    /// Wishlist entries: music the user is after and does not own.
    pub wants: Vec<Want>,
    /// Images carried inside a zip export, by file name.
    pub images: HashMap<String, Vec<u8>>,
    /// Whether the file was a zip. A bare JSON export carries no uploaded
    /// images, so a missing scan means something different for each.
    pub archive: bool,
    pub skipped: Skipped,
}

/// What was in the file and is not coming across, by reason.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    /// `media_type: digital` — a file, not an object on a shelf.
    pub digital: usize,
    /// Books, films, games and everything else that is not music.
    pub not_music: usize,
    /// A music item without an artist or a title.
    pub unreadable: usize,
    /// DVinyl's own curated lists.
    pub lists: usize,
}

/// One record, tape or disc from DVinyl, shaped for the shelf.
#[derive(Debug, Clone)]
pub struct Pressing {
    /// `vinyl`, `cassette`, `cd` or `sacd`.
    pub kind: &'static str,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub discogs_id: Option<String>,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub country: Option<String>,
    /// DVinyl's "Color / Edition".
    pub edition: Option<String>,
    pub location: Option<String>,
    /// The user's comments, then whatever else they wrote about the copy.
    pub notes: Option<String>,
    /// When it went into DVinyl, Unix seconds.
    pub added_at: Option<i64>,
    pub tracks: Vec<PhysicalTrack>,
    /// In DVinyl's order; the first is the cover.
    pub images: Vec<ImageRef>,
}

/// Where one of a pressing's images is.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageRef {
    /// A URL at a provider — Discogs, iTunes — still there to fetch.
    Remote(String),
    /// A file the user uploaded to DVinyl, carried in a zip export under
    /// `images/` by this name.
    Archived(String),
}

/// A wishlist entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub artist: String,
    pub title: String,
    /// The user's own comment on it, if any.
    pub note: Option<String>,
}

/// Read a DVinyl collection export — the zip or the bare JSON.
///
/// The error is a sentence for the person who uploaded the file.
pub fn parse(bytes: &[u8]) -> Result<Export, String> {
    if bytes.starts_with(b"PK\x03\x04") {
        parse_archive(bytes)
    } else {
        let doc: Value = serde_json::from_slice(bytes).map_err(|_| {
            "That is neither a zip nor JSON — is it a DVinyl collection export?".to_string()
        })?;
        parse_document(&doc, HashMap::new(), false)
    }
}

fn parse_archive(bytes: &[u8]) -> Result<Export, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("That zip could not be opened: {e}"))?;

    let doc: Value = {
        let f = zip.by_name("backup.json").map_err(|_| {
            "No backup.json in that zip — is it a DVinyl collection export?".to_string()
        })?;
        let mut text = Vec::new();
        f.take(MAX_JSON_BYTES + 1)
            .read_to_end(&mut text)
            .map_err(|_| "backup.json could not be read".to_string())?;
        if text.len() as u64 > MAX_JSON_BYTES {
            return Err("backup.json is larger than DVinyl itself would write".into());
        }
        serde_json::from_slice(&text).map_err(|e| format!("backup.json is not valid JSON: {e}"))?
    };
    // Checked before a single image is read: an instance backup is refused
    // outright, and there is no reason to have pulled its files into memory.
    refuse_instance_backup(&doc)?;

    let mut images = HashMap::new();
    for i in 0..zip.len() {
        let Ok(f) = zip.by_index(i) else { continue };
        let Some(name) = f.name().strip_prefix("images/").map(str::to_string) else { continue };
        if !is_dvinyl_image_name(&name) || f.size() > MAX_IMAGE_BYTES {
            continue;
        }
        let mut b = Vec::new();
        if f.take(MAX_IMAGE_BYTES).read_to_end(&mut b).is_ok() {
            images.insert(name, b);
        }
    }
    parse_document(&doc, images, true)
}

/// A whole-instance backup carries every account on the DVinyl server —
/// password hashes included — and every collection mixed together. It is
/// refused by its shape, before anything in it is looked at further, and the
/// answer says which export to make instead.
fn refuse_instance_backup(doc: &Value) -> Result<(), String> {
    if doc.get("users").is_some() || doc.get("collections").is_some() {
        return Err(concat!(
            "That is a whole-instance DVinyl backup, which holds every account on the ",
            "server. Export just your collection instead: in DVinyl, Settings → Backup → ",
            "Export collection."
        )
        .into());
    }
    Ok(())
}

/// DVinyl names every upload `item-<uuid>.jpg`. Anything else under `images/`
/// is not one of its files, and the name never reaches a path here anyway.
fn is_dvinyl_image_name(name: &str) -> bool {
    name.strip_prefix("item-")
        .and_then(|s| s.strip_suffix(".jpg"))
        .is_some_and(|id| id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

fn parse_document(
    doc: &Value,
    images: HashMap<String, Vec<u8>>,
    archive: bool,
) -> Result<Export, String> {
    refuse_instance_backup(doc)?;
    let Some(items) = doc.get("albums").and_then(Value::as_array) else {
        return Err("No items in that file — is it a DVinyl collection export?".into());
    };

    // User-defined fields are stored under opaque keys (`custom_528faa1b42c0`)
    // and named in the collection's settings. The label is what the user typed,
    // so that is what the note says.
    let labels: HashMap<&str, &str> = doc
        .pointer("/settings/pluginExtraFields/music")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f.get("name")?.as_str()?, f.get("label")?.as_str()?)))
        .collect();

    let mut out = Export {
        collection: text(doc.get("collectionName")).unwrap_or_else(|| "DVinyl".into()),
        version: text(doc.pointer("/metadata/version")),
        images,
        archive,
        ..Default::default()
    };
    out.skipped.lists = doc.get("lists").and_then(Value::as_array).map_or(0, Vec::len);

    for item in items {
        // Before plugins, DVinyl held only records and its items carried no
        // `kind`; the migration that added it called them all Music.
        let music = match item.get("kind").and_then(Value::as_str) {
            Some(k) => k == "Music",
            None => item.get("artist").is_some(),
        };
        if !music {
            out.skipped.not_music += 1;
            continue;
        }
        let (Some(artist), Some(title)) = (text(item.get("artist")), text(item.get("title")))
        else {
            out.skipped.unreadable += 1;
            continue;
        };

        if item.get("in_wishlist").and_then(Value::as_bool) == Some(true) {
            out.wants.push(Want { artist, title, note: text(item.get("comments")) });
            continue;
        }

        let format = text(item.get("format_type"));
        let kind = match text(item.get("media_type")).as_deref() {
            Some("digital") => {
                out.skipped.digital += 1;
                continue;
            }
            Some("cassette") => "cassette",
            // DVinyl has no SACD of its own; the Discogs format string does.
            Some("cd") if format.as_deref().is_some_and(|f| f.to_lowercase().contains("sacd")) => {
                "sacd"
            }
            Some("cd") => "cd",
            Some("vinyl") => "vinyl",
            // Anything unrecognised: the format string if it names a medium,
            // otherwise DVinyl's own default.
            _ => crate::metadata::release_lookup::kind_from_format(format.as_deref())
                .unwrap_or("vinyl"),
        };

        out.pressings.push(Pressing {
            kind,
            artist,
            title,
            year: year(item.get("year")),
            discogs_id: id_text(item.get("discogs_id")),
            barcode: text(item.get("barcode")),
            catalog_number: text(item.get("catalog_number")),
            label: text(item.get("label")),
            country: text(item.get("country")),
            edition: text(item.get("variant_color")),
            location: text(item.get("location")),
            notes: notes(item, format.as_deref(), &labels),
            added_at: item
                .get("added_at")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp()),
            tracks: tracks(item, kind),
            images: image_refs(item),
        });
    }
    Ok(out)
}

/// A trimmed, non-empty string. DVinyl writes `""` for every field nobody
/// filled in, and a blank barcode must not match every other blank barcode.
fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Discogs ids are numbers in DVinyl and text here.
fn id_text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Number(n) => n.as_i64().filter(|n| *n > 0).map(|n| n.to_string()),
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty() && s != "0"),
        _ => None,
    }
}

/// DVinyl's year is free text ("1973", "1973-03-01", "c. 1973"), so the first
/// four-digit run that is a plausible year is taken.
fn year(v: Option<&Value>) -> Option<i64> {
    let s = match v? {
        Value::Number(n) => return n.as_i64().filter(|y| (1000..=2200).contains(y)),
        Value::String(s) => s,
        _ => return None,
    };
    s.as_bytes()
        .windows(4)
        .filter_map(|w| std::str::from_utf8(w).ok()?.parse::<i64>().ok())
        .find(|y| (1000..=2200).contains(y))
}

/// The user's comments, then the facts about the copy that have no column
/// here, each on its own line.
fn notes(item: &Value, format: Option<&str>, labels: &HashMap<&str, &str>) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    if let Some(c) = text(item.get("comments")) {
        lines.push(c);
    }
    // The bare medium name is DVinyl's default and says nothing the shelf's
    // kind does not; "Vinyl, LP, Album, Reissue, 180 Gram" says a great deal.
    if let Some(f) = format {
        let bare = matches!(f.to_ascii_lowercase().as_str(), "vinyl" | "cd" | "cassette");
        if !bare {
            lines.push(format!("Format: {f}"));
        }
    }
    if let Some(s) = text(item.get("sleeve_condition")) {
        lines.push(format!("Sleeve: {s}"));
    }
    if let Some(q) = item.get("quantity").and_then(Value::as_i64).filter(|q| *q > 1) {
        lines.push(format!("Copies: {q}"));
    }
    if let Some(extra) = item.get("extra").and_then(Value::as_object) {
        for (key, value) in extra {
            let shown = match value {
                Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
                Value::Number(n) => Some(n.to_string()),
                Value::Bool(b) => Some(if *b { "yes".into() } else { "no".into() }),
                Value::Array(a) => {
                    let parts: Vec<String> = a.iter().filter_map(|v| text(Some(v))).collect();
                    Some(parts.join(", ")).filter(|s| !s.is_empty())
                }
                _ => None,
            };
            if let Some(shown) = shown {
                let label = labels.get(key.as_str()).copied().unwrap_or(key.as_str());
                lines.push(format!("{label}: {shown}"));
            }
        }
    }
    Some(lines.join("\n")).filter(|s| !s.is_empty())
}

/// DVinyl's tracklist, which is Discogs' own: `A1`/`B2` positions on a record
/// or a tape, plain numbers on a disc or wherever someone typed them in.
fn tracks(item: &Value, kind: &str) -> Vec<PhysicalTrack> {
    let rows: Vec<DiscogsTrackRow> = item
        .get("tracklist")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let row = DiscogsTrackRow {
                position: t.get("position").and_then(Value::as_str).unwrap_or("").trim(),
                title: t.get("title").and_then(Value::as_str),
                duration: t.get("duration").and_then(Value::as_str),
            };
            // An empty row left behind by the tracklist editor is not a track.
            let blank = row.position.is_empty()
                && row.title.is_none_or(|s| s.trim().is_empty())
                && row.duration.is_none_or(|s| s.trim().is_empty());
            (!blank).then_some(row)
        })
        .collect();
    discogs_tracks(&rows, matches!(kind, "cd" | "sacd"), 2)
}

/// The item's images in DVinyl's order, without duplicates. `images` is the
/// gallery; older items only have `cover_image` and `user_image`.
fn image_refs(item: &Value) -> Vec<ImageRef> {
    let gallery: Vec<&str> = match item.get("images").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a.iter().filter_map(Value::as_str).collect(),
        _ => ["cover_image", "user_image"]
            .iter()
            .filter_map(|k| item.get(*k).and_then(Value::as_str))
            .collect(),
    };
    let mut out: Vec<ImageRef> = Vec::new();
    for raw in gallery {
        let raw = raw.trim();
        let r = if let Some(name) = raw.strip_prefix("/uploads/items/") {
            if !is_dvinyl_image_name(name) {
                continue;
            }
            ImageRef::Archived(name.to_string())
        } else if raw.starts_with("https://") || raw.starts_with("http://") {
            ImageRef::Remote(raw.to_string())
        } else {
            // A placeholder path, or a legacy inline data URL that only the
            // zip export converts into a file.
            continue;
        };
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Which provider a remote image URL belongs to, for the gallery's credit.
pub fn provider_of(url: &str) -> &'static str {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.ends_with("discogs.com") {
        "discogs"
    } else if host.ends_with("mzstatic.com") || host.ends_with("apple.com") {
        "itunes"
    } else if host.ends_with("coverartarchive.org") || host.ends_with("archive.org") {
        "caa"
    } else {
        "dvinyl"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &[u8] = include_bytes!("fixtures/collection-3.2.1.json");
    const ZIP: &[u8] = include_bytes!("fixtures/collection-3.2.1.zip");

    fn find<'a>(e: &'a Export, title: &str) -> &'a Pressing {
        e.pressings.iter().find(|p| p.title == title).expect(title)
    }

    /// The real export: six music items, one of them wished for and one
    /// digital, a book, and an LP with an uploaded scan.
    #[test]
    fn a_real_collection_export_reads() {
        let e = parse(JSON).unwrap();
        assert_eq!(e.collection, "Vinyl");
        assert_eq!(e.version.as_deref(), Some("3.2.1"));
        assert!(!e.archive);
        let titles: Vec<&str> = e.pressings.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(
            titles,
            ["The Dark Side of the Moon", "Untrue", "Paul's Boutique", "Kind of Blue", "Blue"]
        );
        assert_eq!(e.skipped, Skipped { digital: 1, not_music: 1, unreadable: 0, lists: 0 });
        assert_eq!(
            e.wants,
            [Want {
                artist: "Aphex Twin".into(),
                title: "Selected Ambient Works 85-92".into(),
                note: Some("Only the R&S original pressing.".into()),
            }]
        );
    }

    #[test]
    fn every_field_dvinyl_holds_lands_somewhere() {
        let e = parse(JSON).unwrap();
        let p = find(&e, "The Dark Side of the Moon");
        assert_eq!(p.kind, "vinyl");
        assert_eq!(p.artist, "Pink Floyd");
        assert_eq!(p.year, Some(1973));
        assert_eq!(p.discogs_id.as_deref(), Some("1873013"));
        assert_eq!(p.barcode.as_deref(), Some("5099902987613"));
        assert_eq!(p.catalog_number.as_deref(), Some("SHVL 804"));
        assert_eq!(p.label.as_deref(), Some("Harvest"));
        assert_eq!(p.country.as_deref(), Some("Europe"));
        assert_eq!(p.edition.as_deref(), Some("Clear"));
        assert_eq!(p.location.as_deref(), Some("Living room, shelf 2"));
        assert_eq!(p.added_at, Some(1_715_990_400)); // 2024-05-18
        assert_eq!(
            p.notes.as_deref(),
            Some(
                "Bought at the Kreuzberg record fair.\n\
                 Format: Vinyl, LP, Album, Reissue, Remastered, 180 Gram\n\
                 Sleeve: VG+"
            )
        );
        assert_eq!(
            p.images,
            [
                ImageRef::Remote("https://i.discogs.com/example/cover.jpg".into()),
                ImageRef::Archived("item-82f4121a-f240-42d9-901a-6d782c7b3138.jpg".into()),
            ]
        );
    }

    /// DVinyl writes `""` for every unfilled field. A blank barcode that
    /// survived as `Some("")` would match every other blank one on the shelf.
    #[test]
    fn blank_fields_are_absent_not_empty() {
        let e = parse(JSON).unwrap();
        let p = find(&e, "Paul's Boutique");
        assert_eq!(p.kind, "cassette");
        assert_eq!(p.barcode, None);
        assert_eq!(p.discogs_id, None);
        assert_eq!(p.country, None);
        assert_eq!(p.edition, None);
        assert_eq!(p.added_at.is_some(), true);
    }

    /// The point of reading Discogs' positions: the side letters are real, so
    /// a double LP gets four sides where they physically are.
    #[test]
    fn side_letters_come_from_the_positions() {
        let e = parse(JSON).unwrap();
        let p = find(&e, "Untrue");
        let sides: Vec<&str> = p.tracks.iter().map(|t| t.side.as_str()).collect();
        assert_eq!(sides, ["A", "A", "B", "B", "C", "C", "D", "D"]);
        assert_eq!(p.tracks[4].title, "Endorphin");
        assert_eq!(p.tracks[4].position, 1);
        assert_eq!(p.tracks[4].duration_secs, Some(176));
        assert_eq!(p.notes.as_deref(), Some("Format: Vinyl, 12\", Album\nCopies: 2"));
    }

    /// A record typed in with plain numbers still gets a side break, placed by
    /// playing time.
    #[test]
    fn numbered_positions_on_a_record_are_split_by_playing_time() {
        let e = parse(JSON).unwrap();
        let p = find(&e, "Blue");
        let sides: Vec<&str> = p.tracks.iter().map(|t| t.side.as_str()).collect();
        assert_eq!(sides, ["A", "A", "A", "A", "A", "B", "B", "B", "B", "B"]);
        assert_eq!(p.notes.as_deref(), Some("Format: Vinyl, LP, Album\nPrice paid: 24 EUR"));
    }

    /// A disc plays straight through; its tracks share one numbered side.
    #[test]
    fn a_disc_has_no_side_break() {
        let e = parse(JSON).unwrap();
        let p = find(&e, "Kind of Blue");
        assert_eq!(p.kind, "cd");
        assert!(p.tracks.iter().all(|t| t.side == "1"));
        assert_eq!(p.tracks.len(), 3);
    }

    #[test]
    fn a_tape_with_no_durations_keeps_its_sides() {
        let e = parse(JSON).unwrap();
        let p = find(&e, "Paul's Boutique");
        let sides: Vec<&str> = p.tracks.iter().map(|t| t.side.as_str()).collect();
        assert_eq!(sides, ["A", "A", "B"]);
        assert!(p.tracks.iter().all(|t| t.duration_secs.is_none()));
    }

    /// The zip carries the uploaded scan under `images/`, and the item points
    /// at it by the same name.
    #[test]
    fn the_archive_carries_the_uploaded_scan() {
        let e = parse(ZIP).unwrap();
        assert!(e.archive);
        assert_eq!(e.pressings.len(), 5);
        let bytes = e.images.get("item-82f4121a-f240-42d9-901a-6d782c7b3138.jpg").unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]), "a JPEG");
    }

    /// An instance backup holds every account on the server, password hashes
    /// included. It is refused by its shape, with directions to the right
    /// export.
    #[test]
    fn an_instance_backup_is_refused() {
        let doc = br#"{"users":[{"username":"a","password":"$2b$10$x"}],"albums":[],"collections":[],"metadata":{"version":"3.2.1"}}"#;
        let err = parse(doc).unwrap_err();
        assert!(err.contains("Export collection"), "{err}");
    }

    #[test]
    fn something_else_entirely_is_refused() {
        assert!(parse(b"not json").unwrap_err().contains("DVinyl"));
        assert!(parse(br#"{"listens":[]}"#).unwrap_err().contains("DVinyl"));
    }

    /// Items from before DVinyl had plugins carry no `kind`; they were all
    /// records.
    #[test]
    fn a_pre_plugin_item_is_music() {
        let doc = br#"{"albums":[{"title":"Low","artist":"David Bowie","year":"1977","media_type":"vinyl"}]}"#;
        let e = parse(doc).unwrap();
        assert_eq!(e.pressings.len(), 1);
        assert_eq!(e.pressings[0].year, Some(1977));
    }

    #[test]
    fn an_sacd_is_told_apart_by_its_format() {
        let doc = br#"{"albums":[{"kind":"Music","title":"Brothers in Arms","artist":"Dire Straits","media_type":"cd","format_type":"SACD, Hybrid, Album"}]}"#;
        assert_eq!(parse(doc).unwrap().pressings[0].kind, "sacd");
    }

    #[test]
    fn years_are_read_out_of_free_text() {
        let v = |s: &str| year(Some(&Value::String(s.into())));
        assert_eq!(v("1973"), Some(1973));
        assert_eq!(v("1973-03-01"), Some(1973));
        assert_eq!(v("c. 1968"), Some(1968));
        assert_eq!(v("unknown"), None);
        assert_eq!(year(Some(&serde_json::json!(1989))), Some(1989));
    }

    #[test]
    fn only_dvinyls_own_file_names_are_taken() {
        assert!(is_dvinyl_image_name("item-82f4121a-f240-42d9-901a-6d782c7b3138.jpg"));
        assert!(!is_dvinyl_image_name("../../etc/passwd"));
        assert!(!is_dvinyl_image_name("item-x.jpg"));
        assert!(!is_dvinyl_image_name("item-82f4121a-f240-42d9-901a-6d782c7b3138.png"));
    }

    #[test]
    fn providers_are_named_from_the_host() {
        assert_eq!(provider_of("https://i.discogs.com/a.jpg"), "discogs");
        assert_eq!(provider_of("https://is1-ssl.mzstatic.com/a/600x600bb.jpg"), "itunes");
        assert_eq!(provider_of("https://example.org/x.jpg"), "dvinyl");
    }
}
