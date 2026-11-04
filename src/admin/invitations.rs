//! Invitations: making them in a batch on the Invitation links tab, and from the CLI
//! with `innby invite`.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::guests::{Row, guest_url};
use super::{
    lang,
    page::{AdminState, PageError, load_event},
};
use crate::{
    config::Config,
    db::{
        event::{self, Event},
        rsvp::{self, MAX_PARTY_SIZE, Source},
    },
    http::render_with,
    i18n::fill,
    paths::{self, EventLinks},
};

/// A name and a party size: an invitation's, or a group's for each sign-up.
#[derive(Deserialize)]
pub(super) struct InviteForm {
    pub(super) name: String,
    pub(super) max_party_size: i32,
}

impl InviteForm {
    pub(super) fn validate(&self) -> Result<String, String> {
        validate(&self.name, self.max_party_size)
    }
}

pub(super) fn validate(name: &str, max_party_size: i32) -> Result<String, String> {
    let text = lang::text();
    if !(1..=MAX_PARTY_SIZE).contains(&max_party_size) {
        return Err(fill(&text.error_party_size, &[("max", &MAX_PARTY_SIZE)]));
    }
    rsvp::normalize_name(name).ok_or_else(|| text.error_name.clone())
}

/// Several names at once, one per line.
#[derive(Deserialize)]
pub(super) struct BatchForm {
    pub(super) names: String,
    /// For lines that don't give their own.
    pub(super) max_party_size: i32,
}

impl BatchForm {
    /// Each line's name and party size. A line can end in a comma and its own party size;
    /// blank lines are skipped. One invalid line fails the whole batch.
    pub(super) fn parse(&self) -> Result<Vec<(String, i32)>, String> {
        let mut entries = Vec::new();
        for (index, line) in self.names.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (name, max_party_size) = line
                .rsplit_once(',')
                .and_then(|(name, size)| Some((name, size.trim().parse().ok()?)))
                .unwrap_or((line, self.max_party_size));
            let name = validate(name, max_party_size).map_err(|error| {
                let line = index + 1;
                fill(&lang::text().error_line, &[("n", &line), ("error", &error)])
            })?;
            entries.push((name, max_party_size));
        }
        if entries.is_empty() {
            return Err(lang::text().error_no_names.clone());
        }
        Ok(entries)
    }
}

/// Creates an invitation per line, all or none, then shows their links.
pub(super) async fn create_invitations(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<BatchForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let entries = match form.parse() {
        Ok(entries) => entries,
        Err(error) => {
            return invitations_page(&state, event, None, form.names, Some(error)).await;
        }
    };
    let mut tx = state.db.begin().await?;
    let mut ids = Vec::new();
    for (name, max_party_size) in &entries {
        ids.push(
            rsvp::create_invite(&mut *tx, id, name, *max_party_size)
                .await?
                .id,
        );
    }
    tx.commit().await?;
    tracing::info!(count = ids.len(), event = %id, "invitations created");
    // The ids are UUIDv7, so the new ones are those from the first on.
    let page = format!(
        "{}?from={}",
        paths::path(paths::EVENT_INVITATIONS, id),
        ids[0]
    );
    Ok(Redirect::to(&paths::admin(&page)).into_response())
}

#[derive(Template)]
#[template(path = "admin/invitations.html")]
pub(super) struct InvitationsPage {
    pub(super) event: Event,
    pub(super) links: EventLinks,
    /// Only the invitations just created.
    pub(super) new: bool,
    pub(super) invitations: Vec<Invitation>,
    /// The batch form's text, kept when a line needs fixing.
    pub(super) names: String,
    pub(super) max_party_size: i32,
    pub(super) error: Option<String>,
}

pub(super) struct Invitation {
    pub(super) row: Row,
    pub(super) url: String,
}

#[derive(Deserialize)]
pub(super) struct InvitationsQuery {
    pub(super) from: Option<Uuid>,
}

/// Every invitation's link, for sending them out, and the form for new ones.
pub(super) async fn invitations(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<InvitationsQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    invitations_page(&state, event, query.from, String::new(), None).await
}

/// The event's invitations from `from` on, or all of them.
pub(super) async fn invitations_page(
    state: &AdminState,
    event: Event,
    from: Option<Uuid>,
    names: String,
    error: Option<String>,
) -> Result<Response, PageError> {
    let rsvps = rsvp::list(&state.db, event.id).await?;
    let tz = event.details.starts_at.timezone();
    let invitations = rsvps
        .iter()
        .filter(|rsvp| rsvp.source == Source::Invite)
        .filter(|rsvp| from.is_none_or(|from| rsvp.id >= from))
        .map(|rsvp| Invitation {
            row: Row::new(rsvp, tz, &[]),
            url: guest_url(&state.config, rsvp),
        })
        .collect();
    let status = match error {
        Some(_) => StatusCode::UNPROCESSABLE_ENTITY,
        None => StatusCode::OK,
    };
    Ok(render_with(
        status,
        InvitationsPage {
            links: EventLinks::new(event.id),
            event,
            new: from.is_some(),
            invitations,
            names,
            max_party_size: MAX_PARTY_SIZE,
            error,
        },
    ))
}

/// `innby invite`: creates an invitation to the event, or to the only one there is, and
/// returns the guest's link.
pub async fn invite(
    config: &Config,
    db: &PgPool,
    event: Option<Uuid>,
    name: &str,
    max_party_size: i32,
) -> anyhow::Result<String> {
    let form = InviteForm {
        name: name.to_owned(),
        max_party_size,
    };
    let name = form.validate().map_err(anyhow::Error::msg)?;
    let event = match event {
        Some(id) => event::find(db, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("there's no event {id}"))?,
        None => {
            let mut events = event::list(db).await?;
            match events.len() {
                1 => events.remove(0),
                0 => anyhow::bail!("there are no events yet: create one in the admin UI"),
                _ => {
                    let list: Vec<String> = events
                        .iter()
                        .map(|event| format!("{} {}", event.id, event.details.title.nb))
                        .collect();
                    anyhow::bail!(
                        "which event? Pass --event with one of:\n{}",
                        list.join("\n")
                    )
                }
            }
        }
    };
    let invite = rsvp::create_invite(db, event.id, &name, max_party_size).await?;
    tracing::info!(rsvp = %invite.id, "invitation created");
    Ok(guest_url(config, &invite))
}

#[cfg(test)]
mod tests {

    use sqlx::PgPool;

    use super::*;
    use crate::{
        admin::testing::{Admin, links},
        config::tests::config,
        link::{IdLink, Purpose},
        web::testing::{body, location},
    };

    #[sqlx::test]
    async fn creates_invitations_in_a_batch(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let response = admin
            .post(
                &format!("{e}/invitations"),
                "names=Kari+%26+Ola%2C+2%0D%0A%0D%0APer+Hansen%0D%0ANordmann%2C+Anne&max_party_size=1",
            )
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let page = location(&response).to_owned();
        assert!(
            page.starts_with(&format!("{e}/invitations?from=")),
            "{page}"
        );

        let html = body(admin.get(&page).await).await;
        assert!(html.contains("3 invitations created."), "{html}");
        let invites = rsvp::list(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
        )
        .await
        .unwrap();
        let sizes: Vec<_> = invites
            .iter()
            .map(|invite| (invite.name.as_str(), invite.max_party_size))
            .collect();
        assert_eq!(
            sizes,
            [("Kari & Ola", 2), ("Per Hansen", 1), ("Nordmann, Anne", 1)]
        );
        let urls = links(&html);
        assert_eq!(urls.len(), 3);
        let link: IdLink = urls[0]
            .strip_prefix("https://fest.example.com/r/")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(link.id, invites[0].id);
        assert!(
            admin
                .config
                .link_key
                .verify(Purpose::Rsvp, &link, invites[0].link_nonce)
        );

        // Later batches list only their own invitations; the full list has them all.
        let response = admin
            .post(&format!("{e}/invitations"), "names=Eva&max_party_size=1")
            .await;
        let html = body(admin.get(location(&response)).await).await;
        assert_eq!(links(&html).len(), 1);
        let html = body(admin.get(&format!("{e}/invitations")).await).await;
        assert_eq!(links(&html).len(), 4);
    }

    #[sqlx::test]
    async fn rejects_a_batch_with_an_invalid_line(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        for (form, error) in [
            (
                "names=Kari%0D%0AOla%2C+21&max_party_size=2",
                "Line 2: The party size must be between 1 and 20.",
            ),
            (
                "names=Kari%0D%0A+-+%0D%0A&max_party_size=2",
                "Line 2: Enter a name of up to 100 characters.",
            ),
            ("names=+%0D%0A&max_party_size=2", "Enter at least one name."),
            (
                "names=Kari&max_party_size=0",
                "Line 1: The party size must be between 1 and 20.",
            ),
        ] {
            let response = admin.post(&format!("{e}/invitations"), form).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{form}"
            );
            let html = body(response).await;
            assert!(html.contains(error), "{form}: {html}");
        }
        // The text stays in the form to fix.
        let html = body(
            admin
                .post(
                    &format!("{e}/invitations"),
                    "names=Kari%0D%0AOla%2C+21&max_party_size=2",
                )
                .await,
        )
        .await;
        assert!(html.contains(">Kari\r\nOla, 21</textarea>"), "{html}");
        assert!(
            rsvp::list(
                &admin.db,
                crate::db::event::tests::event(&admin.db).await.id
            )
            .await
            .unwrap()
            .is_empty()
        );
    }

    #[sqlx::test]
    async fn invite_command_prints_a_working_link(db: PgPool) {
        let config = config();
        // With no event yet there's nothing to invite to; with one, it's that one.
        assert!(invite(&config, &db, None, "Per", 2).await.is_err());
        let event = crate::db::event::tests::event(&db).await;
        let url = invite(&config, &db, None, "  Per  ", 2).await.unwrap();
        let link: IdLink = url
            .strip_prefix("https://fest.example.com/r/")
            .unwrap()
            .parse()
            .unwrap();
        let invite = rsvp::find(&db, link.id).await.unwrap().unwrap();
        assert_eq!(invite.name, "Per");
        assert!(
            config
                .link_key
                .verify(Purpose::Rsvp, &link, invite.link_nonce)
        );

        assert_eq!(invite.event_id, event.id);
        assert!(super::invite(&config, &db, None, "Per", 0).await.is_err());
        // With two, it has to be told which.
        crate::db::event::create(&db, &crate::db::event::tests::details())
            .await
            .unwrap();
        assert!(super::invite(&config, &db, None, "Ola", 1).await.is_err());
        assert!(
            super::invite(&config, &db, Some(event.id), "Ola", 1)
                .await
                .is_ok()
        );
    }
}
