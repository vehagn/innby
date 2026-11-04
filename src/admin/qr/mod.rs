//! QR codes: the one style every code is drawn in (the QR codes tab), and the SVG files
//! a guest's or a group's page downloads. `draw` draws them.

mod draw;

pub use draw::svg;

use askama::Template;
use axum::{
    Form,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use super::{
    lang,
    page::{AdminState, SavedQuery, redirect},
};
use crate::{
    config::Config,
    db::{self},
    http::{render, render_with, server_error},
    paths::{self},
};

#[derive(Template)]
#[template(path = "admin/qr.html")]
pub(super) struct QrPage {
    pub(super) qr_style: db::qr::Style,
    /// A code for the event page, in that style.
    pub(super) qr_preview: String,
    pub(super) saved: bool,
    pub(super) error: Option<String>,
}

impl QrPage {
    pub(super) fn new(config: &Config, qr_style: db::qr::Style) -> Self {
        Self {
            qr_preview: draw::svg(&config.url(paths::HOME), &qr_style, None),
            qr_style,
            saved: false,
            error: None,
        }
    }
}

/// The QR code style, with an example.
pub(super) async fn qr_style(
    State(state): State<AdminState>,
    Query(query): Query<SavedQuery>,
) -> Response {
    match db::qr::style(&state.db).await {
        Ok(style) => render(QrPage {
            saved: query.saved.is_some(),
            ..QrPage::new(&state.config, style)
        }),
        Err(err) => server_error(err),
    }
}

/// Saves the QR code style that every code on these pages uses.
pub(super) async fn save_qr_style(
    State(state): State<AdminState>,
    Form(form): Form<draw::StyleForm>,
) -> Response {
    let style = match form.validate() {
        Ok(style) => style,
        Err(error) => {
            let saved = match db::qr::style(&state.db).await {
                Ok(saved) => saved,
                Err(err) => return server_error(err),
            };
            let page = QrPage {
                error: Some(error),
                ..QrPage::new(&state.config, saved)
            };
            return render_with(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    if let Err(err) = db::qr::save(&state.db, &style).await {
        return server_error(err);
    }
    tracing::info!("qr style saved");
    redirect(&format!("{}?saved", paths::QR_STYLE))
}

/// A QR code as an SVG file to save, named after what it's for.
pub(super) fn qr_file(url: &str, style: &db::qr::Style, name: &str) -> Response {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "code" } else { slug };
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}\n",
        draw::svg(url, style, Some(1024))
    );
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!(r#"attachment; filename="qr-{slug}.svg""#),
            ),
        ],
        svg,
    )
        .into_response()
}

#[cfg(test)]
mod tests {

    use sqlx::PgPool;

    use super::*;
    use crate::{
        admin::testing::Admin,
        db::rsvp,
        testing::{body, location},
    };

    #[sqlx::test]
    async fn styles_and_downloads_qr_codes(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let html = body(admin.get("/qr-style").await).await;
        assert!(html.contains(r#"value="rounded" checked"#), "{html}");

        // A colour phones can't read on white is refused.
        let response = admin
            .post("/qr-style", "shape=dots&color=%23ffeb3b&icon=none")
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("Pick a darker colour"));
        let response = admin
            .post("/qr-style", "shape=dots&color=%23000000&icon=emoji&emoji=")
            .await;
        assert!(body(response).await.contains("Enter one emoji"));

        let response = admin
            .post(
                "/qr-style",
                "shape=dots&color=%23000000&icon=emoji&emoji=%F0%9F%8E%89",
            )
            .await;
        assert_eq!(location(&response), "/qr-style?saved");
        let html = body(admin.get("/qr-style?saved").await).await;
        assert!(html.contains("QR code style saved."), "{html}");
        assert!(html.contains(r#"value="dots" checked"#), "{html}");
        assert!(html.contains("🎉</text>"), "{html}");

        admin
            .post(
                &format!("{e}/invitations"),
                "names=Kari+Nordmann&max_party_size=1",
            )
            .await;
        let id = rsvp::list(
            &admin.db,
            crate::db::event::tests::event(&admin.db).await.id,
        )
        .await
        .unwrap()[0]
            .id;
        let html = body(admin.get(&format!("/guests/{id}")).await).await;
        assert!(html.contains(&format!(r#"href="/guests/{id}/qr.svg" download"#)));
        let response = admin.get(&format!("/guests/{id}/qr.svg")).await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            r#"attachment; filename="qr-kari-nordmann.svg""#
        );
        let svg = body(response).await;
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains(r##"fill="#000000""##));
        assert!(svg.contains(r#"width="1024""#));
    }
}
