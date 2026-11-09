use axum::http::StatusCode;
use sqlx::PgPool;

use crate::{
    admin::testing::{Admin, links},
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
    let invites = admin.guests().await;
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
    // Each is a link too, which browsers offer to copy when it's held or right-clicked.
    for url in &urls {
        assert!(html.contains(&format!(r#"<a href="{url}""#)), "{html}");
    }
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
    assert!(admin.guests().await.is_empty());
}

#[sqlx::test]
async fn the_same_form_sent_twice_makes_its_invitations_once(db: PgPool) {
    let admin = Admin::new(db);
    let e = admin.event().await;
    let html = body(admin.get(&format!("{e}/invitations")).await).await;
    let token = html
        .split(r#"name="form_token" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned();

    // The answer to the first was lost on the way, so the host presses again.
    let form = format!("names=Kari%0D%0AOla&max_party_size=1&form_token={token}");
    let first = admin.post(&format!("{e}/invitations"), &form).await;
    let again = admin.post(&format!("{e}/invitations"), &form).await;
    assert_eq!(location(&again), location(&first));
    assert_eq!(admin.guests().await.len(), 2);

    // A new form makes new ones, as does one without a token.
    for other in ["form_token=another-form-token-123", ""] {
        let form = format!("names=Per&max_party_size=1&{other}");
        let response = admin.post(&format!("{e}/invitations"), &form).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{other}");
    }
    assert_eq!(admin.guests().await.len(), 4);
}
