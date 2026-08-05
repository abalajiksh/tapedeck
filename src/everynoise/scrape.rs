//! The scrape job.
//!
//! Every Noise at Once has been **frozen since December 2023** and is run by
//! one person. Two consequences shape everything here:
//!
//! * There is nothing to sync. This runs when an admin asks it to and never on
//!   a schedule — a cron against static data would be pure waste aimed at
//!   someone else's server.
//! * Politeness is not optional. One request per second, a `User-Agent` that
//!   says who is asking, and a resume marker so a restart never re-asks for
//!   pages already held.
//!
//! Two tiers, and the difference in cost is the reason they are separate
//! buttons rather than one job:
//!
//! | Tier | Requests | Gets |
//! |---|---|---|
//! | 1 | **1** | all ~6,300 genres with coordinates, colour, prominence |
//! | 2 | ~6,300 | artists per genre with Spotify ids, and the neighbour graph |
//!
//! Tier 1 is a single page load, which is why it is the default and why the
//! genre-level features are worth building on first. Tier 2 is roughly two
//! hours of steady, slow requests.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::parse;
use super::store::EveryNoiseStore;

const BASE: &str = "https://everynoise.com/";

/// One request per second, matching the reference scraper. Not tunable from
/// the UI on purpose: the only reason to raise it is to be less polite.
const REQUEST_DELAY: Duration = Duration::from_secs(1);
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// The genre map. One request.
    Map,
    /// Every genre's page. ~6,300 requests.
    Pages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Done,
    Failed,
    /// Asked to stop; the walk ends after the request in flight.
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    pub tier: Tier,
    pub state: State,
    pub done: i64,
    pub total: i64,
    /// The genre currently being fetched, for a UI that would otherwise show a
    /// bar creeping for two hours with nothing to read.
    pub current: Option<String>,
    pub message: Option<String>,
    pub started_at: i64,
}

/// Live progress for the one scrape that may be running.
///
/// Deliberately a single slot rather than a map of jobs: this is bulk reference
/// data with one copy per instance, so two concurrent scrapes would only
/// contend for the same rows and double the load on the site.
#[derive(Clone, Default)]
pub struct ScrapeRegistry {
    inner: Arc<RwLock<Option<Progress>>>,
}

impl ScrapeRegistry {
    pub async fn get(&self) -> Option<Progress> {
        self.inner.read().await.clone()
    }

    pub async fn is_running(&self) -> bool {
        matches!(self.inner.read().await.as_ref().map(|p| p.state), Some(State::Running))
    }

    async fn set(&self, p: Progress) {
        *self.inner.write().await = Some(p);
    }

    async fn update(&self, f: impl FnOnce(&mut Progress)) {
        if let Some(p) = self.inner.write().await.as_mut() {
            f(p);
        }
    }

    /// Ask a running walk to stop. It ends after the request in flight rather
    /// than aborting mid-write.
    pub async fn cancel(&self) {
        self.update(|p| {
            if p.state == State::Running {
                p.state = State::Cancelled;
            }
        })
        .await;
    }

    async fn cancelled(&self) -> bool {
        matches!(self.inner.read().await.as_ref().map(|p| p.state), Some(State::Cancelled))
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A `User-Agent` that says who is asking and why.
///
/// The contact address is required rather than defaulted — the same rule
/// MusicBrainz enforces and this codebase already follows for it. An anonymous
/// scraper hitting a frozen personal site 6,300 times is exactly the thing that
/// gets a project blocked, and there is no sensible fallback value: a made-up
/// address is worse than none.
fn user_agent(contact: &str) -> String {
    format!(
        "Tapedeck/{} (self-hosted music journal; one-time archival read of a frozen site; contact: {})",
        env!("CARGO_PKG_VERSION"),
        contact
    )
}

fn client(contact: &str) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder().timeout(TIMEOUT).user_agent(user_agent(contact)).build()
}

/// Fetch one page, retrying on transport errors and 5xx with a widening gap.
///
/// A 404 is returned as `Ok(None)`: a genre whose page has gone is a fact to
/// record and move past, not a reason to abandon a two-hour walk.
async fn fetch(
    http: &reqwest::Client,
    url: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut backoff = Duration::from_secs(5);
    for attempt in 1..=MAX_RETRIES {
        match http.get(url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(Some(resp.text().await?)),
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => return Ok(None),
            Ok(resp) if resp.status().is_client_error() => {
                // 4xx other than 404 will not improve by asking again, and
                // hammering after a 403 is how a scrape becomes a block.
                return Err(format!("{} returned {}", url, resp.status()).into());
            }
            Ok(resp) => {
                warn!("Every Noise {} returned {} (attempt {})", url, resp.status(), attempt);
            }
            Err(e) => warn!("Every Noise {} failed: {} (attempt {})", url, e, attempt),
        }
        if attempt < MAX_RETRIES {
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }
    }
    Err(format!("{} failed after {} attempts", url, MAX_RETRIES).into())
}

/// Tier 1 — the genre map, in one request.
pub async fn run_map(
    store: EveryNoiseStore,
    registry: ScrapeRegistry,
    contact: String,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    registry
        .set(Progress {
            tier: Tier::Map,
            state: State::Running,
            done: 0,
            total: 1,
            current: Some("genre map".into()),
            message: None,
            started_at: now(),
        })
        .await;

    let http = client(&contact)?;
    let url = format!("{}engenremap.html", BASE);
    let html = match fetch(&http, &url).await {
        Ok(Some(h)) => h,
        Ok(None) => {
            let msg = "the genre map is no longer at that address".to_string();
            registry
                .update(|p| {
                    p.state = State::Failed;
                    p.message = Some(msg.clone());
                })
                .await;
            return Err(msg.into());
        }
        Err(e) => {
            let msg = e.to_string();
            registry
                .update(|p| {
                    p.state = State::Failed;
                    p.message = Some(msg.clone());
                })
                .await;
            return Err(e);
        }
    };

    let genres = parse::genre_map(&html);
    if genres.is_empty() {
        // Parsing nothing out of a 200 means the page is not what we expect.
        // Writing that result would empty the table, so refuse instead.
        let msg =
            "fetched the page but found no genres in it — the markup may have changed".to_string();
        registry
            .update(|p| {
                p.state = State::Failed;
                p.message = Some(msg.clone());
            })
            .await;
        return Err(msg.into());
    }

    let n = store.put_genres(&genres).await?;
    info!("🗺️ Every Noise: stored {} genres", n);
    registry
        .update(|p| {
            p.state = State::Done;
            p.done = 1;
            p.current = None;
            p.message = Some(format!("{} genres", n));
        })
        .await;
    Ok(n)
}

/// Tier 2 — every genre's page, one per second, resuming where it left off.
pub async fn run_pages(
    store: EveryNoiseStore,
    registry: ScrapeRegistry,
    contact: String,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let pending = store.pending_pages().await?;
    let already = store.summary().await?.pages_scraped;

    registry
        .set(Progress {
            tier: Tier::Pages,
            state: State::Running,
            done: already,
            total: already + pending.len() as i64,
            current: None,
            message: None,
            started_at: now(),
        })
        .await;

    if pending.is_empty() {
        registry
            .update(|p| {
                p.state = State::Done;
                p.message = Some("every genre page is already held".into());
            })
            .await;
        return Ok(0);
    }

    let http = client(&contact)?;
    let mut written = 0u64;
    let mut failures = 0u64;

    for slug in pending {
        if registry.cancelled().await {
            info!("🗺️ Every Noise: walk cancelled after {} pages", written);
            registry.update(|p| p.message = Some(format!("stopped after {} pages", written))).await;
            return Ok(written);
        }
        registry.update(|p| p.current = Some(slug.clone())).await;

        let url = format!("{}engenremap-{}.html", BASE, slug);
        match fetch(&http, &url).await {
            Ok(Some(html)) => {
                let page = parse::genre_page(&html, &slug);
                store.put_genre_page(&slug, &page.artists, &page.neighbours).await?;
                written += 1;
            }
            // Gone, or unparseable. Mark it done with nothing in it so resume
            // moves past rather than retrying it every run forever.
            Ok(None) => {
                store.put_genre_page(&slug, &[], &[]).await?;
                written += 1;
            }
            Err(e) => {
                failures += 1;
                warn!("Every Noise: giving up on {}: {}", slug, e);
                // A run of failures means something systemic — a block, or the
                // site being down. Continuing would be 6,000 more requests at
                // someone who has already said no.
                if failures >= 10 {
                    let msg = format!("stopped after {} consecutive failures", failures);
                    registry
                        .update(|p| {
                            p.state = State::Failed;
                            p.message = Some(msg.clone());
                        })
                        .await;
                    return Err(msg.into());
                }
            }
        }

        registry
            .update(|p| {
                p.done += 1;
            })
            .await;
        tokio::time::sleep(REQUEST_DELAY).await;
    }

    info!("🗺️ Every Noise: walked {} genre pages", written);
    registry
        .update(|p| {
            p.state = State::Done;
            p.current = None;
            p.message = Some(format!("{} genre pages", written));
        })
        .await;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address is in the User-Agent so the site owner can see who is
    /// asking. If this stops carrying it, the scrape becomes anonymous traffic.
    #[test]
    fn the_user_agent_names_a_contact() {
        let ua = user_agent("someone@example.com");
        assert!(ua.contains("someone@example.com"));
        assert!(ua.starts_with("Tapedeck/"));
    }

    #[tokio::test]
    async fn cancelling_only_affects_a_running_walk() {
        let r = ScrapeRegistry::default();
        // Nothing running: cancel must not invent a job.
        r.cancel().await;
        assert!(r.get().await.is_none());

        r.set(Progress {
            tier: Tier::Pages,
            state: State::Running,
            done: 0,
            total: 10,
            current: None,
            message: None,
            started_at: 0,
        })
        .await;
        assert!(r.is_running().await);
        r.cancel().await;
        assert!(r.cancelled().await);
        assert!(!r.is_running().await);

        // A finished job must not be reopened by a late cancel.
        r.update(|p| p.state = State::Done).await;
        r.cancel().await;
        assert_eq!(r.get().await.map(|p| p.state), Some(State::Done));
    }
}
