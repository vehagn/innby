//! Journeys through a group's sign-up link.

use sqlx::PgPool;

use super::{World, admin, en};
use crate::{
    db::rsvp::Response,
    i18n::{count, filled},
};

#[sqlx::test]
async fn someone_signs_up_through_a_group_link_and_keeps_their_own(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());

    // The host makes a link for the climbing club, where each may bring one more.
    let mut host = world.host().await;
    host.click(&a.section_invitations).await;
    host.click(&a.group_links).await;
    host.fill(&a.names, "Climbing club, 2");
    host.press(&a.create_groups).await.expect(&count(
        1,
        &a.groups_created_one,
        &a.groups_created_other,
    ));
    let group_link = host.value(&filled(&a.link_for, "name", "Climbing club"));

    // Per signs up for two from the group chat, and is asked to keep his own link.
    let mut per = world.browser();
    per.open(&group_link)
        .await
        .expect(&filled(&t.join_group, "group", "Climbing club"));
    per.fill(&t.your_name, "Per Hansen")
        .select(&t.how_many, &t.people_count(2));
    per.press(&t.join_button)
        .await
        .expect(&t.save_link_title)
        .expect(&t.answer(Response::Attending, 2));
    let own_link = per.page().url.split('?').next().unwrap().to_owned();
    assert!(own_link.starts_with(&world.site("/r/")), "{own_link}");

    // His own link works on another device; the group's link only signs people up.
    let mut laptop = world.browser();
    laptop
        .open(&own_link)
        .await
        .expect(&filled(&t.greeting, "name", "Per Hansen"))
        .expect_not(&t.save_link_title);
    laptop.open(&group_link).await.expect(&t.join_heading);

    // The host sees him on the group's page.
    host.click("Climbing club")
        .await
        .expect("Per Hansen")
        .expect("2 / 2");
}

#[sqlx::test]
async fn a_new_group_link_retires_the_old_one(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let mut host = world.host().await;
    host.click(&a.section_invitations).await;
    host.click(&a.group_links).await;
    host.fill(&a.names, "Neighbours");
    host.press(&a.create_groups).await;
    let old = host.value(&filled(&a.link_for, "name", "Neighbours"));

    host.click("Neighbours").await;
    host.press(&a.new_link).await.expect(&a.saved);
    let new = host.value(&a.group_link);
    assert_ne!(old, new);

    let mut guest = world.browser();
    guest.open(&old).await.expect(&t.gone_title);
    guest.open(&new).await.expect(&t.join_heading);
}
