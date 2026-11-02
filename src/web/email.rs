//! The optional e-mail address: adding it with consent, the double opt-in, removing it,
//! and `/lost` for guests who lost their link.

use std::net::{IpAddr, Ipv4Addr};

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{MessagePage, render, render_with, server_error},
    rate_limit::ClientIp,
    rsvp::{Notice, RsvpPage, load, redirect},
};
use crate::{
    db::{
        outbox::{self, Kind},
        rsvp,
    },
    i18n::{Lang, Messages},
    link::EmailLink,
};

#[derive(Deserialize)]
pub struct AddForm {
    email: String,
    consent: Option<String>,
}

/// Stores the address unconfirmed and queues the confirmation mail. If another RSVP
/// already has the address, its owner gets a heads-up instead; the guest sees the same
/// response either way.
pub async fn add(
    State(state): State<AppState>,
    lang: Lang,
    ClientIp(client): ClientIp,
    Path(link): Path<String>,
    Form(form): Form<AddForm>,
) -> Response {
    let guest = match load(&state, lang, &link).await {
        Ok(guest) => guest,
        Err(err) => return err.into_response(),
    };
    let invalid = |pick: fn(&Messages) -> &'static str| {
        render_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            RsvpPage::new(&state, lang, &link, &guest).with_error(pick),
        )
    };
    if form.consent.is_none() {
        return invalid(|t| t.consent_required);
    }
    let Some(email) = rsvp::normalize_email(&form.email) else {
        return invalid(|t| t.invalid_email);
    };
    // Each attempt can mail an address the guest typed in, so it's limited like /lost.
    if !state
        .mail_limiter
        .check(client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    {
        tracing::warn!("add e-mail rate limit hit");
        return render_with(
            StatusCode::TOO_MANY_REQUESTS,
            RsvpPage::new(&state, lang, &link, &guest).with_error(|t| t.too_many_attempts),
        );
    }

    match add_email(&state, &guest, &email, lang).await {
        Ok(()) => redirect(&link, lang, Notice::EmailSent),
        Err(err) => server_error(err),
    }
}

async fn add_email(
    state: &AppState,
    guest: &rsvp::Rsvp,
    email: &str,
    lang: Lang,
) -> sqlx::Result<()> {
    if guest.email.as_deref() == Some(email) {
        // Already there: at most resend the confirmation.
        if guest.email_status == rsvp::EmailStatus::Unconfirmed {
            outbox::enqueue_once(&state.db, guest.id, Kind::ConfirmEmail).await?;
        }
        return Ok(());
    }
    let mut tx = state.db.begin().await?;
    if let Some(owner) = rsvp::find_by_email(&mut *tx, email).await? {
        outbox::enqueue_once(&mut *tx, owner.id, Kind::EmailInUse).await?;
        tracing::info!(rsvp = %guest.id, "e-mail address already in use");
        return tx.commit().await;
    }
    match rsvp::set_email(&mut *tx, guest.id, email, lang, PRIVACY_VERSION).await {
        Ok(_) => {}
        // Someone else took the address a moment ago; nothing to tell this guest.
        Err(err)
            if err
                .as_database_error()
                .is_some_and(|err| err.is_unique_violation()) =>
        {
            return Ok(());
        }
        Err(err) => return Err(err),
    }
    // Mail queued for the previous address must not go out.
    outbox::cancel_unsent(&mut *tx, guest.id).await?;
    outbox::enqueue(&mut *tx, guest.id, Kind::ConfirmEmail).await?;
    tx.commit().await?;
    tracing::info!(rsvp = %guest.id, "e-mail address added");
    Ok(())
}

/// Withdraws consent: the address and any mail waiting for it are deleted.
pub async fn remove(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
) -> Response {
    let guest = match load(&state, lang, &link).await {
        Ok(guest) => guest,
        Err(err) => return err.into_response(),
    };
    let result = async {
        let mut tx = state.db.begin().await?;
        rsvp::remove_email(&mut *tx, guest.id).await?;
        outbox::cancel_unsent(&mut *tx, guest.id).await?;
        tx.commit().await
    }
    .await;
    match result {
        Ok(()) => {
            tracing::info!(rsvp = %guest.id, "e-mail address removed");
            redirect(&link, lang, Notice::EmailRemoved)
        }
        Err(err) => server_error(err),
    }
}

#[derive(Template)]
#[template(path = "confirm_email.html")]
struct ConfirmPage {
    lang: Lang,
    t: &'static Messages,
    token: String,
}

fn invalid_confirmation(lang: Lang) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        MessagePage::new(lang, |t| (t.gone_title, t.confirm_invalid_text)),
    )
}

/// Checks a confirmation link against the guest's current address and the clock.
async fn verify(
    state: &AppState,
    token: &str,
) -> Result<Option<(rsvp::Rsvp, String)>, sqlx::Error> {
    let Ok(link) = token.parse::<EmailLink>() else {
        return Ok(None);
    };
    if link.expires <= Utc::now() {
        return Ok(None);
    }
    let Some(guest) = rsvp::find(&state.db, link.id).await? else {
        return Ok(None);
    };
    let valid = guest.email_status == rsvp::EmailStatus::Unconfirmed
        && guest
            .email
            .as_deref()
            .is_some_and(|email| state.config.link_key.verify_email(&link, email));
    Ok(valid.then(|| {
        let email = guest.email.clone().unwrap_or_default();
        (guest, email)
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
            token,
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
    let (guest, email) = match verify(&state, &token).await {
        Ok(Some(found)) => found,
        Ok(None) => return invalid_confirmation(lang),
        Err(err) => return server_error(err),
    };
    let result = async {
        let mut tx = state.db.begin().await?;
        let confirmed = rsvp::confirm_email(&mut *tx, guest.id, &email).await?;
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
            let link = state.config.link_key.rsvp_link(guest.id, guest.link_nonce);
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

pub async fn lost_page(lang: Lang) -> Response {
    render(LostPage {
        lang,
        t: lang.messages(),
        error: None,
    })
}

#[derive(Deserialize)]
pub struct LostForm {
    email: String,
}

/// Mails the guest's link to a confirmed address. The answer never says whether the
/// address is registered.
pub async fn lost(
    State(state): State<AppState>,
    lang: Lang,
    ClientIp(client): ClientIp,
    Form(form): Form<LostForm>,
) -> Response {
    let page = |status, error: fn(&Messages) -> &'static str| {
        let t = lang.messages();
        render_with(
            status,
            LostPage {
                lang,
                t,
                error: Some(error(t)),
            },
        )
    };
    let Some(email) = rsvp::normalize_email(&form.email) else {
        return page(StatusCode::UNPROCESSABLE_ENTITY, |t| t.invalid_email);
    };
    if !state
        .mail_limiter
        .check(client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    {
        tracing::warn!("lost link rate limit hit");
        return page(StatusCode::TOO_MANY_REQUESTS, |t| t.too_many_attempts);
    }
    let result = async {
        match rsvp::find_by_email(&state.db, &email).await? {
            Some(guest) if guest.email_status == rsvp::EmailStatus::Confirmed => {
                let queued = outbox::enqueue_once(&state.db, guest.id, Kind::RsvpLink).await?;
                if queued {
                    tracing::info!(rsvp = %guest.id, "lost link mailed");
                }
                Ok(())
            }
            _ => Ok::<_, sqlx::Error>(()),
        }
    }
    .await;
    match result {
        Ok(()) => render(MessagePage::new(lang, |t| {
            (t.lost_sent_title, t.lost_sent_text)
        })),
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use sqlx::PgPool;

    use super::*;
    use crate::{
        db::rsvp::EmailStatus,
        web::testing::{Site, body, location},
    };

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

    fn confirm_token(site: &Site, guest: &rsvp::Rsvp, email: &str) -> String {
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
        assert_eq!(location(&response), format!("/r/{link}?lang=en&email_sent"));
        let added = site.find(&guest).await.unwrap();
        assert_eq!(added.email.as_deref(), Some("kari@example.com"));
        assert_eq!(
            (added.email_status, added.lang),
            (EmailStatus::Unconfirmed, Lang::En)
        );
        assert_eq!(queued(&site).await, [(guest.id, Kind::ConfirmEmail)]);

        let html = body(site.get(&format!("/r/{link}?lang=en&email_sent")).await).await;
        assert!(html.contains("Check your inbox to confirm the address."));
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
            format!("/r/{link}?lang=en&email_confirmed")
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
            format!("/r/{link}?lang=nb&email_removed")
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

        let response = site.post(&format!("/r/{other_link}/email"), form).await;
        assert_eq!(
            location(&response),
            format!("/r/{other_link}?lang=nb&email_sent")
        );
        assert_eq!(site.find(&other).await.unwrap().email, None);
        assert_eq!(
            queued(&site).await,
            [(owner.id, Kind::ConfirmEmail), (owner.id, Kind::EmailInUse)]
        );
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
        assert_eq!(queued(&site).await, [(guest.id, Kind::RsvpLink)]);

        // Asking again while the mail waits doesn't queue another.
        site.post("/lost", "email=kari%40example.com").await;
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
