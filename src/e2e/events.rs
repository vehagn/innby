//! Journeys across events: the host makes them, and a device can remember invitations
//! to several.

use axum::http::StatusCode;
use sqlx::PgPool;

use super::{World, admin, en};
use crate::{db::rsvp::Response, i18n::filled};

#[sqlx::test]
async fn the_host_creates_an_event_invites_to_it_and_deletes_it(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());

    let mut host = world.admin().await;
    host.page().expect(&a.no_events);
    host.fill(&format!("{} · Bokmål", a.event_title), "Sommerfest")
        .fill(
            &format!("{} · English (Traditional)", a.event_title),
            "Summer party",
        )
        .fill(&a.event_location, "Bygdøy")
        .fill(&a.event_starts, "2026-12-20T15:00");
    host.press(&a.create_event)
        .await
        .expect(&a.event_created)
        .expect("Summer party");

    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann");
    host.press(&a.create_invitations).await;
    let link = host.value(&filled(&a.link_for, "name", "Kari Nordmann"));

    // Kari's page is about the summer party, in her language.
    let mut kari = world.browser();
    kari.open(&link)
        .await
        .expect("Summer party")
        .expect("Bygdøy")
        .expect(&t.when);
    kari.press(&t.attend)
        .await
        .expect(&t.answer(Response::Attending, 1));

    // The host moves it; Kari's page follows.
    host.click(&a.details).await;
    host.fill(&a.event_location, "Huk");
    host.press(&a.save).await.expect(&a.saved);
    kari.open(&link).await.expect("Huk").expect_not("Bygdøy");

    // The list of events shows the headcount, and deleting takes everything with it.
    host.click(&format!("‹ {}", a.all_events))
        .await
        .expect("Summer party");
    host.click("Summer party").await;
    host.click(&a.details).await;
    host.check(&filled(&a.delete_event, "title", "Summer party"));
    host.press(&a.delete)
        .await
        .expect(&a.event_deleted)
        .expect(&a.no_events);
    let gone = kari.open(&link).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    gone.expect(&t.gone_title);
}

#[sqlx::test]
async fn one_device_remembers_invitations_to_two_events(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let christmas = world.invite("Kari Nordmann", 1).await;

    let mut host = world.admin().await;
    host.fill(&format!("{} · Bokmål", a.event_title), "Sommerfest")
        .fill(&a.event_starts, "2027-06-20T15:00");
    host.press(&a.create_event).await;
    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann");
    host.press(&a.create_invitations).await;
    let summer = host.value(&filled(&a.link_for, "name", "Kari Nordmann"));

    // Kari has her phone remember both invitations.
    let mut phone = world.browser();
    for link in [&christmas, &summer] {
        phone.open(link).await;
        phone
            .press(&t.remember_device)
            .await
            .expect(&t.remembered_notice);
    }
    assert_eq!(phone.cookie_names().len(), 2);

    // The front page is about Innby, and leads back to both, the soonest first.
    let front = phone.open(&world.site("/")).await;
    front.expect(&t.site_intro).expect(&t.your_invitations);
    let text = &front.doc.text;
    let (christmas_at, summer_at) = (
        text.find("Christmas party").unwrap(),
        text.find("Sommerfest").unwrap(),
    );
    assert!(christmas_at < summer_at, "{text}");
    phone.click(&t.to_your_page).await.expect("Christmas party");

    // Forgetting one leaves the other.
    phone.open(&world.site("/")).await;
    phone
        .press(&t.forget_device)
        .await
        .expect("Sommerfest")
        .expect_not("Christmas party");
    assert_eq!(phone.cookie_names().len(), 1);
}
