//! An event's message board: whether it has one, and who may post (`BoardState`), its
//! posts and replies, and guests' votes on them. A guest writes as their RSVP and a host
//! as the name they log in with. Posts go with the event; a guest's lose their text with
//! the guest, and a deleted post with replies stays, empty, so the replies keep their
//! place (`delete`, and the `board_post_forget` trigger).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::rsvp::{NameDisplay, Response};

/// The longest a post may be, in characters.
pub const MAX_BODY: usize = 2000;

/// How deep replies nest on the page; deeper ones line up under the last that does.
pub const MAX_DEPTH: usize = 6;

/// The most posts an event's board holds, deleted ones kept for their replies too, and
/// the most a thread holds. With `MAX_BODY`, they bound what a page of the board can be,
/// in memory and on the wire: a page is whole threads, so one full thread at most past
/// `PAGE_POSTS`.
pub const MAX_POSTS: i64 = 500;
pub const MAX_THREAD: i64 = 100;

/// A page of the board holds whole threads until it has this many posts.
pub const PAGE_POSTS: usize = 50;

/// What an event's board is, as its hosts set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, sqlx::Type)]
#[sqlx(type_name = "board_state", rename_all = "lowercase")]
pub enum BoardState {
    /// No board, for anyone; its posts are kept.
    #[default]
    Off,
    /// For hosts alone, as before it opens.
    Hidden,
    /// Hosts post; guests read and vote.
    Hosts,
    /// Guests post too.
    Open,
}

impl BoardState {
    pub const ALL: [Self; 4] = [Self::Off, Self::Hidden, Self::Hosts, Self::Open];

    pub fn code(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Hidden => "hidden",
            Self::Hosts => "hosts",
            Self::Open => "open",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.code() == code)
    }

    /// Whether guests see the board, and vote on it.
    pub fn for_guests(self) -> bool {
        matches!(self, Self::Hosts | Self::Open)
    }

    /// Whether guests may post.
    pub fn guests_post(self) -> bool {
        self == Self::Open
    }

    /// Whether there's a board at all, for hosts.
    pub fn for_hosts(self) -> bool {
        self != Self::Off
    }
}

/// Who wrote a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    /// A guest, with what the guest list needs to show them as they chose.
    Guest {
        rsvp_id: Uuid,
        name: String,
        response: Response,
        name_display: Option<NameDisplay>,
    },
    /// A host, by the name they log in with.
    Host(String),
    /// Nobody now: the post was deleted, or its guest was.
    Gone,
}

/// Who is writing a post.
#[derive(Debug, Clone, Copy)]
pub enum Writer<'a> {
    Guest(Uuid),
    Host(&'a str),
}

/// Why a post wasn't posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The post it answers was deleted, or isn't the event's.
    Gone,
    /// Its thread has `MAX_THREAD` posts.
    ThreadFull,
    /// The board has `MAX_POSTS`.
    BoardFull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    /// The first post of its thread: its own id for that one.
    pub thread_id: Uuid,
    pub author: Author,
    /// Whether it was deleted, and is kept only for its replies.
    pub deleted: bool,
    /// Its text, once `page` has loaded it; None for a deleted post.
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Its votes added up.
    pub score: i64,
    /// The vote of the guest looking, if any: 1, -1 or 0.
    pub vote: i16,
}

/// A guest's post, for their export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Written {
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// A guest's vote, for their export: up (1) or down (-1), and the post's text while it
/// has any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Voted {
    pub vote: i16,
    pub on: Option<String>,
}

/// The event's board, or None for no such event.
pub async fn state(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Option<BoardState>> {
    sqlx::query_scalar!(
        r#"select board as "board: BoardState" from event where id = $1"#,
        event_id
    )
    .fetch_optional(db)
    .await
}

/// Sets what the event's board is. Returns whether there's such an event.
pub async fn set_state(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    state: BoardState,
) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "update event set board = $2 where id = $1",
        event_id,
        state as BoardState,
    )
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Every post on the event's board, deleted ones too, with `voter`'s votes, but not
/// their text, which `page` loads for the posts a page shows.
pub async fn posts(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    voter: Option<Uuid>,
) -> sqlx::Result<Vec<Post>> {
    let rows = sqlx::query!(
        r#"
        select p.id, p.parent_id, p.thread_id, p.rsvp_id, p.host_name,
            p.body is null as "deleted!", p.created_at,
            r.name as "name?", r.response as "response?: Response",
            r.name_display as "name_display: NameDisplay",
            coalesce((select sum(v.vote) from board_vote v where v.post_id = p.id), 0)
                as "score!",
            coalesce((select v.vote from board_vote v
                      where v.post_id = p.id and v.rsvp_id = $2), 0)::smallint as "vote!"
        from board_post p left join rsvp r on r.id = p.rsvp_id
        where p.event_id = $1
        order by p.id
        "#,
        event_id,
        voter,
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Post {
            id: row.id,
            parent_id: row.parent_id,
            thread_id: row.thread_id.unwrap_or(row.id),
            deleted: row.deleted,
            author: match (row.rsvp_id, row.name, row.response, row.host_name) {
                (Some(rsvp_id), Some(name), Some(response), _) => Author::Guest {
                    rsvp_id,
                    name,
                    response,
                    name_display: row.name_display,
                },
                (_, _, _, Some(host)) => Author::Host(host),
                _ => Author::Gone,
            },
            body: None,
            created_at: row.created_at,
            score: row.score,
            vote: row.vote,
        })
        .collect())
}

/// One page of the board: its posts, with their text, and where it is among the pages.
pub struct Page {
    pub posts: Vec<Post>,
    /// From 0.
    pub number: usize,
    /// At least 1.
    pub count: usize,
}

/// Page `number` of the event's board, or its last if there aren't so many: threads in
/// the order `threads` shows them, whole, until a page has `PAGE_POSTS` posts. Only that
/// page's posts' text is loaded.
pub async fn page(
    db: &PgPool,
    event_id: Uuid,
    voter: Option<Uuid>,
    number: usize,
) -> sqlx::Result<Page> {
    let all = posts(db, event_id, voter).await?;
    let mut pages = paginate(&all);
    let count = pages.len();
    let number = number.min(count - 1);
    let shown = std::mem::take(&mut pages[number]);
    let mut posts: Vec<Post> = all
        .into_iter()
        .filter(|post| shown.contains(&post.thread_id))
        .collect();
    let ids: Vec<Uuid> = posts.iter().map(|post| post.id).collect();
    let mut bodies: HashMap<Uuid, String> = sqlx::query!(
        r#"select id, body as "body!" from board_post where id = any($1) and body is not null"#,
        &ids
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|row| (row.id, row.body))
    .collect();
    for post in &mut posts {
        post.body = bodies.remove(&post.id);
    }
    Ok(Page {
        posts,
        number,
        count,
    })
}

/// How many posts the event's board has, deleted ones left out.
pub async fn count(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"select count(*) as "n!" from board_post where event_id = $1 and body is not null"#,
        event_id
    )
    .fetch_one(db)
    .await
}

/// A post's text as the form sends it: trimmed, lines as `\n`, at most `MAX_BODY`
/// characters, and no control characters but lines and tabs, nor the ones that turn
/// text around, with which a post could show a link's address back to front. None if
/// it's empty or won't do.
pub fn body(sent: &str) -> Option<String> {
    let body = sent.replace("\r\n", "\n").replace('\r', "\n");
    let body = body.trim();
    let turns = |c: char| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let fine = !body.is_empty()
        && body.chars().count() <= MAX_BODY
        && !body
            .chars()
            .any(|c| (c.is_control() && c != '\n' && c != '\t') || turns(c));
    fine.then(|| body.to_owned())
}

/// The page that shows the thread of post `post_id`: the first if there's no such post.
pub async fn page_of(db: &PgPool, event_id: Uuid, post_id: Uuid) -> sqlx::Result<usize> {
    let all = posts(db, event_id, None).await?;
    let Some(thread) = all
        .iter()
        .find(|post| post.id == post_id)
        .map(|post| post.thread_id)
    else {
        return Ok(0);
    };
    let pages = paginate(&all);
    Ok(pages
        .iter()
        .position(|page| page.contains(&thread))
        .unwrap_or(0))
}

/// The threads on each page, by their first post: in the order `threads` shows them,
/// whole, until a page has `PAGE_POSTS` posts. One page at least, if empty.
fn paginate(all: &[Post]) -> Vec<Vec<Uuid>> {
    let mut size: HashMap<Uuid, usize> = HashMap::new();
    let mut live: HashMap<Uuid, bool> = HashMap::new();
    for post in all {
        *size.entry(post.thread_id).or_default() += 1;
        *live.entry(post.thread_id).or_default() |= !post.deleted;
    }
    let mut firsts: Vec<&Post> = all
        .iter()
        .filter(|post| post.parent_id.is_none() && live[&post.id])
        .collect();
    firsts.sort_by(|a, b| b.score.cmp(&a.score).then(b.created_at.cmp(&a.created_at)));
    let mut pages: Vec<Vec<Uuid>> = vec![Vec::new()];
    let mut filled = 0;
    for first in firsts {
        if filled >= PAGE_POSTS {
            pages.push(Vec::new());
            filled = 0;
        }
        pages.last_mut().expect("a page").push(first.id);
        filled += size[&first.id];
    }
    pages
}

/// Posts on the event's board, as a reply to `parent` if given, which must be a post of
/// the same event's that isn't deleted, while the board and the thread have room.
/// Returns the new post's id. Whether the writer may post is the caller's to check.
///
/// The board's row is locked meanwhile, so posts sent at once can't pass the limits
/// together.
pub async fn create(
    db: &PgPool,
    event_id: Uuid,
    parent: Option<Uuid>,
    writer: Writer<'_>,
    body: &str,
) -> sqlx::Result<Result<Uuid, Refused>> {
    let (rsvp_id, host_name) = match writer {
        Writer::Guest(id) => (Some(id), None),
        Writer::Host(name) => (None, Some(name)),
    };
    let mut tx = db.begin().await?;
    sqlx::query!("select from event where id = $1 for update", event_id)
        .execute(&mut *tx)
        .await?;
    let posts = sqlx::query_scalar!(
        r#"select count(*) as "n!" from board_post where event_id = $1"#,
        event_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if posts >= MAX_POSTS {
        return Ok(Err(Refused::BoardFull));
    }
    let thread = match parent {
        None => None,
        Some(parent) => {
            let thread = sqlx::query_scalar!(
                r#"select coalesce(thread_id, id) as "thread!" from board_post
                   where id = $1 and event_id = $2 and body is not null"#,
                parent,
                event_id
            )
            .fetch_optional(&mut *tx)
            .await?;
            let Some(thread) = thread else {
                return Ok(Err(Refused::Gone));
            };
            let in_thread = sqlx::query_scalar!(
                r#"select count(*) as "n!" from board_post where thread_id = $1 or id = $1"#,
                thread
            )
            .fetch_one(&mut *tx)
            .await?;
            if in_thread >= MAX_THREAD {
                return Ok(Err(Refused::ThreadFull));
            }
            Some(thread)
        }
    };
    let id = sqlx::query_scalar!(
        r#"
        insert into board_post (event_id, parent_id, thread_id, rsvp_id, host_name, body)
        values ($1, $2, $3, $4, $5, $6)
        returning id
        "#,
        event_id,
        parent,
        thread,
        rsvp_id,
        host_name,
        body,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

/// A guest's vote on a post of their event's, other than their own: up (1) or down
/// (-1). The same vote again takes it back.
pub async fn vote(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    post_id: Uuid,
    rsvp_id: Uuid,
    vote: i16,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        with target as (
            select id from board_post
            where id = $2 and event_id = $1 and body is not null
                and rsvp_id is distinct from $3
        ), taken_back as (
            delete from board_vote
            where post_id in (select id from target) and rsvp_id = $3 and vote = $4
            returning 1
        )
        insert into board_vote (post_id, rsvp_id, vote)
        select id, $3, $4 from target where not exists (select from taken_back)
        on conflict (post_id, rsvp_id) do update set vote = excluded.vote
        "#,
        event_id,
        post_id,
        rsvp_id,
        vote,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes a post of the event's: any, or with `only_by`, only that guest's own. One with
/// replies stays, empty; one without goes, with any deleted posts above it that it was
/// all that was left under. Returns whether there was such a post.
pub async fn delete(
    db: &PgPool,
    event_id: Uuid,
    post_id: Uuid,
    only_by: Option<Uuid>,
) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let found = sqlx::query!(
        r#"
        select parent_id,
            exists (select from board_post c where c.parent_id = p.id) as "replied!"
        from board_post p
        where id = $1 and event_id = $2 and body is not null
            and ($3::uuid is null or rsvp_id = $3)
        for update
        "#,
        post_id,
        event_id,
        only_by,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(found) = found else {
        return Ok(false);
    };
    if found.replied {
        sqlx::query!(
            "update board_post set body = null, rsvp_id = null, host_name = null where id = $1",
            post_id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("delete from board_vote where post_id = $1", post_id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query!("delete from board_post where id = $1", post_id)
            .execute(&mut *tx)
            .await?;
        // Deleted posts kept only for this one go too.
        let mut parent = found.parent_id;
        while let Some(id) = parent {
            parent = sqlx::query_scalar!(
                r#"
                delete from board_post p
                where id = $1 and body is null
                    and not exists (select from board_post c where c.parent_id = p.id)
                returning parent_id
                "#,
                id
            )
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
        }
    }
    tx.commit().await?;
    Ok(true)
}

/// What a guest wrote, for their export.
pub async fn written_by(db: impl PgExecutor<'_>, rsvp_id: Uuid) -> sqlx::Result<Vec<Written>> {
    sqlx::query_as!(
        Written,
        r#"select body as "body!", created_at from board_post
           where rsvp_id = $1 and body is not null order by id"#,
        rsvp_id
    )
    .fetch_all(db)
    .await
}

/// How a guest voted, for their export.
pub async fn votes_by(db: impl PgExecutor<'_>, rsvp_id: Uuid) -> sqlx::Result<Vec<Voted>> {
    sqlx::query_as!(
        Voted,
        r#"select v.vote, p.body as on from board_vote v join board_post p on p.id = v.post_id
           where v.rsvp_id = $1 order by p.id"#,
        rsvp_id
    )
    .fetch_all(db)
    .await
}

/// One step through the board as a page shows it, a list in a list: a post and the list
/// of its replies opens, then closes once they're done.
#[derive(Debug)]
pub enum Step<T> {
    /// A post, shown as `T`. A collapsed one, with a score below zero, is folded away
    /// with its replies.
    Open {
        post: T,
        collapsed: bool,
    },
    Close {
        collapsed: bool,
    },
}

/// The board's posts in the order a page shows them, each shown with `show`: threads
/// with the best score first, then the newest; replies the same, then the oldest, as a
/// conversation reads. A deleted post shows only while it has replies left, and replies
/// below `MAX_DEPTH` line up under the last one that nests.
pub fn threads<T>(posts: Vec<Post>, mut show: impl FnMut(&Post) -> T) -> Vec<Step<T>> {
    let mut replies: HashMap<Option<Uuid>, Vec<Post>> = HashMap::new();
    for post in posts {
        replies.entry(post.parent_id).or_default().push(post);
    }
    // Whether a post, or any reply under it, still has something to show.
    fn live(post: &Post, replies: &HashMap<Option<Uuid>, Vec<Post>>) -> bool {
        let mut stack = vec![post];
        while let Some(post) = stack.pop() {
            if !post.deleted {
                return true;
            }
            stack.extend(replies.get(&Some(post.id)).into_iter().flatten());
        }
        false
    }
    let ordered = |parent: Option<Uuid>| -> Vec<&Post> {
        let mut list: Vec<&Post> = replies
            .get(&parent)
            .into_iter()
            .flatten()
            .filter(|post| live(post, &replies))
            .collect();
        list.sort_by(|a, b| {
            b.score.cmp(&a.score).then(match parent {
                None => b.created_at.cmp(&a.created_at),
                Some(_) => a.created_at.cmp(&b.created_at),
            })
        });
        list
    };
    let mut steps = Vec::new();
    // Each level's posts still to show, deepest last, without recursing.
    let mut levels: Vec<(std::vec::IntoIter<&Post>, bool)> =
        vec![(ordered(None).into_iter(), false)];
    while let Some((next, _)) = levels.last_mut() {
        let Some(post) = next.next() else {
            let (_, collapsed) = levels.pop().expect("a level");
            if !levels.is_empty() {
                steps.push(Step::Close { collapsed });
            }
            continue;
        };
        let collapsed = !post.deleted && post.score < 0;
        steps.push(Step::Open {
            post: show(post),
            collapsed,
        });
        if levels.len() < MAX_DEPTH {
            levels.push((ordered(Some(post.id)).into_iter(), collapsed));
        } else {
            steps.push(Step::Close { collapsed });
            // Too deep to nest: the rest of its replies, in order, beside it.
            let mut flat = Vec::new();
            let mut stack: Vec<&Post> = ordered(Some(post.id)).into_iter().rev().collect();
            while let Some(reply) = stack.pop() {
                flat.push(reply);
                stack.extend(ordered(Some(reply.id)).into_iter().rev());
            }
            for reply in flat {
                let collapsed = !reply.deleted && reply.score < 0;
                steps.push(Step::Open {
                    post: show(reply),
                    collapsed,
                });
                steps.push(Step::Close { collapsed });
            }
        }
    }
    steps
}

#[cfg(test)]
mod tests;
