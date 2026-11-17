//! Helpers for tests of either listener: sending forms and reading what comes back.

use axum::{
    body::Body,
    http::{Request, header},
    response::Response,
};
use http_body_util::BodyExt;

/// A form post from this `Sec-Fetch-Site`, as a browser sends it.
pub fn form_post(uri: &str, form: &str, fetch_site: &str) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("sec-fetch-site", fetch_site)
        .body(Body::from(form.to_owned()))
        .unwrap()
}

pub async fn body(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Where a redirect goes.
pub fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

/// Every `Set-Cookie` header of the response.
pub fn set_cookies(response: &Response) -> Vec<&str> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect()
}

/// A text as a template writes it into HTML.
pub fn escape(text: &str) -> String {
    text.replace('&', "&#38;")
        .replace('<', "&#60;")
        .replace('>', "&#62;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
pub(crate) mod logs {
    use std::{
        io,
        io::Write,
        sync::{Arc, LazyLock, Mutex},
    };

    /// A subscriber that hears nothing, kept for as long as the tests run. While there's
    /// one subscriber in all, tracing asks only the thread that first reaches a log line
    /// whether anyone's listening, and keeps a no for every thread: another test's
    /// thread, which captures nothing, would so turn the line off for the test that
    /// does. With two, it asks them both.
    static QUIET: LazyLock<tracing::Dispatch> =
        LazyLock::new(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));

    #[derive(Clone, Default)]
    struct Writer(Arc<Mutex<Vec<u8>>>);

    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Writer {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    pub struct Capture {
        writer: Writer,
        dispatch: tracing::Dispatch,
    }

    impl Capture {
        pub fn new() -> Self {
            LazyLock::force(&QUIET);
            let writer = Writer::default();
            let subscriber = tracing_subscriber::fmt::Subscriber::builder()
                .without_time()
                .with_ansi(false)
                .with_writer(writer.clone())
                .finish();
            let dispatch = tracing::Dispatch::new(subscriber);
            Self { writer, dispatch }
        }

        pub async fn run<T>(&self, future: impl Future<Output = T>) -> T {
            use tracing::instrument::WithSubscriber;

            future.with_subscriber(self.dispatch.clone()).await
        }

        pub fn text(&self) -> String {
            String::from_utf8(self.writer.0.lock().unwrap().clone()).unwrap()
        }
    }
}
