//! Group sign-up at `/join/{link}`: the host makes a link per group of people, and anyone
//! with it can say they're coming with just a name, and gets their own RSVP link back.

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
    pages::{
        DisplayChoice, EventView, GuestList, GuestsPage, PartySize, not_found, render, render_with,
        server_error,
    },
    rate_limit::ClientIp,
};
use crate::{
    db::{
        group::{self, Group},
        rsvp::{self, NameDisplay, SignUp},
    },
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
    guests_link: String,
    accepts_answers: bool,
    error: Option<&'static str>,
}

impl<'a> JoinPage<'a> {
    fn new(state: &'a AppState, lang: Lang, group: &'a Group, link: &'a str) -> Self {
        let t = lang.messages();
        Self {
            lang,
            t,
            event: EventView::new(&state.config.event, lang),
            group,
            action: paths::localized(&paths::path(paths::JOIN, link), lang),
            name: "",
            party_sizes: PartySize::choices(t, group.max_party_size, 1),
            // Signing up means coming, and coming shows the full name unless chosen otherwise.
            display_choices: DisplayChoice::choices(t, None, NameDisplay::Full),
            guests_link: paths::localized(&paths::path(paths::JOIN_GUESTS, link), lang),
            accepts_answers: state.config.event.accepts_answers(Utc::now()),
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

/// The group behind a sign-up link, unless the link is malformed, forged or revoked.
async fn find_group(state: &AppState, link: &str) -> sqlx::Result<Option<Group>> {
    let Ok(parsed) = link.parse::<IdLink>() else {
        return Ok(None);
    };
    let group = group::find(&state.db, parsed.id).await?;
    Ok(group.filter(|group| {
        state
            .config
            .link_key
            .verify(Purpose::Group, &parsed, group.link_nonce)
    }))
}

pub async fn show(State(state): State<AppState>, lang: Lang, Path(link): Path<String>) -> Response {
    match find_group(&state, &link).await {
        Ok(Some(group)) => render(JoinPage::new(&state, lang, &group, &link)),
        Ok(None) => not_found(lang).await,
        Err(err) => server_error(err),
    }
}

/// The guest list, for someone deciding whether to sign up.
pub async fn guests(
    State(state): State<AppState>,
    lang: Lang,
    Path(link): Path<String>,
) -> Response {
    match find_group(&state, &link).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(lang).await,
        Err(err) => return server_error(err),
    }
    let guests = match GuestList::load(&state.db, None).await {
        Ok(guests) => guests,
        Err(err) => return server_error(err),
    };
    let t = lang.messages();
    render(GuestsPage {
        lang,
        t,
        event: EventView::new(&state.config.event, lang),
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
    let group = match find_group(&state, &link).await {
        Ok(Some(group)) => group,
        Ok(None) => return not_found(lang).await,
        Err(err) => return server_error(err),
    };
    if !form.website.is_empty() {
        tracing::info!("group sign-up honeypot triggered");
        return Redirect::to("/").into_response();
    }
    let name_display = form.name_display.unwrap_or(NameDisplay::Full);
    let mut page = JoinPage::new(&state, lang, &group, &link);
    page.name = &form.name;
    page.display_choices = DisplayChoice::choices(page.t, None, name_display);
    if !page.accepts_answers {
        return render_with(StatusCode::FORBIDDEN, page);
    }
    let Some(name) = rsvp::normalize_name(&form.name) else {
        return page.render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_name);
    };
    if !(1..=group.max_party_size).contains(&form.party_size) {
        return page.render_error(StatusCode::UNPROCESSABLE_ENTITY, |t| &t.invalid_party_size);
    }
    // Only valid submissions count, so a typo doesn't use up a household's allowance.
    if !state.join_limiter.check(client) {
        tracing::warn!("group sign-up rate limit hit");
        return page.render_error(StatusCode::TOO_MANY_REQUESTS, |t| &t.too_many_attempts);
    }

    let sign_up = SignUp {
        group: &group,
        name: &name,
        party_size: form.party_size,
        name_display,
        lang,
    };
    match rsvp::create_open(&state.db, &sign_up, PRIVACY_VERSION).await {
        Ok(rsvp) => {
            tracing::info!(rsvp = %rsvp.id, group = %group.id, "signed up through a group");
            let link = state
                .config
                .link_key
                .link(Purpose::Rsvp, rsvp.id, rsvp.link_nonce);
            let page = paths::localized(&paths::path(paths::RSVP, link), lang);
            Redirect::to(&format!("{page}&new")).into_response()
        }
        Err(err) => server_error(err),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::web::testing::{Site, body, en, escape, location};

    async fn all(site: &Site) -> Vec<rsvp::Rsvp> {
        rsvp::list(&site.db).await.unwrap()
    }

    /// A group for up to two people per sign-up, and its link.
    async fn climbing_club(site: &Site) -> (Group, String) {
        let group = group::create(&site.db, "Climbing club", 2).await.unwrap();
        let link = site
            .config
            .link_key
            .link(Purpose::Group, group.id, group.link_nonce);
        (group, link)
    }

    #[sqlx::test]
    async fn only_a_groups_current_link_works(db: PgPool) {
        let site = Site::new(db);
        let (group, link) = climbing_club(&site).await;
        let rsvp_link = site.config.link_key.link(Purpose::Rsvp, group.id, 0);
        for bad in ["wrong-code", rsvp_link.as_str()] {
            let response = site.get(&format!("/join/{bad}")).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{bad}");
            assert!(body(response).await.contains("Siden finnes ikke"));
            let response = site
                .post(&format!("/join/{bad}"), "name=Per&party_size=1")
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        group::revoke_link(&site.db, group.id).await.unwrap();
        for page in [format!("/join/{link}"), format!("/join/{link}/guests")] {
            let response = site.get(&page).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{page}");
        }
        assert!(all(&site).await.is_empty());
    }

    #[sqlx::test]
    async fn guest_joins_and_gets_their_link(db: PgPool) {
        let site = Site::new(db);
        let (group, link) = climbing_club(&site).await;
        rsvp::create_invite(&site.db, "Kari Nordmann", 1)
            .await
            .unwrap();
        let response = site.get(&format!("/join/{link}?lang=en")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
        let html = body(response).await;
        let t = en();
        assert!(html.contains(&crate::i18n::bold(&t.join_group, "group", "Climbing club")));
        assert!(html.contains(&escape(&t.join_heading)));
        assert!(html.contains(r#"name="website""#));
        assert!(html.contains(&format!(
            r#"<option value="full" selected>{}</option>"#,
            t.display_full
        )));
        // No names on the page the link opens, which link previews fetch: they're on the
        // guest list, with the invited guest who hasn't answered as "Kari N.".
        assert!(!html.contains("Kari"), "{html}");
        let list = format!("/join/{link}/guests?lang=en");
        assert!(html.contains(&format!(r#"href="{list}" rel="nofollow""#)));
        let html = body(site.get(&list).await).await;
        assert!(html.contains("<li>Kari N.</li>"), "{html}");
        assert!(html.contains(&format!(
            r#"href="/join/{link}?lang=en">{}"#,
            t.back_to_sign_up
        )));
        assert!(!html.contains(&t.listed_as));

        let response = site
            .post(
                &format!("/join/{link}?lang=en"),
                "name=+Per+Hansen+&party_size=2&name_display=partial&website=",
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let target = location(&response).to_owned();
        assert!(target.starts_with("/r/") && target.ends_with("?lang=en&new"));

        let guest = all(&site)
            .await
            .into_iter()
            .find(|guest| guest.source == rsvp::Source::Open)
            .unwrap();
        assert_eq!(guest.name, "Per Hansen");
        assert_eq!(guest.group_id, Some(group.id));
        assert_eq!(guest.name_display, Some(NameDisplay::Partial));
        assert_eq!(guest.response, rsvp::Response::Attending);
        assert_eq!((guest.party_size, guest.max_party_size), (2, 2));
        assert_eq!(guest.lang, Lang::EnGb);

        let html = body(site.get(&target).await).await;
        assert!(html.contains(&escape(&t.save_link_title)));
        let path = target.split('?').next().unwrap();
        assert!(html.contains(&format!(r#"value="https://fest.example.com{path}""#)));
        assert!(html.contains(&t.answer(rsvp::Response::Attending, 2)));
        let html = body(site.get(&format!("{path}/guests?lang=en")).await).await;
        assert!(html.contains(&format!(r#"Per H. <span class="muted">{}</span>"#, t.you)));
    }

    #[sqlx::test]
    async fn rejects_bad_input_and_bots(db: PgPool) {
        let site = Site::new(db);
        let (_, link) = climbing_club(&site).await;
        let join = format!("/join/{link}");
        for form in [
            "name=Per&party_size=3",
            "name=+&party_size=1",
            "name=Per&party_size=1&name_display=everyone",
        ] {
            let response = site.post(&join, form).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{form}"
            );
        }
        let response = site
            .post(&join, "name=Bot&party_size=1&website=http%3A%2F%2Fspam")
            .await;
        assert_eq!(location(&response), "/");
        assert!(all(&site).await.is_empty());
    }

    #[sqlx::test]
    async fn limits_sign_ups_per_client(db: PgPool) {
        let site = Site::new(db);
        let (_, link) = climbing_club(&site).await;
        let join = format!("/join/{link}");
        for n in 0..10 {
            let response = site
                .post(&join, &format!("name=Guest+{n}&party_size=1"))
                .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
        }
        let response = site.post(&join, "name=One+more&party_size=1").await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(body(response).await.contains("For mange forsøk"));
        assert_eq!(all(&site).await.len(), 10);
    }

    #[sqlx::test]
    async fn closes_at_the_deadline(db: PgPool) {
        let site = Site::with(db, &[("INNBY_RSVP_DEADLINE", "2020-01-01T12:00")]);
        let (_, link) = climbing_club(&site).await;
        let join = format!("/join/{link}");
        assert!(
            body(site.get(&join).await)
                .await
                .contains("Svarfristen har gått ut.")
        );
        let response = site.post(&join, "name=Per&party_size=1").await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(all(&site).await.is_empty());
    }
}
