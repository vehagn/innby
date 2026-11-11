//! The host's commands, for when the admin UI isn't at hand: `innby invite`, and
//! `innby event list`, `export` and `import`. Each checks what it's given as the admin
//! UI's forms do, says what's wrong in English, and returns what to print.

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    events::{create_event, file::EventFile, group_lines},
    forms::validate,
    guests::guest_url,
    lang, qr as admin_qr,
};
use crate::{
    config::Config,
    db::{
        event::{self, Event},
        group, qr,
        rsvp::{self, NameKind},
    },
    i18n::Lang,
};

/// The language the commands answer in: English, if the site has it, else its base.
fn lang() -> Lang {
    Lang::by_code("en").unwrap_or_else(Lang::base)
}

/// `innby invite`: an invitation to the event, or to the only one there is. Returns the
/// guest's link.
pub async fn invite(
    config: &Config,
    db: &PgPool,
    event: Option<Uuid>,
    name: &str,
    max_party_size: i32,
) -> anyhow::Result<String> {
    lang::within(lang(), async {
        let name = validate(name, max_party_size, NameKind::Person).map_err(anyhow::Error::msg)?;
        let event = match event {
            Some(id) => find_event(db, id).await?,
            None => only_event(db).await?,
        };
        let invite = rsvp::create_invite(db, event.id, &name, max_party_size).await?;
        tracing::info!(rsvp = %invite.id, "invitation created");
        Ok(guest_url(config, &invite))
    })
    .await
}

/// `innby event list`: each event's id, start and title, the soonest first.
pub async fn event_list(db: &PgPool) -> anyhow::Result<String> {
    Ok(lines(&event::list(db).await?))
}

/// `innby event export`: the event's file.
pub async fn export_event(db: &PgPool, id: Uuid) -> anyhow::Result<String> {
    let event = find_event(db, id).await?;
    let groups = group::list(db, id).await?;
    let style = qr::style(db, id).await?;
    Ok(EventFile::new(&event.details, &groups, &style).to_toml())
}

/// `innby event import`: a new event and its group links, from a file. Returns what it
/// made.
pub async fn import_event(db: &PgPool, file: &str) -> anyhow::Result<String> {
    lang::within(lang(), async {
        let (form, groups, style) = EventFile::parse(file)
            .and_then(EventFile::into_form)
            .map_err(|err| anyhow::Error::msg(err.message()))?;
        let details = form.details(Utc::now()).map_err(anyhow::Error::msg)?;
        let groups = group_lines(&groups).map_err(anyhow::Error::msg)?;
        let style = style
            .map(admin_qr::check)
            .transpose()
            .map_err(anyhow::Error::msg)?;
        let mut tx = db.begin().await?;
        let event = create_event(&mut tx, &details, &groups, style.as_ref(), None).await?;
        tx.commit().await?;
        tracing::info!(event = %event.id, groups = groups.len(), "event imported");
        Ok(format!(
            "Made {} ({}) on {}, with {} group link(s).",
            details.title(lang()),
            event.id,
            lang().format_date(details.starts_at),
            groups.len()
        ))
    })
    .await
}

async fn find_event(db: &PgPool, id: Uuid) -> anyhow::Result<Event> {
    event::find(db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("there's no event {id}; `innby event list` shows them"))
}

/// The only event there is. With none or several, says what to do.
async fn only_event(db: &PgPool) -> anyhow::Result<Event> {
    let mut events = event::list(db).await?;
    match events.len() {
        0 => anyhow::bail!("there are no events yet: create one in the admin UI"),
        1 => Ok(events.remove(0)),
        _ => anyhow::bail!("which event? Pass --event with one of:\n{}", lines(&events)),
    }
}

/// A line per event: its id, start and title.
fn lines(events: &[Event]) -> String {
    events
        .iter()
        .map(|event| {
            let details = &event.details;
            format!(
                "{}  {}  {}\n",
                event.id,
                details.starts_at.format("%Y-%m-%d %H:%M %Z"),
                details.title(lang())
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::{
        config::tests::config,
        db::event::tests::{details, event as test_event},
        link::{IdLink, Purpose},
    };

    #[sqlx::test]
    async fn invite_prints_a_working_link(db: PgPool) {
        let config = config();
        // With no event yet there's nothing to invite to; with one, it's that one.
        assert!(invite(&config, &db, None, "Per", 2).await.is_err());
        let event = test_event(&db).await;
        let url = invite(&config, &db, None, "  Per  ", 2).await.unwrap();
        let link: IdLink = url
            .strip_prefix("https://fest.example.com/r/")
            .unwrap()
            .parse()
            .unwrap();
        let made = rsvp::find(&db, link.id).await.unwrap().unwrap();
        assert_eq!((made.name.as_str(), made.event_id), ("Per", event.id));
        assert!(
            config
                .link_key
                .verify(Purpose::Rsvp, &link, made.link_nonce)
        );

        // Mistakes come back in English.
        let err = invite(&config, &db, None, "Per", 0).await.unwrap_err();
        assert!(err.to_string().starts_with("The party size"), "{err}");
        let err = invite(&config, &db, None, "<b>", 1).await.unwrap_err();
        assert!(err.to_string().starts_with("Write the name"), "{err}");

        // With two events, it has to be told which.
        event::create(&db, &details()).await.unwrap();
        let err = invite(&config, &db, None, "Ola", 1).await.unwrap_err();
        assert!(err.to_string().contains(&event.id.to_string()), "{err}");
        assert!(invite(&config, &db, Some(event.id), "Ola", 1).await.is_ok());
    }

    #[sqlx::test]
    async fn lists_exports_and_imports_events(db: PgPool) {
        let event = test_event(&db).await;
        group::create(&db, event.id, "Naboer", 4).await.unwrap();
        let style = qr::Style {
            shape: qr::Shape::Dots,
            color: "#b71c1c".into(),
            icon: qr::Icon::Star,
            emoji: String::new(),
        };
        qr::save(&db, event.id, &style).await.unwrap();
        assert!(
            event_list(&db)
                .await
                .unwrap()
                .contains(&event.id.to_string())
        );

        let file = export_event(&db, event.id).await.unwrap();
        let made = import_event(&db, &file).await.unwrap();
        assert!(made.starts_with("Made Christmas party"), "{made}");
        assert!(made.ends_with("with 1 group link(s)."), "{made}");
        let events = event::list(&db).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].details, event.details);
        assert_eq!(
            group::list(&db, events[1].id).await.unwrap()[0].name,
            "Naboer"
        );
        assert_eq!(qr::style(&db, events[1].id).await.unwrap(), style);

        // Mistakes come back in English, and make nothing.
        let err = import_event(&db, &file.replace("Naboer", "<b>"))
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("Group links: Line 1"), "{err}");
        // Nor from a file kept from long ago: the janitor would delete it at once.
        let long_ago = file.replace("2054-12-12T19:00", "2020-12-12T19:00");
        let err = import_event(&db, &long_ago).await.unwrap_err();
        assert!(err.to_string().contains("30 days after it starts"), "{err}");
        assert!(export_event(&db, Uuid::nil()).await.is_err());
        assert_eq!(event::list(&db).await.unwrap().len(), 2);
    }
}
