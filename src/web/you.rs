//! The Your details tab of a guest's pages, `/r/{link}/you`: their name, their e-mail
//! address, whether this device remembers them, and their data to download or delete.
//! The forms for the address and the device are handled in `email` and `remember`, and
//! come back here.

use askama::Template;
use axum::{
    Form, Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use super::{
    AppState, games,
    pages::{MessagePage, gone},
    remember,
    rsvp::{Guest, Notice, ShowQuery, redirect_you},
    views::{GuestTab, TabLink, guest_tabs},
};
use crate::{
    db::rsvp::{self, EmailStatus, Rsvp},
    http::{render, render_with, server_error},
    i18n::{Lang, Messages},
    paths::GuestLinks,
};

#[derive(Template)]
#[template(path = "you.html")]
pub(super) struct YouPage<'a> {
    lang: Lang,
    t: &'static Messages,
    title: &'a str,
    tabs: Vec<TabLink>,
    links: GuestLinks,
    rsvp: &'a Rsvp,
    email_confirmed: bool,
    /// Whether this device remembers the guest.
    remembered: bool,
    /// A one-off confirmation after a change, e.g. "Saved!".
    flash: Option<&'static str>,
    error: Option<&'static str>,
}

impl<'a> YouPage<'a> {
    /// The page for `guest`, as the device the request came from sees it. It looks up
    /// whether the event offers games, for the tabs, and leaves that tab out if it can't.
    pub(super) async fn new(
        state: &AppState,
        lang: Lang,
        guest: &'a Guest,
        headers: &HeaderMap,
    ) -> Self {
        let t = lang.messages();
        let links = GuestLinks::new(&guest.link, lang);
        let games = games::offers_any(state, &guest.event)
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "games not looked up for the tabs");
                false
            });
        Self {
            lang,
            t,
            title: guest.event.details.title(lang),
            tabs: guest_tabs(t, &links, games, GuestTab::You),
            links,
            rsvp: &guest.rsvp,
            email_confirmed: guest.rsvp.email_status == EmailStatus::Confirmed,
            remembered: remember::remembers(headers, state.config.is_https(), guest),
            flash: None,
            error: None,
        }
    }

    /// The page again with an error message, e.g. for an invalid form.
    pub(super) fn render_error(
        mut self,
        status: StatusCode,
        pick: impl Fn(&'static Messages) -> &'static str,
    ) -> Response {
        self.error = Some(pick(self.t));
        render_with(status, self)
    }
}

pub async fn show(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Response {
    let mut page = YouPage::new(&state, lang, &guest, &headers).await;
    page.flash = Notice::flash(query.notice.as_deref(), page.t);
    render(page)
}

#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<NameForm>,
) -> Response {
    let Some(name) = rsvp::normalize_name(&form.name, rsvp::NameKind::Person) else {
        return YouPage::new(&state, lang, &guest, &headers)
            .await
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name);
    };
    match rsvp::rename(&state.db, guest.rsvp.id, &name).await {
        Ok(true) => {
            tracing::info!(rsvp = %guest.rsvp.id, "rsvp renamed");
            redirect_you(&guest.link, lang, Notice::Saved)
        }
        Ok(false) => gone(lang),
        Err(err) => server_error(err),
    }
}

/// Everything kept about the guest (GDPR Art. 15 and 20): their reply, and their best
/// score in each game.
#[derive(serde::Serialize)]
struct Export<'a> {
    #[serde(flatten)]
    rsvp: &'a Rsvp,
    /// The host's question and the guest's pick, in their words rather than its place.
    #[serde(skip_serializing_if = "Option::is_none")]
    question: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    picked: Option<&'a str>,
    scores: Vec<crate::db::games::Best>,
}

pub async fn export(State(state): State<AppState>, guest: Guest) -> Response {
    let scores = match crate::db::games::of_guest(&state.db, guest.rsvp.id).await {
        Ok(scores) => scores,
        Err(err) => return server_error(err),
    };
    let question = guest.event.details.question(guest.rsvp.lang);
    let picked = guest.rsvp.choice.and_then(|pick| {
        let choices = &question.as_ref()?.1;
        choices.get(usize::try_from(pick).ok()?).copied()
    });
    let export = Export {
        rsvp: &guest.rsvp,
        question: question.as_ref().map(|(question, _)| *question),
        picked,
        scores,
    };
    (
        [(
            header::CONTENT_DISPOSITION,
            r#"attachment; filename="rsvp.json""#,
        )],
        Json(export),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct DeleteForm {
    confirm: Option<String>,
}

/// Deletes the RSVP for good (GDPR Art. 17).
pub async fn delete(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<DeleteForm>,
) -> Response {
    if form.confirm.is_none() {
        return YouPage::new(&state, lang, &guest, &headers)
            .await
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.confirm_delete);
    }
    match rsvp::delete(&state.db, guest.rsvp.id).await {
        Ok(_) => {
            tracing::info!(rsvp = %guest.rsvp.id, "rsvp deleted by guest");
            let mut response = render(MessagePage::new(lang, |t| {
                (&t.deleted_title, &t.deleted_text)
            }));
            // A device that remembers the guest forgets them too.
            let https = state.config.is_https();
            if remember::remembers(&headers, https, &guest) {
                let cookie = remember::forget_cookie(https, guest.event.id);
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests;
