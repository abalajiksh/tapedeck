//! The stations a person can pick from: every service the ARD broadcasters
//! list in their `SI.xml`, merged into one list.
//!
//! **German public radio only, for now.** RadioDNS has no directory — a lookup
//! goes from a bearer you already know to its broadcaster, never the other
//! way — so a list has to start from broadcasters somebody named. The nine ARD
//! members publish complete service lists (about 130 stations between them,
//! each with its FM PI code, DAB service IDs and an internet stream). Commercial
//! stations are reachable through [`super::lookup`] by bearer, and a fuller
//! directory is later work.
//!
//! What SI.xml does **not** carry is worth saying, because the Radio page
//! wants it: ARD publishes FM bearers as `fm:de0.d3c3.*` (every transmitter),
//! so there is no frequency; there is no DAB channel block; and there is no
//! city. Those are the listener's to type on a preset. `region` here is the
//! broadcaster's area, which is true of every station it lists.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::spi::{Service, ServiceInformation};
use crate::sources::egress::Egress;

/// `(fqdn, short name, region)`. The FQDN is what each member's RadioDNS
/// CNAMEs point at, and each has a `_radioepg._tcp` SRV record.
pub const ARD: &[(&str, &str, &str)] = &[
    ("debr.radiodns.ard.de", "BR", "Bayern"),
    ("dehr.radiodns.ard.de", "HR", "Hessen"),
    ("demdr.radiodns.ard.de", "MDR", "Mitteldeutschland"),
    ("dendr.radiodns.ard.de", "NDR", "Norddeutschland"),
    ("derb.radiodns.ard.de", "Radio Bremen", "Bremen"),
    ("derbb.radiodns.ard.de", "RBB", "Berlin-Brandenburg"),
    ("desr.radiodns.ard.de", "SR", "Saarland"),
    ("deswr.radiodns.ard.de", "SWR", "Baden-Württemberg / Rheinland-Pfalz"),
    ("dewdr.radiodns.ard.de", "WDR", "Nordrhein-Westfalen"),
];

/// Service lists change a few times a year; a day is plenty fresh, and the
/// Pi should not fetch 400 KB of XML every time someone opens the page.
const TTL: Duration = Duration::from_secs(24 * 3600);

/// An internet stream a station publishes — what the tuner reads track names
/// from.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct StreamChoice {
    pub url: String,
    pub mime: Option<String>,
    /// kbit/s.
    pub bitrate: Option<u32>,
}

/// One pickable station.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DirectoryStation {
    /// `<gcc>.<pi>`, e.g. `de0.d3c3`. In German public radio the FM PI code
    /// and the DAB service ID are the same number, so one id names the
    /// station on both bands.
    pub id: String,
    pub name: String,
    /// The 8-character name a radio display shows.
    pub short_name: Option<String>,
    /// The broadcaster's FQDN, e.g. `demdr.radiodns.ard.de`.
    pub broadcaster: String,
    /// e.g. `MDR`.
    pub provider: String,
    /// The broadcaster's area — not where this station is heard.
    pub region: String,
    pub description: Option<String>,
    /// The broadcaster's genre labels, e.g. `Pop`, `Information`.
    pub genres: Vec<String>,
    /// The best square logo, when there is one.
    pub logo: Option<String>,
    /// The FM bearer as published, e.g. `fm:de0.d3c3.*`. Absent for a
    /// DAB-only station.
    pub fm: Option<String>,
    /// Every DAB bearer, one per ensemble it is carried in.
    pub dab: Vec<String>,
    /// The stream the tuner would read. Absent means it cannot be logged.
    pub stream: Option<StreamChoice>,
}

impl DirectoryStation {
    pub fn has_band(&self, band: &str) -> bool {
        match band {
            "fm" => self.fm.is_some(),
            "dab" => !self.dab.is_empty(),
            _ => false,
        }
    }
}

/// `fm:de0.d3c3.*` or `dab:de0.10d1.d3c3.0` → `de0.d3c3`.
fn station_id(bearer: &str) -> Option<String> {
    let (scheme, rest) = bearer.split_once(':')?;
    let parts: Vec<&str> = rest.split('.').collect();
    let id = match (scheme, parts.as_slice()) {
        ("fm", [gcc, pi, _]) => format!("{gcc}.{pi}"),
        ("dab", [gcc, _, sid, _]) => format!("{gcc}.{sid}"),
        _ => return None,
    };
    Some(id.to_ascii_lowercase())
}

/// The cheapest stream to keep open all evening that still carries ICY
/// titles: MP3 before anything else (every Icecast MP3 mount interleaves
/// metadata; AAC mounts vary), then the lowest bitrate, since the audio itself
/// is thrown away.
fn pick_stream(service: &Service) -> Option<StreamChoice> {
    let mut streams: Vec<_> = service
        .bearers
        .iter()
        .filter(|b| b.id.starts_with("http://") || b.id.starts_with("https://"))
        .collect();
    streams.sort_by_key(|b| {
        let mp3 = b.mime.as_deref() == Some("audio/mpeg")
            || b.id.contains("/mp3/")
            || b.id.ends_with(".mp3");
        (!mp3, b.bitrate.unwrap_or(u32::MAX))
    });
    streams.first().map(|b| StreamChoice {
        url: b.id.clone(),
        mime: b.mime.clone(),
        bitrate: b.bitrate,
    })
}

/// The largest square logo, else the largest of any shape.
fn pick_logo(service: &Service) -> Option<String> {
    let area = |l: &&super::spi::Logo| l.width.unwrap_or(0) * l.height.unwrap_or(0);
    service
        .logos
        .iter()
        .filter(|l| l.width.is_some() && l.width == l.height)
        .max_by_key(area)
        .or_else(|| service.logos.iter().max_by_key(area))
        .map(|l| l.url.clone())
}

/// One broadcaster's SI.xml as directory entries. A service with neither an
/// FM nor a DAB bearer is a web-only stream and is left out — this is a list
/// of broadcast stations.
pub fn stations_from(
    si: &ServiceInformation,
    fqdn: &str,
    provider: &str,
    region: &str,
) -> Vec<DirectoryStation> {
    si.services
        .iter()
        .filter_map(|s| {
            let fm = s
                .bearers
                .iter()
                .find(|b| b.id.starts_with("fm:"))
                .map(|b| b.id.to_ascii_lowercase());
            let dab: Vec<String> = s
                .bearers
                .iter()
                .filter(|b| b.id.starts_with("dab:"))
                .map(|b| b.id.to_ascii_lowercase())
                .collect();
            let id = fm
                .as_deref()
                .and_then(station_id)
                .or_else(|| dab.first().and_then(|d| station_id(d)))?;
            let name = s.names.best()?.to_string();
            Some(DirectoryStation {
                id,
                short_name: s.names.short.clone(),
                // `medium` reads better than `long` in a list — "MDR KULTUR"
                // over "MDR KULTUR - Das Radio" — when it is not a truncation.
                name: s.names.medium.clone().filter(|m| m.chars().count() < 16).unwrap_or(name),
                broadcaster: fqdn.to_string(),
                provider: provider.to_string(),
                region: region.to_string(),
                description: s.description.clone(),
                genres: s.genres.iter().filter_map(|g| g.name.clone()).collect(),
                logo: pick_logo(s),
                fm,
                dab,
                stream: pick_stream(s),
            })
        })
        .collect()
}

#[derive(Default)]
struct Cached {
    at: Option<Instant>,
    stations: Arc<Vec<DirectoryStation>>,
}

/// The merged list, fetched on first use and kept for a day.
#[derive(Default)]
pub struct Directory {
    cache: Mutex<Cached>,
}

impl Directory {
    /// Every station, refreshed when stale. A broadcaster that fails this
    /// time keeps nothing from last time — but a refresh that fails outright
    /// keeps the old list, so a flaky server never empties the page.
    pub async fn stations(&self) -> Arc<Vec<DirectoryStation>> {
        // Held across the fetch on purpose: two people opening the page at
        // once should cost one round of requests, not two.
        let mut cache = self.cache.lock().await;
        if cache.at.is_some_and(|at| at.elapsed() < TTL) {
            return cache.stations.clone();
        }
        let fresh = fetch_all().await;
        if fresh.is_empty() && !cache.stations.is_empty() {
            warn!("📻 Could not refresh the station list; keeping the one from earlier");
            // Try again in an hour rather than on every request.
            cache.at = Some(Instant::now() - TTL + Duration::from_secs(3600));
            return cache.stations.clone();
        }
        cache.stations = Arc::new(fresh);
        cache.at = Some(Instant::now());
        cache.stations.clone()
    }

    pub async fn find(&self, id: &str) -> Option<DirectoryStation> {
        self.stations().await.iter().find(|s| s.id == id).cloned()
    }
}

async fn fetch_all() -> Vec<DirectoryStation> {
    // The broadcasters are named here, but their SPI servers are named by DNS,
    // so this is held to the public internet like a member's source.
    let egress = Egress::Member;
    let client = super::client(egress);
    let jobs = ARD.iter().map(|(fqdn, provider, region)| {
        let client = client.clone();
        async move {
            let base = super::spi_server(fqdn).await.map_err(|e| e.to_string())?;
            let si = super::service_information(&client, egress, &base)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{fqdn} has no SI.xml"))?;
            Ok::<_, String>(stations_from(&si, fqdn, provider, region))
        }
    });
    let mut out: Vec<DirectoryStation> = Vec::new();
    for (result, (fqdn, ..)) in futures_util::future::join_all(jobs).await.into_iter().zip(ARD) {
        match result {
            Ok(list) => {
                for s in list {
                    // A joint programme can appear under two broadcasters; the
                    // first keeps it.
                    if !out.iter().any(|o| o.id == s.id) {
                        out.push(s);
                    }
                }
            }
            Err(e) => warn!("📻 Station list: {fqdn}: {e}"),
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    info!("📻 Station list: {} stations from {} broadcasters", out.len(), ARD.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_si_document_becomes_pickable_stations() {
        let si = super::super::spi::parse_si(include_str!("fixtures/mdr_SI.xml")).unwrap();
        let list = stations_from(&si, "demdr.radiodns.ard.de", "MDR", "Mitteldeutschland");
        assert_eq!(list.len(), 2);
        let kultur = list.iter().find(|s| s.id == "de0.d3c3").expect("KULTUR by its PI");
        assert_eq!(kultur.name, "MDR KULTUR");
        assert_eq!(kultur.short_name.as_deref(), Some("KULTUR"));
        assert_eq!(kultur.fm.as_deref(), Some("fm:de0.d3c3.*"));
        assert_eq!(kultur.dab.len(), 3);
        assert!(kultur.has_band("fm") && kultur.has_band("dab"));
        assert_eq!(kultur.genres, vec!["Information", "Other Music", "Singer/Songwriter"]);
        // The 600×600 square, not the 32px one.
        assert!(kultur.logo.as_deref().unwrap().ends_with("d8c9dab3ecb177966e59fda5.png"));
        let stream = kultur.stream.as_ref().expect("KULTUR publishes a stream");
        assert!(stream.url.contains("/mp3/"), "{}", stream.url);
    }

    #[test]
    fn the_cheapest_mp3_stream_wins() {
        use super::super::spi::{Names, ServiceBearer};
        let b = |id: &str, mime: Option<&str>, bitrate: Option<u32>| ServiceBearer {
            id: id.into(),
            cost: None,
            mime: mime.map(str::to_string),
            bitrate,
        };
        let s = Service {
            names: Names::default(),
            description: None,
            genres: vec![],
            logos: vec![],
            bearers: vec![
                b("fm:de0.d3c3.*", None, None),
                b("http://x/aac/high", Some("audio/aac"), Some(48)),
                b("http://x/mp3/high/stream.mp3", Some("audio/mpeg"), Some(256)),
                b("http://x/mp3/low/stream.mp3", Some("audio/mpeg"), Some(128)),
            ],
            links: vec![],
        };
        assert_eq!(pick_stream(&s).unwrap().url, "http://x/mp3/low/stream.mp3");
    }

    #[test]
    fn station_ids_come_from_either_band() {
        assert_eq!(station_id("fm:DE0.D3C3.*").as_deref(), Some("de0.d3c3"));
        assert_eq!(station_id("dab:de0.10d1.d3c3.0").as_deref(), Some("de0.d3c3"));
        assert_eq!(station_id("http://x"), None);
        assert_eq!(station_id("dab:de0.10d1"), None);
    }
}
