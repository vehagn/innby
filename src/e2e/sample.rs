//! The sample data in `seed/sample.sql`, which `just db seed` loads: it still fits the
//! schema, and the pages show what the sample says they will.

use sqlx::PgPool;

use super::{World, admin, en};
use crate::db::rsvp::Response;

#[sqlx::test(fixtures(path = "../../seed", scripts("sample")))]
async fn the_sample_shows_on_the_pages(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());

    let mut host = world.admin().await;
    host.page()
        .expect("Christmas party")
        .expect("Summer party")
        .expect("Birthday party");

    // Julebord has guests in every state, and three group links.
    host.click("Christmas party")
        .await
        .expect("Kari & Ola Nordmann")
        .expect("李伟")
        .expect("Tor Eriksen");
    host.click(&a.section_invitations).await;
    host.click(&a.group_links)
        .await
        .expect("Klatreklubben")
        .expect("Naboer i Storgata")
        .expect("Kolleger");
    host.click(&a.section_settings).await;
    host.click(&a.guests).await;
    host.click("Kari & Ola Nordmann").await;
    let kari_link = host.value(&a.guest_link);

    // Each guest is listed as they chose, and those not coming aren't.
    let mut kari = world.browser();
    kari.open(&kari_link)
        .await
        .expect(&t.answer(Response::Attending, 2));
    kari.click(&t.tab_guests)
        .await
        .expect("Mari Bakke")
        .expect("Jonas L.")
        .expect("Familien B.")
        .expect_not("Nils")
        .expect_not("Ingrid")
        .expect_not("Tor Eriksen");

    // Bursdag is over, so its guests can't answer any more.
    host.click(&a.events).await;
    host.click("Birthday party").await;
    host.click("Silje Nilsen").await;
    let silje_link = host.value(&a.guest_link);
    world
        .browser()
        .open(&silje_link)
        .await
        .expect(&t.answers_closed);

    // helper, a user in .env.example with no role of their own, may see who's coming to
    // Julebord, and no other event gives anyone a role.
    host.click(&a.events).await;
    host.click("Christmas party").await;
    host.click(&a.section_settings).await;
    host.click(&a.access)
        .await
        .expect("helper")
        .expect(&a.role_viewer);
    host.click(&a.events).await;
    host.click("Summer party").await;
    host.click(&a.section_settings).await;
    host.click(&a.access).await.expect(&a.no_access);
}
