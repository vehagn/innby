//! A post on an event's message board as a page shows it, for the guests' Board tab
//! (`web::board`) and the hosts' (`admin::board`) alike, which `_board_thread.html`
//! draws, and the links between the board's pages (`_board_pager.html`). The posts
//! themselves are in `db::board`.

use jiff::tz::TimeZone;
use uuid::Uuid;

use crate::{
    admin::role::Role,
    db::{
        board::{self, Author, Page, Post, Refused},
        rsvp::NameDisplay,
    },
    i18n::{Lang, Messages, fill, filled},
};

/// The links between a board's pages, when it has more than one.
pub struct Pager {
    pub previous: Option<String>,
    pub next: Option<String>,
    /// "Page 2 of 3".
    pub at: String,
}

impl Pager {
    /// For `page`, with `url` the address of a page by its number from 0.
    pub fn new(t: &Messages, page: &Page, url: impl Fn(usize) -> String) -> Option<Self> {
        (page.count > 1).then(|| Self {
            previous: page.number.checked_sub(1).map(&url),
            next: (page.number + 1 < page.count).then(|| url(page.number + 1)),
            at: fill(
                &t.board_page,
                &[("n", &(page.number + 1)), ("count", &page.count)],
            ),
        })
    }
}

/// Why a post wasn't posted, as a page says it.
pub fn refusal(t: &Messages, refused: Refused) -> String {
    match refused {
        Refused::Gone => t.board_error_gone.clone(),
        Refused::ThreadFull => filled(&t.board_error_thread, "n", board::MAX_THREAD),
        Refused::BoardFull => filled(&t.board_error_full, "n", board::MAX_POSTS),
    }
}

/// The page asked for as `?page=`, from 1, as a number from 0.
pub fn page_number(asked: Option<usize>) -> usize {
    asked.unwrap_or(1).saturating_sub(1)
}

/// How a host signs a post: by the name they log in with (`Role::signed_name`) and their
/// role for the event, as "Hagen (Editor)", in the page's language; with a role Innby
/// doesn't know, as a host.
pub fn host_signature(t: &Messages, name: &str, role: &str, full_name: bool) -> String {
    let name = Role::signed_name(name, full_name);
    let role = Role::from_name(role).map_or(t.board_host.as_str(), |role| t.admin.role_name(role));
    fill(&t.board_signed, &[("name", &name), ("role", &role)])
}

/// A post, with what the one looking may do with it.
pub struct PostView {
    pub id: Uuid,
    /// Who wrote it, as the one looking may see them.
    pub author: String,
    /// Whether a host wrote it, to say so beside their name.
    pub host: bool,
    /// Who wrote the post it answers, signed the same way, for a reply past
    /// `db::board::MAX_DEPTH`, which lines up with that post rather than nesting under it.
    pub answers: Option<String>,
    /// Whether the one looking did.
    pub you: bool,
    pub when: String,
    /// Its text as HTML, from its Markdown (`crate::markdown`); None for a deleted post,
    /// kept for its replies.
    pub body: Option<String>,
    pub score: i64,
    /// The looking guest's or host's vote: 1, -1 or 0.
    pub vote: i16,
    /// Where the vote buttons post, for someone who may vote on it.
    pub vote_action: Option<String>,
    /// Where a reply posts, for someone who may reply.
    pub reply_action: Option<String>,
    pub delete_action: Option<String>,
    /// A reply that couldn't be posted, to try again.
    pub draft: Option<String>,
}

impl PostView {
    /// The post, with no actions yet, signed by `signature`, as is the post it answers
    /// when `db::board::threads` gives it, past the depth replies nest to.
    pub fn new(
        post: &Post,
        answers: Option<&Post>,
        t: &Messages,
        lang: Lang,
        tz: &TimeZone,
        full_names: bool,
    ) -> Self {
        Self {
            id: post.id,
            author: signature(post, t, full_names),
            host: matches!(post.author, Author::Host { .. }),
            answers: answers.map(|answered| signature(answered, t, full_names)),
            you: false,
            when: lang.format_date(&post.created_at.to_zoned_datetime(tz.clone())),
            body: post.body.as_deref().map(crate::markdown::to_html),
            score: post.score,
            vote: post.vote,
            vote_action: None,
            reply_action: None,
            delete_action: None,
            draft: None,
        }
    }
}

/// Who wrote a post, as the one looking may see them: a guest as the guest list shows
/// them, or in full for a host (`full_names`); a host by `host_signature`; "[deleted]"
/// for a post that was. Only a guest on the guest list may post, so a post always has a
/// name: one who leaves the list later keeps their first name and initial on what they
/// wrote.
fn signature(post: &Post, t: &Messages, full_names: bool) -> String {
    match &post.author {
        Author::Guest {
            name,
            response,
            name_display,
            ..
        } => {
            let shown = match full_names {
                true => Some(name.clone()),
                false => NameDisplay::listed(*response, *name_display).apply(name),
            };
            let unlisted = || NameDisplay::Partial.apply(name).unwrap_or_default();
            shown.unwrap_or_else(unlisted)
        }
        Author::Host { name, role } => host_signature(t, name, role, full_names),
        Author::Gone => t.board_deleted.clone(),
    }
}
