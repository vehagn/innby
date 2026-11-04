//! `innby admin`: the host's UI for events, their invitations, group links and
//! headcount, in the site's languages. Served on its own listener, at the root of its
//! own hostname or under a path of the site's (`INNBY_ADMIN_URL`), behind a login.
//!
//! - `mod.rs`: `router`, which lists every page and the role it needs.
//! - `page`: what pages share: state, tabs, errors.
//! - `events`, `guests`, `invitations`, `groups`, `qr`: the pages, one module per tab.
//! - `auth`: logging in, as the built-in user or through an OIDC provider, and sessions.
//! - `role`: what each role may do. `lang`: the page's language.

pub mod auth;
mod events;
mod groups;
mod guests;
mod invitations;
pub mod lang;
mod page;
mod qr;
pub mod role;
#[cfg(test)]
pub(crate) mod testing;

pub use invitations::invite;
pub use page::{Tab, tabs};

use std::sync::Arc;

use axum::{
    Router, middleware,
    response::Redirect,
    routing::{get, post},
};
use sqlx::PgPool;

use crate::{
    config::{AdminConfig, Config},
    http, paths,
};
use page::AdminState;

pub async fn serve(config: Config, admin: AdminConfig, db: PgPool) -> anyhow::Result<()> {
    let address = admin.listen;
    http::listen(address, router(config, &admin, db)).await
}

tokio::task_local! {
    static PREFIX: Arc<str>;
}

/// The path the admin UI is under, such as `/admin`, or empty at the root of its own
/// hostname. Links read it from here, as they read the language.
pub fn prefix() -> Arc<str> {
    PREFIX
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::from(""))
}

pub(crate) fn router(config: Config, admin_config: &AdminConfig, db: PgPool) -> Router {
    // The admin hostname is served the same way as the public one.
    let https = config.is_https();
    let auth = Arc::new(auth::Auth::new(admin_config, https, db.clone()));
    let state = AdminState {
        config: Arc::new(config),
        db,
        auth: Arc::clone(&auth),
    };
    // Each path needs one role for all its methods; see `role` for what each may do.
    let viewer = Router::new()
        .route(paths::HOME, get(events::list))
        .route(paths::EVENT, get(guests::event_guests));
    let editor = Router::new()
        .route(
            paths::EVENT_INVITATIONS,
            get(invitations::invitations).post(invitations::create_invitations),
        )
        .route(paths::EVENT_GROUPS, get(groups::list).post(groups::create))
        .route(
            paths::EVENT_DETAILS,
            get(events::details).post(events::edit),
        )
        .route(paths::EVENT_CSV, get(guests::export_csv))
        .route(paths::EVENT_REMINDERS, post(guests::queue_reminders))
        .route(paths::GROUP, get(groups::show).post(groups::edit))
        .route(paths::GROUP_REVOKE, post(groups::revoke))
        .route(paths::GROUP_DELETE, post(groups::delete))
        .route(paths::GUEST, get(guests::show).post(guests::edit))
        .route(paths::GUEST_REVOKE, post(guests::revoke))
        .route(paths::GUEST_DELETE, post(guests::delete))
        .route(paths::GUEST_QR, get(guests::guest_qr))
        .route(paths::GROUP_QR, get(groups::qr))
        .route_layer(middleware::from_fn(role::editor));
    let admin = Router::new()
        .route(paths::EVENTS, post(events::create))
        .route(paths::EVENT_DELETE, post(events::delete))
        .route(paths::QR_STYLE, get(qr::qr_style).post(qr::save_qr_style))
        .route_layer(middleware::from_fn(role::admin));
    let mut pages = viewer
        .merge(editor)
        .merge(admin)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth),
            auth::require_login,
        ))
        .route(paths::LOGOUT, post(auth::log_out));
    pages = if admin_config.login.is_some() {
        pages.route(paths::LOGIN, get(auth::login_page).post(auth::log_in))
    } else {
        pages.route(paths::LOGIN, get(auth::login_page))
    };
    if admin_config.oidc.is_some() {
        pages = pages
            .route(paths::LOGIN_OIDC, get(auth::oidc::start))
            .route(paths::LOGIN_OIDC_CALLBACK, get(auth::oidc::callback));
    }
    let prefix: Arc<str> = Arc::from(admin_config.path.as_str());
    let scoped = Arc::clone(&prefix);
    let router: Router = http::no_store(pages)
        // Probes don't log in, and the login page has its style before anyone does.
        .route(paths::HEALTH, get(|| async { "ok" }))
        .route(paths::READY, get(http::ready))
        .route(paths::STYLESHEET, get(http::stylesheet))
        .layer(middleware::from_fn(crate::theme::scope))
        .layer(middleware::from_fn(lang::scope))
        .layer(middleware::from_fn(
            move |request, next: middleware::Next| {
                PREFIX.scope(Arc::clone(&scoped), next.run(request))
            },
        ))
        .with_state(state.clone());
    let router = if prefix.is_empty() {
        router
    } else {
        // Under a path of the site's hostname, as `/admin`. The probes stay at the
        // root, where the kubelet asks.
        let home = paths::admin_under(&prefix, paths::HOME);
        Router::<AdminState>::new()
            .route(paths::HEALTH, get(|| async { "ok" }))
            .route(paths::READY, get(http::ready))
            .with_state(state)
            .nest(&prefix, router)
            .route(
                &format!("{prefix}/"),
                get(move || async move { Redirect::permanent(&home) }),
            )
    };
    http::harden(router, https)
}

#[cfg(test)]
mod tests {
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
            (Method::GET, paths::QR_STYLE.to_owned(), Role::Admin),
            (Method::POST, paths::QR_STYLE.to_owned(), Role::Admin),
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
        for hidden in ["guests.csv", "/reminders\"", "/invitations\"", "/details\""] {
            assert!(!html.contains(hidden), "{hidden}");
        }
        let html = body(viewer.get(paths::HOME).await).await;
        assert!(!html.contains(paths::QR_STYLE) && !html.contains(r#"action="/events""#));

        let editor = Admin::as_role(db.clone(), Role::Editor);
        let html = body(editor.get(&on(paths::EVENT, e)).await).await;
        assert!(html.contains(&on(paths::GUEST, i)) && html.contains("guests.csv"));
        let html = body(editor.get(&on(paths::EVENT_DETAILS, e)).await).await;
        assert!(!html.contains("/delete"));
        let html = body(editor.get(paths::HOME).await).await;
        assert!(!html.contains(paths::QR_STYLE) && !html.contains(r#"action="/events""#));

        let admin = Admin::new(db);
        let html = body(admin.get(&on(paths::EVENT_DETAILS, e)).await).await;
        assert!(html.contains("/delete"));
        let html = body(admin.get(paths::HOME).await).await;
        assert!(html.contains(paths::QR_STYLE) && html.contains(r#"action="/events""#));
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
        assert!(html.contains(r#"href="/admin/qr-style""#), "{html}");
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
        assert!(
            rsvp::list(
                &admin.db,
                crate::db::event::tests::event(&admin.db).await.id
            )
            .await
            .unwrap()
            .is_empty()
        );

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
}
