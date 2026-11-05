//! Journeys of a guest with a personal invitation.

use axum::http::{StatusCode, header};
use sqlx::PgPool;

use super::{World, admin, en};
use crate::{
    db::rsvp::Response,
    i18n::{count, filled},
};

#[sqlx::test]
async fn an_invited_household_answers_and_shows_on_the_guest_list(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());

    // The host invites two households and copies the first one's link.
    let mut host = world.host().await;
    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann, 3\nPer Hansen");
    host.press(&a.create_invitations).await.expect(&count(
        2,
        &a.invitations_created_one,
        &a.invitations_created_other,
    ));
    let link = host.value(&filled(&a.link_for, "name", "Kari Nordmann"));

    // Kari opens it, is told what's kept, and says all three are coming.
    let mut kari = world.browser();
    kari.open(&link)
        .await
        .expect(&filled(&t.greeting, "name", "Kari Nordmann"))
        .expect(&t.answer_pending);
    kari.page().expect(t.first_visit_notice.trim());
    kari.select(&t.how_many, &t.people_count(3));
    kari.press(&t.attend)
        .await
        .expect(&t.saved)
        .expect(&t.answer(Response::Attending, 3));

    // The guest list shows the household with the two Kari brings, and the
    // invitation still waiting for an answer by first name and initial.
    kari.click(&t.see_guest_list)
        .await
        .expect(&format!("{} · {}", t.guest_list_coming, t.people_count(3)))
        .expect(&format!("Kari Nordmann +2 {}", t.you))
        .expect(&format!(
            "{} · {}",
            t.guest_list_pending,
            t.invitation_count(1)
        ))
        .expect("Per H.")
        .expect_not("Per Hansen");

    // The host's headcount has them, and its tile shows just those coming.
    host.click(&a.guests)
        .await
        .expect(&count(3, &a.coming_one, &a.coming_other))
        .expect(&count(1, &a.no_answer, &a.no_answer));
    let tile = format!(
        "{} {}",
        count(3, &a.coming_one, &a.coming_other),
        count(1, &a.parties_one, &a.parties_other)
    );
    host.click(&tile)
        .await
        .expect("Kari Nordmann")
        .expect_not("Per Hansen")
        .expect(&a.show_all);
}

#[sqlx::test]
async fn a_guest_hides_their_name_then_declines(db: PgPool) {
    let world = World::new(db);
    let t = en();
    let link = world.invite("Kari Nordmann", 2).await;
    let mut kari = world.browser();
    kari.open(&link).await;
    kari.press(&t.attend).await;

    // From the guest list Kari asks to be left off it; others then see a count only.
    kari.click(&t.see_guest_list).await;
    kari.press(&t.display_hidden).await.expect(&t.listed_hidden);
    let mut ola = world.browser();
    ola.open(&world.invite("Ola", 1).await).await;
    ola.click(&t.see_guest_list)
        .await
        .expect_not("Kari")
        .expect(&t.more_guests(1));

    // Then Kari can't come after all, and drops off the list.
    kari.click(&t.back_to_your_page).await;
    kari.press(&t.decline)
        .await
        .expect(&t.answer(Response::Declined, 1));
    kari.click(&t.see_guest_list)
        .await
        .expect(&t.listed_declined);
    let list = ola.page().url.clone();
    ola.open(&list).await.expect_not(&t.more_guests(1));
}

#[sqlx::test]
async fn a_guest_downloads_and_deletes_their_data(db: PgPool) {
    let world = World::new(db);
    let t = en();
    let link = world.invite("Kari Nordmann", 1).await;
    let mut kari = world.browser();
    kari.open(&link).await;

    let export = kari.download(&t.export).await;
    assert_eq!(export.headers[header::CONTENT_TYPE], "application/json");
    assert!(
        export.body.contains(r#""name":"Kari Nordmann""#),
        "{}",
        export.body
    );

    // Deleting needs the box ticked.
    kari.press(&t.delete).await.expect(&t.confirm_delete);
    kari.check(&t.delete_confirm);
    kari.press(&t.delete).await.expect(&t.deleted_title);

    let gone = kari.open(&link).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    gone.expect(&t.gone_title);
    let mut host = world.host().await;
    host.page().expect(&admin().no_guests);
    host.click(&admin().invitation_links)
        .await
        .expect(&admin().no_invitations);
}

#[sqlx::test]
async fn a_remembered_device_finds_the_way_back(db: PgPool) {
    let world = World::new(db);
    let t = en();
    let link = world.invite("Kari Nordmann", 1).await;
    let mut phone = world.browser();
    phone.open(&link).await;
    // She ticks the box as she answers.
    phone.check(&t.remember_device);
    phone.press(&t.attend).await.expect(&t.saved);

    // Later, the front page greets Kari and leads back to her page.
    phone
        .open(&world.site("/"))
        .await
        .expect(&filled(&t.welcome_back, "name", "Kari Nordmann"));
    phone
        .click(&t.to_your_page)
        .await
        .expect(&filled(&t.greeting, "name", "Kari Nordmann"));

    // Nobody else's device knows her.
    let mut laptop = world.browser();
    laptop.open(&world.site("/")).await.expect(&t.rsvp_hint);

    // "Not you?" makes the phone forget.
    phone.open(&world.site("/")).await;
    phone.press(&t.forget_device).await.expect(&t.rsvp_hint);
    assert!(
        phone.cookie_names().is_empty(),
        "{:?}",
        phone.cookie_names()
    );
}
