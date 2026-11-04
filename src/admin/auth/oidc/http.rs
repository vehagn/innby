//! A small HTTP client for the identity provider: one request per connection, HTTPS with
//! the Mozilla CA list, no redirects, a timeout and a size limit. Plain HTTP only reaches
//! this machine, for a provider run next to Innby while developing, and for the tests.

use std::{
    net::IpAddr,
    sync::{Arc, LazyLock},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, StatusCode,
    body::Bytes,
    header::{self, HeaderValue},
};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::{
    TlsConnector,
    rustls::{self, pki_types::ServerName},
};
use url::{Host, Url, form_urlencoded};

const TIMEOUT: Duration = Duration::from_secs(10);
/// Discovery documents and key sets are a few kilobytes.
const MAX_BODY: usize = 1 << 20;

static TLS: LazyLock<TlsConnector> = LazyLock::new(|| {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
});

pub struct Response {
    pub status: StatusCode,
    content_type: String,
    pub body: Bytes,
}

impl Response {
    /// The body as JSON, which it must say it is.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        let media_type = self.content_type.split(';').next().unwrap_or("").trim();
        if !media_type.eq_ignore_ascii_case("application/json") {
            bail!("expected JSON, got {:?}", self.content_type);
        }
        serde_json::from_slice(&self.body).context("reading the JSON")
    }

    /// The `error` and `error_description` of an OAuth error response, for the log.
    pub fn error(&self) -> String {
        #[derive(serde::Deserialize)]
        struct OAuthError {
            error: String,
            error_description: Option<String>,
        }
        match serde_json::from_slice::<OAuthError>(&self.body) {
            Ok(err) => format!(
                "{} {}: {}",
                self.status,
                err.error,
                err.error_description.unwrap_or_default()
            ),
            Err(_) => format!("{}", self.status),
        }
    }
}

/// Rejects a URL Innby won't send anything to: not HTTPS, or with a user name.
pub fn check_url(url: &Url) -> Result<()> {
    if !url.username().is_empty() || url.password().is_some() {
        bail!("{url}: no credentials in the URL, please");
    }
    match url.scheme() {
        "https" if url.host().is_some() => Ok(()),
        "http" if is_loopback(url) => Ok(()),
        _ => bail!("{url}: must be https://"),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `GET`s JSON, with an access token when there is one.
pub async fn get(url: &Url, bearer: Option<&str>) -> Result<Response> {
    let mut request = request(Method::GET, url)?;
    if let Some(token) = bearer {
        let value = HeaderValue::try_from(format!("Bearer {token}"))?;
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    send(url, request).await
}

/// `POST`s a form with HTTP Basic client authentication. Both halves are form-encoded
/// first, as RFC 6749 §2.3.1 says and many clients forget.
pub async fn post_form(
    url: &Url,
    (client_id, client_secret): (&str, &str),
    form: &[(&str, &str)],
) -> Result<Response> {
    let encode =
        |value: &str| form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    let basic = STANDARD.encode(format!("{}:{}", encode(client_id), encode(client_secret)));
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let mut request = request(Method::POST, url)?;
    let headers = request.headers_mut();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::try_from(format!("Basic {basic}"))?,
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    *request.body_mut() = Full::new(Bytes::from(body));
    send(url, request).await
}

fn request(method: Method, url: &Url) -> Result<Request<Full<Bytes>>> {
    check_url(url)?;
    let host = url.host_str().context("no host")?;
    let host = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    Ok(Request::builder()
        .method(method)
        .uri(target)
        .header(header::HOST, host)
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, "Innby")
        .body(Full::default())?)
}

async fn send(url: &Url, request: Request<Full<Bytes>>) -> Result<Response> {
    tokio::time::timeout(TIMEOUT, exchange(url, request))
        .await
        .map_err(|_| anyhow!("{url}: no answer in {TIMEOUT:?}"))?
        .with_context(|| format!("request to {url}"))
}

async fn exchange(url: &Url, request: Request<Full<Bytes>>) -> Result<Response> {
    let port = url.port_or_known_default().context("no port")?;
    let tcp = match url.host() {
        Some(Host::Domain(domain)) => TcpStream::connect((domain, port)).await?,
        Some(Host::Ipv4(ip)) => TcpStream::connect((IpAddr::V4(ip), port)).await?,
        Some(Host::Ipv6(ip)) => TcpStream::connect((IpAddr::V6(ip), port)).await?,
        None => bail!("no host"),
    };
    if url.scheme() == "https" {
        let name = match url.host() {
            Some(Host::Domain(domain)) => ServerName::try_from(domain.to_owned())?,
            Some(Host::Ipv4(ip)) => ServerName::from(IpAddr::V4(ip)),
            Some(Host::Ipv6(ip)) => ServerName::from(IpAddr::V6(ip)),
            None => bail!("no host"),
        };
        let tls = TLS.connect(name, tcp).await?;
        over(TokioIo::new(tls), request).await
    } else {
        over(TokioIo::new(tcp), request).await
    }
}

async fn over<T>(io: T, request: Request<Full<Bytes>>) -> Result<Response>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await?;
    let connection = tokio::spawn(connection);
    let response = sender.send_request(request).await;
    let result = async {
        let response = response?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|err| anyhow!("reading the answer: {err}"))?
            .to_bytes();
        Ok(Response {
            status,
            content_type,
            body,
        })
    }
    .await;
    connection.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_machine() {
        let ok = |url: &str| check_url(&url.parse().unwrap()).is_ok();
        assert!(ok("https://authelia.example.com/api/oidc/token"));
        assert!(ok("http://127.0.0.1:9091/token"));
        assert!(ok("http://localhost:9091/token"));
        assert!(ok("http://[::1]:9091/token"));
        assert!(!ok("http://authelia.example.com/token"));
        assert!(!ok("http://10.0.0.1/token"));
        assert!(!ok("ftp://example.com/token"));
        assert!(!ok("https://user:pass@example.com/token"));
    }
}
