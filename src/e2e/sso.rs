//! Logging in to the admin UI through an OpenID Connect provider, and what each role
//! may do there.

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use super::{ADMIN, World, admin};
use crate::i18n::filled;

#[sqlx::test]
async fn a_host_logs_in_through_the_provider_with_their_role(db: PgPool) {
    let world = World::with_sso(db, false).await;
    world.event().await;
    let a = admin();

    // With no password login, the admin UI sends the host straight to the provider,
    // where Kari is logged in already, and back.
    let mut kari = world.browser();
    kari.open(ADMIN)
        .await
        .expect(&a.events)
        .expect("Christmas party");
    // An editor runs events, styling their QR codes too, but doesn't make them.
    kari.page().expect_not(&a.new_event);
    kari.click("Christmas party")
        .await
        .expect(&a.invitation_links)
        .expect(&a.qr_heading);
    kari.click(&a.invitation_links)
        .await
        .expect(&a.create_invitations);

    // Logging out of Innby doesn't log in again by itself.
    kari.press(&a.log_out).await.expect(&a.logged_out);
    let button = filled(&a.log_in_with, "name", "Authelia");
    kari.page().expect(&button).expect_not(&a.password);
    kari.open(ADMIN).await.expect(&a.events);
    assert_eq!(world.provider().calls("authorize"), 2);
}

#[sqlx::test]
async fn a_viewer_sees_whos_coming_but_not_their_links(db: PgPool) {
    let world = World::with_sso(db, false).await;
    world.invite("Ola Nordmann", 2).await;
    world.provider().log_in("per", json!(["viewer"]));
    let a = admin();

    let mut per = world.browser();
    per.open(ADMIN).await.expect(&a.events);
    per.click("Christmas party")
        .await
        .expect("Ola Nordmann")
        .expect_not(&a.invitation_links)
        .expect_not(&a.download_csv)
        .expect_not(&a.send_reminders);
    assert!(
        per.page()
            .doc
            .links
            .iter()
            .all(|link| link.text != "Ola Nordmann")
    );

    // Typing the address of a page for editors doesn't help.
    let invitations = format!("{}/invitations", per.page().url);
    let page = per.open(&invitations).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    page.expect(&a.forbidden);
}

#[sqlx::test]
async fn someone_without_an_innby_role_is_turned_away(db: PgPool) {
    let world = World::with_sso(db, false).await;
    world.provider().log_in("nils", json!(["grafana_admin"]));
    let a = admin();

    let mut nils = world.browser();
    let page = nils.open(ADMIN).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    let refused = crate::i18n::fill(&a.no_role, &[("name", &"Authelia"), ("user", &"nils")]);
    page.expect(&refused).expect_not(&a.events);

    // Saying no at the provider.
    world.provider().with(|fake| fake.user = None);
    let page = nils.open(ADMIN).await;
    assert_eq!(page.status, StatusCode::UNAUTHORIZED);
    page.expect(&a.login_cancelled);

    // A callback that no login in this browser started.
    let page = nils
        .open(&format!(
            "{ADMIN}/login/oidc/callback?code=stolen&state=guess"
        ))
        .await;
    assert_eq!(page.status, StatusCode::BAD_REQUEST);
    page.expect(&a.login_expired);
}

#[sqlx::test]
async fn a_host_given_one_event_sees_it_alone(db: PgPool) {
    let world = World::with_sso(db, true).await;
    world.summer_party().await;
    world.provider().log_in("nils", json!([]));
    let a = admin();
    let sign_on = filled(&a.log_in_with, "name", "Authelia");

    // With no role at the provider, Nils is turned away.
    let mut nils = world.browser();
    nils.open(ADMIN).await;
    assert_eq!(nils.click(&sign_on).await.status, StatusCode::FORBIDDEN);

    // Until an admin makes them an editor of the Christmas party, by their user name.
    let mut host = world.admin().await;
    host.click("Christmas party").await;
    host.click(&a.access).await.expect(&a.no_access);
    host.fill(&a.user_name, "nils").check(&a.role_editor);
    host.press(&a.give_access)
        .await
        .expect(&a.saved)
        .expect("nils")
        .expect(&a.role_editor);

    // Then they see that event alone, and run it, but don't give others access.
    nils.open(ADMIN).await;
    nils.click(&sign_on)
        .await
        .expect("Christmas party")
        .expect_not("Summer party")
        .expect_not(&a.new_event);
    nils.click("Christmas party").await;
    let tabs: Vec<_> = nils
        .page()
        .doc
        .links
        .iter()
        .map(|link| &link.text)
        .collect();
    assert!(tabs.contains(&&a.invitation_links), "{tabs:?}");
    assert!(!tabs.contains(&&a.access), "{tabs:?}");

    // Taken away, it's gone at once.
    host.press(&a.remove_access).await.expect(&a.no_access);
    let page = nils.click(&a.invitation_links).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn the_built_in_user_and_the_provider_can_both_log_in(db: PgPool) {
    let world = World::with_sso(db, true).await;
    let a = admin();

    // The login page offers both.
    let mut host = world.browser();
    host.open(ADMIN)
        .await
        .expect(&filled(&a.log_in_with, "name", "Authelia"))
        .expect(&a.log_in_or);
    host.click(&filled(&a.log_in_with, "name", "Authelia"))
        .await
        .expect(&a.events);

    // The built-in user is an admin.
    let host = world.admin().await;
    host.page().expect(&a.new_event);
}
