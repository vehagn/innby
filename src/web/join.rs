//! Group sign-up at `/join/{link}`: the host makes a link per group of people, and anyone
//! with it can say they're coming with just a name, and gets their own RSVP link back,
//! which their device can remember, as on their page. Each form carries a token of its
//! own, so the same form sent twice, as when the answer to the first was lost on the
//! way, leads to the first sign-up rather than making a second.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use chrono::Utc;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{Page, PageError},
    remember,
    views::{DisplayChoice, EventView, FormPage, GuestList, GuestsPage, PartySize, Question},
};
use crate::{
    db::{
        event::{self, Event},
        group::{self, Group},
        rsvp::{self, NameDisplay, SignUp},
    },
    http::{ClientIp, cookie, render, render_with},
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
    /// This form's own token, so sending it twice signs up once.
    token: String,
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
            token: crate::random::token(),
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
}

impl FormPage for JoinPage<'_> {
    fn error_slot(&mut self) -> (&'static Messages, &mut Option<&'static str>) {
        (self.t, &mut self.error)
    }
}

/// A group behind a sign-up link, and the event it signs people up for.
struct Joining {
    group: Group,
    event: Event,
}

impl Joining {
    /// The group behind a sign-up link. A link that's malformed, forged or revoked is
    /// gone, like a guest's.
    async fn find(state: &AppState, link: &str, lang: Lang) -> Result<Self, PageError> {
        let gone = PageError::Gone(lang);
        let Ok(parsed) = link.parse::<IdLink>() else {
            return Err(gone);
        };
        let group = group::find(&state.db, parsed.id).await?.filter(|group| {
            state
                .config
                .link_key
                .verify(Purpose::Group, &parsed, group.link_nonce)
        });
        let Some(group) = group else {
            return Err(gone);
        };
        let event = event::find(&state.db, group.event_id).await?.ok_or(gone)?;
        Ok(Self { group, event })
    }
}

pub async fn show(State(state): State<AppState>, lang: Lang, Path(link): Path<String>) -> Page {
    let joining = Joining::find(&state, &link, lang).await?;
    Ok(render(JoinPage::new(lang, &joining, &link)))
}

/// The guest list, for someone deciding whether to sign up.
pub async fn guests(State(state): State<AppState>, lang: Lang, Path(link): Path<String>) -> Page {
    let joining = Joining::find(&state, &link, lang).await?;
    let guests = GuestList::load(&state.db, joining.event.id, None).await?;
    let t = lang.messages();
    Ok(render(GuestsPage {
        lang,
        t,
        event: EventView::new(&joining.event.details, lang),
        guests,
        you: None,
        flash: None,
        tabs: Vec::new(),
        back: Some((
            paths::localized(&paths::path(paths::JOIN, &link), lang),
            &t.back_to_sign_up,
        )),
    }))
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
    /// The form's token; see `JoinPage::token`.
    token: Option<String>,
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
) -> Page {
    let joining = Joining::find(&state, &link, lang).await?;
    let group = &joining.group;
    if !form.website.is_empty() {
        tracing::info!("group sign-up honeypot triggered");
        return Ok(Redirect::to("/").into_response());
    }
    let name_display = form.name_display.unwrap_or(NameDisplay::Full);
    let mut page = JoinPage::new(lang, &joining, &link);
    page.name = &form.name;
    page.display_choices = DisplayChoice::choices(page.t, None, name_display);
    page.remember = form.remember.is_some();
    // A token as the form has it, or none from a made-up request: then there's
    // nothing to find a first sign-up by.
    let token = form
        .token
        .as_deref()
        .filter(|token| crate::random::is_token(token));
    if let Some(token) = token {
        // Shown again to fix, it's still the same form.
        token.clone_into(&mut page.token);
    }
    let picked = Question::parse(&joining.event.details, form.choice.as_deref());
    page.question = Question::new(&joining.event.details, lang, picked.unwrap_or(None));
    if !page.accepts_answers {
        return Ok(render_with(StatusCode::FORBIDDEN, page));
    }
    let refuse = |status, pick: fn(&'static Messages) -> &'static str| -> Page {
        Ok(page.render_error(status, pick))
    };
    let Some(name) = rsvp::normalize_name(&form.name, rsvp::NameKind::Person) else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name);
    };
    if !(1..=group.max_party_size).contains(&form.party_size) {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_party_size);
    }
    let Ok(choice) = picked else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_choice);
    };
    // Only valid submissions count, so a typo doesn't use up a household's allowance.
    if !state.join_limiter.check(client) {
        tracing::warn!("group sign-up rate limit hit");
        return refuse(StatusCode::TOO_MANY_REQUESTS, |t| &t.too_many_attempts);
    }

    let sign_up = SignUp {
        group,
        name: &name,
        party_size: form.party_size,
        name_display,
        lang,
        choice,
        token,
    };
    let (rsvp, created) = rsvp::create_open(&state.db, &sign_up, PRIVACY_VERSION).await?;
    if created {
        tracing::info!(rsvp = %rsvp.id, group = %group.id, "signed up through a group");
    } else {
        tracing::info!(rsvp = %rsvp.id, group = %group.id, "sign-up sent again");
    }
    let link = rsvp.link(&state.config.link_key);
    let page = paths::localized(&paths::path(paths::RSVP, &link), lang);
    let redirect = Redirect::to(&format!("{page}&new")).into_response();
    if form.remember.is_none() {
        return Ok(redirect);
    }
    tracing::info!(rsvp = %rsvp.id, "device remembered");
    let https = state.config.is_https();
    let cookie = remember::remember_cookie(https, &joining.event, rsvp.id, &link);
    Ok(cookie::with(redirect, cookie))
}

#[cfg(test)]
mod tests;
