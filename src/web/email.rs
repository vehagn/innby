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
    pages::MessagePage,
    rsvp::{Guest, Notice, RsvpPage, redirect},
};
use crate::{
    db::{
        outbox::{self, Kind},
        rsvp::{self, EmailStatus, Rsvp},
    },
    http::{ClientIp, render, render_with, server_error},
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
    let page = || RsvpPage::new(lang, &guest);
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
    match rsvp::find_by_confirmed_email(&mut *tx, guest.event_id, email).await? {
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

/// Queues the link for every invitation that has confirmed this address, one mail per
/// event.
async fn mail_link(db: &PgPool, email: &str) -> sqlx::Result<()> {
    for guest in rsvp::all_with_confirmed_email(db, email).await? {
        if outbox::enqueue_once(db, guest.id, Kind::RsvpLink).await? {
            tracing::info!(rsvp = %guest.id, "lost link mailed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
