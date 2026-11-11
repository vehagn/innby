//! An event's guests: the Guests tab with its headcount, each guest's page (edit, new
//! link, delete, QR code), the CSV export and reminders.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono_tz::Tz;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    forms::{DeleteForm, NameForm, SavedQuery},
    lang,
    page::{AdminState, PageError, load_event, redirect, saved, timestamp},
    qr::qr_file,
    role::Role,
};
use crate::{
    config::Config,
    db::{
        self,
        event::Event,
        group::{self, Group},
        outbox,
        rsvp::{self, EmailStatus, Headcount, MAX_PARTY_SIZE, NameDisplay, NameKind, Rsvp},
    },
    http::{render, render_with},
    i18n::{count, filled},
    paths::{self, AdminLinks, EventLinks},
};

/// One guest as the admin tables show it.
pub(super) struct GuestRow {
    /// The guest's admin page.
    pub(super) path: String,
    pub(super) name: String,
    /// An invitation, or which group link the guest signed up through: those the row is
    /// given are the event's, or the one it's shown under.
    pub(super) source: String,
    /// The admin page of that group.
    pub(super) group_path: Option<String>,
    pub(super) answer: &'static str,
    /// The answer as a class for its colour: `pending`, `attending` or `declined`.
    pub(super) status: &'static str,
    pub(super) party: String,
    pub(super) email: &'static str,
    /// How other guests see the name in the guest list.
    pub(super) listed: &'static str,
    pub(super) responded: String,
    pub(super) created: String,
    /// Their pick for the host's question, if the event asks one; see `with_choice`.
    pub(super) choice: String,
}

impl GuestRow {
    pub(super) fn new(rsvp: &Rsvp, tz: Tz, groups: &[Group]) -> Self {
        let group = groups.iter().find(|group| Some(group.id) == rsvp.group_id);
        let text = lang::text();
        Self {
            path: paths::admin(&paths::path(paths::GUEST, rsvp.id)),
            name: rsvp.name.clone(),
            source: match group {
                Some(group) => filled(&text.source_group, "group", &group.name),
                None => text.source_invitation.clone(),
            },
            group_path: rsvp
                .group_id
                .map(|id| paths::admin(&paths::path(paths::GROUP, id))),
            answer: match rsvp.response {
                rsvp::Response::Pending => &text.answer_pending,
                rsvp::Response::Attending => &text.answer_attending,
                rsvp::Response::Declined => &text.answer_declined,
            },
            status: rsvp.response.code(),
            party: format!("{} / {}", rsvp.party_size, rsvp.max_party_size),
            email: match rsvp.email_status {
                EmailStatus::None => "",
                EmailStatus::Unconfirmed => &text.email_unconfirmed,
                EmailStatus::Confirmed => &text.email_confirmed,
            },
            listed: match rsvp.listed() {
                NameDisplay::Full => &text.listed_full,
                NameDisplay::Partial => &text.listed_partial,
                NameDisplay::Hidden => &text.listed_hidden,
            },
            responded: rsvp
                .responded_at
                .map(|at| timestamp(at, tz))
                .unwrap_or_default(),
            created: timestamp(rsvp.created_at, tz),
            choice: String::new(),
        }
    }

    /// The row with the guest's pick among `choices`, as the host reads them.
    pub(super) fn with_choice(mut self, choices: &[&str], rsvp: &Rsvp) -> Self {
        self.choice = rsvp
            .pick()
            .and_then(|pick| choices.get(pick))
            .map_or_else(String::new, |choice| (*choice).to_owned());
        self
    }
}

/// The host's question and how many coming picked each choice, for the Guests tab.
pub(super) struct Tally {
    pub(super) question: String,
    pub(super) counts: Vec<(String, usize)>,
}

#[derive(Template)]
#[template(path = "admin/guests.html")]
pub(super) struct GuestsTab {
    pub(super) event: Event,
    pub(super) links: EventLinks,
    pub(super) headcount: Headcount,
    /// Guests with a confirmed address.
    pub(super) subscribers: usize,
    /// The guests the table shows: everyone, or those with one answer.
    pub(super) rows: Vec<GuestRow>,
    /// The answer the table is filtered to, as in `?show=`.
    pub(super) show: Option<&'static str>,
    /// The host's question, if the event asks one, with its picks among those coming.
    pub(super) tally: Option<Tally>,
    pub(super) notice: Option<String>,
    pub(super) error: Option<String>,
}

impl GuestsTab {
    /// Whether the table shows only the guests with this answer.
    pub(super) fn showing(&self, code: &str) -> bool {
        self.show == Some(code)
    }

    /// The table with only the guests with this answer.
    pub(super) fn show_link(&self, code: &str) -> String {
        let page = paths::path(paths::EVENT, self.event.id);
        paths::admin(&format!("{page}?show={code}"))
    }
}

#[derive(Deserialize)]
pub(super) struct GuestsQuery {
    /// `pending`, `attending` or `declined`.
    pub(super) show: Option<String>,
    pub(super) reminders: Option<usize>,
    /// The event was just created.
    pub(super) created: Option<String>,
}

/// The event's guests, with the headcount on top; its figures filter the table.
pub(super) async fn list(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<GuestsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let (rsvps, headcount, groups) = tokio::try_join!(
        rsvp::list(&state.db, id),
        rsvp::headcount(&state.db, id),
        group::list(&state.db, id),
    )?;
    let show = rsvp::Response::ALL
        .into_iter()
        .find(|response| query.show.as_deref() == Some(response.code()));
    let tz = event.details.starts_at.timezone();
    let text = lang::text();
    let question = event.details.question(lang::current());
    let choices = question
        .as_ref()
        .map_or(&[][..], |(_, choices)| &choices[..]);
    let tally = question.as_ref().map(|(question, choices)| Tally {
        question: (*question).to_owned(),
        counts: (0..choices.len())
            .map(|i| {
                let picked = rsvps.iter().filter(|rsvp| {
                    rsvp.response == rsvp::Response::Attending && rsvp.pick() == Some(i)
                });
                (choices[i].to_owned(), picked.count())
            })
            .collect(),
    });
    let rows = rsvps
        .iter()
        .filter(|rsvp| show.is_none_or(|show| rsvp.response == show))
        .map(|rsvp| GuestRow::new(rsvp, tz, &groups).with_choice(choices, rsvp))
        .collect();
    Ok(render(GuestsTab {
        links: EventLinks::new(id),
        event,
        headcount,
        subscribers: rsvps
            .iter()
            .filter(|rsvp| rsvp.email_status == EmailStatus::Confirmed)
            .count(),
        rows,
        show: show.map(rsvp::Response::code),
        tally,
        notice: match (query.reminders, query.created) {
            (Some(n), _) => Some(count(
                n,
                &text.reminders_queued_one,
                &text.reminders_queued_other,
            )),
            (None, Some(_)) => Some(text.event_created.clone()),
            (None, None) => None,
        },
        error: None,
    }))
}

#[derive(Template)]
#[template(path = "admin/guest.html")]
pub(super) struct GuestPage {
    pub(super) event: Event,
    pub(super) rsvp: Rsvp,
    pub(super) row: GuestRow,
    pub(super) links: AdminLinks,
    pub(super) url: String,
    pub(super) qr: String,
    pub(super) max_party_size: i32,
    pub(super) saved: bool,
    pub(super) error: Option<String>,
}

impl GuestPage {
    pub(super) fn new(
        config: &Config,
        event: Event,
        rsvp: Rsvp,
        groups: &[Group],
        style: &db::qr::Style,
    ) -> Self {
        let url = guest_url(config, &rsvp);
        Self {
            row: GuestRow::new(&rsvp, event.details.starts_at.timezone(), groups),
            event,
            links: AdminLinks::guest(rsvp.id),
            qr: super::qr::svg(&url, style, None),
            url,
            rsvp,
            max_party_size: MAX_PARTY_SIZE,
            saved: false,
            error: None,
        }
    }

    /// The guest's page again with an error, for an invalid form.
    pub(super) async fn load(state: &AdminState, id: Uuid) -> Result<Self, PageError> {
        let rsvp = rsvp::find(&state.db, id).await?.ok_or(PageError::Missing)?;
        let (event, groups, style) = tokio::try_join!(
            load_event(state, rsvp.event_id),
            async { Ok::<_, PageError>(group::list(&state.db, rsvp.event_id).await?) },
            async { Ok::<_, PageError>(db::qr::style(&state.db, rsvp.event_id).await?) },
        )?;
        Ok(Self::new(&state.config, event, rsvp, &groups, &style))
    }

    pub(super) async fn render_error(
        state: &AdminState,
        id: Uuid,
        error: String,
    ) -> Result<Response, PageError> {
        let mut page = Self::load(state, id).await?;
        page.error = Some(error);
        Ok(render_with(StatusCode::UNPROCESSABLE_ENTITY, page))
    }
}

pub(super) fn guest_url(config: &Config, rsvp: &Rsvp) -> String {
    config.url(&paths::path(paths::RSVP, rsvp.link(&config.link_key)))
}

/// The guest's QR code as a file, for printing.
pub(super) async fn qr(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let rsvp = rsvp::find(&state.db, id).await?.ok_or(PageError::Missing)?;
    let style = db::qr::style(&state.db, rsvp.event_id).await?;
    Ok(qr_file(
        &guest_url(&state.config, &rsvp),
        &style,
        &rsvp.name,
    ))
}

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let mut page = GuestPage::load(&state, id).await?;
    page.saved = query.saved.is_some();
    Ok(render(page))
}

pub(super) async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<NameForm>,
) -> Result<Response, PageError> {
    let name = match form.validate(NameKind::Person) {
        Ok(name) => name,
        Err(error) => return GuestPage::render_error(&state, id, error).await,
    };
    if !rsvp::update_invite(&state.db, id, &name, form.max_party_size).await? {
        return Err(PageError::Missing);
    }
    Ok(saved(paths::GUEST, id))
}

pub(super) async fn revoke(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    rsvp::revoke_link(&state.db, id)
        .await?
        .ok_or(PageError::Missing)?;
    tracing::info!(rsvp = %id, "link revoked");
    Ok(saved(paths::GUEST, id))
}

pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<DeleteForm>,
) -> Result<Response, PageError> {
    if form.confirm.is_none() {
        let error = lang::text().error_confirm_delete.clone();
        return GuestPage::render_error(&state, id, error).await;
    }
    let rsvp = rsvp::find(&state.db, id).await?.ok_or(PageError::Missing)?;
    rsvp::delete(&state.db, id).await?;
    tracing::info!(rsvp = %id, "rsvp deleted by host");
    let event = paths::path(paths::EVENT, rsvp.event_id);
    Ok(redirect(&event))
}

/// `columns` without those at these places.
fn drop_columns<'a>(columns: Vec<&'a str>, places: &[usize]) -> Vec<&'a str> {
    columns
        .into_iter()
        .enumerate()
        .filter(|(place, _)| !places.contains(place))
        .map(|(_, column)| column)
        .collect()
}

/// The event's guests as CSV, with each guest's address and link for a mail merge; for an
/// organiser, without those, which would let them act as a guest.
pub(super) async fn csv(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let (rsvps, groups) = tokio::try_join!(rsvp::list(&state.db, id), group::list(&state.db, id))?;
    let tz = event.details.starts_at.timezone();
    let text = lang::text();
    let question = event.details.question(lang::current());
    // The byte order mark makes spreadsheet apps read the file as UTF-8 (æ, ø, å).
    let mut csv = String::from("\u{feff}");
    let edit = Role::can_edit();
    let mut header = vec![
        text.col_name.as_str(),
        &text.col_source,
        &text.col_answer,
        &text.col_people,
        &text.col_max_people,
        &text.col_email,
        &text.col_guest_list,
        &text.col_answered,
        &text.col_link,
    ];
    // The address's and the link's columns.
    let private = [5, 8];
    if !edit {
        header = drop_columns(header, &private);
    }
    // The host's question, as its own column, when the event asks one.
    if let Some((question, _)) = &question {
        header.push(question);
    }
    csv_line(&mut csv, &header);
    let choices = question
        .as_ref()
        .map_or(&[][..], |(_, choices)| &choices[..]);
    for rsvp in &rsvps {
        let row = GuestRow::new(rsvp, tz, &groups).with_choice(choices, rsvp);
        let (party, max, url) = (
            rsvp.party_size.to_string(),
            rsvp.max_party_size.to_string(),
            guest_url(&state.config, rsvp),
        );
        let mut line = vec![
            row.name.as_str(),
            &row.source,
            row.answer,
            &party,
            &max,
            row.email,
            row.listed,
            &row.responded,
            &url,
        ];
        if !edit {
            line = drop_columns(line, &private);
        }
        if question.is_some() {
            line.push(&row.choice);
        }
        csv_line(&mut csv, &line);
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                r#"attachment; filename="rsvp.csv""#,
            ),
        ],
        csv,
    )
        .into_response())
}

/// Appends one CSV record. Every field is quoted, and fields that a spreadsheet would
/// run as a formula get a leading apostrophe.
pub(super) fn csv_line(out: &mut String, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
            out.push('\'');
        }
        out.push_str(&field.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n");
}

/// Queues a reminder for everyone with a confirmed address who hasn't declined. Each
/// guest gets at most one.
pub(super) async fn remind(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let count = outbox::queue_reminders(&state.db, event.id).await?;
    tracing::info!(count, event = %id, "reminders queued");
    let page = format!("{}?reminders={count}", paths::path(paths::EVENT, id));
    Ok(redirect(&page))
}

#[cfg(test)]
mod tests {

    use sqlx::PgPool;

    use super::*;
    use crate::{
        admin::testing::Admin,
        testing::{body, location},
    };

    #[sqlx::test]
    async fn edits_revokes_and_deletes(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let invite = admin.invite("Kari", 1).await;
        let path = format!("/guests/{}", invite.id);

        let response = admin
            .post(&path, "name=Kari+Nordmann&max_party_size=3")
            .await;
        assert_eq!(location(&response), format!("{path}?saved"));
        let edited = rsvp::find(&admin.db, invite.id).await.unwrap().unwrap();
        assert_eq!(
            (edited.name.as_str(), edited.max_party_size),
            ("Kari Nordmann", 3)
        );

        admin.post(&format!("{path}/revoke"), "").await;
        let revoked = rsvp::find(&admin.db, invite.id).await.unwrap().unwrap();
        assert_eq!(revoked.link_nonce, invite.link_nonce + 1);

        let response = admin.post(&format!("{path}/delete"), "").await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response = admin.post(&format!("{path}/delete"), "confirm=yes").await;
        assert_eq!(location(&response), e);
        assert!(rsvp::find(&admin.db, invite.id).await.unwrap().is_none());
        assert_eq!(admin.get(&path).await.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn the_headcount_filters_the_guests(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let kari = admin.invite("Kari", 3).await;
        admin.invite("Ola", 1).await;
        rsvp::respond(
            &admin.db,
            kari.id,
            &rsvp::Reply {
                response: rsvp::Response::Attending,
                party_size: 3,
                name_display: None,
                choice: None,
            },
            crate::i18n::Lang::EN,
            crate::web::PRIVACY_VERSION,
        )
        .await
        .unwrap();

        let html = body(admin.get(&e).await).await;
        assert!(html.contains("<strong>3</strong> people coming"), "{html}");
        assert!(html.contains("<strong>1</strong> no answer"), "{html}");
        assert!(
            html.contains(&format!(r#"<a href="{e}" aria-current="page"><svg"#)),
            "{html}"
        );
        assert!(html.contains("Kari") && html.contains("Ola"));
        assert!(!html.contains("Show everyone"));

        let html = body(admin.get(&format!("{e}?show=attending")).await).await;
        assert!(
            html.contains(&format!(r#"href="{e}?show=attending" aria-current="true""#)),
            "{html}"
        );
        assert!(
            html.contains(r#"<span class="status attending">coming</span>"#),
            "{html}"
        );
        assert!(html.contains("Kari") && !html.contains(">Ola<"), "{html}");
        assert!(html.contains("Show everyone"));
        // An unknown answer shows everyone.
        let html = body(admin.get(&format!("{e}?show=maybe")).await).await;
        assert!(html.contains(">Ola<") && !html.contains("Show everyone"));
    }

    #[sqlx::test]
    async fn exports_csv_with_links(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        admin.invite("=cmd|' /C calc'!A0", 1).await;
        admin.invite("Kari \"K\" Ø", 2).await;

        let response = admin.get(&format!("{e}/guests.csv")).await;
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/csv; charset=utf-8"
        );
        let csv = body(response).await;
        let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with(r#""Name","Source","Answer""#));
        assert!(lines[1].starts_with(r#""'=cmd|' /C calc'!A0","invitation","no answer","1","1""#));
        assert!(lines[2].starts_with(r#""Kari ""K"" Ø","#));
        assert!(lines[2].contains(r#""https://fest.example.com/r/"#));
    }
}
