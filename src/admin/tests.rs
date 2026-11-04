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
        (Method::POST, paths::EVENTS.to_owned(), Role::Admin),
        (Method::POST, on(paths::EVENT_DELETE, e), Role::Admin),
        (Method::GET, on(paths::EVENT_EXPORT, e), Role::Editor),
        (Method::POST, paths::EVENTS_IMPORT.to_owned(), Role::Admin),
        (Method::GET, on(paths::EVENT_QR, e), Role::Editor),
        (Method::POST, on(paths::EVENT_QR, e), Role::Editor),
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
    for hidden in [
        "guests.csv",
        "/reminders\"",
        "/invitations\"",
        "/details\"",
        "/qr-style\"",
    ] {
        assert!(!html.contains(hidden), "{hidden}");
    }
    let html = body(viewer.get(paths::HOME).await).await;
    assert!(!html.contains(r#"action="/events#"#));

    let editor = Admin::as_role(db.clone(), Role::Editor);
    let html = body(editor.get(&on(paths::EVENT, e)).await).await;
    assert!(html.contains(&on(paths::GUEST, i)) && html.contains("guests.csv"));
    assert!(html.contains(&on(paths::EVENT_QR, e)));
    let html = body(editor.get(&on(paths::EVENT_DETAILS, e)).await).await;
    assert!(!html.contains("/delete"));
    let html = body(editor.get(paths::HOME).await).await;
    assert!(!html.contains(r#"action="/events#"#));

    let admin = Admin::new(db);
    let html = body(admin.get(&on(paths::EVENT_DETAILS, e)).await).await;
    assert!(html.contains("/delete"));
    let html = body(admin.get(paths::HOME).await).await;
    assert!(html.contains(r#"action="/events#"#));
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
    assert!(
        html.contains(r#"action="/admin/events#new-event""#),
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
    let response = admin.send(Request::post("/events"), "title=Fest").await;
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
