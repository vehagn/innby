//! QR codes: each event's style, which all its codes are drawn in (the event's QR codes
//! tab), and the SVG files a guest's or a group's page downloads. `draw` draws them.

mod draw;

pub use draw::{check, svg};

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, load_event, redirect},
};
use crate::{
    config::Config,
    db::{self, event::Event},
    http::{render, render_with},
    paths::{self, EventLinks},
};
use uuid::Uuid;

#[derive(Template)]
#[template(path = "admin/qr.html")]
pub(super) struct QrPage {
    pub(super) event: Event,
    pub(super) links: EventLinks,
    pub(super) qr_style: db::qr::Style,
    /// A code for the event page, in that style.
    pub(super) qr_preview: String,
    pub(super) saved: bool,
    pub(super) error: Option<String>,
}

impl QrPage {
    pub(super) fn new(config: &Config, event: Event, qr_style: db::qr::Style) -> Self {
        Self {
            links: EventLinks::new(event.id),
            event,
            qr_preview: draw::svg(&config.url(paths::HOME), &qr_style, None),
            qr_style,
            saved: false,
            error: None,
        }
    }
}

/// The event's QR code style, with an example.
pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let style = db::qr::style(&state.db, id).await?;
    Ok(render(QrPage {
        saved: query.saved.is_some(),
        ..QrPage::new(&state.config, event, style)
    }))
}

/// Saves the style every QR code of the event is drawn in.
pub(super) async fn save(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<draw::StyleForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let style = match form.validate() {
        Ok(style) => style,
        Err(error) => {
            let saved = db::qr::style(&state.db, id).await?;
            let page = QrPage {
                error: Some(error),
                ..QrPage::new(&state.config, event, saved)
            };
            return Ok(render_with(StatusCode::UNPROCESSABLE_ENTITY, page));
        }
    };
    if !db::qr::save(&state.db, id, &style).await? {
        return Err(PageError::Missing);
    }
    tracing::info!(event = %id, "qr style saved");
    Ok(redirect(&format!(
        "{}?saved",
        paths::path(paths::EVENT_QR, id)
    )))
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
        testing::{body, location},
    };

    #[sqlx::test]
    async fn styles_and_downloads_qr_codes(db: PgPool) {
        let admin = Admin::new(db);
        let e = admin.event().await;
        let page = format!("{e}/qr-style");
        let html = body(admin.get(&page).await).await;
        assert!(html.contains(r#"value="rounded" checked"#), "{html}");

        // A colour phones can't read on white is refused.
        let response = admin
            .post(&page, "shape=dots&color=%23ffeb3b&icon=none")
            .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body(response).await.contains("Pick a darker colour"));
        let response = admin
            .post(&page, "shape=dots&color=%23000000&icon=emoji&emoji=")
            .await;
        assert!(body(response).await.contains("Enter one emoji"));

        let response = admin
            .post(
                &page,
                "shape=dots&color=%23000000&icon=emoji&emoji=%F0%9F%8E%89",
            )
            .await;
        assert_eq!(location(&response), format!("{page}?saved"));
        let html = body(admin.get(&format!("{page}?saved")).await).await;
        assert!(html.contains("QR code style saved."), "{html}");
        assert!(html.contains(r#"value="dots" checked"#), "{html}");
        assert!(html.contains("🎉</text>"), "{html}");

        admin
            .post(
                &format!("{e}/invitations"),
                "names=Kari+Nordmann&max_party_size=1",
            )
            .await;
        let id = admin.guests().await[0].id;
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

        // Another event keeps its own style: the default, until its host picks one.
        let summer = crate::db::event::create(&admin.db, &crate::db::event::tests::details())
            .await
            .unwrap();
        let invite = crate::db::rsvp::create_invite(&admin.db, summer.id, "Ola", 1)
            .await
            .unwrap();
        let svg = body(admin.get(&format!("/guests/{}/qr.svg", invite.id)).await).await;
        assert!(svg.contains(r##"fill="#00695c""##), "{svg}");
        assert!(!svg.contains("🎉"));
        let html = body(admin.get(&format!("/events/{}/qr-style", summer.id)).await).await;
        assert!(html.contains(r#"value="rounded" checked"#), "{html}");
    }
}
