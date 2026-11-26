//! An event's Board tab: what its message board is (`db::board::BoardState`), and,
//! while there is one, its posts with every guest's name in full, for hosts to post,
//! reply, vote and delete. A host posts signed with the name they log in with and their
//! role for the event (`crate::board::host_signature`), and votes once a post, as each
//! guest does on their Board tab (`web::board`).

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
    lang,
    page::{AdminState, PageError, load_event, redirect, saved},
    role::Host,
};
use crate::{
    board::{Draft, Drafts, Pager, PostForm, PostView, VoteForm, page_number, refusal},
    db::{
        board::{self, Author, BoardState, Refused, Step, Voter, Writer},
        event::Event,
    },
    http::render_with,
    i18n::{AdminMessages, filled},
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
    pager: Option<Pager>,
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

/// The board's page `number`, from 0, maybe at a post.
fn url(event: Uuid, number: usize, post: Option<Uuid>) -> String {
    let mut url = paths::path(paths::EVENT_BOARD, event);
    if number > 0 {
        url.push_str(&format!("?page={}", number + 1));
    }
    if let Some(post) = post {
        url.push_str(&format!("#post-{post}"));
    }
    url
}

async fn page(
    state: &AdminState,
    id: Uuid,
    number: usize,
    draft: Option<Draft>,
) -> Result<BoardPage, PageError> {
    let event = load_event(state, id).await?;
    let board_state = board::state(&state.db, event.id)
        .await?
        .ok_or(PageError::Missing)?;
    let lang = lang::current();
    let (t, a) = (lang.messages(), lang::text());
    let links = EventLinks::new(event.id);
    let (mut steps, mut kept, mut pager) = (Vec::new(), 0, None);
    let drafts = Drafts::new(draft);
    if board_state.for_hosts() {
        let tz = event.details.starts_at.time_zone();
        let host = Host::current();
        let voter = Voter::Host(&host.user);
        let shown = board::page(&state.db, event.id, Some(voter), number).await?;
        pager = Pager::new(t, &shown, |n| paths::admin(&url(event.id, n, None)));
        let at = |pattern: &str, post: Uuid| {
            paths::admin(&paths::path(pattern, event.id).replace("{post}", &post.to_string()))
        };
        steps = board::threads(shown.posts, |post, answers| {
            let mine = matches!(&post.author, Author::Host { name, .. } if **name == *host.user);
            let mut view = PostView::new(post, answers, t, lang, tz, true);
            view.you = mine;
            if !mine {
                view.vote_action = Some(at(paths::EVENT_BOARD_VOTE, post.id));
            }
            view.reply_action = Some(links.board_posts.clone());
            view.delete_action = Some(at(paths::EVENT_BOARD_DELETE, post.id));
            view.draft = drafts.reply_to(post.id);
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
        pager,
        draft: drafts.thread,
        saved: false,
        error: None,
    })
}

#[derive(Deserialize)]
pub(super) struct BoardQuery {
    saved: Option<String>,
    /// From 1.
    page: Option<usize>,
}

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Query(query): Query<BoardQuery>,
) -> Result<Response, PageError> {
    let mut page = page(&state, id, page_number(query.page), None).await?;
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
    let Ok(parent) = form.parent() else {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let t = lang::current().messages();
    let refuse = |draft: Draft, error: String| {
        let state = &state;
        async move {
            // The page with the thread it answers, or the first.
            let number = match draft.parent {
                Some(parent) => board::page_of(&state.db, id, parent).await?,
                None => 0,
            };
            let mut page = page(state, id, number, Some(draft)).await?;
            page.error = Some(error);
            Ok::<_, PageError>(render_with(StatusCode::UNPROCESSABLE_ENTITY, page))
        }
    };
    let Some(body) = board::body(&form.body) else {
        let draft = Draft {
            parent,
            body: form.body,
        };
        return refuse(draft, filled(&t.board_error_body, "n", board::MAX_BODY)).await;
    };
    let host = Host::current();
    // Signed with their role for this event, which the board's routes ask for.
    let Some(role) = host.role else {
        return Ok(StatusCode::FORBIDDEN.into_response());
    };
    let writer = Writer::Host {
        name: &host.user,
        role: role.name(),
    };
    let post = match board::create(&state.db, event.id, parent, writer, &body).await? {
        Ok(post) => post,
        Err(refused) => {
            let parent = parent.filter(|_| refused != Refused::Gone);
            return refuse(Draft { parent, body }, refusal(t, refused)).await;
        }
    };
    tracing::info!(event = %event.id, %post, "host posted on the board");
    let number = board::page_of(&state.db, event.id, post).await?;
    Ok(redirect(&url(event.id, number, Some(post))))
}

#[derive(Deserialize)]
pub(super) struct PostPath {
    event: Uuid,
    post: Uuid,
}

/// Votes a post up or down as this host, other than their own; the same again takes it
/// back.
pub(super) async fn vote(
    State(state): State<AdminState>,
    Path(PostPath { event, post }): Path<PostPath>,
    Form(form): Form<VoteForm>,
) -> Result<Response, PageError> {
    let event = load_event(&state, event).await?;
    let Some(vote) = form.value() else {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let host = Host::current();
    board::vote(&state.db, event.id, post, Voter::Host(&host.user), vote).await?;
    // A vote can move its thread to another page.
    let number = board::page_of(&state.db, event.id, post).await?;
    Ok(redirect(&url(event.id, number, Some(post))))
}

/// Deletes any post on the board.
pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(PostPath { event, post }): Path<PostPath>,
) -> Result<Response, PageError> {
    let event = load_event(&state, event).await?;
    // Back to where it was.
    let number = board::page_of(&state.db, event.id, post).await?;
    if !board::delete(&state.db, event.id, post, None).await? {
        return Err(PageError::Missing);
    }
    tracing::info!(event = %event.id, %post, "host deleted a post on the board");
    let to = url(event.id, number, None);
    let to = match to.contains('?') {
        true => format!("{to}&saved"),
        false => format!("{to}?saved"),
    };
    Ok(redirect(&to))
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
        let shown = board::page(&db, event, None, 0).await.unwrap().posts;
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
    async fn a_host_signs_with_their_role_and_votes_once(db: PgPool) {
        let editor = Admin::as_user(db.clone(), "hagen", Some(Role::Editor));
        let event = editor.event_id().await;
        board::set_state(&db, event, BoardState::Open)
            .await
            .unwrap();
        let kari = rsvp::create_invite(&db, event, "Kari Nordmann", 1)
            .await
            .unwrap();
        let hers = board::create(&db, event, None, Writer::Guest(kari.id), "Hello")
            .await
            .unwrap()
            .unwrap();
        let posts = paths::path(paths::EVENT_BOARD_POSTS, event);
        editor.post(&posts, "body=Welcome").await;
        let mine = board::posts(&db, event, None)
            .await
            .unwrap()
            .into_iter()
            .find(|post| post.id != hers)
            .unwrap();
        assert_eq!(
            mine.author,
            board::Author::Host {
                name: "hagen".into(),
                role: "editor".into()
            }
        );

        // Hosts vote on others' posts, once each, and not on their own.
        let vote = |post: uuid::Uuid| {
            paths::path(paths::EVENT_BOARD_VOTE, event).replace("{post}", &post.to_string())
        };
        let tab = paths::path(paths::EVENT_BOARD, event);
        let response = editor.post(&vote(hers), "vote=up").await;
        assert!(
            location(&response).starts_with(&tab),
            "{}",
            location(&response)
        );
        editor.post(&vote(hers), "vote=down").await;
        let admin = Admin::new(db.clone());
        admin.post(&vote(hers), "vote=down").await;
        admin.post(&vote(mine.id), "vote=up").await;
        editor.post(&vote(mine.id), "vote=up").await;
        let score =
            |posts: &[board::Post], id| posts.iter().find(|post| post.id == id).unwrap().score;
        let shown = board::posts(&db, event, None).await.unwrap();
        assert_eq!((score(&shown, hers), score(&shown, mine.id)), (-2, 1));
        assert_eq!(
            editor.post(&vote(hers), "vote=sideways").await.status(),
            StatusCode::BAD_REQUEST
        );

        // The page has vote buttons for Kari's post but not for the host's own.
        let html = body(editor.get(&tab).await).await;
        assert!(html.contains(&vote(hers)), "{html}");
        assert!(!html.contains(&vote(mine.id)), "{html}");
        assert!(html.contains("hagen (Editor)"), "{html}");
    }

    #[sqlx::test]
    async fn hosts_see_whom_a_deep_reply_answers_in_full(db: PgPool) {
        let admin = Admin::new(db.clone());
        let event = admin.event_id().await;
        board::set_state(&db, event, BoardState::Open)
            .await
            .unwrap();
        let ola = rsvp::create_invite(&db, event, "Ola Hansen", 1)
            .await
            .unwrap();
        let mut last = None;
        for _ in 0..=board::MAX_DEPTH + 1 {
            let reply = board::create(&db, event, last, Writer::Guest(ola.id), "More")
                .await
                .unwrap()
                .unwrap();
            last = Some(reply);
        }
        let html = body(admin.get(&paths::path(paths::EVENT_BOARD, event)).await).await;
        assert!(html.contains("↪&nbsp;Ola Hansen"), "{html}");
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
