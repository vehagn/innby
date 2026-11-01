use askama::Template;
use axum::{
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};

use super::AppState;
use crate::i18n::{Lang, Messages};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage<'a> {
    lang: Lang,
    t: &'static Messages,
    title: &'a str,
    description: &'a str,
    location: &'a str,
    starts_at: String,
    starts_at_iso: String,
}

pub async fn index(State(state): State<AppState>, lang: Lang) -> Response {
    let event = &state.config.event;
    let t = lang.messages();
    render(IndexPage {
        lang,
        t,
        title: event.title.get(lang),
        description: event.description.get(lang),
        location: &event.location,
        starts_at: event
            .starts_at
            .format_localized(t.date_format, lang.locale())
            .to_string(),
        starts_at_iso: event.starts_at.to_rfc3339(),
    })
}

pub async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../../static/style.css"),
    )
}

fn render(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "rendering template");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
