//! The Your details tab of a guest's pages, `/r/{link}/you`: what they tell the host
//! besides whether they're coming (how many, how their name shows on the guest list, the
//! host's questions), their name, their e-mail address, whether this device remembers
//! them, and their data to download or delete. Their reply page asks only whether
//! they're coming, so it stays one tap. The forms for the address and the device are
//! handled in `email` and `remember`, and come back here.

use askama::Template;
use axum::{
    Form, Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use jiff::Timestamp;
use serde::Deserialize;

use super::{
    AppState, PRIVACY_VERSION,
    pages::{MessagePage, Page, PageError},
    remember,
    rsvp::{Guest, Notice, ShowQuery},
    views::{DisplayChoice, FormPage, GuestTab, OpenQuestion, PartySize, Question, SentAnswers},
};
use crate::{
    db::rsvp::{self, EmailStatus, NameDisplay, Rsvp},
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
    /// Whether the details may still change: not after the event's answers close.
    accepts_answers: bool,
    party_sizes: Vec<PartySize>,
    /// The guest's choice, or their full name if they haven't made one, since that's
    /// the default once they're coming.
    display_choices: Vec<DisplayChoice>,
    /// The host's question, with the guest's pick, if the event asks one.
    question: Option<Question>,
    /// The open questions, with the guest's answers.
    open_questions: Vec<OpenQuestion>,
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
        let (rsvp, event) = (&guest.rsvp, &guest.event.details);
        let chosen = rsvp.name_display.unwrap_or(NameDisplay::Full);
        Self {
            lang,
            t,
            title: event.title(lang),
            tabs: guest.tabs(state, &links, GuestTab::You),
            links,
            rsvp,
            accepts_answers: event.accepts_answers(Timestamp::now()),
            party_sizes: PartySize::choices(t, rsvp.max_party_size, rsvp.party_size),
            display_choices: DisplayChoice::choices(t, Some(&rsvp.name), chosen),
            question: Question::new(event, lang, rsvp.choice),
            open_questions: OpenQuestion::all(event, lang, &rsvp.answers),
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
    guest: Guest,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Response {
    let lang = guest.lang;
    let mut page = YouPage::new(&state, lang, &guest, &headers);
    page.flash = Notice::flash(query.notice.as_deref(), page.t);
    render(page)
}

#[derive(Deserialize)]
pub struct DetailsForm {
    party_size: i32,
    name_display: Option<NameDisplay>,
    /// Their pick for the host's question: empty for none.
    choice: Option<String>,
    /// Their answers to the open questions, a field each; see `OpenQuestion::parse`.
    #[serde(default)]
    answer_0: String,
    #[serde(default)]
    answer_1: String,
    #[serde(default)]
    answer_2: String,
    #[serde(default)]
    answer_3: String,
    #[serde(default)]
    answer_4: String,
}

impl DetailsForm {
    fn answers(&self) -> SentAnswers<'_> {
        [
            &self.answer_0,
            &self.answer_1,
            &self.answer_2,
            &self.answer_3,
            &self.answer_4,
        ]
    }
}

/// Changes how many are coming, how the guest's name shows on the guest list, and their
/// answers to the host's questions, leaving whether they're coming as it is. None is
/// needed before a "coming" counts: a guest who says so and no more is one person, with
/// nothing picked or answered.
pub async fn details(
    State(state): State<AppState>,
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<DetailsForm>,
) -> Page {
    let lang = guest.lang;
    let event = &guest.event.details;
    let refuse = |status, pick: fn(&'static Messages) -> &'static str| -> Page {
        let mut page = YouPage::new(&state, lang, &guest, &headers);
        // Shown again as sent, so nothing typed is lost.
        let sent: Vec<String> = form.answers().iter().map(|&a| a.to_owned()).collect();
        page.open_questions = OpenQuestion::all(event, lang, &sent);
        Ok(page.render_error(status, pick))
    };
    if !event.accepts_answers(Timestamp::now()) {
        return refuse(StatusCode::FORBIDDEN, |t| &t.answers_closed);
    }
    if !(1..=guest.rsvp.max_party_size).contains(&form.party_size) {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_party_size);
    }
    let Ok(choice) = Question::parse(event, form.choice.as_deref()) else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_choice);
    };
    let Ok(answers) = OpenQuestion::parse(event, form.answers()) else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_answer);
    };
    // The form preselects the full name for a guest who hasn't chosen, as that's what
    // coming shows. Left so, the choice stays unmade: a guest who hasn't answered yet
    // goes on showing as first name and initial, as they would have before saving.
    let name_display = match (guest.rsvp.name_display, form.name_display) {
        (None, Some(NameDisplay::Full)) => None,
        (_, sent) => sent,
    };
    let details = rsvp::Details {
        party_size: form.party_size,
        name_display,
        choice,
        answers,
    };
    rsvp::set_details(&state.db, guest.rsvp.id, &details, lang, PRIVACY_VERSION)
        .await?
        .ok_or(PageError::Gone(lang))?;
    tracing::info!(rsvp = %guest.rsvp.id, "rsvp details changed");
    Ok(Notice::Saved.redirect(paths::RSVP_YOU, &guest.link, lang))
}

#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<NameForm>,
) -> Page {
    let lang = guest.lang;
    let Some(name) = rsvp::normalize_name(&form.name, rsvp::NameKind::Person) else {
        return Ok(YouPage::new(&state, lang, &guest, &headers)
            .render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name));
    };
    // The same name again changes nothing, so it isn't counted.
    if name != guest.rsvp.name && !state.name_limiter.check_guest(guest.rsvp.id) {
        return Ok(YouPage::new(&state, lang, &guest, &headers)
            .render_error(StatusCode::TOO_MANY_REQUESTS, |t| &t.name_error_busy));
    }
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
    /// The open questions they answered, each with its answer, in their words too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    open_answers: Vec<OpenAnswer<'a>>,
    scores: Vec<crate::db::games::Best>,
    /// What they wrote on the event's board, and how they voted there.
    posts: Vec<crate::db::board::Written>,
    votes: Vec<crate::db::board::Voted>,
}

#[derive(serde::Serialize)]
struct OpenAnswer<'a> {
    question: &'a str,
    answer: &'a str,
}

pub async fn export(State(state): State<AppState>, guest: Guest) -> Page {
    let scores = crate::db::games::of_guest(&state.db, guest.rsvp.id).await?;
    let posts = crate::db::board::written_by(&state.db, guest.rsvp.id).await?;
    let votes = crate::db::board::votes_by(&state.db, guest.rsvp.id).await?;
    let question = guest.event.details.question(guest.rsvp.lang);
    let picked = guest.rsvp.pick().and_then(|pick| {
        let choices = &question.as_ref()?.1;
        choices.get(pick).copied()
    });
    let open_answers = guest
        .event
        .details
        .open_questions(guest.rsvp.lang)
        .into_iter()
        .enumerate()
        .map(|(i, question)| OpenAnswer {
            question,
            answer: guest.rsvp.answer(i),
        })
        .filter(|open| !open.answer.is_empty())
        .collect();
    let export = Export {
        rsvp: &guest.rsvp,
        question: question.as_ref().map(|(question, _)| *question),
        picked,
        open_answers,
        scores,
        posts,
        votes,
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
    guest: Guest,
    headers: HeaderMap,
    Form(form): Form<DeleteForm>,
) -> Page {
    let lang = guest.lang;
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
