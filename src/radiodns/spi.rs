//! The two SPI documents RadioDNS serves: `SI.xml`, what a broadcaster's
//! services are, and `<date>_PI.xml`, what one of them airs that day. Both are
//! ETSI TS 102 818 (the schema namespace ends `/spi/31`); on-air DAB EPG
//! decodes to the same model, so this parser would serve that too.
//!
//! Read by walking a small element tree built from quick-xml events rather
//! than through serde, for the reason `sources::plex` gives: serde fails on
//! repeated same-name elements that are not contiguous, and in SPI `genre`,
//! `link` and `mediaDescription` interleave freely. Elements are matched by
//! local name, so a broadcaster that prefixes the namespace still parses.
//!
//! Everything is optional on the way out. A broadcaster meeting only the
//! "Project Logo" minimum publishes names and logos and nothing else, and a
//! missing field must cost that field, never the service.

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

/// One element: its local name, attributes, the text directly inside it, and
/// its children. CDATA counts as text — MDR wraps every programme name in it.
#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// A child's text, trimmed, or `None` when it is absent or empty.
    fn text_of(&self, name: &str) -> Option<String> {
        self.child(name).map(|c| c.text.trim().to_string()).filter(|t| !t.is_empty())
    }
}

fn local(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

fn element(e: &quick_xml::events::BytesStart<'_>) -> Node {
    let attrs = e
        .attributes()
        .flatten()
        .map(|a| {
            let value = a
                .unescape_value()
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
            (local(a.key.local_name().as_ref()), value)
        })
        .collect();
    Node { name: local(e.local_name().as_ref()), attrs, ..Node::default() }
}

/// The document's root element, or why there is none.
fn parse_tree(xml: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Node> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => stack.push(element(e)),
            Ok(Event::Empty(ref e)) => {
                let node = element(e);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => return Ok(node),
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop().ok_or("unbalanced XML")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => return Ok(node),
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(top) = stack.last_mut() {
                    match t.unescape() {
                        Ok(s) => top.text.push_str(&s),
                        Err(_) => top.text.push_str(&String::from_utf8_lossy(&t)),
                    }
                }
            }
            Ok(Event::CData(c)) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&c.into_inner()));
                }
            }
            Ok(Event::Eof) => {
                return Err("the document ended before its root element closed".into())
            }
            Ok(_) => {}
            Err(e) => return Err(format!("not XML: {e}")),
        }
    }
}

/// The three lengths of name SPI gives everything. `medium` is often a
/// truncation of `long` (MDR's "MDR KULTUR am Mo"), so [`Names::best`] reads
/// the longest one there is.
#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
#[schema(as = SpiNames)]
pub struct Names {
    /// At most 8 characters.
    pub short: Option<String>,
    /// At most 16 characters.
    pub medium: Option<String>,
    /// At most 128 characters.
    pub long: Option<String>,
}

impl Names {
    fn of(node: &Node) -> Self {
        Names {
            short: node.text_of("shortName"),
            medium: node.text_of("mediumName"),
            long: node.text_of("longName"),
        }
    }

    pub fn best(&self) -> Option<&str> {
        self.long.as_deref().or(self.medium.as_deref()).or(self.short.as_deref())
    }
}

/// A TV-Anytime genre: the classification URN and the label the broadcaster
/// gave it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SpiGenre {
    /// e.g. `urn:tva:metadata:cs:ContentCS:2011:3.6.4.1`.
    pub href: String,
    pub name: Option<String>,
}

fn genres(node: &Node) -> Vec<SpiGenre> {
    node.all("genre")
        .filter_map(|g| {
            let href = g.attr("href")?.to_string();
            let name = Some(g.text.trim().to_string()).filter(|t| !t.is_empty());
            Some(SpiGenre { href, name })
        })
        .collect()
}

/// The longest description present: `longDescription` when there is one,
/// otherwise the short one.
fn description(node: &Node) -> Option<String> {
    let mut short = None;
    for md in node.all("mediaDescription") {
        if let Some(long) = md.text_of("longDescription") {
            return Some(long);
        }
        if short.is_none() {
            short = md.text_of("shortDescription");
        }
    }
    short
}

/// A station logo, as a URL on the broadcaster's server. Never fetched here.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[schema(as = SpiLogo)]
pub struct Logo {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `logo_colour_square`, `logo_colour_rectangle` or `logo_unrestricted`.
    pub kind: Option<String>,
    pub mime: Option<String>,
}

/// One way to receive a service: `fm:…`, `dab:…`, or an `http(s)://` stream.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[schema(as = SpiBearer)]
pub struct ServiceBearer {
    /// The bearer URI as published. An FM bearer may end in `*`, meaning
    /// every frequency the PI code is heard on.
    pub id: String,
    /// The broadcaster's preference between bearers; lower is preferred.
    pub cost: Option<u32>,
    pub mime: Option<String>,
    /// kbit/s, given for streams.
    pub bitrate: Option<u32>,
}

/// One service from a broadcaster's `SI.xml`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[schema(as = SpiService)]
pub struct Service {
    pub names: Names,
    pub description: Option<String>,
    pub genres: Vec<SpiGenre>,
    pub logos: Vec<Logo>,
    pub bearers: Vec<ServiceBearer>,
    /// Web pages the broadcaster lists (`http(s)` links only; postal and
    /// telephone links are left out).
    pub links: Vec<String>,
}

fn parse_u32(s: Option<&str>) -> Option<u32> {
    s.and_then(|v| v.trim().parse().ok())
}

fn service(node: &Node) -> Service {
    let logos = node
        .all("mediaDescription")
        .flat_map(|md| md.all("multimedia"))
        .filter_map(|m| {
            Some(Logo {
                url: m.attr("url")?.to_string(),
                width: parse_u32(m.attr("width")),
                height: parse_u32(m.attr("height")),
                kind: m.attr("type").map(str::to_string),
                mime: m.attr("mimeValue").map(str::to_string),
            })
        })
        .collect();
    let bearers = node
        .all("bearer")
        .filter_map(|b| {
            Some(ServiceBearer {
                id: b.attr("id")?.trim().to_string(),
                cost: parse_u32(b.attr("cost")),
                mime: b.attr("mimeValue").map(str::to_string),
                bitrate: parse_u32(b.attr("bitrate")),
            })
        })
        .collect();
    let links = node
        .all("link")
        .filter_map(|l| l.attr("uri"))
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .map(str::to_string)
        .collect();
    Service {
        names: Names::of(node),
        description: description(node),
        genres: genres(node),
        logos,
        bearers,
        links,
    }
}

/// A broadcaster's `SI.xml`: who they are, and every service they list.
#[derive(Debug, Clone)]
pub struct ServiceInformation {
    pub provider: Option<Names>,
    pub services: Vec<Service>,
}

pub fn parse_si(xml: &str) -> Result<ServiceInformation, String> {
    let root = parse_tree(xml)?;
    if root.name != "serviceInformation" {
        return Err(format!("expected <serviceInformation>, found <{}>", root.name));
    }
    let Some(services) = root.child("services") else {
        return Ok(ServiceInformation { provider: None, services: Vec::new() });
    };
    Ok(ServiceInformation {
        provider: services.child("serviceProvider").map(Names::of),
        services: services.all("service").map(service).collect(),
    })
}

/// One programme from a `PI.xml` schedule.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[schema(as = SpiProgramme)]
pub struct Programme {
    /// The broadcaster's CRID, e.g. `crid://MDR.de/MDR+KULTUR/7409527`.
    pub id: Option<String>,
    pub names: Names,
    /// Unix seconds. The broadcaster's `actualTime` when it gives one (the
    /// time it really went out), otherwise the scheduled `time`.
    pub start: i64,
    /// Seconds, from the matching `actualDuration` or `duration`.
    pub duration: Option<u32>,
    pub description: Option<String>,
    pub genres: Vec<SpiGenre>,
}

/// `PT1H25M`, `PT35M`, `PT30S`, `P1DT2H` → seconds. `None` for anything else,
/// which costs the duration and not the programme.
pub fn iso_duration(s: &str) -> Option<u32> {
    let rest = s.trim().strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total: u64 = 0;
    let mut take = |part: &str, units: &[(char, u64)]| -> Option<()> {
        let mut num = String::new();
        for ch in part.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
            } else {
                let (_, mult) = units.iter().find(|(u, _)| *u == ch)?;
                total += num.parse::<u64>().ok()? * mult;
                num.clear();
            }
        }
        num.is_empty().then_some(())
    };
    take(date, &[('D', 86_400)])?;
    take(time, &[('H', 3_600), ('M', 60), ('S', 1)])?;
    u32::try_from(total).ok()
}

fn programme(node: &Node) -> Option<Programme> {
    let time = node.all("location").flat_map(|l| l.all("time")).next()?;
    let (when, length) = match time.attr("actualTime") {
        Some(t) => (t, time.attr("actualDuration").or(time.attr("duration"))),
        None => (time.attr("time")?, time.attr("duration")),
    };
    let start = chrono::DateTime::parse_from_rfc3339(when).ok()?.timestamp();
    Some(Programme {
        id: node.attr("id").map(str::to_string),
        names: Names::of(node),
        start,
        duration: length.and_then(iso_duration),
        description: description(node),
        genres: genres(node),
    })
}

/// A day's schedule, in start order. A programme with no parseable start is
/// left out, because a schedule entry without a time places nothing.
pub fn parse_pi(xml: &str) -> Result<Vec<Programme>, String> {
    let root = parse_tree(xml)?;
    if root.name != "epg" {
        return Err(format!("expected <epg>, found <{}>", root.name));
    }
    let mut out: Vec<Programme> =
        root.all("schedule").flat_map(|s| s.all("programme")).filter_map(programme).collect();
    out.sort_by_key(|p| p.start);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from demdr.radiodns.ard.de on 2026-10-17, trimmed to the
    // provider and two services, and to the first four programmes.
    const SI: &str = include_str!("fixtures/mdr_SI.xml");
    const PI: &str = include_str!("fixtures/mdr_kultur_20261017_PI.xml");

    #[test]
    fn a_real_si_document_reads_names_logos_genres_and_bearers() {
        let si = parse_si(SI).unwrap();
        assert_eq!(
            si.provider.as_ref().and_then(|p| p.long.as_deref()),
            Some("MITTELDEUTSCHER RUNDFUNK")
        );
        assert_eq!(si.services.len(), 2);

        let kultur =
            si.services.iter().find(|s| s.names.medium.as_deref() == Some("MDR KULTUR")).unwrap();
        assert_eq!(kultur.names.short.as_deref(), Some("KULTUR"));
        assert_eq!(kultur.names.best(), Some("MDR KULTUR - Das Radio"));
        assert!(kultur.description.as_deref().unwrap().starts_with("MDR KULTUR begleitet"));
        assert_eq!(kultur.logos.len(), 5);
        let square = &kultur.logos[0];
        assert_eq!((square.width, square.height), (Some(32), Some(32)));
        assert_eq!(square.kind.as_deref(), Some("logo_colour_square"));
        assert_eq!(kultur.genres.len(), 3);
        assert_eq!(kultur.genres[2].name.as_deref(), Some("Singer/Songwriter"));
        assert_eq!(kultur.genres[2].href, "urn:tva:metadata:cs:ContentCS:2011:3.6.4.17");

        let ids: Vec<&str> = kultur.bearers.iter().map(|b| b.id.as_str()).collect();
        assert!(ids.contains(&"fm:de0.d3c3.*"), "{ids:?}");
        assert!(ids.contains(&"dab:de0.10d1.d3c3.0"), "{ids:?}");
        let stream = kultur.bearers.iter().find(|b| b.id.starts_with("http")).unwrap();
        assert_eq!((stream.bitrate, stream.cost), (Some(128), Some(100)));
        assert_eq!(stream.mime.as_deref(), Some("audio/mpeg"));
        // Postal links are not web pages.
        assert_eq!(kultur.links, vec!["https://www.mdr.de/kultur/radio/index.html"]);
    }

    #[test]
    fn a_real_pi_document_reads_cdata_names_and_times() {
        let progs = parse_pi(PI).unwrap();
        assert_eq!(progs.len(), 4);
        let first = &progs[0];
        assert_eq!(first.id.as_deref(), Some("crid://MDR.de/MDR+KULTUR/7409527"));
        // CDATA, and the medium name truncated exactly as MDR publishes it.
        assert_eq!(first.names.medium.as_deref(), Some("MDR KULTUR am Mo"));
        assert_eq!(first.names.best(), Some("MDR KULTUR am Morgen"));
        // 2026-10-17T06:00:00+02:00
        assert_eq!(first.start, 1_792_209_600);
        assert_eq!(first.duration, Some(3 * 3600));
        assert!(first.description.as_deref().unwrap().contains("06:45"));
        assert_eq!(progs[1].duration, Some(35 * 60));
        assert_eq!(progs[2].duration, Some(85 * 60));
        assert!(progs.windows(2).all(|w| w[0].start <= w[1].start));
    }

    #[test]
    fn the_wrong_document_is_refused_rather_than_read_as_empty() {
        assert!(parse_si(PI).is_err());
        assert!(parse_pi(SI).is_err());
        assert!(parse_pi("<html><body>Not found</body></html>").is_err());
        assert!(parse_pi("not xml at all <").is_err());
    }

    #[test]
    fn a_programme_without_a_time_is_left_out_and_a_bad_duration_costs_only_the_duration() {
        let xml = r#"<epg><schedule>
            <programme id="a"><longName>No time</longName></programme>
            <programme id="b"><longName>Odd length</longName>
              <location><time time="2026-10-17T06:00:00Z" duration="three hours"/></location>
            </programme>
        </schedule></epg>"#;
        let progs = parse_pi(xml).unwrap();
        assert_eq!(progs.len(), 1);
        assert_eq!(progs[0].id.as_deref(), Some("b"));
        assert_eq!(progs[0].duration, None);
    }

    #[test]
    fn a_prefixed_namespace_still_parses() {
        let xml = r#"<spi:epg xmlns:spi="http://www.worlddab.org/schemas/spi/31"><spi:schedule>
            <spi:programme><spi:mediumName>Prefixed</spi:mediumName>
              <spi:location><spi:time time="2026-10-17T06:00:00Z" duration="PT1H"/></spi:location>
            </spi:programme></spi:schedule></spi:epg>"#;
        let progs = parse_pi(xml).unwrap();
        assert_eq!(progs[0].names.best(), Some("Prefixed"));
    }

    #[test]
    fn iso_durations() {
        assert_eq!(iso_duration("PT3H"), Some(10_800));
        assert_eq!(iso_duration("PT1H25M"), Some(5_100));
        assert_eq!(iso_duration("PT30S"), Some(30));
        assert_eq!(iso_duration("P1DT2H"), Some(93_600));
        assert_eq!(iso_duration("PT"), Some(0));
        assert_eq!(iso_duration("3H"), None);
        assert_eq!(iso_duration("PT3X"), None);
        assert_eq!(iso_duration("PT3"), None);
    }
}
