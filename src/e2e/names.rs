//! Journeys with names: letters in any script and the punctuation names have, and
//! nothing else, wherever a name is typed.

use sqlx::PgPool;

use super::{World, admin, en};
use crate::i18n::{count, fill, filled};

#[sqlx::test]
async fn the_host_and_guests_type_only_names_as_names(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());

    // A batch with markup in one line makes nothing, and says which line.
    let mut host = world.host().await;
    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann\n<b>Per</b> Hansen");
    let line_2 = fill(&a.error_line, &[("n", &2), ("error", &a.error_name)]);
    host.press(&a.create_invitations)
        .await
        .expect(&line_2)
        .expect_not(&filled(&a.link_for, "name", "Kari Nordmann"));
    // Households, hyphens and apostrophes are names.
    host.fill(&a.names, "Kari & Ola Nordmann\nPer O'Hansen-Berg");
    host.press(&a.create_invitations).await.expect(&count(
        2,
        &a.invitations_created_one,
        &a.invitations_created_other,
    ));
    let link = host.value(&filled(&a.link_for, "name", "Kari & Ola Nordmann"));

    // Kari can't rename herself to an emoji or a number; the old name stays.
    let mut kari = world.browser();
    kari.open(&link).await;
    kari.click(&t.tab_you).await;
    for name in ["Kari 👋", "Kari2", "<script>alert(1)</script>"] {
        kari.fill(&t.name_heading, name);
        kari.press(&t.change_name).await.expect(&t.invalid_name);
        assert_eq!(kari.value(&t.name_heading), "Kari & Ola Nordmann");
    }
    kari.fill(&t.name_heading, "Kari Nordmann-Hansen");
    kari.press(&t.change_name).await.expect(&t.saved);
    kari.click(&t.tab_reply)
        .await
        .expect(&filled(&t.greeting, "name", "Kari Nordmann-Hansen"));

    // A group's name may have digits; who signs up through it still needs a name.
    host.click(&a.group_links).await;
    host.fill(&a.names, "Class of 2010, 2");
    host.press(&a.create_groups).await;
    let group = host.value(&filled(&a.link_for, "name", "Class of 2010"));
    let mut wang = world.browser();
    wang.open(&group).await;
    wang.fill(&t.your_name, "<b>Wang</b>");
    wang.press(&t.join_button).await.expect(&t.invalid_name);
    wang.fill(&t.your_name, "王小明");
    wang.press(&t.join_button).await.expect(&t.save_link_title);
}
