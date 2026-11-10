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
    pages::{MessagePage, Page, PageError},
    remember,
    rsvp::{Guest, Notice, ShowQuery},
    views::{FormPage, GuestTab, guest_tabs},
};
use crate::{
    db::rsvp::{self, EmailStatus, Rsvp},
    http::{cookie, render},
    i18n::{Lang, Messages},
    icons::TabLink,
    paths::{self, GuestLinks},
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
    /// The page for `guest`, as the device the request came from sees it.
    pub(super) fn new(state: &AppState, lang: Lang, guest: &'a Guest, headers: &HeaderMap) -> Self {
        let t = lang.messages();
        let links = GuestLinks::new(&guest.link, lang);
        let games = games::offers_any(state, guest);
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
}

impl FormPage for YouPage<'_> {
    fn error_slot(&mut self) -> (&'static Messages, &mut Option<&'static str>) {
        (self.t, &mut self.error)
    }
}

pub async fn show(
    State(state): State<AppState>,
    lang: Lang,
    guest: Guest,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Response {
    let mut page = YouPage::new(&state, lang, &guest, &headers);
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
) -> Page {
    let Some(name) = rsvp::normalize_name(&form.name, rsvp::NameKind::Person) else {
        return Ok(YouPage::new(&state, lang, &guest, &headers)
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name));
    };
    if !rsvp::rename(&state.db, guest.rsvp.id, &name).await? {
        return Err(PageError::Gone(lang));
    }
    tracing::info!(rsvp = %guest.rsvp.id, "rsvp renamed");
    Ok(Notice::Saved.redirect(paths::RSVP_YOU, &guest.link, lang))
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

pub async fn export(State(state): State<AppState>, guest: Guest) -> Page {
    let scores = crate::db::games::of_guest(&state.db, guest.rsvp.id).await?;
    let question = guest.event.details.question(guest.rsvp.lang);
    let picked = guest.rsvp.pick().and_then(|pick| {
        let choices = &question.as_ref()?.1;
        choices.get(pick).copied()
    });
    let export = Export {
        rsvp: &guest.rsvp,
        question: question.as_ref().map(|(question, _)| *question),
        picked,
        scores,
    };
    let disposition = (
        header::CONTENT_DISPOSITION,
        r#"attachment; filename="rsvp.json""#,
    );
    Ok(([disposition], Json(export)).into_response())
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
) -> Page {
    if form.confirm.is_none() {
        return Ok(YouPage::new(&state, lang, &guest, &headers)
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.confirm_delete));
    }
    rsvp::delete(&state.db, guest.rsvp.id).await?;
    tracing::info!(rsvp = %guest.rsvp.id, "rsvp deleted by guest");
    let deleted = render(MessagePage::new(lang, |t| {
        (&t.deleted_title, &t.deleted_text)
    }));
    // A device that remembers the guest forgets them too.
    let https = state.config.is_https();
    Ok(match remember::remembers(&headers, https, &guest) {
        true => cookie::with(deleted, remember::forget_cookie(https, guest.rsvp.id)),
        false => deleted,
    })
}

#[cfg(test)]
mod tests;
