//! Journeys of the host in the admin UI.

use axum::http::header;
use sqlx::PgPool;

use super::{PASSWORD, USER, World, admin, en, link_in};
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
    host.press(&a.log_in).await.expect(&a.events);

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
async fn the_host_styles_each_events_qr_codes(db: PgPool) {
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

    // The summer party's codes keep their own style.
    world.summer_party().await;
    host.click(&a.events).await;
    host.click("Summer party").await;
    host.click(&a.qr_heading).await;
    let svg = &host.page().body;
    assert!(!svg.contains("🥂") && !svg.contains("#1a237e"), "{svg}");
}

#[sqlx::test]
async fn the_admin_ui_can_live_under_the_sites_hostname(db: PgPool) {
    let world = World::with_admin_under_the_site(db);
    world.event().await;
    let a = admin();

    // The host logs in at /admin, and every link stays under it.
    let mut host = world.admin().await;
    assert_eq!(host.page().url, world.site("/admin"));
    host.click("Christmas party").await.expect(&a.guests);
    assert!(host.page().url.starts_with(&world.site("/admin/events/")));
    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann, 2");
    host.press(&a.create_invitations)
        .await
        .expect("Kari Nordmann");
    let link = host.value(&filled(&a.link_for, "name", "Kari Nordmann"));

    // The login is the admin's alone: the site on the same hostname never sees it.
    assert!(host.cookie_names().is_empty(), "{:?}", host.cookie_names());
    host.open(&link).await.expect("Kari Nordmann");
    host.open(&world.site("/admin/")).await.expect(&a.events);
    host.press(&a.log_out).await.expect(&a.logged_out);
    assert_eq!(host.page().url, world.site("/admin/login?logged_out"));
}

#[sqlx::test]
async fn a_stranger_guessing_cannot_lock_the_host_out(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    let there: std::net::IpAddr = "203.0.113.9".parse().unwrap();
    let mut stranger = world.browser_at(there);
    stranger.open(&world.admin_url).await;
    for _ in 0..10 {
        stranger.fill(&a.user_name, USER).fill(&a.password, "guess");
        stranger.press(&a.log_in).await.expect(&a.wrong_login);
    }
    // Then not even the right password works from there, nor from a second device on
    // the same network.
    stranger
        .fill(&a.user_name, USER)
        .fill(&a.password, PASSWORD);
    stranger.press(&a.log_in).await.expect(&a.too_many_logins);
    let mut neighbour = world.browser_at(there);
    neighbour.open(&world.admin_url).await;
    neighbour
        .fill(&a.user_name, USER)
        .fill(&a.password, PASSWORD);
    neighbour.press(&a.log_in).await.expect(&a.too_many_logins);

    // The host, elsewhere, logs in as usual.
    world.admin().await.page().expect(&a.events);
}

#[sqlx::test]
async fn a_new_user_sets_their_password_from_a_mail_and_resets_it(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    let mut host = world.admin().await;

    // An admin makes Kari with an address and no password: she gets a link.
    host.click(&a.users).await;
    host.fill(&a.user_name, "kari")
        .fill(&a.email, "Kari@Example.com")
        .check(&a.role_organiser);
    host.press(&a.create_user).await.expect(&a.no_password_yet);
    let mail = world.receive("kari@example.com").await;
    assert!(mail.body.contains("kari"), "{}", mail.body);
    let first = link_in(&mail).to_owned();

    let mut kari = world.browser_at("192.0.2.20".parse().unwrap());
    kari.open(&first).await.expect(&a.set_password_title);
    kari.fill(&a.new_password, "short");
    kari.press(&a.set_password).await.expect("at least 10");
    kari.fill(&a.new_password, "a long first password");
    kari.press(&a.set_password)
        .await
        .expect(&a.password_set_notice);
    // Used once, the link is done.
    kari.open(&first).await.expect(&a.link_dead);
    kari.open("https://admin.fest.example.com/login").await;

    kari.fill(&a.user_name, "kari")
        .fill(&a.password, "a long first password");
    kari.press(&a.log_in).await.expect(&a.events);
    // Her account page: a new address, and a new password that needs the current one.
    kari.click(&a.account)
        .await
        .expect(&filled(&a.account_hint, "name", "kari"));
    kari.fill(&a.email, "kari.n@example.com");
    kari.press(&a.save_email).await.expect(&a.saved);
    kari.fill(&a.current_password, "wrong")
        .fill(&a.new_password, "a long second password");
    kari.press(&a.change_password)
        .await
        .expect(&a.error_current_password);
    kari.fill(&a.current_password, "a long first password")
        .fill(&a.new_password, "a long second password");
    kari.press(&a.change_password).await.expect(&a.saved);
    kari.press(&a.log_out).await.expect(&a.logged_out);

    // She forgets it. Asking by an address nobody has says the same, and sends nothing.
    kari.click(&a.forgot_password).await.expect(&a.forgot_hint);
    kari.fill(&a.forgot_field, "nobody@example.com");
    kari.press(&a.forgot_send).await.expect(&a.forgot_sent);
    kari.click(&a.log_in).await;
    kari.click(&a.forgot_password).await;
    kari.fill(&a.forgot_field, "kari.n@example.com");
    kari.press(&a.forgot_send).await.expect(&a.forgot_sent);
    let reset = link_in(&world.receive("kari.n@example.com").await).to_owned();
    assert!(world.inbox("nobody@example.com").is_empty());
    assert!(
        world.inbox("kari@example.com").len() == 1,
        "the old address gets nothing more"
    );
    kari.open(&reset).await;
    kari.fill(&a.new_password, "a long third password");
    kari.press(&a.set_password)
        .await
        .expect(&a.password_set_notice);
    kari.fill(&a.user_name, "kari")
        .fill(&a.password, "a long third password");
    kari.press(&a.log_in).await.expect(&a.events);
}

#[sqlx::test]
async fn only_users_made_here_have_an_account_page(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    let mut host = world.admin().await;
    host.page().expect_not(&a.account);
    host.open("https://admin.fest.example.com/account")
        .await
        .expect(&a.not_found);
}
