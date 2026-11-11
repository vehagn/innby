use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::*;
use crate::{
    admin::{
        role::Role,
        testing::{Admin, set_cookies},
    },
    db::{group, rsvp},
    testing::{body, escape, location},
};

#[sqlx::test]
async fn each_role_reaches_only_its_pages(db: PgPool) {
    let event = crate::db::event::tests::event(&db).await;
    let invite = rsvp::create_invite(&db, event.id, "Kari", 2).await.unwrap();
    let group = group::create(&db, event.id, "Climbing club", 2)
        .await
        .unwrap();
    let on = |pattern: &str, id: Uuid| paths::path(pattern, id);
    let (e, i, g) = (event.id, invite.id, group.id);
    // Every page behind the login, and the role it needs. Forms are sent empty, so
    // the ones a role may use turn the request down for lack of a confirmation or
    // a name, rather than change anything worth checking here.
    let pages = [
        (Method::GET, paths::HOME.to_owned(), Role::Viewer),
        (Method::GET, on(paths::EVENT, e), Role::Viewer),
        (Method::GET, on(paths::EVENT_INVITATIONS, e), Role::Editor),
        (Method::POST, on(paths::EVENT_INVITATIONS, e), Role::Editor),
        (Method::GET, on(paths::EVENT_GROUPS, e), Role::Editor),
        (Method::POST, on(paths::EVENT_GROUPS, e), Role::Editor),
        (Method::GET, on(paths::EVENT_DETAILS, e), Role::Editor),
        (Method::POST, on(paths::EVENT_DETAILS, e), Role::Editor),
        (Method::GET, on(paths::EVENT_CSV, e), Role::Editor),
        (Method::POST, on(paths::EVENT_REMINDERS, e), Role::Editor),
        (Method::GET, on(paths::GUEST, i), Role::Editor),
        (Method::POST, on(paths::GUEST, i), Role::Editor),
        (Method::GET, on(paths::GUEST_QR, i), Role::Editor),
        (Method::POST, on(paths::GUEST_DELETE, i), Role::Editor),
        (Method::GET, on(paths::GROUP, g), Role::Editor),
        (Method::POST, on(paths::GROUP, g), Role::Editor),
        (Method::GET, on(paths::GROUP_QR, g), Role::Editor),
        (Method::POST, on(paths::GROUP_DELETE, g), Role::Editor),
        (Method::GET, paths::EVENTS_NEW.to_owned(), Role::Creator),
        (Method::POST, paths::EVENTS_NEW.to_owned(), Role::Creator),
        (Method::POST, on(paths::EVENT_DELETE, e), Role::Admin),
        (Method::GET, on(paths::EVENT_EXPORT, e), Role::Editor),
        (Method::POST, paths::EVENTS_IMPORT.to_owned(), Role::Creator),
        (Method::GET, paths::USERS.to_owned(), Role::Admin),
        (Method::GET, paths::LANGUAGES.to_owned(), Role::Admin),
        (Method::GET, on(paths::EVENT_QR, e), Role::Editor),
        (Method::POST, on(paths::EVENT_QR, e), Role::Editor),
        (Method::GET, on(paths::EVENT_GAMES, e), Role::Editor),
        (Method::POST, on(paths::EVENT_GAMES, e), Role::Editor),
        (Method::GET, on(paths::EVENT_ACCESS, e), Role::Admin),
        (Method::POST, on(paths::EVENT_ACCESS, e), Role::Admin),
        (Method::POST, on(paths::EVENT_ACCESS_REMOVE, e), Role::Admin),
    ];
    for role in Role::ALL {
        let host = Admin::as_role(db.clone(), role);
        for (method, path, needs) in &pages {
            let request = Request::builder().method(method.clone()).uri(path);
            let response = host.send(host.logged_in(request), "").await;
            let forbidden = response.status() == StatusCode::FORBIDDEN;
            assert_eq!(forbidden, role < *needs, "{role:?} {method} {path}");
        }
    }
    // The revoking forms change something even when empty, so they're tried last.
    for (pattern, id) in [(paths::GUEST_REVOKE, i), (paths::GROUP_REVOKE, g)] {
        let path = on(pattern, id);
        let viewer = Admin::as_role(db.clone(), Role::Viewer);
        assert_eq!(viewer.post(&path, "").await.status(), StatusCode::FORBIDDEN);
        let editor = Admin::as_role(db.clone(), Role::Editor);
        assert_ne!(editor.post(&path, "").await.status(), StatusCode::FORBIDDEN);
    }

    // Pages show only what the role may use.
    let viewer = Admin::as_role(db.clone(), Role::Viewer);
    let html = body(viewer.get(&on(paths::EVENT, e)).await).await;
    assert!(html.contains("Kari") && !html.contains(&on(paths::GUEST, i)));
    assert!(!html.contains("<th>E-mail</th>"));
    for hidden in [
        "guests.csv",
        "/reminders\"",
        "/invitations\"",
        "/details\"",
        "/qr-style\"",
        "/access\"",
    ] {
        assert!(!html.contains(hidden), "{hidden}");
    }
    let html = body(viewer.get(paths::HOME).await).await;
    assert!(!html.contains(r#"href="/events/new""#));
    // With one tab each, there's no tab bar to repeat the heading.
    assert!(!html.contains(r#"class="tabs""#));
    let html = body(viewer.get(&on(paths::EVENT, e)).await).await;
    assert!(!html.contains(r#"class="tabs""#));

    let editor = Admin::as_role(db.clone(), Role::Editor);
    let html = body(editor.get(&on(paths::EVENT, e)).await).await;
    assert!(html.contains(&on(paths::GUEST, i)) && html.contains("guests.csv"));
    let copy = format!(r#"href="/events/new?copy={e}#new-event""#);
    assert!(html.contains("<th>E-mail</th>"));
    assert!(html.contains(&on(paths::EVENT_QR, e)));
    let html = body(editor.get(&on(paths::EVENT_DETAILS, e)).await).await;
    assert!(!html.contains("/delete") && !html.contains("/access\""));
    assert!(!html.contains(&copy), "{html}");
    let html = body(editor.get(paths::HOME).await).await;
    assert!(!html.contains(r#"href="/events/new""#));

    // A creator makes events, and copies them, but neither deletes them nor sees the
    // admin's tabs.
    let creator = Admin::as_role(db.clone(), Role::Creator);
    let html = body(creator.get(&on(paths::EVENT_DETAILS, e)).await).await;
    assert!(html.contains(&copy), "{html}");
    assert!(!html.contains("/delete") && !html.contains("/access\""));
    let html = body(creator.get(paths::HOME).await).await;
    assert!(html.contains(r#"href="/events/new""#), "{html}");
    assert!(!html.contains(r#"href="/users""#) && !html.contains(r#"href="/languages""#));

    let admin = Admin::new(db);
    let html = body(admin.get(&on(paths::EVENT_DETAILS, e)).await).await;
    assert!(html.contains("/delete") && html.contains("/access\""));
    let html = body(admin.get(paths::HOME).await).await;
    assert!(html.contains(r#"href="/events/new""#) && html.contains(r#"class="tabs""#));
}

#[sqlx::test]
async fn the_host_picks_an_events_games_and_sees_their_scores(db: PgPool) {
    let mut admin = Admin::new(db.clone());
    let event = admin.event_id().await;
    let games_tab = paths::path(paths::EVENT_GAMES, event);
    // Without games in the config, there's no tab.
    let html = body(admin.get(&admin.event().await).await).await;
    assert!(!html.contains(&format!(r#"href="{games_tab}""#)));

    admin.config = crate::config::tests::parse(&[
        ("INNBY_GAMES_URL", "https://games.example.com"),
        (
            "INNBY_GAMES",
            "quiz:time:Quick quiz; pinball:points:Pinball",
        ),
    ])
    .unwrap();
    let html = body(admin.get(&admin.event().await).await).await;
    assert!(html.contains(&format!(r#"href="{games_tab}""#)), "{html}");

    // Ticking the quiz, and a game the config doesn't have, offers only the quiz.
    let response = admin.post(&games_tab, "game=quiz&game=chess").await;
    assert_eq!(location(&response), format!("{games_tab}?saved"));
    assert_eq!(
        crate::db::games::offered(&db, event).await.unwrap(),
        ["quiz"]
    );
    let kari = admin.invite("Kari Nordmann", 1).await;
    crate::db::games::record(&db, kari.id, "quiz", 61_500, false)
        .await
        .unwrap();
    let html = body(admin.get(&games_tab).await).await;
    // The full name, which hosts see.
    assert!(
        html.contains("<td>Kari Nordmann</td><td>61.50 s</td>"),
        "{html}"
    );
    assert!(html.contains(r#"value="quiz" checked"#) && html.contains(r#"value="pinball">"#));

    // Unticked, the quiz goes with its scores.
    admin.post(&games_tab, "").await;
    assert!(
        crate::db::games::offered(&db, event)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        crate::db::games::of_guest(&db, kari.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test]
async fn an_admin_makes_password_users_and_their_changes_count_at_once(db: PgPool) {
    let admin = Admin::new(db.clone());
    let app = router(admin.config.clone(), &admin.admin, db.clone());
    let send = |method: &str, uri: &str, cookie: &str, form: &str| {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT_LANGUAGE, "en-GB")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(form.to_owned()))
            .unwrap();
        let app = app.clone();
        async move { app.oneshot(request).await.unwrap() }
    };
    let log_in = |user: &str, password: &str| {
        let form = format!("user={user}&password={password}");
        async move {
            let response = send("POST", "/login", "", &form).await;
            let cookies: Vec<_> = set_cookies(&response).into_iter().map(|(c, _)| c).collect();
            let to = response.headers().get(header::LOCATION).map_or_else(
                || response.status().to_string(),
                |to| to.to_str().unwrap().to_owned(),
            );
            (to, cookies.join("; "))
        }
    };
    let event = crate::db::event::tests::event(&db).await;
    let invitations = paths::path(paths::EVENT_INVITATIONS, event.id);

    // Only admins see users, and the config's are there, to read.
    let editor = Admin::as_role(db.clone(), Role::Editor);
    assert_eq!(
        editor.get(paths::USERS).await.status(),
        StatusCode::FORBIDDEN
    );
    let html = body(admin.get(paths::USERS).await).await;
    assert!(html.contains("<td>ola.n</td>") && html.contains("No users made here yet."));
    let editor_page = body(editor.get(paths::HOME).await).await;
    assert!(!editor_page.contains(r#"href="/users""#));

    // A short password, a config user's name, or a name that can't be typed, refused.
    for form in [
        "name=kari&role=editor&password=short",
        "name=ola.n&role=editor&password=long-enough-1",
        "name=k%C3%A5re&role=editor&password=long-enough-1",
        "name=kari&role=owner&password=long-enough-1",
    ] {
        let response = admin.post(paths::USERS, form).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{form}"
        );
    }
    let response = admin
        .post(paths::USERS, "name=kari&role=editor&password=long-enough-1")
        .await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        body(response).await
    );
    let response = admin.get("/users/kari?saved").await;
    assert!(body(response).await.contains("Saved."));

    // Kari logs in, and runs the event.
    let (home, kari) = log_in("kari", "long-enough-1").await;
    assert_eq!(home, "/");
    assert_eq!(
        send("GET", &invitations, &kari, "").await.status(),
        StatusCode::OK
    );

    // A new role counts on her next request, without logging in again.
    admin.post("/users/kari", "role=viewer").await;
    assert_eq!(
        send("GET", &invitations, &kari, "").await.status(),
        StatusCode::FORBIDDEN
    );
    let guests = paths::path(paths::EVENT, event.id);
    assert_eq!(
        send("GET", &guests, &kari, "").await.status(),
        StatusCode::OK
    );

    // A new password logs her out everywhere, refresh token and all.
    admin.post("/users/kari", "password=another-one-2").await;
    assert_eq!(location(&send("GET", &guests, &kari, "").await), "/login");
    let (refused, _) = log_in("kari", "long-enough-1").await;
    assert_ne!(refused, "/");
    let (home, kari) = log_in("kari", "another-one-2").await;
    assert_eq!(home, "/");

    // Deleting her ends that too, and takes the roles events gave her.
    crate::db::access::give(&db, event.id, "kari", crate::db::access::EventRole::Editor)
        .await
        .unwrap();
    let response = admin.post("/users/kari/delete", "").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    admin.post("/users/kari/delete", "confirm=yes").await;
    assert_eq!(location(&send("GET", &guests, &kari, "").await), "/login");
    assert!(!crate::db::access::has_any(&db, "kari").await.unwrap());

    // An admin can't change or delete themselves here.
    admin
        .post(paths::USERS, "name=boss&role=admin&password=long-enough-1")
        .await;
    let (_, boss) = log_in("boss", "long-enough-1").await;
    let response = send("POST", "/users/boss", &boss, "role=none").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = send("POST", "/users/boss/delete", &boss, "confirm=yes").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(send("GET", paths::USERS, &boss, "").await).await;
    assert!(html.contains(r#"<a href="/users/boss">boss</a>"#), "{html}");
}

async fn status(host: &Admin, path: String) -> StatusCode {
    host.get(&path).await.status()
}

#[sqlx::test]
async fn an_event_gives_a_host_a_role_for_it_alone(db: PgPool) {
    let admin = Admin::new(db.clone());
    let ours = crate::db::event::tests::event(&db).await;
    let mut other = crate::db::event::tests::details();
    other.title = crate::i18n::Texts::from([(crate::i18n::Lang::NB, "Sommerfest")]);
    let other = crate::db::event::create(&db, &other).await.unwrap();
    let guest = rsvp::create_invite(&db, ours.id, "Kari", 2).await.unwrap();
    let club = group::create(&db, ours.id, "Climbing club", 2)
        .await
        .unwrap();
    let elsewhere = rsvp::create_invite(&db, other.id, "Per", 1).await.unwrap();
    let on = |pattern: &str, id: Uuid| paths::path(pattern, id);
    let access = on(paths::EVENT_ACCESS, ours.id);

    // Nils has no role of their own, so sees no event at first.
    let nils = Admin::as_user(db.clone(), "nils", None);
    let html = body(nils.get(paths::HOME).await).await;
    assert!(
        html.contains(&escape("You don't have access to any events yet.")),
        "{html}"
    );
    assert_eq!(
        status(&nils, on(paths::EVENT, ours.id)).await,
        StatusCode::FORBIDDEN
    );

    // An admin makes them a viewer of the Christmas party.
    let response = admin.post(&access, "user=+nils+&role=viewer").await;
    assert_eq!(location(&response), format!("{access}?saved"));
    let html = body(admin.get(&format!("{access}?saved")).await).await;
    assert!(
        html.contains("<td>nils</td>") && html.contains("<td>Viewer</td>"),
        "{html}"
    );
    let html = body(nils.get(paths::HOME).await).await;
    assert!(
        html.contains("Christmas party") && !html.contains("Sommerfest"),
        "{html}"
    );
    assert_eq!(
        status(&nils, on(paths::EVENT, ours.id)).await,
        StatusCode::OK
    );
    for path in [
        on(paths::EVENT_INVITATIONS, ours.id),
        on(paths::GUEST, guest.id),
        on(paths::EVENT, other.id),
    ] {
        assert_eq!(
            status(&nils, path.clone()).await,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }

    // Then an editor of it, in place of a viewer: the event's guests and groups too,
    // but not another event's, nor an admin's pages.
    admin.post(&access, "user=nils&role=editor").await;
    for path in [
        on(paths::EVENT_INVITATIONS, ours.id),
        on(paths::EVENT_DETAILS, ours.id),
        on(paths::GUEST, guest.id),
        on(paths::GROUP_QR, club.id),
    ] {
        assert_eq!(status(&nils, path.clone()).await, StatusCode::OK, "{path}");
    }
    for path in [
        on(paths::GUEST, elsewhere.id),
        on(paths::EVENT_INVITATIONS, other.id),
        access.clone(),
    ] {
        assert_eq!(
            status(&nils, path.clone()).await,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let response = nils
        .post(&on(paths::EVENT_DELETE, ours.id), "confirm=on")
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let html = body(nils.get(&on(paths::EVENT_DETAILS, ours.id)).await).await;
    assert!(!html.contains("/delete") && !html.contains("/access\""));

    // A viewer of every event gets an editor's role for this one alone.
    let per = Admin::as_user(db.clone(), "per", Some(Role::Viewer));
    admin.post(&access, "user=per&role=editor").await;
    assert_eq!(
        status(&per, on(paths::GUEST, guest.id)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&per, on(paths::GUEST, elsewhere.id)).await,
        StatusCode::FORBIDDEN
    );
    let html = body(per.get(paths::HOME).await).await;
    assert!(html.contains("Sommerfest"));

    // Names are refused with a reason, and taken away again.
    let response = admin.post(&access, "user=%07&role=editor").await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(response).await.contains("on one line"));
    admin
        .post(&on(paths::EVENT_ACCESS_REMOVE, ours.id), "user=nils")
        .await;
    assert_eq!(
        status(&nils, on(paths::EVENT, ours.id)).await,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test]
async fn serves_under_a_path_with_the_probes_at_the_root(db: PgPool) {
    let mut admin = Admin::new(db);
    admin.admin.path = "/admin".into();
    let status = |response: Response| response.status();
    for probe in [paths::HEALTH, paths::READY] {
        assert_eq!(
            status(admin.send(Request::get(probe), "").await),
            StatusCode::OK
        );
    }
    let response = admin.send(Request::get("/admin"), "").await;
    assert_eq!(location(&response), "/admin/login");
    let response = admin.send(Request::get("/admin/"), "").await;
    assert_eq!(location(&response), "/admin");
    let html = body(admin.send(Request::get("/admin/login"), "").await).await;
    assert!(html.contains(r#"href="/admin/static/style.css"#), "{html}");
    assert!(html.contains(r#"action="/admin/login""#));
    let css = admin.send(
        Request::get(paths::STYLESHEET).uri("/admin/static/style.css"),
        "",
    );
    assert_eq!(status(css.await), StatusCode::OK);
    // Nothing of the admin's outside its path.
    assert_eq!(status(admin.get("/events").await), StatusCode::NOT_FOUND);

    let response = admin.get("/admin").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(
        html.contains(r#"<a class="brand" href="/admin">"#),
        "{html}"
    );
    assert!(html.contains(r#"href="/admin/events/new""#), "{html}");
    let html = body(admin.get("/admin/events/new").await).await;
    assert!(
        html.contains(r#"action="/admin/events/new#new-event""#),
        "{html}"
    );
}

#[sqlx::test]
async fn requires_login_except_for_probes(db: PgPool) {
    let admin = Admin::new(db);
    for cookie in ["", "token=garbage", "refresh_token=garbage"] {
        let response = admin
            .send(Request::get("/").header(header::COOKIE, cookie), "")
            .await;
        assert_eq!(location(&response), "/login", "{cookie}");
    }
    // A form posted without a login doesn't run either.
    let response = admin.send(Request::post("/events/new"), "title=Fest").await;
    assert_eq!(location(&response), "/login");
    assert!(admin.guests().await.is_empty());

    let response = admin.send(Request::get("/healthz"), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = admin.send(Request::get("/login"), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body(response).await.contains(r#"type="password""#));

    let response = admin.get("/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[sqlx::test]
async fn logs_in_refreshes_and_logs_out(db: PgPool) {
    let admin = Admin::new(db);
    let app = router(admin.config.clone(), &admin.admin, admin.db.clone());
    let send = |method: &str, uri: &str, cookie: &str, form: &str| {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT_LANGUAGE, "en-GB")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(form.to_owned()))
            .unwrap();
        let app = app.clone();
        async move { app.oneshot(request).await.unwrap() }
    };

    let response = send("POST", "/login", "", "user=admin&password=wrong").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(set_cookies(&response).is_empty());
    let html = body(response).await;
    assert!(html.contains("Wrong user name or password."), "{html}");
    assert!(html.contains(r#"value="admin""#), "{html}");

    let response = send("POST", "/login", "", "user=admin&password=secret").await;
    assert_eq!(location(&response), "/");
    let cookies = set_cookies(&response);
    let [(token, token_header), (refresh, refresh_header)] = &cookies[..] else {
        panic!("{cookies:?}");
    };
    assert!(token.starts_with("token=ey"), "{token}");
    assert!(token_header.ends_with("; HttpOnly; SameSite=Lax; Secure"));
    assert!(refresh.starts_with("refresh_token="), "{refresh}");
    assert!(refresh_header.contains("; Max-Age=2592000;"));
    let both = format!("{token}; {refresh}");

    let response = send("GET", "/", &both, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(set_cookies(&response).is_empty());
    // Without a JWT, as once it expires, the refresh token gets a new one.
    let response = send("GET", "/", refresh, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let renewed = set_cookies(&response);
    assert!(renewed[0].0.starts_with("token=ey"), "{renewed:?}");
    assert_ne!(&renewed[0].0, token);

    let response = send("POST", "/logout", &both, "").await;
    assert_eq!(location(&response), "/login?logged_out");
    let cleared = set_cookies(&response);
    assert_eq!(
        cleared
            .iter()
            .map(|(cookie, _)| cookie.as_str())
            .collect::<Vec<_>>(),
        ["token=", "refresh_token="]
    );
    assert!(cleared[0].1.contains("; Max-Age=0;"));
    let html = body(send("GET", "/login?logged_out", "", "").await).await;
    assert!(html.contains(&escape("You're logged out.")), "{html}");

    // Copies of the cookies stop working too.
    for cookie in [both.as_str(), token, refresh] {
        let response = send("GET", "/", cookie, "").await;
        assert_eq!(location(&response), "/login", "{cookie}");
    }
    // Another login still works.
    let response = send("POST", "/login", "", "user=admin&password=secret").await;
    assert_eq!(location(&response), "/");

    // Another user has the role the config gives them, and keeps it when the refresh
    // token renews their JWT. Their refresh token is no one else's.
    let response = send("POST", "/login", "", "user=ola.n&password=ola").await;
    let cookies = set_cookies(&response);
    let [(token, _), (refresh, _)] = &cookies[..] else {
        panic!("{cookies:?}");
    };
    assert!(refresh.starts_with("refresh_token=ola.n."), "{refresh}");
    let delete = paths::path(paths::EVENT_DELETE, Uuid::nil());
    for cookie in [token, refresh] {
        let details = paths::path(paths::EVENT_DETAILS, Uuid::nil());
        assert_eq!(
            send("GET", &details, cookie, "").await.status(),
            StatusCode::NOT_FOUND
        );
        let response = send("POST", &delete, cookie, "").await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{cookie}");
    }
    let as_admin = refresh.replace("ola.n.", "admin.");
    assert_eq!(location(&send("GET", "/", &as_admin, "").await), "/login");
}

#[sqlx::test]
async fn speaks_the_browsers_language_or_the_one_picked(db: PgPool) {
    let admin = Admin::new(db);
    let e = admin.event().await;
    let request = admin
        .logged_in(Request::get(format!("{e}/invitations")))
        .header(header::ACCEPT_LANGUAGE, "nn-NO")
        .body(Body::empty())
        .unwrap();
    let app = router(admin.config.clone(), &admin.admin, admin.db.clone());
    let html = body(app.oneshot(request).await.unwrap()).await;
    assert!(html.contains(r#"<html lang="nn""#), "{html}");
    assert!(html.contains("Nye invitasjonar"), "{html}");
    // The browser's language needs no carrying.
    assert!(
        html.contains(&format!(r#"action="{e}/invitations""#)),
        "{html}"
    );

    // A picked one is carried, through forms and redirects too.
    let html = body(
        admin
            .get(&format!("{e}/invitations?lang=zh-hant&theme=dark"))
            .await,
    )
    .await;
    assert!(html.contains("新邀請"), "{html}");
    assert!(
        html.contains(&format!(
            r#"action="{e}/invitations?lang=zh-hant&#38;theme=dark""#
        )),
        "{html}"
    );
    let response = admin
        .post(
            &format!("{e}/groups?lang=zh-hant"),
            "names=%E9%84%B0%E5%B1%85&max_party_size=1",
        )
        .await;
    assert_eq!(
        location(&response),
        format!("{e}/groups?created=1&lang=zh-hant")
    );
    let html = body(admin.get(location(&response)).await).await;
    assert!(html.contains("已建立 1 個群組。"), "{html}");
    // A form with a mistake says so in the picked language.
    let response = admin
        .post(
            &format!("{e}/invitations?lang=nb"),
            "names=Kari%2C+0&max_party_size=1",
        )
        .await;
    let html = body(response).await;
    assert!(
        html.contains("Linje 1: Antall personer må være mellom 1 og 20."),
        "{html}"
    );
}

#[sqlx::test]
async fn links_carry_the_colour_scheme(db: PgPool) {
    let admin = Admin::new(db);
    let e = admin.event().await;
    let html = body(admin.get(&format!("{e}?theme=dark")).await).await;
    assert!(html.contains(r#"data-theme="dark""#), "{html}");
    assert!(
        html.contains(&format!(r#"href="{e}/invitations?theme=dark""#)),
        "{html}"
    );
    let html = body(admin.get(&format!("{e}/invitations?theme=dark")).await).await;
    assert!(
        html.contains(&format!(r#"action="{e}/invitations?theme=dark""#)),
        "{html}"
    );
    assert!(html.contains(r#"action="/logout?theme=dark""#), "{html}");

    let response = admin
        .post(
            &format!("{e}/invitations?theme=dark"),
            "names=Eva&max_party_size=1",
        )
        .await;
    assert!(
        location(&response).ends_with("&theme=dark"),
        "{}",
        location(&response)
    );
    let response = admin
        .post(
            &format!("{e}/groups?theme=light"),
            "names=Climbers&max_party_size=1",
        )
        .await;
    assert_eq!(
        location(&response),
        format!("{e}/groups?created=1&theme=light")
    );

    // The login page has its style before anyone logs in.
    let response = admin.send(Request::get("/static/style.css"), "").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[sqlx::test]
async fn each_page_says_where_it_is(db: PgPool) {
    let admin = Admin::new(db);
    let e = admin.event().await;
    let invite = admin.invite("Kari", 1).await;
    let club = group::create(&admin.db, admin.event_id().await, "Climbing club", 2)
        .await
        .unwrap();
    // The trail's steps, as the page shows them: a link above, the page's own level last.
    let trail = |html: &str| {
        let nav = html
            .split(r#"<nav class="crumb""#)
            .nth(1)
            .and_then(|rest| rest.split("</nav>").next())
            .unwrap_or_default()
            .to_owned();
        nav.split("<li>")
            .skip(1)
            .map(|step| {
                let label = step.split('>').nth(1).unwrap().split('<').next().unwrap();
                match step.split(r#"href=""#).nth(1) {
                    Some(href) => format!("{label} @ {}", href.split('"').next().unwrap()),
                    None => label.to_owned(),
                }
            })
            .collect::<Vec<_>>()
    };
    let events = "Events @ /".to_owned();
    let party = format!("Christmas party @ {e}");

    let html = body(admin.get(&format!("{e}/details")).await).await;
    assert_eq!(trail(&html), [events.as_str(), "Christmas party"]);
    let html = body(admin.get(&format!("/guests/{}", invite.id)).await).await;
    assert_eq!(trail(&html), [events.as_str(), party.as_str(), "Kari"]);
    let html = body(admin.get(&format!("/groups/{}", club.id)).await).await;
    assert_eq!(
        trail(&html),
        [
            events.as_str(),
            party.as_str(),
            &format!("Group links @ {e}/groups"),
            "Climbing club"
        ]
    );
    assert!(html.contains(r#"<span aria-current="page">Climbing club</span>"#));
    // The events themselves are the top: no trail.
    assert!(trail(&body(admin.get("/").await).await).is_empty());
}
