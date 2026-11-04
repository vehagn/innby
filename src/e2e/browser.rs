//! A browser for the tests: it opens pages, follows links and redirects, fills in forms
//! by their labels and presses buttons by their text, and keeps its cookies. Each
//! `Browser` is one person's device.

use std::{fmt, net::IpAddr};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::{
    World,
    html::{Document, Field, Kind},
};

pub struct Browser<'w> {
    world: &'w World,
    /// Where its requests come from, for the rate limits.
    address: IpAddr,
    cookies: Vec<Cookie>,
    page: Option<Page>,
}

struct Cookie {
    origin: String,
    /// The paths it's sent to: this one and those under it.
    path: String,
    name: String,
    value: String,
}

impl Cookie {
    fn sent_to(&self, origin: &str, path: &str) -> bool {
        let path = path.split('?').next().unwrap();
        self.origin == origin
            && (self.path == "/"
                || path == self.path
                || path.starts_with(&format!("{}/", self.path)))
    }
}

/// The page a browser shows: its address, and what came back.
pub struct Page {
    pub url: String,
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    pub doc: Document,
}

impl<'w> Browser<'w> {
    pub(super) fn new(world: &'w World, address: IpAddr) -> Self {
        Self {
            world,
            address,
            cookies: Vec::new(),
            page: None,
        }
    }

    /// The page open now.
    #[track_caller]
    pub fn page(&self) -> &Page {
        self.page.as_ref().expect("no page open yet")
    }

    /// Opens an address, as if typed in or opened from a mail.
    pub async fn open(&mut self, url: &str) -> &Page {
        let url = self.resolve(url);
        self.navigate(Method::GET, url, None).await
    }

    /// Follows the link with this text on the page.
    #[track_caller]
    pub fn click(&mut self, text: &str) -> impl Future<Output = &Page> {
        let url = self.link(text);
        async move { self.navigate(Method::GET, url, None).await }
    }

    /// Saves the file a link with this text leads to, staying on the page.
    #[track_caller]
    pub fn download(&mut self, text: &str) -> impl Future<Output = Page> {
        let url = self.link(text);
        async move {
            let page = self.page.take();
            self.navigate(Method::GET, url, None).await;
            let file = std::mem::replace(&mut self.page, page);
            file.unwrap()
        }
    }

    #[track_caller]
    fn link(&self, text: &str) -> String {
        let page = self.page();
        let link = page
            .doc
            .links
            .iter()
            .find(|link| link.text == text)
            .unwrap_or_else(|| {
                let texts: Vec<_> = page.doc.links.iter().map(|link| &link.text).collect();
                panic!("no link {text:?} on {}; links: {texts:?}", page.url)
            });
        self.resolve(&link.href)
    }

    /// Types into the field with this label.
    #[track_caller]
    pub fn fill(&mut self, label: &str, value: &str) -> &mut Self {
        let field = self.field(label, &[Kind::Text, Kind::Textarea]);
        field.value = value.to_owned();
        self
    }

    /// Picks the option with this text in the select with this label.
    #[track_caller]
    pub fn select(&mut self, label: &str, option: &str) -> &mut Self {
        let field = self.field(label, &[Kind::Select]);
        let value = field
            .options
            .iter()
            .find(|(_, text)| text == option)
            .unwrap_or_else(|| panic!("{label:?} has no option {option:?}: {:?}", field.options))
            .0
            .clone();
        field.value = value;
        self
    }

    /// Ticks the checkbox, or picks the radio button, with this label.
    #[track_caller]
    pub fn check(&mut self, label: &str) -> &mut Self {
        let ticks = [Kind::Checkbox, Kind::Radio];
        let field = self.field(label, &ticks);
        let (kind, name) = (field.kind, field.name.clone());
        if kind == Kind::Radio {
            // Picking one radio button lets go of the others in its group.
            for form in &mut self.page.as_mut().unwrap().doc.forms {
                for other in form.fields.iter_mut().filter(|f| f.name == name) {
                    other.checked = false;
                }
            }
        }
        self.field(label, &ticks).checked = true;
        self
    }

    /// The value of the field with this label, such as a link shown to copy.
    #[track_caller]
    pub fn value(&mut self, label: &str) -> String {
        self.field(label, &[Kind::Text, Kind::Textarea])
            .value
            .clone()
    }

    /// Presses the button with this text, sending its form.
    #[track_caller]
    pub fn press(&mut self, text: &str) -> impl Future<Output = &Page> {
        let page = self.page();
        let (form, button) = page
            .doc
            .forms
            .iter()
            .find_map(|form| {
                let button = form.buttons.iter().find(|button| button.text == text)?;
                Some((form, button))
            })
            .unwrap_or_else(|| {
                let buttons: Vec<_> = page
                    .doc
                    .forms
                    .iter()
                    .flat_map(|form| form.buttons.iter().map(|b| &b.text))
                    .collect();
                panic!("no button {text:?} on {}; buttons: {buttons:?}", page.url)
            });
        let body = form.encode(button);
        let url = self.resolve(form.action.as_deref().unwrap_or(&page.url));
        let (method, url, body) = if form.post {
            (Method::POST, url, Some(body))
        } else {
            let separator = if url.contains('?') { '&' } else { '?' };
            (Method::GET, format!("{url}{separator}{body}"), None)
        };
        async move { self.navigate(method, url, body).await }
    }

    #[track_caller]
    /// The first field of one of these kinds with this label: two fields can share a
    /// label, such as a choice and the box that goes with it.
    fn field(&mut self, label: &str, kinds: &[Kind]) -> &mut Field {
        let doc = &mut self.page.as_mut().expect("no page open yet").doc;
        let labels: Vec<String> = doc
            .fields
            .iter()
            .chain(doc.forms.iter().flat_map(|form| &form.fields))
            .filter(|field| kinds.contains(&field.kind) && !field.label.is_empty())
            .map(|field| field.label.clone())
            .collect();
        doc.fields
            .iter_mut()
            .chain(doc.forms.iter_mut().flat_map(|form| &mut form.fields))
            .find(|field| kinds.contains(&field.kind) && field.label == label)
            .unwrap_or_else(|| panic!("no {kinds:?} field labelled {label:?}; labels: {labels:?}"))
    }

    /// Sends a request and follows redirects, as a browser does.
    async fn navigate(&mut self, method: Method, url: String, body: Option<String>) -> &Page {
        let (mut method, mut url, mut body) = (method, url, body);
        for _ in 0..10 {
            let (origin, path) = split(&url);
            let mut request = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::HOST, origin.split("://").last().unwrap())
                // The tests read the British texts.
                .header(header::ACCEPT_LANGUAGE, "en-GB");
            let cookies: Vec<String> = self
                .cookies
                .iter()
                .filter(|cookie| cookie.sent_to(origin, path))
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect();
            if !cookies.is_empty() {
                request = request.header(header::COOKIE, cookies.join("; "));
            }
            if body.is_some() {
                request = request
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header("sec-fetch-site", "same-origin");
            }
            let mut request = request
                .body(Body::from(body.take().unwrap_or_default()))
                .unwrap();
            // As the listener records the peer's address.
            let peer = std::net::SocketAddr::new(self.address, 50000);
            request.extensions_mut().insert(ConnectInfo(peer));
            let response = self.world.router(&url).oneshot(request).await.unwrap();
            self.store_cookies(origin, response.headers());
            let status = response.status();
            let headers = response.headers().clone();
            if status.is_redirection() {
                let location = headers[header::LOCATION].to_str().unwrap();
                url = resolve(&url, location);
                method = Method::GET;
                continue;
            }
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body = String::from_utf8_lossy(&bytes).into_owned();
            let doc = Document::parse(&body);
            return self.page.insert(Page {
                url,
                status,
                headers,
                body,
                doc,
            });
        }
        panic!("too many redirects from {url}");
    }

    fn store_cookies(&mut self, origin: &str, headers: &HeaderMap) {
        for header in headers.get_all(header::SET_COOKIE) {
            let header = header.to_str().unwrap();
            let (pair, attributes) = header.split_once(';').unwrap_or((header, ""));
            let (name, value) = pair.split_once('=').unwrap();
            let attributes: Vec<&str> = attributes.split(';').map(str::trim).collect();
            let path = attributes
                .iter()
                .find_map(|attribute| attribute.strip_prefix("Path="))
                .unwrap_or("/")
                .to_owned();
            self.cookies.retain(|cookie| {
                !(cookie.origin == origin && cookie.name == name && cookie.path == path)
            });
            let expired = attributes
                .iter()
                .any(|attribute| attribute.eq_ignore_ascii_case("max-age=0"));
            if !expired {
                self.cookies.push(Cookie {
                    origin: origin.to_owned(),
                    path,
                    name: name.to_owned(),
                    value: value.to_owned(),
                });
            }
        }
    }

    /// The names of the cookies this browser keeps for the public site.
    pub fn cookie_names(&self) -> Vec<&str> {
        let site = self.world.site_origin();
        self.cookies
            .iter()
            .filter(|cookie| cookie.sent_to(site, "/"))
            .map(|cookie| cookie.name.as_str())
            .collect()
    }

    fn resolve(&self, href: &str) -> String {
        match &self.page {
            Some(page) => resolve(&page.url, href),
            None => href.to_owned(),
        }
    }
}

impl Page {
    /// Whether a reader would see this text on the page.
    pub fn shows(&self, text: &str) -> bool {
        self.doc.text.contains(text)
    }

    /// Fails the test unless the page shows this text.
    #[track_caller]
    pub fn expect(&self, text: &str) -> &Self {
        assert!(self.shows(text), "expected {text:?} on {self}");
        self
    }

    /// Fails the test if the page shows this text.
    #[track_caller]
    pub fn expect_not(&self, text: &str) -> &Self {
        assert!(!self.shows(text), "didn't expect {text:?} on {self}");
        self
    }
}

/// A page prints as its address, status and text, for messages of failed tests.
impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}):\n{}", self.url, self.status, self.doc.text)
    }
}

/// The origin and the path of an absolute address.
pub(super) fn split(url: &str) -> (&str, &str) {
    let after_scheme = url.find("://").map_or(0, |at| at + 3);
    let path_at = url[after_scheme..]
        .find('/')
        .map_or(url.len(), |at| after_scheme + at);
    let path = &url[path_at..];
    let path = path.split('#').next().unwrap();
    (&url[..path_at], if path.is_empty() { "/" } else { path })
}

/// An address on a page, made absolute.
fn resolve(base: &str, href: &str) -> String {
    let (origin, path) = split(base);
    if href.contains("://") {
        href.to_owned()
    } else if href.starts_with('/') {
        format!("{origin}{href}")
    } else if href.starts_with('?') {
        format!("{origin}{}{href}", path.split('?').next().unwrap())
    } else if href.starts_with('#') || href.is_empty() {
        base.to_owned()
    } else {
        let dir = &path[..=path.rfind('/').unwrap_or(0)];
        format!("{origin}{dir}{href}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_addresses_like_a_browser() {
        let base = "https://fest.example.com/r/abc?lang=en";
        assert_eq!(split(base), ("https://fest.example.com", "/r/abc?lang=en"));
        assert_eq!(split("https://x.example"), ("https://x.example", "/"));
        assert_eq!(
            resolve(base, "/privacy"),
            "https://fest.example.com/privacy"
        );
        assert_eq!(
            resolve(base, "?theme=dark"),
            "https://fest.example.com/r/abc?theme=dark"
        );
        assert_eq!(resolve(base, "guests"), "https://fest.example.com/r/guests");
        assert_eq!(resolve(base, "https://other/x"), "https://other/x");
    }
}
