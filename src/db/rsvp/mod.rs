//! Guests' answers: invitations the host makes and sign-ups through group links, one
//! row each in `rsvp`.
//!
//! - `mod.rs`: the `Rsvp` row and the queries that make, find, change and delete one.
//! - `names`: cleaning up names and e-mail addresses, and how a name shows on the guest
//!   list (`NameDisplay`).
//! - `guest_list`: the guest list and the headcount of an event.

mod guest_list;
mod names;

pub use guest_list::{Headcount, guest_list, headcount};
pub use names::{NameDisplay, NameKind, normalize_email, normalize_name};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgExecutor;
use uuid::Uuid;

use super::group::Group;
use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "rsvp_response", rename_all = "lowercase")]
pub enum Response {
    Pending,
    Attending,
    Declined,
}

impl Response {
    pub const ALL: [Self; 3] = [Self::Pending, Self::Attending, Self::Declined];

    /// Its name in the database, in the admin's `?show=`, and as the CSS class for its
    /// colour.
    pub fn code(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Attending => "attending",
            Self::Declined => "declined",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "email_status", rename_all = "lowercase")]
pub enum EmailStatus {
    None,
    Unconfirmed,
    Confirmed,
}

/// One invitation, or one sign-up through a group link: a guest or a household, and
/// their answer.
#[derive(Debug, Clone, Serialize)]
pub struct Rsvp {
    pub id: Uuid,
    pub event_id: Uuid,
    pub name: String,
    pub response: Response,
    pub party_size: i32,
    pub max_party_size: i32,
    pub link_nonce: i32,
    pub lang: Lang,
    pub email: Option<String>,
    pub email_status: EmailStatus,
    pub email_consent_at: Option<DateTime<Utc>>,
    pub privacy_version: Option<String>,
    pub responded_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// The group whose link the guest signed up through.
    pub group_id: Option<Uuid>,
    /// The guest's choice for the guest list, if they've made one.
    pub name_display: Option<NameDisplay>,
}

impl Rsvp {
    /// Whether the host invited them, rather than their signing up through a group link.
    pub fn is_invitation(&self) -> bool {
        self.group_id.is_none()
    }

    /// How the guest's name shows in the guest list right now.
    pub fn listed(&self) -> NameDisplay {
        NameDisplay::listed(self.response, self.name_display)
    }
}

/// `query_as!` for whole `Rsvp` rows. The query macros only take string literals (or
/// literals joined by `+`), so this splices in the column list, with type overrides for
/// the enum columns:
///
/// - `query_rsvp!(select "from rsvp where …", args…)`
/// - `query_rsvp!("update rsvp set … where …" returning, args…)`
macro_rules! query_rsvp {
    (select $tail:tt $(, $arg:expr)* $(,)?) => {
        query_rsvp!(@ ["select"] [$tail] $(, $arg)*)
    };
    ($head:tt returning $(, $arg:expr)* $(,)?) => {
        query_rsvp!(@ [$head " returning"] [] $(, $arg)*)
    };
    (@ [$($head:tt)*] [$($tail:tt)*] $(, $arg:expr)*) => {
        sqlx::query_as!(
            Rsvp,
            $($head +)*
            r#" id, event_id, name, response as "response: Response",
                party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
                email_status as "email_status: EmailStatus", email_consent_at,
                privacy_version, responded_at, created_at, group_id,
                name_display as "name_display: NameDisplay" "#
            $(+ $tail)*
            $(, $arg)*
        )
    };
}

/// Creates an invitation to the event that the guest answers later through their link.
pub async fn create_invite(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    name: &str,
    max_party_size: i32,
) -> sqlx::Result<Rsvp> {
    query_rsvp!(
        "insert into rsvp (event_id, name, max_party_size) values ($1, $2, $3)" returning,
        event_id,
        name,
        max_party_size,
    )
    .fetch_one(db)
    .await
}

/// What someone gives when they sign up through a group's link.
pub struct SignUp<'a> {
    pub group: &'a Group,
    pub name: &'a str,
    pub party_size: i32,
    pub name_display: NameDisplay,
    pub lang: Lang,
}

/// Creates an RSVP through a group link, which is "attending" from the start.
pub async fn create_open(
    db: impl PgExecutor<'_>,
    sign_up: &SignUp<'_>,
    privacy_version: &str,
) -> sqlx::Result<Rsvp> {
    query_rsvp!(
        "insert into rsvp (event_id, group_id, name, response, party_size, \
            max_party_size, name_display, lang, privacy_version, responded_at) \
         values ($8, $1, $2, 'attending', $3, $4, $5, $6, $7, now())" returning,
        sign_up.group.id,
        sign_up.name,
        sign_up.party_size,
        sign_up.group.max_party_size,
        sign_up.name_display as NameDisplay,
        sign_up.lang as Lang,
        privacy_version,
        sign_up.group.event_id,
    )
    .fetch_one(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Rsvp>> {
    query_rsvp!(select "from rsvp where id = $1", id)
        .fetch_optional(db)
        .await
}

/// The RSVP to this event that has confirmed the address. Others may hold it
/// unconfirmed.
pub async fn find_by_confirmed_email(
    db: impl PgExecutor<'_>,
    event_id: Uuid,
    email: &str,
) -> sqlx::Result<Option<Rsvp>> {
    query_rsvp!(
        select "from rsvp where event_id = $1 and email = $2 and email_status = 'confirmed'",
        event_id,
        email,
    )
    .fetch_optional(db)
    .await
}

/// Every RSVP that has confirmed the address, one per event at most.
pub async fn all_with_confirmed_email(
    db: impl PgExecutor<'_>,
    email: &str,
) -> sqlx::Result<Vec<Rsvp>> {
    query_rsvp!(select "from rsvp where email = $1 and email_status = 'confirmed' order by id", email)
        .fetch_all(db)
        .await
}

/// The event's RSVPs, oldest first (the ids are UUIDv7, so they sort by creation time).
pub async fn list(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Vec<Rsvp>> {
    query_rsvp!(select "from rsvp where event_id = $1 order by id", event_id)
        .fetch_all(db)
        .await
}

/// Records the guest's answer, and their choice for the guest list if they made one. A
/// party size outside `1..=max_party_size` violates a check constraint, so callers
/// validate it first.
pub async fn respond(
    db: impl PgExecutor<'_>,
    id: Uuid,
    response: Response,
    party_size: i32,
    name_display: Option<NameDisplay>,
    lang: Lang,
    privacy_version: &str,
) -> sqlx::Result<Option<Rsvp>> {
    query_rsvp!(
        "update rsvp \
         set response = $2, party_size = $3, name_display = coalesce($4, name_display), \
            lang = $5, privacy_version = $6, responded_at = now() \
         where id = $1" returning,
        id,
        response as Response,
        party_size,
        name_display as Option<NameDisplay>,
        lang as Lang,
        privacy_version,
    )
    .fetch_optional(db)
    .await
}

pub async fn set_name_display(
    db: impl PgExecutor<'_>,
    id: Uuid,
    name_display: NameDisplay,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "update rsvp set name_display = $2 where id = $1",
        id,
        name_display as NameDisplay,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub const MAX_PARTY_SIZE: i32 = 20;

pub async fn rename(db: impl PgExecutor<'_>, id: Uuid, name: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!("update rsvp set name = $2 where id = $1", id, name)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Stores a new, unconfirmed address with the guest's consent.
pub async fn set_email(
    db: impl PgExecutor<'_>,
    id: Uuid,
    email: &str,
    lang: Lang,
    privacy_version: &str,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        update rsvp
        set email = $2, email_status = 'unconfirmed', email_consent_at = now(), lang = $3,
            privacy_version = $4
        where id = $1
        "#,
        id,
        email,
        lang as Lang,
        privacy_version,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Confirms the address, as long as it's still the one the confirmation was sent to and
/// no other RSVP to the same event has confirmed it first.
pub async fn confirm_email(db: impl PgExecutor<'_>, id: Uuid, email: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        update rsvp set email_status = 'confirmed'
        where id = $1 and email = $2 and email_status = 'unconfirmed'
            and not exists (
                select from rsvp other
                where other.event_id = rsvp.event_id and other.email = $2
                    and other.email_status = 'confirmed'
            )
        "#,
        id,
        email,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Forgets the address and the consent that came with it. The RSVP itself stays.
pub async fn remove_email(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        update rsvp set email = null, email_status = 'none', email_consent_at = null
        where id = $1 and email is not null
        "#,
        id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Changes an invitation's label and size. A party already larger than the new maximum
/// shrinks to fit.
pub async fn update_invite(
    db: impl PgExecutor<'_>,
    id: Uuid,
    name: &str,
    max_party_size: i32,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "update rsvp set name = $2, max_party_size = $3, party_size = least(party_size, $3) \
         where id = $1",
        id,
        name,
        max_party_size,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Invalidates the guest's link by bumping its nonce. Returns the new nonce.
pub async fn revoke_link(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar!(
        "update rsvp set link_nonce = link_nonce + 1 where id = $1 returning link_nonce",
        id,
    )
    .fetch_optional(db)
    .await
}

/// Deletes the RSVP and, through `on delete cascade`, its pending mail.
pub async fn delete(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!("delete from rsvp where id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
pub(crate) mod tests;
