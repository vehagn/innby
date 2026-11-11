//! The event as a calendar file, `/r/{link}/event.ics`, for a guest to add to their own
//! calendar from their page; `crate::calendar` writes it.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};

use super::{AppState, rsvp::Guest};
use crate::calendar;

/// The guest's event, in their language.
pub async fn show(State(state): State<AppState>, guest: Guest) -> Response {
    let lang = guest.lang;
    let file = calendar::file(&guest.event, lang, &state.config.base_url);
    (
        [
            (header::CONTENT_TYPE, calendar::CONTENT_TYPE),
            // Inline, so a phone offers to add it to the calendar rather than save it.
            (
                header::CONTENT_DISPOSITION,
                r#"inline; filename="event.ics""#,
            ),
        ],
        file,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
