//! The event's message board on a guest's Event tab, `/r/{link}`, below their answer,
//! when its hosts have it on for guests (`db::board::BoardState`). Guests read and vote on
//! what hosts post, and when it's open post and reply too, signed as the guest list
//! shows them, in a little Markdown (`crate::markdown`), so many an hour. Nobody posts
//! anonymously: a guest who isn't on the guest list, having hidden their name or said
//! they aren't coming, reads and votes but doesn't post. A guest may delete their own
//! posts; hosts any, in the admin UI. The page itself is `rsvp`'s; this is its board
//! (`Section`), and the forms on it.

use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AppState,
    pages::Page,
    rsvp::{Guest, Notice, RsvpPage},
};
use crate::{
    board::{Pager, PostView, refusal},
    db::{
        board::{self, Author, Refused, Step, Voter, Writer},
        rsvp::{NameDisplay, Response as Answer},
    },
    http::render_with,
    i18n::{Messages, filled},
    paths::{self, GuestLinks},
};

/// The board as the Event tab shows it, below the answer.
#[derive(Template)]
#[template(path = "_guest_board.html")]
pub(super) struct Section {
    t: &'static Messages,
    /// What the guest may do here.
    hint: &'static str,
    /// The guest list, where a guest who hid their name may show it to post.
    show_name: Option<String>,
    /// Where a new thread posts, when the guest may post.
    post_action: Option<String>,
    steps: Vec<Step<PostView>>,
    pager: Option<Pager>,
    /// A new thread that couldn't be posted, to try again.
    draft: String,
    /// What a post or a deletion did, here rather than at the top of the page, which
    /// the guest has scrolled away from.
    pub(super) flash: Option<&'static str>,
    error: Option<String>,
}

/// A post that couldn't be posted: its text, and what it answered.
pub(super) struct Draft {
    parent: Option<Uuid>,
    body: String,
}

/// Whether the guest is on the guest list, as a guest must be to post: their posts show
/// their name.
fn listed(guest: &Guest) -> bool {
    guest.rsvp.listed() != NameDisplay::Hidden
}

/// The Event tab with the board's page `number`, from 0, as `paths::RSVP` with
/// `?page=`, and maybe a notice, scrolled to a post or else to the board.
fn url(guest: &Guest, number: usize, notice: Option<Notice>, post: Option<Uuid>) -> String {
    let mut url = GuestLinks::new(&guest.link, guest.lang).page;
    if number > 0 {
        url.push_str(&format!("&page={}", number + 1));
    }
    if let Some(notice) = notice {
        url.push_str(&format!("&notice={}", notice.code()));
    }
    match post {
        Some(post) => url.push_str(&format!("#post-{post}")),
        None => url.push_str("#board"),
    }
    url
}

/// The board's page `number` for `guest`, if the board is for guests, with a post that
/// couldn't be posted kept to try again.
pub(super) async fn section(
    state: &AppState,
    guest: &Guest,
    number: usize,
    draft: Option<Draft>,
) -> sqlx::Result<Option<Section>> {
    if !guest.board.for_guests() {
        return Ok(None);
    }
    let (lang, t) = (guest.lang, guest.lang.messages());
    let links = GuestLinks::new(&guest.link, lang);
    let open = guest.board.guests_post() && listed(guest);
    let you = guest.rsvp.id;
    let tz = guest.event.details.starts_at.time_zone();
    let shown = board::page(&state.db, guest.event.id, Some(Voter::Guest(you)), number).await?;
    let pager = Pager::new(t, &shown, |n| url(guest, n, None, None));
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
    let steps = board::threads(shown.posts, |post| {
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
    // Open to guests, but not to this one: say why, and where to change it if they can.
    let (hint, show_name) = match (open, guest.board.guests_post(), guest.rsvp.response) {
        (true, ..) => (t.board_hint_open.as_str(), None),
        (false, false, _) => (t.board_hint_hosts.as_str(), None),
        (false, true, Answer::Declined) => (t.board_hint_declined.as_str(), None),
        (false, true, _) => (t.board_hint_unlisted.as_str(), Some(links.guests.clone())),
    };
    Ok(Some(Section {
        t,
        hint,
        show_name,
        post_action: open.then(|| links.board.clone()),
        steps,
        pager,
        draft: thread_draft,
        flash: None,
        error: None,
    }))
}

/// Back to the guest's Event tab when the board isn't for guests.
fn no_board(guest: &Guest) -> Response {
    Redirect::to(&GuestLinks::new(&guest.link, guest.lang).page).into_response()
}

#[derive(Deserialize)]
pub struct PostForm {
    body: String,
    /// The post it answers; empty for a new thread.
    #[serde(default)]
    parent: String,
}

/// Posts a thread or a reply, when the board is open to guests and the guest is on the
/// guest list.
pub async fn create(
    State(state): State<AppState>,
    guest: Guest,
    headers: HeaderMap,
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
    let t = guest.lang.messages();
    if !listed(&guest) {
        let draft = Draft {
            parent,
            body: form.body,
        };
        let error = t.board_error_unlisted.clone();
        return Ok(refuse(
            &state,
            &guest,
            &headers,
            draft,
            StatusCode::FORBIDDEN,
            error,
        )
        .await?);
    }
    let Some(body) = board::body(&form.body) else {
        let draft = Draft {
            parent,
            body: form.body,
        };
        let error = filled(&t.board_error_body, "n", board::MAX_BODY);
        return Ok(refuse(
            &state,
            &guest,
            &headers,
            draft,
            StatusCode::UNPROCESSABLE_ENTITY,
            error,
        )
        .await?);
    };
    // Kept to so many an hour each, so nobody can flood the board; counted by the guest
    // rather than their address, so a household on one network doesn't share it.
    if !state.board_limiter.check_guest(guest.rsvp.id) {
        let draft = Draft { parent, body };
        let error = t.board_error_busy.clone();
        return Ok(refuse(
            &state,
            &guest,
            &headers,
            draft,
            StatusCode::TOO_MANY_REQUESTS,
            error,
        )
        .await?);
    }
    let writer = Writer::Guest(guest.rsvp.id);
    let id = match board::create(&state.db, guest.event.id, parent, writer, &body).await? {
        Ok(id) => id,
        Err(refused) => {
            // What it answered was deleted meanwhile: a thread of its own, then.
            let parent = parent.filter(|_| refused != Refused::Gone);
            let draft = Draft { parent, body };
            let error = refusal(t, refused);
            let status = StatusCode::UNPROCESSABLE_ENTITY;
            return Ok(refuse(&state, &guest, &headers, draft, status, error).await?);
        }
    };
    tracing::info!(rsvp = %guest.rsvp.id, post = %id, "posted on the board");
    let number = board::page_of(&state.db, guest.event.id, id).await?;
    let to = url(&guest, number, Some(Notice::Posted), Some(id));
    Ok(Redirect::to(&to).into_response())
}

/// The Event tab again, with the post kept on its board to try again and why it
/// wasn't posted.
async fn refuse(
    state: &AppState,
    guest: &Guest,
    headers: &HeaderMap,
    draft: Draft,
    status: StatusCode,
    error: String,
) -> sqlx::Result<Response> {
    // The page with the thread it answers, or the first, for a thread of its own.
    let number = match draft.parent {
        Some(parent) => board::page_of(&state.db, guest.event.id, parent).await?,
        None => 0,
    };
    let mut page = RsvpPage::new(state, guest, headers);
    page.board = section(state, guest, number, Some(draft)).await?;
    if let Some(board) = &mut page.board {
        board.error = Some(error);
    }
    Ok(render_with(status, page))
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
    let voter = Voter::Guest(guest.rsvp.id);
    board::vote(&state.db, guest.event.id, post, voter, vote).await?;
    // A vote can move its thread to another page.
    let number = board::page_of(&state.db, guest.event.id, post).await?;
    Ok(Redirect::to(&url(&guest, number, None, Some(post))).into_response())
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
    // Back to where it was.
    let number = board::page_of(&state.db, guest.event.id, post).await?;
    let deleted = board::delete(&state.db, guest.event.id, post, Some(guest.rsvp.id)).await?;
    if !deleted {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    tracing::info!(rsvp = %guest.rsvp.id, %post, "deleted a post on the board");
    let to = url(&guest, number, Some(Notice::PostDeleted), None);
    Ok(Redirect::to(&to).into_response())
}

#[cfg(test)]
mod tests;
