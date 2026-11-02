//! The optional e-mail address: adding it with consent, the double opt-in, removing it,
//! and `/lost` for guests who lost their link.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{MessagePage, render, render_with, server_error},
    rate_limit::ClientIp,
    rsvp::{Guest, Notice, RsvpPage, redirect},
};
use crate::{
    db::{
        outbox::{self, Kind},
        rsvp::{self, EmailStatus, Rsvp},
    },
    i18n::{Lang, Messages},
    link::{EmailLink, Purpose},
    paths,
};

#[derive(Deserialize)]
pub struct AddForm {
    email: String,
    consent: Option<String>,
}

/// Stores the address unconfirmed and queues the confirmation mail, or a heads-up for
/// whoever has already confirmed the address. The guest's page shows the address waiting
/// for confirmation either way, so it never tells whether someone else has it.
pub async fn add(
    State(state): State<AppState>,
    lang: Lang,
    client: ClientIp,
    guest: Guest,
    Form(form): Form<AddForm>,
) -> Response {
    let page = || RsvpPage::new(&state, lang, &guest);
    if form.consent.is_none() {
        return page().render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.consent_required);
    }
    let Some(email) = rsvp::normalize_email(&form.email) else {
        return page().render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_email);
    };
    // Each attempt can mail an address the guest typed in, so it's limited like /lost.
    if !state.may_send_mail(client) {
        tracing::warn!("add e-mail rate limit hit");
        return page().render_error(StatusCode::TOO_MANY_REQUESTS, |t| &t.too_many_attempts);
    }

    match add_email(&state.db, &guest.rsvp, &email, lang).await {
        Ok(()) => redirect(&guest.link, lang, Notice::EmailSent),
        Err(err) => server_error(err),
    }
}

async fn add_email(db: &PgPool, guest: &Rsvp, email: &str, lang: Lang) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    if guest.email.as_deref() != Some(email) {
        rsvp::set_email(&mut *tx, guest.id, email, lang, PRIVACY_VERSION).await?;
        // Mail queued for the previous address must not go out.
        outbox::cancel_unsent(&mut *tx, guest.id).await?;
    } else if guest.email_status == EmailStatus::Confirmed {
        return Ok(());
    }
    // Asking again only queues mail that isn't already waiting.
    match rsvp::find_by_confirmed_email(&mut *tx, email).await? {
        Some(owner) => {
            outbox::enqueue_once(&mut *tx, owner.id, Kind::EmailInUse).await?;
            tracing::info!(rsvp = %guest.id, "e-mail address already in use");
        }
        None => {
            outbox::enqueue_once(&mut *tx, guest.id, Kind::ConfirmEmail).await?;
            tracing::info!(rsvp = %guest.id, "e-mail address added");
        }
    }
    tx.commit().await
}

/// Withdraws consent: the address and any mail waiting for it are deleted.
pub async fn remove(State(state): State<AppState>, lang: Lang, guest: Guest) -> Response {
    let result = async {
        let mut tx = state.db.begin().await?;
        rsvp::remove_email(&mut *tx, guest.rsvp.id).await?;
        outbox::cancel_unsent(&mut *tx, guest.rsvp.id).await?;
        tx.commit().await
    }
    .await;
    match result {
        Ok(()) => {
            tracing::info!(rsvp = %guest.rsvp.id, "e-mail address removed");
            redirect(&guest.link, lang, Notice::EmailRemoved)
        }
        Err(err) => server_error(err),
    }
}

#[derive(Template)]
#[template(path = "confirm_email.html")]
struct ConfirmPage {
    lang: Lang,
    t: &'static Messages,
    action: String,
}

fn invalid_confirmation(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (&t.gone_title, &t.confirm_invalid_text)),
    )
}

/// The guest a confirmation link is for, if it's unexpired and signed for their current,
/// still unconfirmed address.
async fn verify(state: &AppState, token: &str) -> sqlx::Result<Option<Rsvp>> {
    let Ok(link) = token.parse::<EmailLink>() else {
        return Ok(None);
    };
    if link.expires <= Utc::now() {
        return Ok(None);
    }
    let guest = rsvp::find(&state.db, link.id).await?;
    Ok(guest.filter(|guest| {
        guest.email_status == EmailStatus::Unconfirmed
            && guest
                .email
                .as_deref()
                .is_some_and(|email| state.config.link_key.verify_email(&link, email))
    }))
}

/// Shows a button rather than confirming on GET, so mail scanners that open links
/// can't confirm on the guest's behalf.
pub async fn confirm_page(
    State(state): State<AppState>,
    lang: Lang,
    Path(token): Path<String>,
) -> Response {
    match verify(&state, &token).await {
        Ok(Some(_)) => render(ConfirmPage {
            lang,
            t: lang.messages(),
            action: paths::localized(&paths::path(paths::CONFIRM_EMAIL, token), lang),
        }),
        Ok(None) => invalid_confirmation(lang),
        Err(err) => server_error(err),
    }
}

pub async fn confirm(
    State(state): State<AppState>,
    lang: Lang,
    Path(token): Path<String>,
) -> Response {
    let guest = match verify(&state, &token).await {
        Ok(Some(guest)) => guest,
        Ok(None) => return invalid_confirmation(lang),
        Err(err) => return server_error(err),
    };
    let email = guest.email.as_deref().unwrap_or_default();
    let result = async {
        let mut tx = state.db.begin().await?;
        let confirmed = rsvp::confirm_email(&mut *tx, guest.id, email).await?;
        if confirmed {
            outbox::enqueue(&mut *tx, guest.id, Kind::RsvpLink).await?;
        }
        tx.commit().await.map(|()| confirmed)
    }
    .await;
    match result {
        Ok(true) => {
            tracing::info!(rsvp = %guest.id, "e-mail address confirmed");
            // Holding the confirmation link proves the address, which belongs to this
            // RSVP, so the guest may go straight to their page.
            let link = state
                .config
                .link_key
                .link(Purpose::Rsvp, guest.id, guest.link_nonce);
            redirect(&link, lang, Notice::EmailConfirmed)
        }
        Ok(false) => invalid_confirmation(lang),
        Err(err) => server_error(err),
    }
}

#[derive(Template)]
#[template(path = "lost.html")]
struct LostPage {
    lang: Lang,
    t: &'static Messages,
    error: Option<&'static str>,
}

impl LostPage {
    fn new(lang: Lang) -> Self {
        Self {
            lang,
            t: lang.messages(),
            error: None,
        }
    }
}

pub async fn lost_page(lang: Lang) -> Response {
    render(LostPage::new(lang))
}

#[derive(Deserialize)]
pub struct LostForm {
    email: String,
}

/// Mails the guest's link to a confirmed address. The answer never says whether the
/// address is registered, and neither does how long it takes: the lookup runs after the
/// response.
pub async fn lost(
    State(state): State<AppState>,
    lang: Lang,
    client: ClientIp,
    Form(form): Form<LostForm>,
) -> Response {
    let fail = |status, pick: fn(&'static Messages) -> &'static str| {
        let mut page = LostPage::new(lang);
        page.error = Some(pick(page.t));
        render_with(status, page)
    };
    let Some(email) = rsvp::normalize_email(&form.email) else {
        return fail(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_email);
    };
    if !state.may_send_mail(client) {
        tracing::warn!("lost link rate limit hit");
        return fail(StatusCode::TOO_MANY_REQUESTS, |t| &t.too_many_attempts);
    }
    let db = state.db.clone();
    tokio::spawn(async move {
        if let Err(err) = mail_link(&db, &email).await {
            tracing::error!(error = %err, "queueing a lost link failed");
        }
    });
    render(MessagePage::new(lang, |t| {
        (&t.lost_sent_title, &t.lost_sent_text)
    }))
}

/// Queues the link for the guest with this address, if it's confirmed.
async fn mail_link(db: &PgPool, email: &str) -> sqlx::Result<()> {
    let Some(guest) = rsvp::find_by_confirmed_email(db, email).await? else {
        return Ok(());
    };
    if outbox::enqueue_once(db, guest.id, Kind::RsvpLink).await? {
        tracing::info!(rsvp = %guest.id, "lost link mailed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use sqlx::PgPool;

    use super::*;
    use crate::web::testing::{Site, body, en, escape, location};

    async fn queued(site: &Site) -> Vec<(uuid::Uuid, Kind)> {
        sqlx::query!(
            r#"select rsvp_id, kind as "kind: Kind" from outbox where sent_at is null order by id"#
        )
        .fetch_all(&site.db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.rsvp_id, row.kind))
        .collect()
    }

    /// Mail that `/lost` queues after it has responded, once it's there.
    async fn queued_soon(site: &Site) -> Vec<(uuid::Uuid, Kind)> {
        for _ in 0..100 {
            let queued = queued(site).await;
            if !queued.is_empty() {
                return queued;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("nothing was queued");
    }

    fn confirm_token(site: &Site, guest: &Rsvp, email: &str) -> String {
        site.config
            .link_key
            .email_link(guest.id, email, Utc::now() + Duration::hours(48))
    }

    #[sqlx::test]
    async fn add_confirm_and_remove(db: PgPool) {
        let site = Site::new(db);
        let (guest, link) = site.invite("Kari", 1).await;

        let response = site
            .post(
                &format!("/r/{link}/email?lang=en"),
                "email=+Kari%40Example.com&consent=yes",
            )
            .await;
        assert_eq!(
            location(&response),
            format!("/r/{link}?lang=en&notice=email_sent")
        );
        let added = site.find(&guest).await.unwrap();
        assert_eq!(added.email.as_deref(), Some("kari@example.com"));
        assert_eq!(
            (added.email_status, added.lang),
            (EmailStatus::Unconfirmed, Lang::EnGb)
        );
        assert_eq!(queued(&site).await, [(guest.id, Kind::ConfirmEmail)]);

        let html = body(
            site.get(&format!("/r/{link}?lang=en&notice=email_sent"))
                .await,
        )
        .await;
        assert!(html.contains(&escape(&en().email_sent_notice)));
        assert!(html.contains("<strong>kari@example.com</strong>"));

        // GET only shows a button.
        let token = confirm_token(&site, &added, "kari@example.com");
        let response = site.get(&format!("/e/{token}?lang=en")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            site.find(&guest).await.unwrap().email_status,
            EmailStatus::Unconfirmed
        );

        let response = site.post(&format!("/e/{token}?lang=en"), "").await;
        assert_eq!(
            location(&response),
            format!("/r/{link}?lang=en&notice=email_confirmed")
        );
        assert_eq!(
            site.find(&guest).await.unwrap().email_status,
            EmailStatus::Confirmed
        );
        assert_eq!(
            queued(&site).await,
            [(guest.id, Kind::ConfirmEmail), (guest.id, Kind::RsvpLink)]
        );
        // A second click finds nothing left to confirm.
        let response = site.post(&format!("/e/{token}"), "").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = site.post(&format!("/r/{link}/email/remove"), "").await;
        assert_eq!(
            location(&response),
            format!("/r/{link}?lang=nb&notice=email_removed")
        );
        let removed = site.find(&guest).await.unwrap();
        assert_eq!(
            (removed.email, removed.email_status),
            (None, EmailStatus::None)
        );
        assert!(queued(&site).await.is_empty());
    }

    #[sqlx::test]
    async fn requires_consent_and_a_valid_address(db: PgPool) {
        let site = Site::new(db);
        let (guest, link) = site.invite("Kari", 1).await;
        let uri = format!("/r/{link}/email");

        let response = site.post(&uri, "email=kari%40example.com").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("Kryss av i boksen"));
        let response = site.post(&uri, "email=kari&consent=yes").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        assert_eq!(site.find(&guest).await.unwrap().email, None);
        assert!(queued(&site).await.is_empty());
    }

    #[sqlx::test]
    async fn address_in_use_warns_its_owner_and_looks_the_same(db: PgPool) {
        let site = Site::new(db);
        let (owner, owner_link) = site.invite("Kari", 1).await;
        let (other, other_link) = site.invite("Ola", 1).await;
        let form = "email=kari%40example.com&consent=yes";
        site.post(&format!("/r/{owner_link}/email"), form).await;
        rsvp::confirm_email(&site.db, owner.id, "kari@example.com")
            .await
            .unwrap();
        sqlx::query!("delete from outbox")
            .execute(&site.db)
            .await
            .unwrap();

        let response = site.post(&format!("/r/{other_link}/email"), form).await;
        let page = location(&response).to_owned();
        assert_eq!(page, format!("/r/{other_link}?lang=nb&notice=email_sent"));
        // The page shows the address waiting for confirmation, as for a new address.
        assert!(
            body(site.get(&page).await)
                .await
                .contains("<strong>kari@example.com</strong>")
        );
        let stored = site.find(&other).await.unwrap();
        assert_eq!(
            (stored.email.as_deref(), stored.email_status),
            (Some("kari@example.com"), EmailStatus::Unconfirmed)
        );
        assert_eq!(queued(&site).await, [(owner.id, Kind::EmailInUse)]);
        // Only the owner's confirmation counts.
        let token = confirm_token(&site, &stored, "kari@example.com");
        let response = site.post(&format!("/e/{token}"), "").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn unconfirmed_address_can_be_added_twice(db: PgPool) {
        let site = Site::new(db);
        let (first, first_link) = site.invite("Kari", 1).await;
        let (second, second_link) = site.invite("Ola", 1).await;
        let form = "email=kari%40example.com&consent=yes";
        site.post(&format!("/r/{first_link}/email"), form).await;
        site.post(&format!("/r/{second_link}/email"), form).await;
        assert_eq!(
            queued(&site).await,
            [
                (first.id, Kind::ConfirmEmail),
                (second.id, Kind::ConfirmEmail)
            ]
        );

        let first_token = confirm_token(&site, &first, "kari@example.com");
        let second_token = confirm_token(&site, &second, "kari@example.com");
        let response = site.post(&format!("/e/{second_token}"), "").await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let response = site.post(&format!("/e/{first_token}"), "").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn changing_the_address_cancels_old_mail(db: PgPool) {
        let site = Site::new(db);
        let (guest, link) = site.invite("Kari", 1).await;
        let uri = format!("/r/{link}/email");
        site.post(&uri, "email=kari%40example.com&consent=yes")
            .await;
        let old_token = confirm_token(&site, &guest, "kari@example.com");
        site.post(&uri, "email=kari%40example.org&consent=yes")
            .await;

        assert_eq!(queued(&site).await, [(guest.id, Kind::ConfirmEmail)]);
        // The old confirmation link was for the old address.
        let response = site.post(&format!("/e/{old_token}"), "").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            site.find(&guest).await.unwrap().email.as_deref(),
            Some("kari@example.org")
        );
    }

    #[sqlx::test]
    async fn expired_or_forged_confirmations_fail(db: PgPool) {
        let site = Site::new(db);
        let (guest, link) = site.invite("Kari", 1).await;
        site.post(
            &format!("/r/{link}/email"),
            "email=kari%40example.com&consent=yes",
        )
        .await;

        let expired = site.config.link_key.email_link(
            guest.id,
            "kari@example.com",
            Utc::now() - Duration::seconds(1),
        );
        let forged = confirm_token(&site, &guest, "someone@example.com");
        for token in [expired.as_str(), &forged, "garbage"] {
            let response = site.get(&format!("/e/{token}?lang=en")).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{token}");
            assert!(body(response).await.contains("has expired"));
            assert_eq!(
                site.post(&format!("/e/{token}"), "").await.status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            site.find(&guest).await.unwrap().email_status,
            EmailStatus::Unconfirmed
        );
    }

    #[sqlx::test]
    async fn lost_link_only_goes_to_confirmed_addresses(db: PgPool) {
        let site = Site::new(db);
        let (guest, link) = site.invite("Kari", 1).await;
        site.post(
            &format!("/r/{link}/email"),
            "email=kari%40example.com&consent=yes",
        )
        .await;
        rsvp::confirm_email(&site.db, guest.id, "kari@example.com")
            .await
            .unwrap();
        sqlx::query!("delete from outbox")
            .execute(&site.db)
            .await
            .unwrap();

        let answers = [
            site.post("/lost?lang=en", "email=nobody%40example.com")
                .await,
            site.post("/lost?lang=en", "email=KARI%40example.com").await,
        ];
        let [unknown, known] = answers;
        assert_eq!(unknown.status(), known.status());
        assert_eq!(body(unknown).await, body(known).await);
        assert_eq!(queued_soon(&site).await, [(guest.id, Kind::RsvpLink)]);

        // Asking again while the mail waits doesn't queue another.
        mail_link(&site.db, "kari@example.com").await.unwrap();
        assert_eq!(queued(&site).await.len(), 1);
    }

    #[sqlx::test]
    async fn mail_requests_are_rate_limited(db: PgPool) {
        let site = Site::new(db);
        for _ in 0..5 {
            site.post("/lost", "email=someone%40example.com").await;
        }
        let response = site.post("/lost", "email=someone%40example.com").await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
