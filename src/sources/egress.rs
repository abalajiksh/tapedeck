//! Where a source may be pointed.
//!
//! A source URL is fetched **by the server** — on save, on test, on every poll,
//! and whenever the artwork proxy, a playlist push or a ratings pull reaches
//! the library. So an account that can set one can make the instance talk to
//! any address it can route to, and read back enough (an error class, a status,
//! a decoded body) to map the network it runs on. The Strix scan of v0.120.0
//! found exactly that from a member account, replica mode included.
//!
//! The rule is per owner, decided by **address, never by spelling**:
//!
//! * An admin's sources go anywhere. The admin runs the box and could reach
//!   its network from a shell; refusing them buys nothing and would break the
//!   ordinary household setup, a Plex on 192.168.x.
//! * A member's never reach the server itself, link-local (the cloud metadata
//!   range among it) or unroutable space — no setting changes that.
//! * A member's reach private space (RFC 1918, CGNAT, unique-local IPv6) only
//!   inside the networks the operator lists in `TAPEDECK_SOURCE_NETWORKS`.
//!   Empty by default: a member pointing at the LAN is the operator's call.
//!
//! A third rule, [`Egress::Stranger`], is for URLs nobody on this instance
//! chose: an actor document named by the `keyId` of an unauthenticated
//! federation delivery. It reaches the public internet only, over `https`
//! only, on every hop — td-security#8 was the Patch inbox applying those
//! guards to the first URL and then following redirects anywhere.
//!
//! Enforced twice. The handlers check up front so a refusal is a clear 400
//! instead of a connection error ([`Egress::check_url_resolved`]). The member
//! client re-checks at connect time — its resolver drops refused addresses and
//! its redirect policy refuses refused literals — which is what closes DNS
//! rebinding and a public server redirecting inward, and what covers rows
//! saved before the rule existed. IP-literal URLs never reach a resolver, so
//! every place that builds a client from a stored row also calls
//! [`Egress::check_url`] first.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ipnet::IpNet;
use tracing::warn;

static NETWORKS: OnceLock<Vec<IpNet>> = OnceLock::new();

/// Install the operator's allowed private networks. Called once from `main`;
/// an entry that does not parse is logged and skipped rather than widening
/// anything. A bare address is its own /32 or /128.
pub fn set_networks(entries: &[String]) {
    let parsed = entries
        .iter()
        .filter_map(|e| {
            let e = e.trim();
            let net = e.parse::<IpNet>().ok().or_else(|| e.parse::<IpAddr>().ok().map(IpNet::from));
            if net.is_none() {
                warn!("⚠️ TAPEDECK_SOURCE_NETWORKS: ignoring {e:?}, not a network or address");
            }
            net
        })
        .collect();
    let _ = NETWORKS.set(parsed);
}

fn networks() -> &'static [IpNet] {
    NETWORKS.get().map(Vec::as_slice).unwrap_or(&[])
}

/// Whose rules a source's traffic follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Egress {
    /// An admin's source: anywhere.
    Open,
    /// A member's source: the public internet, plus the operator's networks.
    Member,
    /// A URL a stranger chose — a federation `keyId`. The public internet
    /// only, `https` only, and no operator networks: nobody on this instance
    /// vouched for it.
    Stranger,
}

/// Why an address was refused. Its text is what the member sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

#[derive(Debug, PartialEq, Eq)]
enum Class {
    Public,
    /// Private, but an operator may allow it.
    Private,
    /// The server itself, link-local, multicast or unroutable.
    Never,
}

fn classify_v4(ip: Ipv4Addr) -> Class {
    let [a, b, c, _] = ip.octets();
    if ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 192 && b == 0 && c == 0)
    {
        return Class::Never;
    }
    // CGNAT is where Tailscale lives, and 198.18/15 is a lab range; both are
    // somebody's private network rather than the internet.
    if ip.is_private() || (a == 100 && (64..128).contains(&b)) || (a == 198 && (b & 0xfe) == 18) {
        return Class::Private;
    }
    Class::Public
}

fn classify(ip: IpAddr) -> Class {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => {
            // Every IPv6 spelling of an IPv4 address is judged as that address:
            // mapped (::ffff:a.b.c.d), NAT64 (64:ff9b::/96) and 6to4
            // (2002:AABB:CCDD::). Otherwise ::ffff:127.0.0.1 walks straight past.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return classify_v4(v4);
            }
            let s = v6.segments();
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return classify_v4(embedded_v4(s[6], s[7]));
            }
            if s[0] == 0x2002 {
                return classify_v4(embedded_v4(s[1], s[2]));
            }
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || s[..6] == [0; 6] // IPv4-compatible, deprecated
                || (s[0] & 0xffc0) == 0xfe80
            {
                return Class::Never;
            }
            // fc00::/7 unique-local, and the deprecated fec0::/10 site-local.
            if (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfec0 {
                return Class::Private;
            }
            Class::Public
        }
    }
}

fn embedded_v4(hi: u16, lo: u16) -> Ipv4Addr {
    Ipv4Addr::from(((hi as u32) << 16) | lo as u32)
}

/// The address a network list is matched against: a mapped IPv6 address is
/// the IPv4 one, so `192.168.178.0/24` covers `::ffff:192.168.178.20` too.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

const NEVER: &str =
    "A source can't point at this server, a link-local address or an unroutable one";
const STRANGER: &str = "A remote server can't send this server to a private address";
const PRIVATE: &str =
    "A source can't point at a private address unless an admin allows its network \
                       (TAPEDECK_SOURCE_NETWORKS)";

impl Egress {
    pub fn for_owner(is_admin: bool) -> Self {
        if is_admin {
            Egress::Open
        } else {
            Egress::Member
        }
    }

    /// May this owner's traffic reach `ip`?
    pub fn check_ip(self, ip: IpAddr) -> Result<(), Refusal> {
        if self == Egress::Open {
            return Ok(());
        }
        match classify(ip) {
            Class::Public => Ok(()),
            Class::Never => Err(Refusal(NEVER.into())),
            Class::Private if self == Egress::Stranger => Err(Refusal(STRANGER.into())),
            Class::Private if networks().iter().any(|n| n.contains(&canonical(ip))) => Ok(()),
            Class::Private => Err(Refusal(PRIVATE.into())),
        }
    }

    /// The checks that need no lookup: an http(s) scheme, and the address
    /// when the URL names one. A hostname passes here and is judged by the
    /// client's resolver at connect time.
    pub fn check_url(self, url: &str) -> Result<(), Refusal> {
        if self == Egress::Open {
            return Ok(());
        }
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| Refusal("That doesn't look like a server URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Refusal("A source URL has to be http or https".into()));
        }
        // Key material fetched in the clear is key material whoever is on the
        // path chose. A redirect from https to http is refused here too, which
        // is the downgrade td-security#8 found.
        if self == Egress::Stranger && parsed.scheme() != "https" {
            return Err(Refusal("A remote server has to be reached over https".into()));
        }
        match parsed.host_str().map(|h| h.trim_start_matches('[').trim_end_matches(']')) {
            // The URL parser has already normalised the integer, octal and
            // short spellings of an IPv4 address, so this sees 127.0.0.1 for
            // http://2130706433/.
            Some(host) => match host.parse::<IpAddr>() {
                Ok(ip) => self.check_ip(ip),
                Err(_) => Ok(()),
            },
            None => Err(Refusal("That doesn't look like a server URL".into())),
        }
    }

    /// [`check_url`](Self::check_url), then the hostname's addresses, so a
    /// save or a test is refused with a reason instead of failing to connect.
    ///
    /// Refused only when **every** address is: the client connects to the
    /// allowed ones and never the others. A name that does not resolve passes,
    /// so a typo still reads as an unreachable server.
    pub async fn check_url_resolved(self, url: &str) -> Result<(), Refusal> {
        self.check_url(url)?;
        if self == Egress::Open {
            return Ok(());
        }
        let Ok(parsed) = reqwest::Url::parse(url) else { return Ok(()) };
        let (Some(host), Some(port)) = (parsed.host_str(), parsed.port_or_known_default()) else {
            return Ok(());
        };
        self.check_host_resolved(host, port).await
    }

    /// The up-front check a save or a test makes for a source of `kind`. A
    /// Roon Core is named by `host[:port]`, everything else by a URL.
    pub async fn check_source(self, kind: &str, address: &str) -> Result<(), Refusal> {
        if kind != "roon" {
            return self.check_url_resolved(address).await;
        }
        // Parsed the way the engine parses it when it connects.
        let addr = address.trim().trim_start_matches("http://");
        let (host, port) = match addr.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().unwrap_or(9330)),
            None => (addr, 9330),
        };
        self.check_host_resolved(host, port).await
    }

    /// The same for a bare `host[:port]`, which is how a Roon Core is named.
    /// Roon talks over its own socket rather than this module's client, so
    /// this is the whole of its check.
    pub async fn check_host_resolved(self, host: &str, port: u16) -> Result<(), Refusal> {
        if self == Egress::Open {
            return Ok(());
        }
        let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
        if let Ok(ip) = host.parse::<IpAddr>() {
            return self.check_ip(ip);
        }
        let addrs: Vec<IpAddr> = match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(a) => a.map(|a| a.ip()).collect(),
            Err(_) => return Ok(()),
        };
        let results: Vec<_> = addrs.into_iter().map(|ip| self.check_ip(ip)).collect();
        if results.is_empty() || results.iter().any(Result::is_ok) {
            return Ok(());
        }
        results.into_iter().find_map(Result::err).map_or(Ok(()), Err)
    }

    /// The HTTP client every source request goes through. A member's resolves
    /// through [`GuardedResolver`] and follows a redirect only to an address
    /// it may reach.
    pub fn client(self, timeout: Duration) -> reqwest::Client {
        let builder = reqwest::Client::builder().timeout(timeout).user_agent(format!(
            "{}/{}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ));
        let builder = match self {
            Egress::Open => builder,
            // Every hop is judged by the same rule as the first: its scheme
            // and literal here, its resolved addresses by the resolver. A
            // stranger gets two hops — enough for a host that has moved, few
            // enough that a chain cannot be walked somewhere interesting.
            Egress::Member | Egress::Stranger => {
                let hops = if self == Egress::Stranger { 2 } else { 10 };
                builder.dns_resolver(Arc::new(GuardedResolver(self))).redirect(
                    reqwest::redirect::Policy::custom(move |attempt| {
                        if attempt.previous().len() >= hops {
                            return attempt.error("too many redirects");
                        }
                        match self.check_url(attempt.url().as_str()) {
                            Ok(()) => attempt.follow(),
                            Err(e) => attempt.error(e),
                        }
                    }),
                )
            }
        };
        builder.build().unwrap_or_else(|_| reqwest::Client::new())
    }
}

/// Resolves hostnames and hands the connector only the addresses its rule
/// allows. Checking at this point rather than before the request is what
/// defeats rebinding: the address judged is the address used.
struct GuardedResolver(Egress);

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let rule = self.0;
        Box::pin(async move {
            // getaddrinfo blocks, so it runs off the runtime, the way
            // reqwest's own resolver does it.
            let addrs = tokio::task::spawn_blocking(move || (host.as_str(), 0).to_socket_addrs())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)??;
            let mut refusal = None;
            let allowed: Vec<SocketAddr> = addrs
                .filter(|a| match rule.check_ip(a.ip()) {
                    Ok(()) => true,
                    Err(e) => {
                        refusal = Some(e);
                        false
                    }
                })
                .collect();
            if allowed.is_empty() {
                let e = refusal.unwrap_or_else(|| Refusal("No address for that server".into()));
                return Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(allowed.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn an_admin_goes_anywhere() {
        for s in ["127.0.0.1", "169.254.169.254", "192.168.178.20", "::1"] {
            assert!(Egress::Open.check_ip(ip(s)).is_ok(), "{s}");
        }
        assert!(Egress::Open.check_url("file:///etc/passwd").is_ok(), "an admin's URL is theirs");
    }

    #[test]
    fn a_member_never_reaches_the_server_or_link_local() {
        for s in [
            "127.0.0.1",
            "127.8.9.10",
            "0.0.0.0",
            "169.254.169.254",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "::",
            "::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
        ] {
            assert_eq!(classify(ip(s)), Class::Never, "{s}");
            assert!(Egress::Member.check_ip(ip(s)).is_err(), "{s}");
        }
    }

    #[test]
    fn private_space_is_private_and_the_internet_is_public() {
        for s in [
            "10.0.0.1",
            "172.16.5.4",
            "192.168.178.148",
            "100.100.1.1",
            "fd00::1",
            "::ffff:10.1.1.1",
        ] {
            assert_eq!(classify(ip(s)), Class::Private, "{s}");
        }
        for s in ["1.1.1.1", "142.250.1.1", "2a00:1450::1", "172.32.0.1", "100.128.0.1"] {
            assert_eq!(classify(ip(s)), Class::Public, "{s}");
            assert!(Egress::Member.check_ip(ip(s)).is_ok(), "{s}");
        }
    }

    #[test]
    fn urls_are_judged_by_the_address_they_name() {
        // Every one of these is 127.0.0.1 or the metadata address once parsed.
        for u in [
            "http://127.0.0.1:8080",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]:4533",
        ] {
            assert!(Egress::Member.check_url(u).is_err(), "{u}");
        }
        assert!(Egress::Member.check_url("gopher://example.com").is_err());
        assert!(Egress::Member.check_url("not a url").is_err());
        // A hostname is the resolver's job, not this one's.
        assert!(Egress::Member.check_url("http://navidrome.example.com").is_ok());
        assert!(Egress::Member.check_url("https://8.8.8.8").is_ok());
    }

    #[tokio::test]
    async fn localhost_by_name_is_still_localhost() {
        assert!(Egress::Member.check_url_resolved("http://localhost:4533").await.is_err());
        assert!(Egress::Open.check_url_resolved("http://localhost:4533").await.is_ok());
        // Unresolvable passes, so the source reports its own error.
        assert!(Egress::Member.check_url_resolved("http://no-such-host.invalid").await.is_ok());
    }

    /// The connect-time half: a member's client refuses `localhost` even
    /// though no handler checked the URL, which is the rebinding case.
    #[tokio::test]
    async fn a_member_client_will_not_connect_inward() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                if let Ok((mut s, _)) = listener.accept().await {
                    use tokio::io::AsyncWriteExt;
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
                }
            }
        });
        let url = format!("http://localhost:{port}/");
        let open = Egress::Open.client(Duration::from_secs(5)).get(&url).send().await;
        assert!(open.is_ok(), "the stub answers an open client: {open:?}");
        let member = Egress::Member.client(Duration::from_secs(5)).get(&url).send().await;
        let e = member.expect_err("a member client must not reach loopback");
        let chain = format!("{:?}", e);
        assert!(chain.contains("can't point at this server"), "{chain}");
    }

    /// td-security#8: a stranger's URL reaches the public internet over https
    /// and nothing else — not the operator's networks, which a member may
    /// reach, and never plain http.
    #[test]
    fn a_stranger_gets_public_https_only() {
        assert!(Egress::Stranger.check_url("https://mastodon.example/users/a").is_ok());
        assert!(Egress::Stranger.check_url("https://93.184.216.34/users/a").is_ok());
        for url in [
            "http://mastodon.example/users/a",
            "https://127.0.0.1/users/a",
            "https://169.254.169.254/latest/meta-data/",
            "https://192.168.178.20/users/a",
            "https://[::1]/users/a",
            "https://[::ffff:127.0.0.1]/users/a",
            "https://2130706433/users/a",
        ] {
            assert!(Egress::Stranger.check_url(url).is_err(), "{url}");
        }
        assert!(Egress::Stranger.check_ip(ip("100.100.1.1")).is_err(), "CGNAT is a LAN");
    }

    /// The finding itself, live: a server the first URL is allowed to reach
    /// answers with a redirect inward. The stranger's client must refuse the
    /// hop — to plain http, to a private literal, and to a name that resolves
    /// to loopback — where the old `Policy::limited(2)` client followed all
    /// three. The first request goes to the stub directly (its address is a
    /// literal, which the caller's `check_url` would refuse; this test is
    /// about the hops after it).
    #[tokio::test]
    async fn a_stranger_client_will_not_follow_a_redirect_inward() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                if let Ok((mut s, _)) = listener.accept().await {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let to = if req.starts_with("GET /http ") {
                        "http://mastodon.example/actor".to_string()
                    } else if req.starts_with("GET /literal ") {
                        "https://169.254.169.254/latest/meta-data/".to_string()
                    } else {
                        format!("https://localhost:{port}/actor")
                    };
                    let resp = format!(
                        "HTTP/1.1 302 Found\r\nlocation: {to}\r\ncontent-length: 0\r\n\r\n"
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                }
            }
        });
        let client = Egress::Stranger.client(Duration::from_secs(5));
        for (path, why) in [
            ("http", "https"),
            ("literal", "can't point at this server"),
            ("name", "can't point at this server"),
        ] {
            let e =
                client.get(format!("http://127.0.0.1:{port}/{path}")).send().await.expect_err(path);
            let chain = format!("{e:?}");
            assert!(chain.contains(why), "{path}: {chain}");
        }
    }
}
