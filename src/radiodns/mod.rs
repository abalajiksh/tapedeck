//! RadioDNS: from a broadcast identifier to the station's own description of
//! itself and its schedule. ETSI TS 103 270 for the lookup, TS 102 818 for the
//! documents (see [`spi`]).
//!
//! The identifier is a **bearer URI**, the one form both standards share:
//!
//! * `fm:<gcc>.<pi>.<freq>` — `fm:de0.d3c3.09440` is 94.40 MHz, PI D3C3, Germany
//! * `dab:<gcc>.<eid>.<sid>.<scids>` — `dab:de0.10d1.d3c3.0`
//!
//! `<gcc>` is the country code from the PI or SId plus the ECC: Germany is
//! `de0`. The same strings appear in every `SI.xml`, which is how a lookup
//! finds its own service among the broadcaster's.
//!
//! Then: a CNAME on `<freq>.<pi>.<gcc>.fm.radiodns.org` names the broadcaster,
//! an SRV on `_radioepg._tcp.<that>` names their SPI server, and two plain
//! HTTP fetches return `SI.xml` and `<YYYYMMDD>_PI.xml`.
//!
//! **Groundwork only.** Nothing here records a listen or decides that radio is
//! one; whether a radio listen is scrobbled at all is still undecided. It
//! answers "what is this station and what is on today", for a caller to use.
//!
//! Coverage is uneven and the response says so rather than hiding it: ARD
//! stations publish real schedules, and much of Radioplayer's German coverage
//! meets only the "Project Logo" minimum — names and logos, no `PI.xml`.

pub mod directory;
pub mod spi;

use std::time::Duration;

use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::Resolver;
use serde::Serialize;
use tracing::debug;

use crate::sources::egress::Egress;

/// Long enough for a slow broadcaster's server, short enough that a person
/// waiting on the answer is not left wondering.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// A whole German public broadcaster's `SI.xml` is 56 KB. Ten megabytes is
/// generous for any real one and stops a hostile one filling the Pi's memory.
const MAX_DOCUMENT: usize = 10 * 1024 * 1024;

/// A parsed bearer URI. Hex fields are held lower-case, which is how RadioDNS
/// names are published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bearer {
    Fm { gcc: String, pi: String, freq: String },
    Dab { gcc: String, eid: String, sid: String, scids: String },
}

fn hex(s: &str, lens: &[usize]) -> Option<String> {
    (lens.contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| s.to_ascii_lowercase())
}

impl Bearer {
    /// Parse `fm:…` or `dab:…`. The message on failure is what the caller sees.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (scheme, rest) = s
            .split_once(':')
            .ok_or("A bearer looks like fm:de0.d3c3.09440 or dab:de0.10d1.d3c3.0")?;
        let parts: Vec<&str> = rest.split('.').collect();
        match (scheme.to_ascii_lowercase().as_str(), parts.as_slice()) {
            ("fm", [gcc, pi, freq]) => {
                let gcc =
                    hex(gcc, &[3]).ok_or("The country code (gcc) is three hex digits, e.g. de0")?;
                let pi = hex(pi, &[4]).ok_or("The PI code is four hex digits, e.g. d3c3")?;
                if freq.len() != 5 || !freq.chars().all(|c| c.is_ascii_digit()) {
                    return Err(
                        "The frequency is five digits in 10 kHz steps: 94.4 MHz is 09440".into()
                    );
                }
                Ok(Bearer::Fm { gcc, pi, freq: freq.to_string() })
            }
            ("dab", [gcc, eid, sid, scids]) => {
                let gcc =
                    hex(gcc, &[3]).ok_or("The country code (gcc) is three hex digits, e.g. de0")?;
                let eid = hex(eid, &[4]).ok_or("The ensemble ID (EId) is four hex digits")?;
                let sid =
                    hex(sid, &[4, 8]).ok_or("The service ID (SId) is four or eight hex digits")?;
                let scids = hex(scids, &[1])
                    .ok_or("The component ID (SCIdS) is one hex digit, usually 0")?;
                Ok(Bearer::Dab { gcc, eid, sid, scids })
            }
            ("fm", _) => {
                Err("An FM bearer is fm:<gcc>.<pi>.<frequency>, e.g. fm:de0.d3c3.09440".into())
            }
            ("dab", _) => {
                Err("A DAB bearer is dab:<gcc>.<eid>.<sid>.<scids>, e.g. dab:de0.10d1.d3c3.0"
                    .into())
            }
            _ => Err("Only fm: and dab: bearers can be looked up".into()),
        }
    }

    /// The bearer URI in its canonical spelling.
    pub fn uri(&self) -> String {
        match self {
            Bearer::Fm { gcc, pi, freq } => format!("fm:{gcc}.{pi}.{freq}"),
            Bearer::Dab { gcc, eid, sid, scids } => format!("dab:{gcc}.{eid}.{sid}.{scids}"),
        }
    }

    /// The name RadioDNS publishes a CNAME on (TS 103 270 §5.1). Fully
    /// qualified, so no search domain is ever appended to it.
    pub fn radiodns_name(&self) -> String {
        match self {
            Bearer::Fm { gcc, pi, freq } => format!("{freq}.{pi}.{gcc}.fm.radiodns.org."),
            Bearer::Dab { gcc, eid, sid, scids } => {
                format!("{scids}.{sid}.{eid}.{gcc}.dab.radiodns.org.")
            }
        }
    }

    /// The bearer's part of a `PI.xml` path (TS 102 818 §5.1): the URI with
    /// its separators turned into slashes.
    pub fn spi_path(&self) -> String {
        match self {
            Bearer::Fm { gcc, pi, freq } => format!("fm/{gcc}/{pi}/{freq}"),
            Bearer::Dab { gcc, eid, sid, scids } => format!("dab/{gcc}/{eid}/{sid}/{scids}"),
        }
    }

    /// A bearer to fetch a schedule with, from a published one. `fm:…*` names
    /// no frequency, so it needs the listener's `mhz` (`"95.8"`).
    pub fn from_published(published: &str, mhz: Option<&str>) -> Option<Self> {
        if let Some(prefix) = published.strip_suffix(".*") {
            return Bearer::parse(&format!("{prefix}.{}", fm_code(mhz?)?)).ok();
        }
        Bearer::parse(published).ok()
    }

    /// Does a bearer id from an `SI.xml` name this one? An FM bearer there may
    /// give `*` for the frequency — every transmitter carrying the PI code.
    pub fn matches(&self, published: &str) -> bool {
        let Some((scheme, rest)) = published.trim().split_once(':') else { return false };
        let parts: Vec<String> = rest.split('.').map(str::to_ascii_lowercase).collect();
        match (self, scheme.to_ascii_lowercase().as_str()) {
            (Bearer::Fm { gcc, pi, freq }, "fm") => {
                parts.len() == 3
                    && parts[0] == *gcc
                    && parts[1] == *pi
                    && (parts[2] == *freq || parts[2] == "*")
            }
            (Bearer::Dab { gcc, eid, sid, scids }, "dab") => {
                parts.len() == 4
                    && parts[0] == *gcc
                    && parts[1] == *eid
                    && parts[2] == *sid
                    && parts[3] == *scids
            }
            _ => false,
        }
    }
}

/// `"95.8"` → `"09580"`, the bearer's frequency in 10 kHz steps. Only the FM
/// broadcast band (87.5–108 MHz) is accepted.
pub fn fm_code(mhz: &str) -> Option<String> {
    let f: f64 = mhz.trim().replace(',', ".").parse().ok()?;
    (87.5..=108.0).contains(&f).then(|| format!("{:05}", (f * 100.0).round() as u32))
}

/// A DAB channel block as printed on a radio: `5A`–`13F`. Band III only —
/// the blocks every German multiplex uses.
pub fn dab_channel(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_uppercase();
    let (num, letter) = s.split_at(s.len().checked_sub(1)?);
    let n: u8 = num.parse().ok()?;
    let ok = match n {
        5..=12 => matches!(letter, "A" | "B" | "C" | "D"),
        13 => matches!(letter, "A" | "B" | "C" | "D" | "E" | "F"),
        _ => false,
    };
    ok.then_some(s)
}

/// Why a lookup produced no answer.
#[derive(Debug)]
pub enum LookupError {
    /// RadioDNS has no record for this bearer: not a RadioDNS station, or the
    /// identifiers are wrong.
    NotInRadioDns(String),
    /// The station is in RadioDNS but its broadcaster runs no SPI service.
    NoSpi { fqdn: String },
    /// DNS itself failed — not "no such record", a failure to ask.
    Dns(String),
    /// The SPI server named in DNS could not be fetched or did not answer
    /// with SPI.
    Fetch(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::NotInRadioDns(name) => write!(f, "RadioDNS has no record for {name}"),
            LookupError::NoSpi { fqdn } => {
                write!(f, "{fqdn} is in RadioDNS but publishes no service or programme information")
            }
            LookupError::Dns(e) => write!(f, "DNS lookup failed: {e}"),
            LookupError::Fetch(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LookupError {}

/// Whether a day's schedule came back, in words a client can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// `PI.xml` was there and parsed; `programmes` is the day (possibly empty).
    Published,
    /// The broadcaster's server answered 404: names and logos only.
    NotPublished,
    /// The fetch or the parse failed; `programmes` is empty and says nothing.
    Unavailable,
}

/// Everything one lookup found.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct StationLookup {
    /// The bearer looked up, in canonical spelling.
    pub bearer: String,
    /// The broadcaster RadioDNS pointed at, e.g. `demdr.radiodns.ard.de`.
    pub broadcaster: String,
    /// The SPI server the documents came from.
    pub spi_server: String,
    /// The broadcaster's own name for itself, when `SI.xml` gives one.
    pub provider: Option<spi::Names>,
    /// This bearer's service from `SI.xml`. Absent when the broadcaster's list
    /// does not carry it, which happens: the schedule is still tried.
    pub service: Option<spi::Service>,
    /// The day asked for, `YYYY-MM-DD`.
    pub date: String,
    pub schedule_status: ScheduleStatus,
    pub programmes: Vec<spi::Programme>,
}

fn resolver() -> Result<hickory_resolver::TokioResolver, LookupError> {
    Resolver::builder_tokio().and_then(|b| b.build()).map_err(|e| LookupError::Dns(e.to_string()))
}

/// The broadcaster RadioDNS names for a bearer — the CNAME target.
async fn broadcaster_for(bearer: &Bearer) -> Result<String, LookupError> {
    let name = bearer.radiodns_name();
    let cname = match resolver()?.lookup(name.as_str(), RecordType::CNAME).await {
        Ok(l) => l.answers().iter().find_map(|r| match &r.data {
            RData::CNAME(c) => Some(c.0.to_utf8()),
            _ => None,
        }),
        Err(e) if e.is_no_records_found() || e.is_nx_domain() => None,
        Err(e) => return Err(LookupError::Dns(e.to_string())),
    };
    cname
        .map(|c| c.trim_end_matches('.').to_string())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| LookupError::NotInRadioDns(bearer.uri()))
}

/// A broadcaster's SPI server, as a base URL, from its `_radioepg._tcp` SRV
/// record.
pub async fn spi_server(fqdn: &str) -> Result<String, LookupError> {
    let srv_name = format!("_radioepg._tcp.{}.", fqdn.trim_end_matches('.'));
    let mut targets: Vec<(u16, u16, String, u16)> =
        match resolver()?.lookup(srv_name.as_str(), RecordType::SRV).await {
            Ok(l) => l
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    RData::SRV(s) => Some((s.priority, s.weight, s.target.to_utf8(), s.port)),
                    _ => None,
                })
                .collect(),
            Err(e) if e.is_no_records_found() || e.is_nx_domain() => Vec::new(),
            Err(e) => return Err(LookupError::Dns(e.to_string())),
        };
    // Lowest priority first, then the heaviest weight. Weighted random choice
    // is for spreading load across many clients; one household is not that.
    targets.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let no_spi = || LookupError::NoSpi { fqdn: fqdn.to_string() };
    let (_, _, host, port) = targets.into_iter().next().ok_or_else(no_spi)?;
    // "." is the SRV way of saying the service is deliberately not offered.
    let host = host.trim_end_matches('.').to_string();
    if host.is_empty() {
        return Err(no_spi());
    }
    Ok(spi_base(&host, port))
}

fn spi_base(host: &str, port: u16) -> String {
    match port {
        443 => format!("https://{host}"),
        80 => format!("http://{host}"),
        p => format!("http://{host}:{p}"),
    }
}

/// GET a document, refusing one larger than [`MAX_DOCUMENT`]. `Ok(None)` is
/// a 404 — the one failure that means something here.
async fn fetch(
    client: &reqwest::Client,
    egress: Egress,
    url: &str,
) -> Result<Option<String>, String> {
    egress.check_url(url).map_err(|e| e.to_string())?;
    let mut resp =
        client.get(url).send().await.map_err(|e| format!("Could not reach {url}: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("{url} answered {}", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("Reading {url}: {e}"))? {
        if body.len() + chunk.len() > MAX_DOCUMENT {
            return Err(format!("{url} is larger than any SPI document should be"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}

/// Look a bearer up and fetch its service information and the schedule for
/// `date`. `egress` is the caller's — the SPI server is named by DNS, which
/// is somebody else's data, so a member's lookup must not be steerable at the
/// LAN.
pub async fn lookup(
    bearer: &Bearer,
    date: chrono::NaiveDate,
    egress: Egress,
) -> Result<StationLookup, LookupError> {
    let fqdn = broadcaster_for(bearer).await?;
    let base = spi_server(&fqdn).await?;
    let client = egress.client(HTTP_TIMEOUT);

    let si = match service_information(&client, egress, &base).await? {
        Some(si) => si,
        // An SRV record with no SI.xml behind it is a broadcaster whose SPI
        // service is a placeholder; say that rather than "fetch failed".
        None => return Err(LookupError::NoSpi { fqdn }),
    };
    let service = si.services.into_iter().find(|s| s.bearers.iter().any(|b| bearer.matches(&b.id)));
    let (schedule_status, programmes) = schedule(&client, egress, &base, bearer, date).await;

    Ok(StationLookup {
        bearer: bearer.uri(),
        broadcaster: fqdn,
        spi_server: base,
        provider: si.provider,
        service,
        date: date.format("%Y-%m-%d").to_string(),
        schedule_status,
        programmes,
    })
}

/// A broadcaster's `SI.xml`. `Ok(None)` when the server answers 404.
pub async fn service_information(
    client: &reqwest::Client,
    egress: Egress,
    base: &str,
) -> Result<Option<spi::ServiceInformation>, LookupError> {
    let url = format!("{base}/radiodns/spi/3.1/SI.xml");
    match fetch(client, egress, &url).await {
        Ok(Some(xml)) => {
            spi::parse_si(&xml).map(Some).map_err(|e| LookupError::Fetch(format!("{url}: {e}")))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(LookupError::Fetch(e)),
    }
}

/// One day's `PI.xml` for a bearer. Never an error: a schedule is extra, and
/// what went wrong is in the status.
pub async fn schedule(
    client: &reqwest::Client,
    egress: Egress,
    base: &str,
    bearer: &Bearer,
    date: chrono::NaiveDate,
) -> (ScheduleStatus, Vec<spi::Programme>) {
    let url =
        format!("{base}/radiodns/spi/3.1/{}/{}_PI.xml", bearer.spi_path(), date.format("%Y%m%d"));
    match fetch(client, egress, &url).await {
        Ok(Some(xml)) => match spi::parse_pi(&xml) {
            Ok(p) => (ScheduleStatus::Published, p),
            Err(e) => {
                debug!("📻 {url}: {e}");
                (ScheduleStatus::Unavailable, Vec::new())
            }
        },
        Ok(None) => (ScheduleStatus::NotPublished, Vec::new()),
        Err(e) => {
            debug!("📻 {e}");
            (ScheduleStatus::Unavailable, Vec::new())
        }
    }
}

/// The client every RadioDNS document is fetched with, for `egress`.
pub fn client(egress: Egress) -> reqwest::Client {
    egress.client(HTTP_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fm_bearers_parse_and_spell_their_radiodns_name() {
        let b = Bearer::parse("fm:DE0.D3C3.09440").unwrap();
        assert_eq!(b.uri(), "fm:de0.d3c3.09440");
        assert_eq!(b.radiodns_name(), "09440.d3c3.de0.fm.radiodns.org.");
        assert_eq!(b.spi_path(), "fm/de0/d3c3/09440");
    }

    #[test]
    fn dab_bearers_reverse_their_fields_for_dns_but_not_for_the_path() {
        let b = Bearer::parse("dab:de0.10d1.d3c3.0").unwrap();
        assert_eq!(b.radiodns_name(), "0.d3c3.10d1.de0.dab.radiodns.org.");
        assert_eq!(b.spi_path(), "dab/de0/10d1/d3c3/0");
        assert!(
            Bearer::parse("dab:de0.10d1.1234abcd.0").is_ok(),
            "an eight-digit SId is a data service"
        );
    }

    #[test]
    fn malformed_bearers_say_what_is_wrong() {
        for bad in [
            "",
            "d3c3",
            "am:de0.d3c3.09440",
            "fm:de0.d3c3",
            "fm:de.d3c3.09440",
            "fm:de0.d3c.09440",
            "fm:de0.d3c3.94.4",
            "fm:de0.d3c3.944",
            "fm:de0.zzzz.09440",
            "dab:de0.10d1.d3c3",
            "dab:de0.10d1.d3c3.00",
            // Nothing here can become a different DNS name.
            "fm:de0.d3c3.09440.evil.example",
            "fm:de0.d3c3.0944/",
        ] {
            assert!(Bearer::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_si_bearer_matches_by_identity_and_a_star_matches_any_frequency() {
        let fm = Bearer::parse("fm:de0.d3c3.09440").unwrap();
        assert!(fm.matches("fm:de0.d3c3.*"));
        assert!(fm.matches("fm:DE0.D3C3.09440"));
        assert!(!fm.matches("fm:de0.d3c3.10120"));
        assert!(!fm.matches("fm:de0.d3c2.*"));
        assert!(!fm.matches("dab:de0.10d1.d3c3.0"));
        assert!(!fm.matches("http://mdr-284310-0.sslcast.mdr.de/mdr/284310/0/mp3/low/stream.mp3"));

        let dab = Bearer::parse("dab:de0.10d1.d3c3.0").unwrap();
        assert!(dab.matches("dab:de0.10d1.d3c3.0"));
        assert!(!dab.matches("dab:de0.10e5.d3c3.0"));
    }

    /// The fixture's KULTUR service is found from either band.
    #[test]
    fn a_lookup_finds_its_own_service_in_a_real_si_document() {
        let si = spi::parse_si(include_str!("fixtures/mdr_SI.xml")).unwrap();
        for uri in ["fm:de0.d3c3.09440", "dab:de0.1135.d3c3.0"] {
            let b = Bearer::parse(uri).unwrap();
            let found = si.services.iter().find(|s| s.bearers.iter().any(|x| b.matches(&x.id)));
            assert_eq!(found.and_then(|s| s.names.medium.as_deref()), Some("MDR KULTUR"), "{uri}");
        }
    }

    #[test]
    fn dial_positions_parse_the_way_a_radio_prints_them() {
        assert_eq!(fm_code("95.8").as_deref(), Some("09580"));
        assert_eq!(fm_code("100,6").as_deref(), Some("10060"));
        assert_eq!(fm_code("87.5").as_deref(), Some("08750"));
        assert_eq!(fm_code("120"), None);
        assert_eq!(fm_code("abc"), None);
        assert_eq!(dab_channel("5c").as_deref(), Some("5C"));
        assert_eq!(dab_channel("13F").as_deref(), Some("13F"));
        assert_eq!(dab_channel("12E"), None);
        assert_eq!(dab_channel("4A"), None);
        assert_eq!(dab_channel(""), None);
    }

    #[test]
    fn a_published_wildcard_needs_the_listeners_frequency() {
        assert_eq!(
            Bearer::from_published("fm:de0.d3c3.*", Some("94.4")).unwrap().uri(),
            "fm:de0.d3c3.09440"
        );
        assert!(Bearer::from_published("fm:de0.d3c3.*", None).is_none());
        assert_eq!(
            Bearer::from_published("dab:de0.10d1.d3c3.0", None).unwrap().uri(),
            "dab:de0.10d1.d3c3.0"
        );
    }

    #[test]
    fn the_spi_base_drops_default_ports() {
        assert_eq!(spi_base("demdr.radiodns.ard.de", 80), "http://demdr.radiodns.ard.de");
        assert_eq!(spi_base("spi.example", 443), "https://spi.example");
        assert_eq!(spi_base("spi.example", 8080), "http://spi.example:8080");
    }
}
