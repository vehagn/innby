use sqlx::PgPool;

use super::*;
use crate::{
    admin::testing::{Admin, links},
    config::tests::config,
    link::{IdLink, Purpose},
    testing::{body, escape, location},
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
            "Line 2: Write the name in letters and spaces, with - ' . , & if need be, up to 100 characters.",
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
        assert!(html.contains(&escape(error)), "{form}: {html}");
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
