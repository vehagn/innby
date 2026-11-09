//! Group sign-up at `/join/{link}`: the host makes a link per group of people, and anyone
//! with it can say they're coming with just a name, and gets their own RSVP link back,
//! which their device can remember, as on their page.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::gone,
    remember,
    views::{DisplayChoice, EventView, GuestList, GuestsPage, PartySize, Question},
};
use crate::{
    db::{
        event::{self, Event},
        group::{self, Group},
        rsvp::{self, NameDisplay, SignUp},
    },
    http::{ClientIp, cookie, render, render_with, server_error},
    i18n::{Lang, Messages},
    link::{IdLink, Purpose},
    paths,
};

#[derive(Template)]
#[template(path = "join.html")]
struct JoinPage<'a> {
    lang: Lang,
    t: &'static Messages,
    event: EventView<'a>,
    group: &'a Group,
    action: String,
    name: &'a str,
    party_sizes: Vec<PartySize>,
    display_choices: Vec<DisplayChoice>,
    /// The host's question, if the event asks one.
    question: Option<Question>,
    guests_link: String,
    accepts_answers: bool,
    /// Whether "remember me on this device" is ticked, as it was sent.
    remember: bool,
    error: Option<&'static str>,
}

impl<'a> JoinPage<'a> {
    fn new(lang: Lang, joining: &'a Joining, link: &'a str) -> Self {
        let t = lang.messages();
        let group = &joining.group;
        Self {
            lang,
            t,
            event: EventView::new(&joining.event.details, lang),
            group,
            action: paths::localized(&paths::path(paths::JOIN, link), lang),
            name: "",
            party_sizes: PartySize::choices(t, group.max_party_size, 1),
            // Signing up means coming, and coming shows the full name unless chosen otherwise.
            display_choices: DisplayChoice::choices(t, None, NameDisplay::Full),
            question: Question::new(&joining.event.details, lang, None),
            guests_link: paths::localized(&paths::path(paths::JOIN_GUESTS, link), lang),
            accepts_answers: joining.event.details.accepts_answers(Utc::now()),
            remember: false,
            error: None,
        }
    }

    fn render_error(
        mut self,
        status: StatusCode,
        pick: impl Fn(&'static Messages) -> &'static str,
    ) -> Response {
        self.error = Some(pick(self.t));
        render_with(status, self)
    }
}

/// A group behind a sign-up link, and the event it signs people up for.
struct Joining {
    group: Group,
    event: Event,
}

/// The group behind a sign-up link, unless the link is malformed, forged or revoked.
async fn find_group(state: &AppState, link: &str) -> sqlx::Result<Option<Joining>> {
    let Ok(parsed) = link.parse::<IdLink>() else {
        return Ok(None);
    };
    let group = group::find(&state.db, parsed.id).await?.filter(|group| {
        state
            .config
            .link_key
            .verify(Purpose::Group, &parsed, group.link_nonce)
    });
    let Some(group) = group else {
        return Ok(None);
    };
    let event = event::find(&state.db, group.event_id).await?;
    Ok(event.map(|event| Joining { group, event }))
}

pub async fn show(State(state): State<AppState>, lang: Lang, Path(link): Path<String>) -> Response {
    match find_group(&state, &link).await {
        Ok(Some(joining)) => render(JoinPage::new(lang, &joining, &link)),
        Ok(None) => gone(lang),
        Err(err) => server_error(err),
    }
}

/// The guest list, for someone deciding whether to sign up.
pub async fn guests(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
) -> Response {
    let joining = match find_group(&state, &link).await {
        Ok(Some(joining)) => joining,
        Ok(None) => return gone(lang),
        Err(err) => return server_error(err),
    };
    let guests = match GuestList::load(&state.db, joining.event.id, None).await {
        Ok(guests) => guests,
        Err(err) => return server_error(err),
    };
    let t = lang.messages();
    render(GuestsPage {
        lang,
        t,
        event: EventView::new(&joining.event.details, lang),
        guests,
        you: None,
        flash: None,
        back: paths::localized(&paths::path(paths::JOIN, &link), lang),
        back_label: &t.back_to_sign_up,
    })
}

#[derive(Deserialize)]
pub struct JoinForm {
    name: String,
    party_size: i32,
    name_display: Option<NameDisplay>,
    /// Ticked to have this device remember the new guest's link; see `remember`.
    remember: Option<String>,
    /// Their pick for the host's question: empty for none.
    choice: Option<String>,
    /// Hidden from people; bots fill it in.
    #[serde(default)]
    website: String,
}

pub async fn join(
    State(state): State<AppState>,
    lang: Lang,
    client: ClientIp,
    Path(link): Path<String>,
    Form(form): Form<JoinForm>,
) -> Response {
    let joining = match find_group(&state, &link).await {
        Ok(Some(joining)) => joining,
        Ok(None) => return gone(lang),
        Err(err) => return server_error(err),
    };
    let group = &joining.group;
    if !form.website.is_empty() {
        tracing::info!("group sign-up honeypot triggered");
        return Redirect::to("/").into_response();
    }
    let name_display = form.name_display.unwrap_or(NameDisplay::Full);
    let mut page = JoinPage::new(lang, &joining, &link);
    page.name = &form.name;
    page.display_choices = DisplayChoice::choices(page.t, None, name_display);
    page.remember = form.remember.is_some();
    let picked = Question::parse(&joining.event.details, form.choice.as_deref());
    page.question = Question::new(&joining.event.details, lang, picked.unwrap_or(None));
    if !page.accepts_answers {
        return render_with(StatusCode::FORBIDDEN, page);
    }
    let Some(name) = rsvp::normalize_name(&form.name, rsvp::NameKind::Person) else {
        return page.render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name);
    };
    if !(1..=group.max_party_size).contains(&form.party_size) {
        return page.render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_party_size);
    }
    let Ok(choice) = picked else {
        return page.render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_choice);
    };
    // Only valid submissions count, so a typo doesn't use up a household's allowance.
    if !state.join_limiter.check(client) {
        tracing::warn!("group sign-up rate limit hit");
        return page.render_error(StatusCode::TOO_MANY_REQUESTS, |t| &t.too_many_attempts);
    }

    let sign_up = SignUp {
        group,
        name: &name,
        party_size: form.party_size,
        name_display,
        lang,
    };
    let created = async {
        let mut tx = state.db.begin().await?;
        let rsvp = rsvp::create_open(&mut *tx, &sign_up, PRIVACY_VERSION).await?;
        rsvp::set_choice(&mut *tx, rsvp.id, choice).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(rsvp)
    };
    match created.await {
        Ok(rsvp) => {
            tracing::info!(rsvp = %rsvp.id, group = %group.id, "signed up through a group");
            let link = state
                .config
                .link_key
                .link(Purpose::Rsvp, rsvp.id, rsvp.link_nonce);
            let page = paths::localized(&paths::path(paths::RSVP, &link), lang);
            let redirect = Redirect::to(&format!("{page}&new")).into_response();
            if form.remember.is_none() {
                return redirect;
            }
            tracing::info!(rsvp = %rsvp.id, "device remembered");
            let https = state.config.is_https();
            cookie::with(
                redirect,
                remember::remember_cookie(https, &joining.event, &link),
            )
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests;
