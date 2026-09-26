//! AudioMuse-AI — **amai** — the sonic sidecar.
//!
//! amai analyses the actual audio files in a media server library and stores,
//! per *recording*: a 200-dimension MusiCNN embedding, a 512-dimension DCLAP
//! embedding, tempo, key, scale, energy, a top-N mood vector, and six
//! continuous axes (`danceable`, `aggressive`, `happy`, `party`, `relaxed`,
//! `sad`) computed as the cosine between the track's CLAP audio embedding and
//! cached CLAP *text* embeddings of those labels.
//!
//! **That is the one thing this application cannot derive from its own data**,
//! and two roadmap items are explicitly blocked on it. `playlist_lab` says so
//! in its own module docs — its six Sound axes come from Every Noise via the
//! track's *genres*, genres attach to **artists**, so every track by one artist
//! shares its coordinates exactly and nearest-neighbour search in Sound space
//! returns that artist's own tracks first. The vision doc's manifold view says
//! it harder: *"needs an embedding per track that Tapedeck does not have and
//! cannot derive from its own data. Do not start this without settling where
//! the vectors come from."*
//!
//! amai settles it, and better than the agreed-but-never-built AcousticBrainz
//! plan would have: AcousticBrainz stopped accepting submissions in 2022, so
//! its coverage of a mostly Korean/OST catalogue would be thin, where amai
//! analyses *this* library and its coverage is therefore whatever is on the
//! shelf.
//!
//! ## What this module is, and what it is deliberately not
//!
//! It is the **talking layer only**. Nothing here reads a vector into a
//! feature, because no feature consumes one yet — and shipping a client method
//! nothing invokes is the exact pattern recorded seven times under *"Three
//! features shipped switched off"*. [`Amai::score`], [`Amai::servers`] and
//! [`Amai::reachable`] each have a caller today; `get_embedding`,
//! `similar_tracks`, `similar_artists`, `find_path` and `alchemy` are ~15 lines
//! each and land with the feature that needs them, not before.
//!
//! ## The join key, which is the pleasant surprise
//!
//! Name matching was the expected cost and it is not needed.
//! `app_external.py::_resolve_external_id` resolves a caller-supplied id
//! through their `resolve_input_item_id`, and a `?server=` parameter (their
//! display **name** or internal id; absent means their default server) says
//! which media server that id belongs to. So the call is
//! `GET /external/get_score?id=<plex ratingKey>&server=<name>`.
//!
//! `server::artwork::remote_item_id` already parses exactly that id out of
//! `scrobbles.source_id`, built in v0.50.0 for the artwork proxy and reusable
//! here unchanged. That sidesteps the ceiling the TIDAL work hit: 57% of this
//! history carries no recording MBID, so `isrc_map` can never cover more than
//! 43% of it — and none of that matters to a join on a provider id.
//!
//! ## Three things about talking to it
//!
//! * **`/api/health` is outside their auth barrier.** `app_auth.py`'s single
//!   `before_request` guard exempts `/static/` and `/api/health` and nothing
//!   else, so *reachable* and *credentialled* are two separately answerable
//!   questions. That is the inverse of the Discogs trap, where a bad credential
//!   is served as a normal 200 and a typo is indistinguishable from success:
//!   here a bad token is a clean 401 and it is **reachability** that needs its
//!   own step. [`Amai::reachable`] is that step.
//! * **A miss is an answer; unreachable is not.** A 404 from `get_score` means
//!   amai has not analysed that track, which is a fact about the library.
//!   Folding that into the same value as "the box is off" is the defect this
//!   codebase has now found five times — `needs_setup`'s `unwrap_or(true)`
//!   telling an owner their history was gone, the fidelity cards scoring "we
//!   never knew" as "not lossless", `LyricsClient::coverage` reporting a broken
//!   cache as zero tracks. Hence [`Lookup`], which is not an `Option`.
//! * **No SSRF guard, deliberately.** The whole point of this address is that
//!   it is a private LAN box, it is typed by an admin, and it is the same trust
//!   this application already extends to every `user_sources` URL. `is_private_host`
//!   exists and must **not** be reached for here — applying it would refuse the
//!   only addresses that can work.
//!
//! ## Both projects are AGPL-3.0-only
//!
//! So there is no licence friction in either direction, including for a future
//! amai *plugin* written on this side. Nothing is vendored either way today.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The breaker key and the label Settings → Operations shows.
///
/// It goes through `sinks::health` like MusicBrainz and the Cover Art Archive
/// do — that registry is keyed by name and knows nothing about what kind of
/// service it holds, and a second one would be a second answer to "what
/// outbound service is failing" with Operations having to read both.
pub const SERVICE: &str = "AudioMuse-AI";

/// `app_settings` keys. Server-wide and admin-set, like Discogs and the TIDAL
/// registration: it is the *operator's* sidecar, not a per-user connection.
pub const URL_KEY: &str = "amai_url";
pub const TOKEN_KEY: &str = "amai_token";
pub const SERVER_KEY: &str = "amai_server";

/// Their `before_request` barrier exempts exactly this path, so it answers
/// without a credential.
const HEALTH_PATH: &str = "/api/health";

/// A LAN box either answers or it is not there; five seconds is already
/// generous. Separate from the request timeout because a blackholed route
/// otherwise holds a lookup for the whole of it — the v0.102.0 finding.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The figure `lb_get_page` already uses. These are indexed reads on their
/// side, not analysis, so nothing here should take anywhere near it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether amai has an answer, kept apart from whether it could be asked.
///
/// `Result<Lookup<T>>` reads exactly as the two questions do: `Err` is *could
/// not ask*, `Ok(NotAnalysed)` is *asked, and it has not analysed that track*.
/// An `Option` would collapse them, and the collapsed version is wrong in the
/// direction that matters — an operator told "amai has none of your library"
/// by a box that is simply switched off would go looking for the wrong problem.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup<T> {
    Found(T),
    NotAnalysed,
}

/// One of amai's configured media servers, as `/api/servers` reports it.
///
/// **Only these three fields are read.** That response also carries a `creds`
/// map — masked, but still theirs — and `music_libraries`. Nothing here stores
/// or logs either: a credential belonging to another application has no
/// business crossing into this one even in masked form.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AmaiServer {
    pub server_id: String,
    pub name: String,
    #[serde(default)]
    pub server_type: String,
    #[serde(default)]
    pub is_default: bool,
}

/// The `score` row: everything amai knows about a recording that is not a raw
/// vector.
///
/// `mood_vector` and `other_features` arrive as **text**, not as JSON objects —
/// their schema declares both `TEXT` — so they are carried verbatim rather than
/// parsed into a shape this side has invented. The first consumer can decide
/// what to make of them; guessing now would be a schema nobody agreed to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Their column for the artist is `author`.
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub tempo: Option<f64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub scale: Option<String>,
    #[serde(default)]
    pub energy: Option<f64>,
    #[serde(default)]
    pub mood_vector: Option<String>,
    #[serde(default)]
    pub other_features: Option<String>,
}

/// Where an instance is and how to talk to it.
#[derive(Debug, Clone)]
pub struct Amai {
    base: String,
    token: Option<String>,
    /// Which of amai's media servers our provider ids belong to. `None` means
    /// their default server, which is the whole answer on a single-server
    /// install.
    server: Option<String>,
    http: reqwest::Client,
}

/// Trim a trailing slash so `join`-free path concatenation cannot produce `//`.
fn normalise_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

impl Amai {
    pub fn new(url: &str, token: Option<String>, server: Option<String>) -> Result<Self> {
        let base = normalise_base(url);
        if base.is_empty() {
            return Err(anyhow!("AudioMuse-AI has no address configured"));
        }
        if !base.starts_with("http://") && !base.starts_with("https://") {
            return Err(anyhow!(
                "AudioMuse-AI's address must start with http:// or https:// — got {base}"
            ));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            base,
            token: token.filter(|t| !t.trim().is_empty()),
            server: server.filter(|s| !s.trim().is_empty()),
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn server(&self) -> Option<&str> {
        self.server.as_deref()
    }

    /// Every request goes through here, so the breaker, the bearer header and
    /// the cause chain are stated once rather than at each call site.
    ///
    /// The same shape as `musicbrainz::api_get`, and for the same reason it was
    /// made "one door" in v0.102.0: five call sites each doing their own header
    /// and their own error formatting is five places to remember a rule and one
    /// to forget it.
    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Response> {
        if let Some(left) = crate::sinks::health().skip_for(SERVICE) {
            return Err(anyhow!("{SERVICE} is being skipped for another {}s", left.as_secs()));
        }
        let mut req = self.http.get(format!("{}{path}", self.base));
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        match req.send().await {
            Ok(resp) => {
                crate::sinks::health().record_ok(SERVICE);
                Ok(resp)
            }
            Err(e) => {
                // The cause chain, never bare Display. `error sending request
                // for url (...)` is what reqwest says for a refused connection,
                // a DNS failure and a TLS handshake the peer cut off alike —
                // two days of logs said nothing that could tell them apart.
                let detail = crate::logging::error_chain(&e);
                crate::sinks::health().record_failure(SERVICE, &detail);
                Err(anyhow!("{detail}"))
            }
        }
    }

    /// Is anything there at all?
    ///
    /// Unauthenticated on purpose — see the module docs. A success here beside
    /// a 401 from [`Amai::servers`] is precisely "running, and the token is
    /// wrong", which is a different thing to tell an operator than "not
    /// running".
    pub async fn reachable(&self) -> Result<()> {
        let resp = self.get(HEALTH_PATH, &[]).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("AudioMuse-AI answered {status} at {HEALTH_PATH}"));
        }
        Ok(())
    }

    /// The media servers amai has configured.
    ///
    /// Doubles as the credential check, and it is the one call that says
    /// whether the `server` setting matters at all: with one server configured
    /// an absent `?server=` is unambiguous, and with two an unmatched id is
    /// indistinguishable from an unanalysed one.
    pub async fn servers(&self) -> Result<Vec<AmaiServer>> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            servers: Vec<AmaiServer>,
        }
        let resp = self.get("/api/servers", &[]).await?;
        self.decode(resp, "/api/servers").await.map(|w: Wire| w.servers)
    }

    /// Everything amai has computed for one media-server track id.
    ///
    /// The id is the caller's own provider id — a Plex `ratingKey`, a Jellyfin
    /// item id, a Navidrome id — exactly as `remote_item_id` parses it out of a
    /// listen.
    pub async fn score(&self, provider_track_id: &str) -> Result<Lookup<Score>> {
        let mut query: Vec<(&str, &str)> = vec![("id", provider_track_id)];
        if let Some(s) = &self.server {
            query.push(("server", s));
        }
        let resp = self.get("/external/get_score", &query).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            // An answer: amai is running and has not analysed this track.
            return Ok(Lookup::NotAnalysed);
        }
        self.decode(resp, "/external/get_score").await.map(Lookup::Found)
    }

    /// Status check and JSON decode, with the two failures kept apart.
    ///
    /// A 401 is reported as a credential problem in words rather than as a bare
    /// status, because that is the one an operator can act on and the message
    /// is what the Settings panel shows them.
    async fn decode<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
        path: &str,
    ) -> Result<T> {
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow!(
                "AudioMuse-AI rejected the API token ({status}). Check API_TOKEN on that instance."
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "AudioMuse-AI answered {status} at {path}: {}",
                body.chars().take(200).collect::<String>()
            ));
        }
        resp.json::<T>()
            .await
            .map_err(|e| anyhow!("AudioMuse-AI sent something unreadable at {path}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_without_a_scheme_is_refused_rather_than_guessed_at() {
        // Guessing http:// would work on a LAN and silently downgrade an
        // instance the operator meant to reach over TLS.
        assert!(Amai::new("192.168.178.168:8000", None, None).is_err());
        assert!(Amai::new("", None, None).is_err());
        assert!(Amai::new("http://192.168.178.168:8000", None, None).is_ok());
    }

    #[test]
    fn a_trailing_slash_does_not_become_a_double_one() {
        let a = Amai::new("http://box:8000/", None, None).unwrap();
        assert_eq!(a.base_url(), "http://box:8000");
    }

    /// A blank string in `app_settings` must read as "not set", not as a token
    /// of zero length that then goes out as `Authorization: Bearer `.
    #[test]
    fn a_blank_credential_is_no_credential() {
        let a = Amai::new("http://box:8000", Some("   ".into()), Some("".into())).unwrap();
        assert!(!a.has_token());
        assert!(a.server().is_none());
    }

    /// The distinction the whole module turns on. A `Lookup` that could be
    /// built from a failure would put "amai has none of your library" and "amai
    /// is switched off" in one value, which is the shape that told an instance
    /// owner their history was gone in v0.81.0.
    #[test]
    fn not_analysed_is_an_answer_and_not_a_failure() {
        let miss: Lookup<Score> = Lookup::NotAnalysed;
        assert!(!matches!(miss, Lookup::Found(_)));
        // There is deliberately no `From<Error> for Lookup`, and no
        // `Lookup::Unknown`: the error path is the `Result`.
    }
}
