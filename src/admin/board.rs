//! An event's Board tab: what its message board is (`db::board::BoardState`), and,
//! while there is one, its posts with every guest's name in full, for hosts to post,
//! reply and delete. Hosts don't vote; guests do, on their Board tab (`web::board`).

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, load_event, redirect, saved},
    role::Host,
};
use crate::{
    board::PostView,
    db::{
        board::{self, BoardState, Step, Writer},
        event::Event,
    },
    http::render_with,
    i18n::AdminMessages,
    paths::{self, EventLinks},
};

/// One of the board's states, for the form.
struct Choice {
    code: &'static str,
    name: &'static str,
    hint: &'static str,
    checked: bool,
}

#[derive(Template)]
#[template(path = "admin/board.html")]
struct BoardPage {
    event: Event,
    links: EventLinks,
    choices: Vec<Choice>,
    state: BoardState,
    /// While it's off, how many posts it keeps.
    kept: i64,
    steps: Vec<Step<PostView>>,
    /// A new thread that couldn't be posted, to try again.
    draft: String,
    saved: bool,
    error: Option<String>,
}

fn choices(a: &'static AdminMessages, state: BoardState) -> Vec<Choice> {
    BoardState::ALL
        .into_iter()
        .map(|choice| {
            let (name, hint) = match choice {
                BoardState::Off => (&a.board_off, &a.board_off_hint),
                BoardState::Hidden => (&a.board_hidden, &a.board_hidden_hint),
                BoardState::Hosts => (&a.board_hosts, &a.board_hosts_hint),
                BoardState::Open => (&a.board_open, &a.board_open_hint),
            };
            Choice {
                code: choice.code(),
                name,
                hint,
                checked: choice == state,
            }
        })
        .collect()
}

/// A post that couldn't be posted: its text, and what it answered.
struct Draft {
    parent: Option<Uuid>,
    body: String,
}

async fn page(state: &AdminState, id: Uuid, draft: Option<Draft>) -> Result<BoardPage, PageError> {
    let event = load_event(state, id).await?;
    let board_state = board::state(&state.db, event.id)
        .await?
        .ok_or(PageError::Missing)?;
    let lang = lang::current();
    let (t, a) = (lang.messages(), lang::text());
    let links = EventLinks::new(event.id);
    let (mut steps, mut kept) = (Vec::new(), 0);
    let (mut thread_draft, mut reply_draft) = (String::new(), None);
    match draft {
        Some(Draft { parent: None, body }) => thread_draft = body,
        Some(Draft {
            parent: Some(parent),
            body,
        }) => reply_draft = Some((parent, body)),
        None => {}
    }
    if board_state.for_hosts() {
        let tz = event.details.starts_at.timezone();
        let posts = board::posts(&state.db, event.id, None).await?;
        steps = board::threads(posts, |post| {
            let mut view = PostView::new(post, t, lang, tz, true);
            view.reply_action = Some(links.board_posts.clone());
            view.delete_action = Some(paths::admin(
                &paths::path(paths::EVENT_BOARD_DELETE, event.id)
                    .replace("{post}", &post.id.to_string()),
            ));
            if let Some((parent, body)) = &reply_draft
                && *parent == post.id
            {
                view.draft = Some(body.clone());
            }
            view
        });
    } else {
        kept = board::count(&state.db, event.id).await?;
    }
    Ok(BoardPage {
        choices: choices(a, board_state),
        state: board_state,
        event,
        links,
        kept,
        steps,
        draft: thread_draft,
        saved: false,
        error: None,
    })
}

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let mut page = page(&state, id, None).await?;
    page.saved = query.saved.is_some();
    Ok(render_with(StatusCode::OK, page))
}

#[derive(Deserialize)]
pub(super) struct StateForm {
    state: String,
}

/// Sets what the board is.
pub(super) async fn edit(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<StateForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let Some(board_state) = BoardState::parse(&form.state) else {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    board::set_state(&state.db, event.id, board_state).await?;
    tracing::info!(event = %event.id, board = board_state.code(), "board set");
    Ok(saved(paths::EVENT_BOARD, event.id))
}

#[derive(Deserialize)]
pub(super) struct PostForm {
    body: String,
    /// The post it answers; empty for a new thread.
    #[serde(default)]
    parent: String,
}

/// Posts a thread or a reply as a host, while there's a board.
pub(super) async fn create(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Form(form): Form<PostForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, id).await?;
    let open = board::state(&state.db, event.id)
        .await?
        .is_some_and(BoardState::for_hosts);
    if !open {
        return Ok(saved(paths::EVENT_BOARD, event.id));
    }
    let parent = match form.parent.as_str() {
        "" => None,
        id => match id.parse() {
            Ok(id) => Some(id),
            Err(_) => return Ok(StatusCode::BAD_REQUEST.into_response()),
        },
    };
    let refuse = |draft: Draft, pick: fn(&'static crate::i18n::Messages) -> &'static str| {
        let state = &state;
        async move {
            let mut page = page(state, id, Some(draft)).await?;
            page.error = Some(pick(lang::current().messages()).to_owned());
            Ok::<_, PageError>(render_with(StatusCode::UNPROCESSABLE_ENTITY, page))
        }
    };
    let Some(body) = board::body(&form.body) else {
        let draft = Draft {
            parent,
            body: form.body,
        };
        return refuse(draft, |t| &t.board_error_body).await;
    };
    let host = Host::current();
    let posted =
        board::create(&state.db, event.id, parent, Writer::Host(&host.user), &body).await?;
    let Some(post) = posted else {
        return refuse(Draft { parent: None, body }, |t| &t.board_error_gone).await;
    };
    tracing::info!(event = %event.id, %post, "host posted on the board");
    Ok(redirect(&format!(
        "{}#post-{post}",
        paths::path(paths::EVENT_BOARD, event.id)
    )))
}

#[derive(Deserialize)]
pub(super) struct PostPath {
    event: Uuid,
    post: Uuid,
}

/// Deletes any post on the board.
pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(PostPath { event, post }): Path<PostPath>,
) -> Result<Response, PageError> {
    let event = load_event(&state, event).await?;
    if !board::delete(&state.db, event.id, post, None).await? {
        return Err(PageError::Missing);
    }
    tracing::info!(event = %event.id, %post, "host deleted a post on the board");
    Ok(saved(paths::EVENT_BOARD, event.id))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use sqlx::PgPool;

    use crate::{
        admin::{role::Role, testing::Admin},
        db::{
            board::{self, BoardState, Writer},
            rsvp,
        },
        paths,
        testing::{body, location},
    };

    #[sqlx::test]
    async fn hosts_set_the_board_post_and_delete_any_post(db: PgPool) {
        let admin = Admin::new(db.clone());
        let event = admin.event_id().await;
        let tab = paths::path(paths::EVENT_BOARD, event);
        let html = body(admin.get(&tab).await).await;
        assert!(!html.contains(r#"name="body""#), "off: nothing to post on");

        let response = admin.post(&tab, "state=hidden").await;
        assert_eq!(location(&response), format!("{tab}?saved"));
        assert_eq!(
            board::state(&db, event).await.unwrap(),
            Some(BoardState::Hidden)
        );
        assert_eq!(
            admin.post(&tab, "state=loud").await.status(),
            StatusCode::BAD_REQUEST
        );

        // Hosts see guests' names in full, and post signed by theirs.
        let kari = rsvp::create_invite(&db, event, "Kari Nordmann", 1)
            .await
            .unwrap();
        let hers = board::create(&db, event, None, Writer::Guest(kari.id), "Hello")
            .await
            .unwrap()
            .unwrap();
        let posts = paths::path(paths::EVENT_BOARD_POSTS, event);
        let response = admin
            .post(&posts, &format!("body=Welcome&parent={hers}"))
            .await;
        assert!(
            location(&response).starts_with(&tab),
            "{}",
            location(&response)
        );
        let html = body(admin.get(&tab).await).await;
        assert!(html.contains("Kari Nordmann"), "{html}");
        assert!(html.contains("Welcome"), "{html}");

        let delete =
            paths::path(paths::EVENT_BOARD_DELETE, event).replace("{post}", &hers.to_string());
        assert_eq!(
            location(&admin.post(&delete, "").await),
            format!("{tab}?saved")
        );
        let shown = board::posts(&db, event, None).await.unwrap();
        assert_eq!(shown.len(), 2, "hers stays, empty, for the reply");
        assert!(
            shown
                .iter()
                .any(|post| post.id == hers && post.body.is_none())
        );

        // Off keeps the posts.
        admin.post(&tab, "state=off").await;
        assert_eq!(board::count(&db, event).await.unwrap(), 1);
    }

    #[sqlx::test]
    async fn a_viewer_has_no_board_tab(db: PgPool) {
        let admin = Admin::as_role(db, Role::Viewer);
        let event = admin.event_id().await;
        let tab = paths::path(paths::EVENT_BOARD, event);
        let html = body(admin.get(&admin.event().await).await).await;
        assert!(!html.contains(&format!(r#"href="{tab}""#)), "{html}");
        assert_eq!(
            admin.post(&tab, "state=open").await.status(),
            StatusCode::FORBIDDEN
        );
    }
}
