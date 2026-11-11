//! Metrics for Prometheus, at `/metrics` on a port of their own (`INNBY_METRICS_LISTEN`),
//! which no route or ingress reaches, from each long-running workload: `serve`, `admin`
//! and `mailer`. The janitor is a CronJob, gone before a scrape, and logs what it deletes.
//!
//! Anonymous by construction. They say how Innby is doing, never who did what: requests
//! by route, method and status, how long they took, mail sent or failed, the outbox, the
//! database connections and the memory. Nothing per event or per guest, so a count can't
//! single anyone out. A label's value is a `Label`, which can only be made from a string
//! in the code, a route's pattern or a status code, so nothing a request carries (its
//! path, with the secret link in it, an address, a header) can end up in one.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt::Write,
    net::SocketAddr,
    sync::{LazyLock, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    extract::{MatchedPath, State},
    http::{Method, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use sqlx::PgPool;

use crate::{db, paths};

/// A label's value: from the code, a route's pattern or a status code, never from what a
/// request carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Label(Cow<'static, str>);

impl From<&'static str> for Label {
    fn from(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }
}

impl From<StatusCode> for Label {
    fn from(status: StatusCode) -> Self {
        Self(Cow::Owned(status.as_u16().to_string()))
    }
}

impl Label {
    /// The pattern a request matched, as `/r/{link}`, never its path; `unmatched` for a
    /// request no route took.
    pub fn route(route: Option<&MatchedPath>) -> Self {
        route.map_or("unmatched".into(), |route| {
            Self(Cow::Owned(route.as_str().to_owned()))
        })
    }

    /// The method, of the few the sites answer: a client may send any word as one.
    pub fn method(method: &Method) -> Self {
        match *method {
            Method::GET => "GET",
            Method::HEAD => "HEAD",
            Method::POST => "POST",
            _ => "other",
        }
        .into()
    }
}

type Labels = Vec<(&'static str, Label)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
    Histogram,
}

/// A metric's name, what it measures and its kind.
pub struct Metric {
    name: &'static str,
    help: &'static str,
    kind: Kind,
}

/// The upper bounds of a histogram's buckets, in seconds.
const BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

enum Series {
    Counter(u64),
    Gauge(f64),
    Histogram {
        // How many fell at or under each bound; `count` is the +Inf bucket.
        buckets: [u64; BUCKETS.len()],
        sum: f64,
        count: u64,
    },
}

/// A metric and its series, by their labels.
type Family = (&'static Metric, BTreeMap<Labels, Series>);

/// Every metric so far, by name, then by labels: sorted, so the page reads the same way
/// each time.
static SERIES: LazyLock<Mutex<BTreeMap<&'static str, Family>>> = LazyLock::new(Default::default);

impl Metric {
    const fn new(name: &'static str, kind: Kind, help: &'static str) -> Self {
        Self { name, help, kind }
    }

    fn update(&'static self, labels: &[(&'static str, Label)], change: impl FnOnce(&mut Series)) {
        let mut all = SERIES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_, series) = all
            .entry(self.name)
            .or_insert_with(|| (self, BTreeMap::new()));
        let series = series
            .entry(labels.to_vec())
            .or_insert_with(|| match self.kind {
                Kind::Counter => Series::Counter(0),
                Kind::Gauge => Series::Gauge(0.0),
                Kind::Histogram => Series::Histogram {
                    buckets: [0; BUCKETS.len()],
                    sum: 0.0,
                    count: 0,
                },
            });
        change(series);
    }

    /// Adds one to a counter.
    pub fn inc(&'static self, labels: &[(&'static str, Label)]) {
        debug_assert!(self.kind == Kind::Counter, "{} isn't a counter", self.name);
        self.update(labels, |series| {
            if let Series::Counter(count) = series {
                *count += 1;
            }
        });
    }

    /// Sets a gauge.
    pub fn set(&'static self, labels: &[(&'static str, Label)], value: f64) {
        debug_assert!(self.kind == Kind::Gauge, "{} isn't a gauge", self.name);
        self.update(labels, |series| {
            if let Series::Gauge(gauge) = series {
                *gauge = value;
            }
        });
    }

    /// Counts a duration, in seconds, into a histogram.
    pub fn observe(&'static self, labels: &[(&'static str, Label)], seconds: f64) {
        debug_assert!(
            self.kind == Kind::Histogram,
            "{} isn't a histogram",
            self.name
        );
        self.update(labels, |series| {
            if let Series::Histogram {
                buckets,
                sum,
                count,
            } = series
            {
                for (bucket, bound) in buckets.iter_mut().zip(BUCKETS) {
                    if seconds <= bound {
                        *bucket += 1;
                    }
                }
                *sum += seconds;
                *count += 1;
            }
        });
    }
}

pub static HTTP_REQUESTS: Metric = Metric::new(
    "innby_http_requests_total",
    Kind::Counter,
    "Requests answered, by route pattern, method and status.",
);
pub static HTTP_DURATION: Metric = Metric::new(
    "innby_http_request_duration_seconds",
    Kind::Histogram,
    "How long requests took to answer, by route pattern and method.",
);
pub static MAILS: Metric = Metric::new(
    "innby_mails_total",
    Kind::Counter,
    "Mail jobs the mailer handled: sent, failed (to be tried again) or dropped.",
);
static OUTBOX: Metric = Metric::new(
    "innby_outbox_jobs",
    Kind::Gauge,
    "Mail jobs not sent: waiting, or given up on after too many attempts.",
);
static DB_CONNECTIONS: Metric = Metric::new(
    "innby_db_connections",
    Kind::Gauge,
    "The database pool's connections, idle or in use.",
);
static MEMORY: Metric = Metric::new(
    "innby_process_resident_memory_bytes",
    Kind::Gauge,
    "The process's resident memory.",
);
static STARTED: Metric = Metric::new(
    "innby_process_start_time_seconds",
    Kind::Gauge,
    "When the process started, in seconds since the Unix epoch.",
);
static BUILD: Metric = Metric::new(
    "innby_build_info",
    Kind::Gauge,
    "Always 1, labelled with the build's release and commit.",
);

/// The page Prometheus reads, in its text format.
pub fn render() -> String {
    let all = SERIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut out = String::new();
    for (name, (metric, series)) in all.iter() {
        let kind = match metric.kind {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
            Kind::Histogram => "histogram",
        };
        let _ = writeln!(out, "# HELP {name} {}\n# TYPE {name} {kind}", metric.help);
        for (labels, series) in series {
            match series {
                Series::Counter(count) => {
                    let _ = writeln!(out, "{name}{} {count}", braces(labels, None));
                }
                Series::Gauge(value) => {
                    let _ = writeln!(out, "{name}{} {value}", braces(labels, None));
                }
                Series::Histogram {
                    buckets,
                    sum,
                    count,
                } => {
                    for (bound, bucket) in BUCKETS.iter().zip(buckets) {
                        let le = bound.to_string();
                        let _ =
                            writeln!(out, "{name}_bucket{} {bucket}", braces(labels, Some(&le)));
                    }
                    let _ = writeln!(out, "{name}_bucket{} {count}", braces(labels, Some("+Inf")));
                    let _ = writeln!(out, "{name}_sum{} {sum}", braces(labels, None));
                    let _ = writeln!(out, "{name}_count{} {count}", braces(labels, None));
                }
            }
        }
    }
    out
}

/// `{name="value",…}`, with `le` last for a histogram's bucket; nothing for no labels.
fn braces(labels: &Labels, le: Option<&str>) -> String {
    let mut pairs: Vec<String> = labels
        .iter()
        .map(|(name, Label(value))| format!("{name}=\"{}\"", escape(value)))
        .collect();
    pairs.extend(le.map(|le| format!("le=\"{le}\"")));
    match pairs.is_empty() {
        true => String::new(),
        false => format!("{{{}}}", pairs.join(",")),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
}

/// The resident memory, from Linux's `/proc`; None elsewhere.
fn resident_memory() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024.0)
}

#[derive(Clone)]
struct Scrape {
    db: PgPool,
    /// Whether to count the outbox, which only the mailer does.
    outbox: bool,
}

/// Reads what's measured as it is rather than as it happens, then the page.
async fn metrics(State(scrape): State<Scrape>) -> impl IntoResponse {
    let (size, idle) = (scrape.db.size(), scrape.db.num_idle() as u32);
    DB_CONNECTIONS.set(&[("state", "idle".into())], f64::from(idle));
    DB_CONNECTIONS.set(
        &[("state", "in_use".into())],
        f64::from(size.saturating_sub(idle)),
    );
    if let Some(bytes) = resident_memory() {
        MEMORY.set(&[], bytes);
    }
    if scrape.outbox {
        match db::outbox::unsent(&scrape.db).await {
            Ok((waiting, given_up)) => {
                OUTBOX.set(&[("state", "waiting".into())], waiting as f64);
                OUTBOX.set(&[("state", "given_up".into())], given_up as f64);
            }
            Err(err) => tracing::warn!(error = %err, "counting the outbox failed"),
        }
    }
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        render(),
    )
}

/// The metrics' own listener: `/metrics` and nothing else. Not `http::harden`, which
/// would log every scrape and count it among the site's requests.
fn router(db: PgPool, outbox: bool) -> Router {
    Router::new()
        .route(paths::METRICS, get(metrics))
        .with_state(Scrape { db, outbox })
}

/// Runs `workload`, and serves its metrics beside it if `address` is set, until both
/// stop at SIGTERM or Ctrl-C.
pub async fn alongside(
    address: Option<SocketAddr>,
    db: PgPool,
    outbox: bool,
    workload: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let Some(address) = address else {
        return workload.await;
    };
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    STARTED.set(&[], started);
    BUILD.set(
        &[
            ("release", Label(Cow::Owned(crate::version::release()))),
            (
                "commit",
                crate::version::short_commit().unwrap_or("unknown").into(),
            ),
        ],
        1.0,
    );
    tokio::try_join!(workload, crate::http::listen(address, router(db, outbox))).map(drop)
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    static TEST_COUNTER: Metric = Metric::new("innby_test_total", Kind::Counter, "A test's.");
    static TEST_HISTOGRAM: Metric = Metric::new("innby_test_seconds", Kind::Histogram, "A test's.");

    #[test]
    fn writes_prometheus_text() {
        TEST_COUNTER.inc(&[
            ("route", "/r/{link}".into()),
            ("status", StatusCode::OK.into()),
        ]);
        TEST_COUNTER.inc(&[
            ("route", "/r/{link}".into()),
            ("status", StatusCode::OK.into()),
        ]);
        TEST_HISTOGRAM.observe(&[], 0.03);
        let page = render();
        assert!(page.contains("# TYPE innby_test_total counter\n"), "{page}");
        assert!(
            page.contains("innby_test_total{route=\"/r/{link}\",status=\"200\"} 2\n"),
            "{page}"
        );
        assert!(
            page.contains("innby_test_seconds_bucket{le=\"0.025\"} 0\n"),
            "{page}"
        );
        assert!(
            page.contains("innby_test_seconds_bucket{le=\"0.05\"} 1\n"),
            "{page}"
        );
        assert!(
            page.contains("innby_test_seconds_bucket{le=\"+Inf\"} 1\n"),
            "{page}"
        );
        assert!(page.contains("innby_test_seconds_count 1\n"), "{page}");
    }

    #[test]
    fn a_method_is_one_of_a_few() {
        let made_up = Method::from_bytes(b"SECRET").unwrap();
        assert_eq!(Label::method(&made_up), "other".into());
        assert_eq!(Label::method(&Method::POST), "POST".into());
        assert_eq!(Label::route(None), "unmatched".into());
    }

    #[test]
    fn escapes_label_values() {
        assert_eq!(escape("a\"b\\c\nd"), r#"a\"b\\c\nd"#);
    }

    #[sqlx::test]
    async fn serves_the_page(db: PgPool) {
        let response = router(db, true)
            .oneshot(Request::get(paths::METRICS).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page = crate::testing::body(response).await;
        assert!(
            page.contains("innby_db_connections{state=\"idle\"}"),
            "{page}"
        );
        assert!(
            page.contains("innby_outbox_jobs{state=\"waiting\"} 0"),
            "{page}"
        );
    }
}
