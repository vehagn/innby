//! An in-memory rate limit, per client or for all clients together, for the forms anyone
//! can post (group sign-up and the ones that send mail) and for admin logins. Addresses
//! are only held in memory for the length of a window and never logged or stored.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::request::Parts,
};

/// A fixed-window counter per client, or for everyone with `check_all`.
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    hits: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: Mutex::default(),
        }
    }

    /// Counts a request and says whether it's within the limit.
    pub fn check(&self, ClientIp(client): ClientIp) -> bool {
        self.check_at(client, Instant::now())
    }

    /// Counts a request against a limit all clients share, for when one attacker can
    /// have many addresses.
    pub fn check_all(&self) -> bool {
        self.check_at(IpAddr::V4(Ipv4Addr::UNSPECIFIED), Instant::now())
    }

    fn check_at(&self, client: IpAddr, now: Instant) -> bool {
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Forget finished windows now and then, so the map can't grow without bound.
        if hits.len() >= 1024 {
            hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
        }
        let (start, count) = hits.entry(network(client)).or_insert((now, 0));
        if now.duration_since(*start) >= self.window {
            (*start, *count) = (now, 0);
        }
        *count += 1;
        *count <= self.limit
    }
}

/// IPv6 clients usually control a whole /64, so they're counted per /64.
fn network(client: IpAddr) -> IpAddr {
    match client {
        IpAddr::V4(_) => client,
        IpAddr::V6(v6) => {
            let prefix = u128::from(v6) & !((1u128 << 64) - 1);
            IpAddr::V6(Ipv6Addr::from(prefix))
        }
    }
}

/// The client's address: the TCP peer, or behind a trusted gateway the address it
/// appended to `X-Forwarded-For`. Clients without either (only in tests) share
/// `0.0.0.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    fn from_parts(parts: &Parts, trust_forwarded: bool) -> Self {
        let forwarded = || {
            parts
                .headers
                .get_all("x-forwarded-for")
                .iter()
                .next_back()
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.rsplit(',').next())
                .and_then(|last| last.trim().parse().ok())
        };
        let peer = || {
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(address)| address.ip())
        };
        let client = trust_forwarded.then(forwarded).flatten().or_else(peer);
        Self(client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    }
}

impl FromRequestParts<super::AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &super::AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self::from_parts(parts, state.config.trust_forwarded))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;

    fn ip(address: &str) -> IpAddr {
        address.parse().unwrap()
    }

    #[test]
    fn limits_each_client_per_window() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.check_at(ip("192.0.2.1"), start));
        assert!(limiter.check_at(ip("192.0.2.1"), start));
        assert!(!limiter.check_at(ip("192.0.2.1"), start));
        assert!(limiter.check_at(ip("192.0.2.2"), start));
        assert!(limiter.check_at(ip("192.0.2.1"), start + Duration::from_secs(60)));
    }

    #[test]
    fn check_all_counts_every_client_together() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.check_all());
        assert!(limiter.check_all());
        assert!(!limiter.check_all());
    }

    #[test]
    fn counts_ipv6_per_64() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let start = Instant::now();
        assert!(limiter.check_at(ip("2001:db8:1:2::1"), start));
        assert!(!limiter.check_at(ip("2001:db8:1:2:ffff::9"), start));
        assert!(limiter.check_at(ip("2001:db8:1:3::1"), start));
    }

    #[test]
    fn trusts_forwarded_for_only_when_told() {
        let (mut parts, ()) = Request::get("/")
            .header("x-forwarded-for", "203.0.113.9, 198.51.100.7")
            .body(())
            .unwrap()
            .into_parts();
        parts
            .extensions
            .insert(ConnectInfo::<SocketAddr>("10.0.0.5:1234".parse().unwrap()));

        assert_eq!(ClientIp::from_parts(&parts, true).0, ip("198.51.100.7"));
        assert_eq!(ClientIp::from_parts(&parts, false).0, ip("10.0.0.5"));
    }
}
