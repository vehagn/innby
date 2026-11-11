//! A post on an event's message board as a page shows it, for the guests' Board tab
//! (`web::board`) and the hosts' (`admin::board`) alike, which `_board_thread.html`
//! draws. The posts themselves are in `db::board`.

use chrono_tz::Tz;
use uuid::Uuid;

use crate::{
    db::{
        board::{Author, Post},
        rsvp::NameDisplay,
    },
    i18n::{Lang, Messages},
};

/// A post, with what the one looking may do with it.
pub struct PostView {
    pub id: Uuid,
    /// Who wrote it, as the one looking may see them.
    pub author: String,
    /// Whether a host wrote it, to say so beside their name; guests see hosts only as
    /// hosts, which says it already.
    pub host: bool,
    /// Whether the one looking did.
    pub you: bool,
    pub when: String,
    /// None for a deleted post, kept for its replies.
    pub body: Option<String>,
    pub score: i64,
    /// The looking guest's vote: 1, -1 or 0.
    pub vote: i16,
    /// Where the vote buttons post, for a guest who may vote on it.
    pub vote_action: Option<String>,
    /// Where a reply posts, for someone who may reply.
    pub reply_action: Option<String>,
    pub delete_action: Option<String>,
    /// A reply that couldn't be posted, to try again.
    pub draft: Option<String>,
}

impl PostView {
    /// The post, with no actions yet: its guest as the guest list shows them, or in full
    /// for a host (`full_names`); a host as a host, or by name for hosts.
    pub fn new(post: &Post, t: &Messages, lang: Lang, tz: Tz, full_names: bool) -> Self {
        let (author, host) = match &post.author {
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
                (shown.unwrap_or_else(|| t.a_guest.clone()), false)
            }
            Author::Host(name) if full_names => (name.clone(), true),
            // "The hosts" says it already.
            Author::Host(_) => (t.board_a_host.clone(), false),
            Author::Gone => (String::new(), false),
        };
        Self {
            id: post.id,
            author,
            host,
            you: false,
            when: lang.format_date(post.created_at.with_timezone(&tz)),
            body: post.body.clone(),
            score: post.score,
            vote: post.vote,
            vote_action: None,
            reply_action: None,
            delete_action: None,
            draft: None,
        }
    }
}
