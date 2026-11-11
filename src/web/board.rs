//! The Board tab of a guest's pages, `/r/{link}/board`: the event's message board, when
//! its hosts have it on for guests (`db::board::BoardState`). Guests read and vote on
//! what hosts post, and when it's open post and reply too, signed as the guest list
//! shows them. A guest may delete their own posts; hosts any, in the admin UI.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AppState,
    pages::Page,
    rsvp::{Guest, Notice, ShowQuery},
    views::GuestTab,
};
use crate::{
    board::PostView,
    db::board::{self, Author, Step, Writer},
    http::{render, render_with},
    i18n::{Lang, Messages},
    icons::TabLink,
    paths::{self, GuestLinks},
};

#[derive(Template)]
#[template(path = "board.html")]
struct BoardPage {
    lang: Lang,
    t: &'static Messages,
    title: String,
    tabs: Vec<TabLink>,
    /// Where a new thread posts, when guests may post.
    post_action: Option<String>,
    steps: Vec<Step<PostView>>,
    /// A new thread that couldn't be posted, to try again.
    draft: String,
    flash: Option<&'static str>,
    error: Option<&'static str>,
}

/// A post that couldn't be posted: its text, and what it answered.
struct Draft {
    parent: Option<Uuid>,
    body: String,
}

async fn page(
    state: &AppState,
    guest: &Guest,
    draft: Option<Draft>,
    flash: Option<&'static str>,
) -> sqlx::Result<BoardPage> {
    let (lang, t) = (guest.lang, guest.lang.messages());
    let links = GuestLinks::new(&guest.link, lang);
    let open = guest.board.guests_post();
    let you = guest.rsvp.id;
    let tz = guest.event.details.starts_at.timezone();
    let posts = board::posts(&state.db, guest.event.id, Some(you)).await?;
    let at = |pattern: &str, post: Uuid| {
        paths::localized(
            &paths::path(pattern, &guest.link).replace("{post}", &post.to_string()),
            lang,
        )
    };
    let (mut thread_draft, mut reply_draft) = (String::new(), None);
    match draft {
        Some(Draft { parent: None, body }) => thread_draft = body,
        Some(Draft {
            parent: Some(parent),
            body,
        }) => reply_draft = Some((parent, body)),
        None => {}
    }
    let steps = board::threads(posts, |post| {
        let mine = matches!(post.author, Author::Guest { rsvp_id, .. } if rsvp_id == you);
        let mut view = PostView::new(post, t, lang, tz, false);
        view.you = mine;
        if !mine {
            view.vote_action = Some(at(paths::RSVP_BOARD_VOTE, post.id));
        }
        if mine {
            view.delete_action = Some(at(paths::RSVP_BOARD_DELETE, post.id));
        }
        if open {
            view.reply_action = Some(links.board.clone());
        }
        if let Some((parent, body)) = &reply_draft
            && *parent == post.id
        {
            view.draft = Some(body.clone());
        }
        view
    });
    Ok(BoardPage {
        lang,
        t,
        title: guest.event.details.title(lang).to_owned(),
        tabs: guest.tabs(state, &links, GuestTab::Board),
        post_action: open.then(|| links.board.clone()),
        steps,
        draft: thread_draft,
        flash,
        error: None,
    })
}

/// Back to the guest's reply when the board isn't for guests.
fn no_board(guest: &Guest) -> Response {
    Redirect::to(&GuestLinks::new(&guest.link, guest.lang).page).into_response()
}

pub async fn show(
    State(state): State<AppState>,
    guest: Guest,
    Query(query): Query<ShowQuery>,
) -> Page {
    if !guest.board.for_guests() {
        return Ok(no_board(&guest));
    }
    let flash = Notice::flash(query.notice.as_deref(), guest.lang.messages());
    Ok(render(page(&state, &guest, None, flash).await?))
}

#[derive(Deserialize)]
pub struct PostForm {
    body: String,
    /// The post it answers; empty for a new thread.
    #[serde(default)]
    parent: String,
}

/// Posts a thread or a reply, when the board is open to guests.
pub async fn create(
    State(state): State<AppState>,
    guest: Guest,
    Form(form): Form<PostForm>,
) -> Page {
    if !guest.board.guests_post() {
        return Ok(no_board(&guest));
    }
    let parent = match form.parent.as_str() {
        "" => None,
        id => match id.parse() {
            Ok(id) => Some(id),
            Err(_) => return Ok(StatusCode::BAD_REQUEST.into_response()),
        },
    };
    let Some(body) = board::body(&form.body) else {
        let draft = Draft {
            parent,
            body: form.body,
        };
        return Ok(refuse(&state, &guest, draft, |t| &t.board_error_body).await?);
    };
    let writer = Writer::Guest(guest.rsvp.id);
    let posted = board::create(&state.db, guest.event.id, parent, writer, &body).await?;
    let Some(id) = posted else {
        // What it answered was deleted meanwhile.
        let draft = Draft { parent: None, body };
        return Ok(refuse(&state, &guest, draft, |t| &t.board_error_gone).await?);
    };
    tracing::info!(rsvp = %guest.rsvp.id, post = %id, "posted on the board");
    Ok(Notice::Posted.redirect_to(
        paths::RSVP_BOARD,
        &guest.link,
        guest.lang,
        &format!("post-{id}"),
    ))
}

/// The board again, with the post kept to try again and what was wrong with it.
async fn refuse(
    state: &AppState,
    guest: &Guest,
    draft: Draft,
    pick: fn(&'static Messages) -> &'static str,
) -> sqlx::Result<Response> {
    let mut page = page(state, guest, Some(draft), None).await?;
    page.error = Some(pick(page.t));
    Ok(render_with(StatusCode::UNPROCESSABLE_ENTITY, page))
}

#[derive(Deserialize)]
pub struct PostPath {
    post: Uuid,
}

#[derive(Deserialize)]
pub struct VoteForm {
    vote: String,
}

/// Votes a post up or down; the same again takes it back.
pub async fn vote(
    State(state): State<AppState>,
    guest: Guest,
    Path(PostPath { post }): Path<PostPath>,
    Form(form): Form<VoteForm>,
) -> Page {
    if !guest.board.for_guests() {
        return Ok(no_board(&guest));
    }
    let vote = match form.vote.as_str() {
        "up" => 1,
        "down" => -1,
        _ => return Ok(StatusCode::BAD_REQUEST.into_response()),
    };
    board::vote(&state.db, guest.event.id, post, guest.rsvp.id, vote).await?;
    let board = GuestLinks::new(&guest.link, guest.lang).board;
    Ok(Redirect::to(&format!("{board}#post-{post}")).into_response())
}

/// Deletes one of the guest's own posts.
pub async fn delete(
    State(state): State<AppState>,
    guest: Guest,
    Path(PostPath { post }): Path<PostPath>,
) -> Page {
    if !guest.board.for_guests() {
        return Ok(no_board(&guest));
    }
    let deleted = board::delete(&state.db, guest.event.id, post, Some(guest.rsvp.id)).await?;
    if !deleted {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    tracing::info!(rsvp = %guest.rsvp.id, %post, "deleted a post on the board");
    Ok(Notice::PostDeleted.redirect(paths::RSVP_BOARD, &guest.link, guest.lang))
}

#[cfg(test)]
mod tests;
