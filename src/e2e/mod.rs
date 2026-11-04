//! End-to-end tests: whole journeys through the public site, the admin UI and the mailer,
//! driven the way people use them.
//!
//! A [`World`] is one event on a throwaway database. Each [`Browser`] in it is one
//! person's device: it opens links, fills in fields by their labels, presses buttons by
//! their text and keeps its own cookies. Mail goes to an inbox the test can read instead
//! of to SMTP.
//!
//! The pages are the real ones, so these tests find what a person would look for:
//! labels and button texts, taken from the British texts (`en()`, `admin()`) so that a
//! reworded text doesn't break them. Prefer checking what a page shows over what's in
//! the database; the unit tests next to each module cover the details.
//!
//! To add a journey, add a `#[sqlx::test]` to the file for its area: guests, groups,
//! e-mail or the host.

mod browser;
mod email;
mod groups;
mod guests;
mod host;
mod html;

use std::{collections::HashMap, sync::Mutex, time::Duration};

use axum::Router;
use lettre::message::Mailbox;
use sqlx::PgPool;

pub use browser::Browser;

use crate::{
    admin::{self, auth},
    config::{Config, tests::config},
    i18n::{AdminMessages, Lang, Messages},
    mailer::{self, Mail, Sender},
    web,
};

/// Where the tests reach the admin UI, which in the cluster has a hostname of its own.
const ADMIN: &str = "https://admin.fest.example.com";

/// The admin's user name and password in the tests.
pub const USER: &str = "admin";
pub const PASSWORD: &str = "secret";

pub struct World {
    pub db: PgPool,
    pub config: Config,
    site: Router,
    admin: Router,
    /// Mail the mailer has sent.
    sent: Inbox,
    /// How many mails each address has read with `receive`.
    read: Mutex<HashMap<String, usize>>,
}

#[derive(Default)]
struct Inbox(Mutex<Vec<Mail>>);

impl Sender for Inbox {
    async fn send(&self, _from: &Mailbox, mail: Mail) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(mail);
        Ok(())
    }
}

impl World {
    pub fn new(db: PgPool) -> Self {
        let config = config();
        Self {
            site: web::router(config.clone(), db.clone()),
            admin: admin::router(config.clone(), &auth::tests::admin_config(), db.clone()),
            sent: Inbox::default(),
            read: Mutex::default(),
            db,
            config,
        }
    }

    /// A new device, with no cookies and no page open.
    pub fn browser(&self) -> Browser<'_> {
        Browser::new(self)
    }

    /// The host's browser, logged in to the admin UI on its Guests tab.
    pub async fn host(&self) -> Browser<'_> {
        let mut host = self.browser();
        host.open(ADMIN).await.expect(&admin().log_in_text);
        host.fill(&admin().user_name, USER)
            .fill(&admin().password, PASSWORD);
        host.press(&admin().log_in).await.expect(&admin().guests);
        host
    }

    /// An invitation made straight in the database, for journeys that don't start
    /// with the host. Returns the guest's link.
    pub async fn invite(&self, name: &str, max_party_size: i32) -> String {
        let rsvp = crate::db::rsvp::create_invite(&self.db, name, max_party_size)
            .await
            .unwrap();
        let link = self
            .config
            .link_key
            .link(crate::link::Purpose::Rsvp, rsvp.id, rsvp.link_nonce);
        self.site(&crate::paths::path(crate::paths::RSVP, link))
    }

    /// Runs the mailer until nothing is due, as its loop would.
    pub async fn deliver_mail(&self) {
        let from: Mailbox = "Julebord <fest@example.com>".parse().unwrap();
        while mailer::process_batch(&self.config, &from, &self.db, &self.sent)
            .await
            .unwrap()
            > 0
        {}
    }

    /// The next mail to this address that the test hasn't read yet. Some mail is queued
    /// after the page has answered, so this waits a little for it.
    #[track_caller]
    pub fn receive(&self, address: &str) -> impl Future<Output = Mail> {
        let caller = std::panic::Location::caller();
        async move {
            for _ in 0..100 {
                self.deliver_mail().await;
                let mails = self.inbox(address);
                let next = {
                    let mut read = self.read.lock().unwrap();
                    let seen = read.entry(address.to_owned()).or_default();
                    let next = mails.get(*seen).cloned();
                    *seen += usize::from(next.is_some());
                    next
                };
                if let Some(mail) = next {
                    return mail;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("no new mail to {address} (expected at {caller})");
        }
    }

    /// Every mail sent to this address so far, oldest first.
    pub fn inbox(&self, address: &str) -> Vec<Mail> {
        let sent = self.sent.0.lock().unwrap();
        sent.iter()
            .filter(|mail| mail.to == address)
            .cloned()
            .collect()
    }

    /// The public site's address for a path.
    pub fn site(&self, path: &str) -> String {
        self.config.url(path)
    }

    fn site_origin(&self) -> &str {
        &self.config.base_url
    }

    fn router(&self, origin: &str) -> Router {
        match origin {
            ADMIN => self.admin.clone(),
            origin if origin == self.site_origin() => self.site.clone(),
            origin => panic!("the tests have no site at {origin}"),
        }
    }
}

/// The link in a mail.
#[track_caller]
pub fn link_in(mail: &Mail) -> &str {
    mail.body
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .unwrap_or_else(|| panic!("no link in {:?}", mail.body))
}

/// The British texts, which the browsers ask for.
pub fn en() -> &'static Messages {
    Lang::EnGb.messages()
}

/// The admin UI's British texts.
pub fn admin() -> &'static AdminMessages {
    &en().admin
}
