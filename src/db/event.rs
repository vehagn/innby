//! Events, which the host creates in the admin UI. Every invitation and group link
//! belongs to one, and is deleted with it.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::{
    i18n::{Lang, Texts},
    janitor::KEEP_AFTER_EVENT,
};

#[derive(Debug, Clone)]
pub struct Event {
    pub id: Uuid,
    pub details: Details,
}

/// What the host says about an event.
#[derive(Debug, Clone, PartialEq)]
pub struct Details {
    pub title: Texts,
    pub description: Texts,
    pub location: String,
    /// In the event's time zone, in which every date about it shows.
    pub starts_at: DateTime<Tz>,
    /// Guests can answer until then; `None` means until the event starts.
    pub rsvp_deadline: Option<DateTime<Tz>>,
}

impl Details {
    /// Whether guests can still answer or change their answer.
    pub fn accepts_answers(&self, now: DateTime<Utc>) -> bool {
        now < self.rsvp_deadline.unwrap_or(self.starts_at)
    }

    pub fn title(&self, lang: Lang) -> &str {
        self.title.get(lang)
    }
}

impl Event {
    /// When the event and everything in it is deleted.
    pub fn deleted_at(&self) -> DateTime<Utc> {
        self.details.starts_at.to_utc() + KEEP_AFTER_EVENT
    }
}

/// A row as stored, with each language in a column of its own.
struct Row {
    id: Uuid,
    title: String,
    title_nn: Option<String>,
    title_en: Option<String>,
    title_en_us: Option<String>,
    title_zh_hans: Option<String>,
    title_zh_hant: Option<String>,
    description: String,
    description_nn: Option<String>,
    description_en: Option<String>,
    description_en_us: Option<String>,
    description_zh_hans: Option<String>,
    description_zh_hant: Option<String>,
    location: String,
    starts_at: DateTime<Utc>,
    time_zone: String,
    rsvp_deadline: Option<DateTime<Utc>>,
}

impl From<Row> for Event {
    fn from(row: Row) -> Self {
        // Only valid zones are stored; an unknown one shows as UTC rather than failing.
        let tz: Tz = row.time_zone.parse().unwrap_or(Tz::UTC);
        Self {
            id: row.id,
            details: Details {
                title: Texts {
                    nb: row.title,
                    nn: row.title_nn,
                    en_gb: row.title_en,
                    en_us: row.title_en_us,
                    zh_hans: row.title_zh_hans,
                    zh_hant: row.title_zh_hant,
                },
                description: Texts {
                    nb: row.description,
                    nn: row.description_nn,
                    en_gb: row.description_en,
                    en_us: row.description_en_us,
                    zh_hans: row.description_zh_hans,
                    zh_hant: row.description_zh_hant,
                },
                location: row.location,
                starts_at: row.starts_at.with_timezone(&tz),
                rsvp_deadline: row.rsvp_deadline.map(|at| at.with_timezone(&tz)),
            },
        }
    }
}

/// `query_as!` for whole events, with the details as parameters `$2` to `$17` in the
/// order `bind_details!` gives them.
macro_rules! bind_details {
    ($query:literal, $id:expr, $details:expr) => {{
        let details: &Details = $details;
        let (title, description) = (&details.title, &details.description);
        sqlx::query_as!(
            Row,
            $query,
            $id,
            title.nb,
            title.nn,
            title.en_gb,
            title.en_us,
            title.zh_hans,
            title.zh_hant,
            description.nb,
            description.nn,
            description.en_gb,
            description.en_us,
            description.zh_hans,
            description.zh_hant,
            details.location,
            details.starts_at.to_utc(),
            details.starts_at.timezone().name(),
            details.rsvp_deadline.map(|at| at.to_utc()),
        )
    }};
}

pub async fn create(db: impl PgExecutor<'_>, details: &Details) -> sqlx::Result<Event> {
    bind_details!(
        "insert into event (id, title, title_nn, title_en, title_en_us, title_zh_hans, \
            title_zh_hant, description, description_nn, description_en, description_en_us, \
            description_zh_hans, description_zh_hant, location, starts_at, time_zone, \
            rsvp_deadline) \
         values (coalesce($1, uuidv7()), $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
            $14, $15, $16, $17) \
         returning *",
        None::<Uuid>,
        details
    )
    .fetch_one(db)
    .await
    .map(Event::from)
}

/// Changes what the host says about the event. Returns `None` if there's no such event.
pub async fn update(
    db: impl PgExecutor<'_>,
    id: Uuid,
    details: &Details,
) -> sqlx::Result<Option<Event>> {
    bind_details!(
        "update event set title = $2, title_nn = $3, title_en = $4, title_en_us = $5, \
            title_zh_hans = $6, title_zh_hant = $7, description = $8, description_nn = $9, \
            description_en = $10, description_en_us = $11, description_zh_hans = $12, \
            description_zh_hant = $13, location = $14, starts_at = $15, time_zone = $16, \
            rsvp_deadline = $17 \
         where id = $1 returning *",
        id,
        details
    )
    .fetch_optional(db)
    .await
    .map(|row| row.map(Event::from))
}

/// Fills in the event that older data was given when events moved into the database,
/// from the settings that used to describe it. Returns whether there was one to fill.
pub async fn adopt(db: &PgPool, details: &Details) -> sqlx::Result<bool> {
    let placeholder = sqlx::query_scalar!("select id from event where title = ''")
        .fetch_optional(db)
        .await?;
    match placeholder {
        Some(id) => Ok(update(db, id, details).await?.is_some()),
        None => Ok(false),
    }
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Event>> {
    sqlx::query_as!(Row, "select * from event where id = $1", id)
        .fetch_optional(db)
        .await
        .map(|row| row.map(Event::from))
}

/// Every event, the soonest first.
pub async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Event>> {
    sqlx::query_as!(Row, "select * from event order by starts_at, id")
        .fetch_all(db)
        .await
        .map(|rows| rows.into_iter().map(Event::from).collect())
}

/// Deletes the event and, through the cascades, its invitations, groups and mail.
pub async fn delete(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!("delete from event where id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes every event that started by `cutoff`, with all that belongs to it. Returns
/// how many guests' data went with them.
pub async fn delete_started_by(
    db: impl PgExecutor<'_>,
    cutoff: DateTime<Utc>,
) -> sqlx::Result<u64> {
    sqlx::query_scalar!(
        r#"
        with gone as (delete from event where starts_at <= $1 returning id)
        select count(*) as "guests!" from rsvp where event_id in (select id from gone)
        "#,
        cutoff,
    )
    .fetch_one(db)
    .await
    .map(|guests| guests as u64)
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::TimeZone;

    use super::*;

    /// The tests' event: the Christmas party at Storgata 1 on 12 December 2026.
    pub fn details() -> Details {
        Details {
            title: Texts {
                nb: "Julebord".into(),
                en_gb: Some("Christmas party".into()),
                ..Texts::default()
            },
            description: Texts {
                nb: "Velkommen!".into(),
                ..Texts::default()
            },
            location: "Storgata 1, Oslo".into(),
            starts_at: chrono_tz::Europe::Oslo
                .with_ymd_and_hms(2026, 12, 12, 19, 0, 0)
                .unwrap(),
            rsvp_deadline: None,
        }
    }

    /// `details()`, answering until noon on 1 December.
    pub fn details_with_deadline() -> Details {
        Details {
            rsvp_deadline: Some(
                chrono_tz::Europe::Oslo
                    .with_ymd_and_hms(2026, 12, 1, 12, 0, 0)
                    .unwrap(),
            ),
            ..details()
        }
    }

    /// The test database's event, made the first time it's needed.
    pub async fn event(db: &PgPool) -> Event {
        match list(db).await.unwrap().into_iter().next() {
            Some(event) => event,
            None => create(db, &details()).await.unwrap(),
        }
    }

    #[sqlx::test]
    async fn stores_texts_per_language_and_times_in_the_events_zone(db: PgPool) {
        let mut details = details();
        details.title.set(Lang::ZhHans, "圣诞晚会");
        let event = create(&db, &details).await.unwrap();
        let found = find(&db, event.id).await.unwrap().unwrap();
        assert_eq!(found.details, details);
        assert_eq!(found.details.title(Lang::Nn), "Julebord");
        assert_eq!(found.details.title(Lang::EnUs), "Christmas party");
        assert_eq!(found.details.title(Lang::ZhHant), "圣诞晚会");
        assert_eq!(
            found.details.starts_at.to_rfc3339(),
            "2026-12-12T19:00:00+01:00"
        );

        details.location = "Storgata 2".into();
        let updated = update(&db, event.id, &details).await.unwrap().unwrap();
        assert_eq!(updated.details.location, "Storgata 2");
        assert!(update(&db, Uuid::nil(), &details).await.unwrap().is_none());
    }

    #[test]
    fn answers_close_at_the_deadline_or_the_start() {
        let at = |time: &str| time.parse::<DateTime<Utc>>().unwrap();
        let details = details();
        assert!(details.accepts_answers(at("2026-12-12T17:59:00Z")));
        assert!(!details.accepts_answers(at("2026-12-12T18:00:00Z")));
        let details = details_with_deadline();
        assert!(details.accepts_answers(at("2026-12-01T10:59:00Z")));
        assert!(!details.accepts_answers(at("2026-12-01T11:00:00Z")));
    }

    #[sqlx::test]
    async fn the_placeholder_for_older_data_is_adopted_once(db: PgPool) {
        assert!(!adopt(&db, &details()).await.unwrap());
        sqlx::query!("insert into event (title, starts_at) values ('', now())")
            .execute(&db)
            .await
            .unwrap();
        assert!(adopt(&db, &details()).await.unwrap());
        assert_eq!(event(&db).await.details, details());
        assert!(!adopt(&db, &details()).await.unwrap());
    }

    #[sqlx::test]
    async fn events_go_with_everything_in_them(db: PgPool) {
        let mut past = details();
        past.starts_at = chrono_tz::Europe::Oslo
            .with_ymd_and_hms(2026, 1, 1, 19, 0, 0)
            .unwrap();
        let past = create(&db, &past).await.unwrap();
        let future = create(&db, &details()).await.unwrap();
        crate::db::rsvp::create_invite(&db, past.id, "Kari", 1)
            .await
            .unwrap();
        crate::db::group::create(&db, past.id, "Climbers", 2)
            .await
            .unwrap();

        let cutoff = at_utc("2026-06-01T00:00:00Z");
        assert_eq!(delete_started_by(&db, cutoff).await.unwrap(), 1);
        assert!(find(&db, past.id).await.unwrap().is_none());
        assert!(find(&db, future.id).await.unwrap().is_some());
        assert!(
            crate::db::rsvp::list(&db, past.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(delete(&db, future.id).await.unwrap());
    }

    fn at_utc(time: &str) -> DateTime<Utc> {
        time.parse().unwrap()
    }
}
