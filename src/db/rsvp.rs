use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgExecutor;
use uuid::Uuid;

use super::group::Group;
use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "rsvp_source", rename_all = "lowercase")]
pub enum Source {
    Invite,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "rsvp_response", rename_all = "lowercase")]
pub enum Response {
    Pending,
    Attending,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "email_status", rename_all = "lowercase")]
pub enum EmailStatus {
    None,
    Unconfirmed,
    Confirmed,
}

/// How a guest's name shows in the guest list other guests see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "name_display", rename_all = "lowercase")]
pub enum NameDisplay {
    Full,
    /// First name and an initial: "Kari N.".
    Partial,
    Hidden,
}

impl NameDisplay {
    pub const ALL: [Self; 3] = [Self::Full, Self::Partial, Self::Hidden];

    /// The form value.
    pub fn code(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Hidden => "hidden",
        }
    }

    /// How a name shows in the list. Until the guest chooses, it's their first name and
    /// an initial, or their full name once they're coming. Declined guests aren't listed.
    pub fn listed(response: Response, chosen: Option<Self>) -> Self {
        match (response, chosen) {
            (Response::Declined, _) => Self::Hidden,
            (_, Some(chosen)) => chosen,
            (Response::Attending, None) => Self::Full,
            (Response::Pending, None) => Self::Partial,
        }
    }

    /// The name as this choice shows it, or `None` for hidden.
    pub fn apply(self, name: &str) -> Option<String> {
        match self {
            Self::Full => Some(name.to_owned()),
            Self::Partial => Some(partial_name(name)),
            Self::Hidden => None,
        }
    }
}

/// The first word and the initial of the last: "Kari Nordmann" becomes "Kari N.".
fn partial_name(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().unwrap_or_default();
    match words.next_back().and_then(|last| last.chars().next()) {
        Some(initial) => format!("{first} {}.", initial.to_uppercase()),
        None => first.to_owned(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Rsvp {
    pub id: Uuid,
    pub source: Source,
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
    /// How the guest's name shows in the guest list right now.
    pub fn listed(&self) -> NameDisplay {
        NameDisplay::listed(self.response, self.name_display)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Headcount {
    /// Invitations and sign-ups that answered "attending".
    pub attending_parties: i64,
    /// The sum of their party sizes: the number of people coming.
    pub attending_people: i64,
    pub declined: i64,
    pub pending: i64,
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
            r#" id, source as "source: Source", name, response as "response: Response",
                party_size, max_party_size, link_nonce, lang as "lang: Lang", email,
                email_status as "email_status: EmailStatus", email_consent_at,
                privacy_version, responded_at, created_at, group_id,
                name_display as "name_display: NameDisplay" "#
            $(+ $tail)*
            $(, $arg)*
        )
    };
}

/// Creates an invitation that the guest answers later through their link.
pub async fn create_invite(
    db: impl PgExecutor<'_>,
    name: &str,
    max_party_size: i32,
) -> sqlx::Result<Rsvp> {
    query_rsvp!(
        "insert into rsvp (source, name, max_party_size) values ('invite', $1, $2)" returning,
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
        "insert into rsvp (source, group_id, name, response, party_size, max_party_size, \
            name_display, lang, privacy_version, responded_at) \
         values ('open', $1, $2, 'attending', $3, $4, $5, $6, $7, now())" returning,
        sign_up.group.id,
        sign_up.name,
        sign_up.party_size,
        sign_up.group.max_party_size,
        sign_up.name_display as NameDisplay,
        sign_up.lang as Lang,
        privacy_version,
    )
    .fetch_one(db)
    .await
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Rsvp>> {
    query_rsvp!(select "from rsvp where id = $1", id)
        .fetch_optional(db)
        .await
}

/// The RSVP that has confirmed this address. Others may hold it unconfirmed.
pub async fn find_by_confirmed_email(
    db: impl PgExecutor<'_>,
    email: &str,
) -> sqlx::Result<Option<Rsvp>> {
    query_rsvp!(select "from rsvp where email = $1 and email_status = 'confirmed'", email)
        .fetch_optional(db)
        .await
}

/// All RSVPs, oldest first (the ids are UUIDv7, so they sort by creation time).
pub async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Rsvp>> {
    query_rsvp!(select "from rsvp order by id")
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

/// What the guest list needs about one guest.
pub struct Listing {
    pub id: Uuid,
    pub name: String,
    pub response: Response,
    pub name_display: Option<NameDisplay>,
}

/// Everyone who hasn't declined, for the guest list.
pub async fn guest_list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Listing>> {
    sqlx::query_as!(
        Listing,
        r#"
        select id, name, response as "response: Response",
            name_display as "name_display: NameDisplay"
        from rsvp
        where response <> 'declined'
        "#
    )
    .fetch_all(db)
    .await
}

/// Collapses whitespace, drops invisible characters and checks the length the database
/// allows. A name needs a letter or digit, so one can't be blank to the eye. Control and
/// bidirectional characters, which could make a name show as someone else's, are refused
/// rather than stripped, since no keyboard produces them.
pub fn normalize_name(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|&c| !is_invisible(c)).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = name.chars().count();
    let valid = (1..=100).contains(&length)
        && name.chars().any(char::is_alphanumeric)
        && !name.chars().any(|c| c.is_control() || is_bidi_control(c));
    valid.then_some(name)
}

/// Characters that take up no space, which a pasted name can carry along. The joiners
/// stay, since emoji and some scripts need them.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{AD}' | '\u{180E}' | '\u{200B}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}'
    )
}

/// Characters that reorder the text around them.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

pub const MAX_PARTY_SIZE: i32 = 20;

/// Trims and lower-cases an address and checks that it looks deliverable. Lower-casing
/// the local part is technically wrong, but no real mailbox depends on it, and it keeps
/// one address from being registered twice.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let (local, domain) = email.split_once('@')?;
    let valid = email.len() <= 254
        && (1..=64).contains(&local.len())
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && !email
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>(),;:\\\"[]".contains(c));
    valid.then_some(email)
}

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
/// no other RSVP has confirmed it first.
pub async fn confirm_email(db: impl PgExecutor<'_>, id: Uuid, email: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        update rsvp set email_status = 'confirmed'
        where id = $1 and email = $2 and email_status = 'unconfirmed'
            and not exists (select from rsvp where email = $2 and email_status = 'confirmed')
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

pub async fn headcount(db: impl PgExecutor<'_>) -> sqlx::Result<Headcount> {
    sqlx::query_as!(
        Headcount,
        r#"
        select
            count(*) filter (where response = 'attending') as "attending_parties!",
            coalesce(sum(party_size) filter (where response = 'attending'), 0)
                as "attending_people!",
            count(*) filter (where response = 'declined') as "declined!",
            count(*) filter (where response = 'pending') as "pending!"
        from rsvp
        "#,
    )
    .fetch_one(db)
    .await
}

#[cfg(test)]
pub(crate) mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::{db::group, web::PRIVACY_VERSION};

    /// Signs `name` up, in English, through a new group link for up to two people.
    pub async fn sign_up(pool: &PgPool, name: &str, party_size: i32) -> sqlx::Result<Rsvp> {
        let group = group::create(pool, "Climbing club", 2).await?;
        let sign_up = SignUp {
            group: &group,
            name,
            party_size,
            name_display: NameDisplay::Full,
            lang: Lang::En,
        };
        create_open(pool, &sign_up, PRIVACY_VERSION).await
    }
    const CHECK_VIOLATION: &str = "23514";

    fn is_check_violation(err: &sqlx::Error) -> bool {
        err.as_database_error()
            .and_then(|err| err.code())
            .is_some_and(|code| code == CHECK_VIOLATION)
    }

    #[sqlx::test]
    async fn invitation_starts_pending(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_invite(&pool, "Kari & Ola", 2).await?;
        assert_eq!(rsvp.source, Source::Invite);
        assert_eq!(rsvp.response, Response::Pending);
        assert_eq!((rsvp.party_size, rsvp.max_party_size), (1, 2));
        assert_eq!((rsvp.link_nonce, rsvp.lang), (0, Lang::Nb));
        assert_eq!(rsvp.email_status, EmailStatus::None);
        assert_eq!(rsvp.id.get_version_num(), 7);
        assert!(rsvp.responded_at.is_none());
        Ok(())
    }

    #[sqlx::test]
    async fn open_rsvp_is_attending_immediately(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = sign_up(&pool, "Per", 2).await?;
        assert_eq!(rsvp.source, Source::Open);
        assert!(rsvp.group_id.is_some());
        assert_eq!(rsvp.name_display, Some(NameDisplay::Full));
        assert_eq!(rsvp.response, Response::Attending);
        assert_eq!((rsvp.party_size, rsvp.lang), (2, Lang::En));
        assert_eq!(rsvp.privacy_version.as_deref(), Some(PRIVACY_VERSION));
        assert!(rsvp.responded_at.is_some());
        Ok(())
    }

    #[sqlx::test]
    async fn respond_records_the_answer(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari & Ola", 2).await?;
        let answered = respond(
            &pool,
            invite.id,
            Response::Attending,
            2,
            None,
            Lang::En,
            PRIVACY_VERSION,
        )
        .await?
        .expect("rsvp exists");
        assert_eq!(answered.response, Response::Attending);
        assert_eq!((answered.party_size, answered.lang), (2, Lang::En));
        assert!(answered.responded_at.is_some());

        let reread = find(&pool, invite.id).await?.expect("rsvp exists");
        assert_eq!(reread.response, Response::Attending);
        Ok(())
    }

    #[sqlx::test]
    async fn respond_rejects_party_larger_than_invited(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari & Ola", 2).await?;
        let err = respond(
            &pool,
            invite.id,
            Response::Attending,
            3,
            None,
            Lang::Nb,
            PRIVACY_VERSION,
        )
        .await
        .unwrap_err();
        assert!(is_check_violation(&err), "{err}");
        Ok(())
    }

    #[sqlx::test]
    async fn unknown_id_is_not_found(pool: PgPool) -> sqlx::Result<()> {
        let id = Uuid::max();
        assert!(find(&pool, id).await?.is_none());
        assert!(
            respond(
                &pool,
                id,
                Response::Declined,
                1,
                None,
                Lang::Nb,
                PRIVACY_VERSION
            )
            .await?
            .is_none()
        );
        assert!(!rename(&pool, id, "Nobody").await?);
        assert!(revoke_link(&pool, id).await?.is_none());
        assert!(!delete(&pool, id).await?);
        Ok(())
    }

    #[sqlx::test]
    async fn guest_list_leaves_out_declined_guests(pool: PgPool) -> sqlx::Result<()> {
        let kari = create_invite(&pool, "Kari Nordmann", 1).await?;
        let ola = create_invite(&pool, "Ola", 1).await?;
        respond(
            &pool,
            ola.id,
            Response::Declined,
            1,
            Some(NameDisplay::Full),
            Lang::Nb,
            PRIVACY_VERSION,
        )
        .await?;
        assert!(set_name_display(&pool, kari.id, NameDisplay::Hidden).await?);

        let [listed] = guest_list(&pool).await?.try_into().ok().unwrap();
        assert_eq!(
            (listed.id, listed.name_display),
            (kari.id, Some(NameDisplay::Hidden))
        );
        // Answering without a choice keeps the one made before.
        let answered = respond(
            &pool,
            kari.id,
            Response::Attending,
            1,
            None,
            Lang::Nb,
            PRIVACY_VERSION,
        )
        .await?
        .unwrap();
        assert_eq!(answered.listed(), NameDisplay::Hidden);
        Ok(())
    }

    #[sqlx::test]
    async fn rename_enforces_name_length(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari", 1).await?;
        assert!(rename(&pool, invite.id, "Kari Nordmann").await?);
        assert_eq!(find(&pool, invite.id).await?.unwrap().name, "Kari Nordmann");

        let err = rename(&pool, invite.id, "").await.unwrap_err();
        assert!(is_check_violation(&err), "{err}");
        Ok(())
    }

    #[test]
    fn normalizes_names() {
        assert_eq!(
            normalize_name("  Kari \t Nordmann ").unwrap(),
            "Kari Nordmann"
        );
        assert_eq!(
            normalize_name(&"ø".repeat(100)).unwrap().chars().count(),
            100
        );
        assert!(normalize_name("   ").is_none());
        assert!(normalize_name(&"a".repeat(101)).is_none());
        assert!(normalize_name("Kari\u{0}").is_none());

        assert_eq!(normalize_name("\u{feff}Ka\u{200b}ri").unwrap(), "Kari");
        assert_eq!(
            normalize_name("Kari 👨\u{200d}👩").unwrap(),
            "Kari 👨\u{200d}👩"
        );
        for blank in ["\u{200b}", "\u{200d}\u{200d}", "- . -"] {
            assert!(normalize_name(blank).is_none(), "{blank:?}");
        }
        for reordering in ["Bob\u{202e}gnp.exe", "\u{2067}Kari"] {
            assert!(normalize_name(reordering).is_none(), "{reordering:?}");
        }
    }

    #[test]
    fn partial_names_keep_the_first_name_and_an_initial() {
        assert_eq!(partial_name("Kari Nordmann"), "Kari N.");
        assert_eq!(partial_name("Kari & Ola  Østby"), "Kari Ø.");
        assert_eq!(partial_name("Per"), "Per");
        assert_eq!(partial_name("anne van der berg"), "anne B.");
    }

    #[test]
    fn lists_names_by_answer_and_choice() {
        use NameDisplay::{Full, Hidden, Partial};
        use Response::{Attending, Declined, Pending};
        assert_eq!(NameDisplay::listed(Pending, None), Partial);
        assert_eq!(NameDisplay::listed(Attending, None), Full);
        assert_eq!(NameDisplay::listed(Pending, Some(Full)), Full);
        assert_eq!(NameDisplay::listed(Attending, Some(Hidden)), Hidden);
        assert_eq!(NameDisplay::listed(Declined, Some(Full)), Hidden);
        assert_eq!(Partial.apply("Kari Nordmann").as_deref(), Some("Kari N."));
        assert_eq!(Hidden.apply("Kari Nordmann"), None);
    }

    #[test]
    fn normalizes_emails() {
        assert_eq!(
            normalize_email("  Kari@Example.COM ").unwrap(),
            "kari@example.com"
        );
        assert_eq!(
            normalize_email("ola+fest@sub.example.no").unwrap(),
            "ola+fest@sub.example.no"
        );
        for bad in [
            "",
            "kari",
            "kari@",
            "@example.com",
            "kari@example",
            "kari@@example.com",
            "kari@exa mple.com",
            "kari@.example.com",
            "kari@example.com.",
            "<kari@example.com>",
        ] {
            assert!(normalize_email(bad).is_none(), "{bad:?}");
        }
        assert!(normalize_email(&format!("{}@example.com", "a".repeat(65))).is_none());
    }

    #[sqlx::test]
    async fn email_lifecycle(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_invite(&pool, "Kari", 1).await?;
        assert!(
            set_email(
                &pool,
                rsvp.id,
                "kari@example.com",
                Lang::En,
                PRIVACY_VERSION
            )
            .await?
        );
        let stored = find(&pool, rsvp.id).await?.unwrap();
        assert_eq!(stored.email.as_deref(), Some("kari@example.com"));
        assert_eq!(stored.email_status, EmailStatus::Unconfirmed);
        assert_eq!(stored.lang, Lang::En);
        assert!(stored.email_consent_at.is_some());
        assert!(
            find_by_confirmed_email(&pool, "kari@example.com")
                .await?
                .is_none()
        );

        // Until it's confirmed, another RSVP may hold the address too.
        let other = create_invite(&pool, "Ola", 1).await?;
        assert!(
            set_email(
                &pool,
                other.id,
                "kari@example.com",
                Lang::Nb,
                PRIVACY_VERSION
            )
            .await?
        );

        assert!(!confirm_email(&pool, rsvp.id, "other@example.com").await?);
        assert!(confirm_email(&pool, rsvp.id, "kari@example.com").await?);
        assert!(!confirm_email(&pool, rsvp.id, "kari@example.com").await?);
        let confirmed = find_by_confirmed_email(&pool, "kari@example.com")
            .await?
            .unwrap();
        assert_eq!(confirmed.id, rsvp.id);
        // The first to confirm keeps it, and the database refuses a second.
        assert!(!confirm_email(&pool, other.id, "kari@example.com").await?);
        let err = sqlx::query("update rsvp set email_status = 'confirmed' where id = $1")
            .bind(other.id)
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(
            err.as_database_error()
                .is_some_and(|err| err.is_unique_violation())
        );

        assert!(remove_email(&pool, rsvp.id).await?);
        assert!(!remove_email(&pool, rsvp.id).await?);
        let removed = find(&pool, rsvp.id).await?.unwrap();
        assert_eq!(
            (removed.email, removed.email_status),
            (None, EmailStatus::None)
        );
        assert!(removed.email_consent_at.is_none());
        Ok(())
    }

    #[sqlx::test]
    async fn shrinking_an_invitation_shrinks_the_party(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Familien Hansen", 4).await?;
        respond(
            &pool,
            invite.id,
            Response::Attending,
            4,
            None,
            Lang::Nb,
            PRIVACY_VERSION,
        )
        .await?;
        assert!(update_invite(&pool, invite.id, "Hansen", 2).await?);
        let rsvp = find(&pool, invite.id).await?.unwrap();
        assert_eq!(
            (rsvp.name.as_str(), rsvp.party_size, rsvp.max_party_size),
            ("Hansen", 2, 2)
        );
        assert!(!update_invite(&pool, Uuid::max(), "Nobody", 1).await?);
        Ok(())
    }

    #[sqlx::test]
    async fn revoke_link_bumps_nonce(pool: PgPool) -> sqlx::Result<()> {
        let invite = create_invite(&pool, "Kari", 1).await?;
        assert_eq!(revoke_link(&pool, invite.id).await?, Some(1));
        assert_eq!(revoke_link(&pool, invite.id).await?, Some(2));
        Ok(())
    }

    #[sqlx::test]
    async fn delete_removes_pending_mail(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = sign_up(&pool, "Per", 1).await?;
        sqlx::query("insert into outbox (rsvp_id, kind) values ($1, 'rsvp_link')")
            .bind(rsvp.id)
            .execute(&pool)
            .await?;

        assert!(delete(&pool, rsvp.id).await?);
        let mails: i64 = sqlx::query_scalar("select count(*) from outbox")
            .fetch_one(&pool)
            .await?;
        assert_eq!(mails, 0);
        Ok(())
    }

    #[sqlx::test]
    async fn email_requires_consent_and_status(pool: PgPool) -> sqlx::Result<()> {
        let rsvp = create_invite(&pool, "Kari", 1).await?;
        let err = sqlx::query("update rsvp set email = 'kari@example.com' where id = $1")
            .bind(rsvp.id)
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(is_check_violation(&err), "{err}");

        sqlx::query(
            "update rsvp set email = 'kari@example.com', email_status = 'unconfirmed', \
             email_consent_at = now() where id = $1",
        )
        .bind(rsvp.id)
        .execute(&pool)
        .await?;
        let rsvp = find(&pool, rsvp.id).await?.unwrap();
        assert_eq!(rsvp.email_status, EmailStatus::Unconfirmed);
        Ok(())
    }

    #[sqlx::test]
    async fn headcount_sums_attending_parties(pool: PgPool) -> sqlx::Result<()> {
        assert_eq!(
            headcount(&pool).await?,
            Headcount {
                attending_parties: 0,
                attending_people: 0,
                declined: 0,
                pending: 0,
            }
        );

        let family = create_invite(&pool, "Familien Hansen", 4).await?;
        respond(
            &pool,
            family.id,
            Response::Attending,
            3,
            None,
            Lang::Nb,
            PRIVACY_VERSION,
        )
        .await?;
        let busy = create_invite(&pool, "Ola", 1).await?;
        respond(
            &pool,
            busy.id,
            Response::Declined,
            1,
            None,
            Lang::Nb,
            PRIVACY_VERSION,
        )
        .await?;
        create_invite(&pool, "Kari", 1).await?;
        sign_up(&pool, "Per", 2).await?;

        assert_eq!(
            headcount(&pool).await?,
            Headcount {
                attending_parties: 2,
                attending_people: 5,
                declined: 1,
                pending: 1,
            }
        );
        Ok(())
    }
}
