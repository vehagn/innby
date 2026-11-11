//! An in-memory rate limit, per client or for all clients together, for the forms anyone
//! can post (group sign-up and the ones that send mail) and for admin logins; or per
//! guest, by their RSVP's id, for posts on a message board. Addresses and ids are only
//! held in memory for the length of a window and never logged or stored, and only so
//! many at once, so nobody can fill the memory with them.

use std::{
    collections::HashMap,
    hash::Hash,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{extract::ConnectInfo, http::request::Parts};
use uuid::Uuid;

/// The most clients a limiter counts at once, a megabyte or so. Someone with a great
/// many addresses (an IPv6 /48 has 65 536 networks) could otherwise make it remember
/// them all for a window, in pods with little memory to spare. Past this, clients it
/// doesn't know yet share one count, which soon refuses them until windows end.
const MOST_CLIENTS: usize = 20_000;

/// The one count shared by the clients there's no room for: no client's own address.
const EVERYONE_ELSE: IpAddr = IpAddr::V6(Ipv6Addr::UNSPECIFIED);

/// A fixed-window counter per client, or for everyone with `check_all`; or, as a
/// `RateLimiter<Uuid>`, per guest.
pub struct RateLimiter<K = IpAddr> {
    limit: u32,
    window: Duration,
    hits: Mutex<HashMap<K, (Instant, u32)>>,
}

impl<K: Copy + Eq + Hash> RateLimiter<K> {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: Mutex::default(),
        }
    }

    /// Counts a request by `key`, which shares `overflow`'s count when there are already
    /// `most` others.
    fn count(&self, mut key: K, now: Instant, most: usize, overflow: K) -> bool {
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Forget finished windows now and then; if it's still full of running ones,
        // there's no room for a newcomer, who's counted with everyone else.
        if hits.len() >= most.min(1024) && !hits.contains_key(&key) {
            hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
            if hits.len() >= most {
                key = overflow;
            }
        }
        let (start, count) = hits.entry(key).or_insert((now, 0));
        if now.duration_since(*start) >= self.window {
            (*start, *count) = (now, 0);
        }
        *count += 1;
        *count <= self.limit
    }
}

impl RateLimiter<Uuid> {
    /// Counts a request by a guest, and says whether it's within the limit.
    pub fn check_guest(&self, rsvp_id: Uuid) -> bool {
        self.count(rsvp_id, Instant::now(), MOST_CLIENTS, Uuid::nil())
    }
}

impl RateLimiter {
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
        self.check_among(client, now, MOST_CLIENTS)
    }

    fn check_among(&self, client: IpAddr, now: Instant, most: usize) -> bool {
        self.count(network(client), now, most, EVERYONE_ELSE)
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
    pub fn from_parts(parts: &Parts, trust_forwarded: bool) -> Self {
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
    fn remembers_only_so_many_clients() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let start = Instant::now();
        let most = 3;
        for client in ["192.0.2.1", "192.0.2.2", "192.0.2.3"] {
            assert!(limiter.check_among(ip(client), start, most));
        }
        // Those it knows keep their own count; newcomers share one, which runs out.
        assert!(limiter.check_among(ip("192.0.2.1"), start, most));
        assert!(limiter.check_among(ip("192.0.2.4"), start, most));
        assert!(limiter.check_among(ip("192.0.2.5"), start, most));
        assert!(!limiter.check_among(ip("192.0.2.6"), start, most));
        assert!(limiter.hits.lock().unwrap().len() <= most + 1);
        // Once the windows end there's room again.
        let later = start + Duration::from_secs(60);
        assert!(limiter.check_among(ip("192.0.2.6"), later, most));
        assert_eq!(limiter.hits.lock().unwrap().len(), 1);
    }

    #[test]
    fn counts_each_guest_apart() {
        let limiter = RateLimiter::<Uuid>::new(1, Duration::from_secs(60));
        let (kari, ola) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert!(limiter.check_guest(kari));
        assert!(!limiter.check_guest(kari));
        assert!(limiter.check_guest(ola));
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
