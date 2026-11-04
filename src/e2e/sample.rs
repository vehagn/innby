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
    host.click(&a.group_links)
        .await
        .expect("Klatreklubben")
        .expect("Naboer i Storgata")
        .expect("Kolleger");
    host.click(&a.details).await;
    host.click(&a.guests).await;
    host.click("Kari & Ola Nordmann").await;
    let kari_link = host.value(&a.guest_link);

    // Each guest is listed as they chose, and those not coming aren't.
    let mut kari = world.browser();
    kari.open(&kari_link)
        .await
        .expect(&t.answer(Response::Attending, 2));
    kari.click(&t.see_guest_list)
        .await
        .expect("Mari Bakke")
        .expect("Jonas L.")
        .expect("Familien B.")
        .expect_not("Nils")
        .expect_not("Ingrid")
        .expect_not("Tor Eriksen");

    // Bursdag is over, so its guests can't answer any more.
    host.click(&format!("‹ {}", a.all_events)).await;
    host.click("Birthday party").await;
    host.click("Silje Nilsen").await;
    let silje_link = host.value(&a.guest_link);
    world
        .browser()
        .open(&silje_link)
        .await
        .expect(&t.answers_closed);
}
