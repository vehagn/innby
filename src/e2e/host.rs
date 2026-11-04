//! Journeys of the host in the admin UI.

use axum::http::header;
use sqlx::PgPool;

use super::{PASSWORD, USER, World, admin, en};
use crate::i18n::filled;

#[sqlx::test]
async fn the_host_logs_in_and_out(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    let mut host = world.browser();
    host.open("https://admin.fest.example.com/invitations")
        .await
        .expect(&a.log_in_text);

    host.fill(&a.user_name, USER).fill(&a.password, "wrong");
    host.press(&a.log_in).await.expect(&a.wrong_login);
    host.fill(&a.password, PASSWORD);
    host.press(&a.log_in).await.expect(&a.guests);

    host.press(&a.log_out).await.expect(&a.logged_out);
    host.open("https://admin.fest.example.com/")
        .await
        .expect(&a.log_in_text);
}

#[sqlx::test]
async fn the_host_edits_a_guest_and_replaces_a_leaked_link(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let old = world.invite("Kari", 1).await;
    let mut host = world.host().await;

    host.click("Kari").await;
    host.fill(&a.name_or_household, "Kari & Ola Nordmann")
        .fill(&a.max_people, "2");
    host.press(&a.save).await.expect(&a.saved);
    host.press(&a.new_link).await.expect(&a.saved);
    let new = host.value(&a.guest_link);
    assert_ne!(old, new);

    let mut kari = world.browser();
    kari.open(&old).await.expect(&t.gone_title);
    kari.open(&new)
        .await
        .expect(&filled(&t.greeting, "name", "Kari & Ola Nordmann"))
        .expect(&t.people_count(2));

    // The QR code and the CSV carry the new link.
    let qr = host.download(&a.qr_download).await;
    assert_eq!(qr.headers[header::CONTENT_TYPE], "image/svg+xml");
    host.click(&a.guests).await;
    let csv = host.download(&a.download_csv).await;
    assert!(csv.body.contains(&new), "{}", csv.body);
    assert!(!csv.body.contains(&old));
}

#[sqlx::test]
async fn the_host_styles_the_qr_codes(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    let mut host = world.host().await;
    host.click(&a.qr_heading).await;

    // A colour too light to scan is refused.
    host.fill(&a.qr_color, "#ffeb3b");
    host.press(&a.qr_save).await.expect(&a.error_qr_color);

    host.check(&a.qr_dots)
        .fill(&a.qr_color, "#1a237e")
        .check(&a.qr_emoji)
        .fill(&a.qr_emoji, "🥂");
    host.press(&a.qr_save).await.expect(&a.qr_saved);
    let svg = &host.page().body;
    assert!(svg.contains(r##"fill="#1a237e""##), "{svg}");
    assert!(svg.contains("🥂</text>"));
}
