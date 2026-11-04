//! Events, which the host creates in the admin UI. Every invitation and group link
//! belongs to one, and is deleted with it.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sqlx::{PgExecutor, types::Json};
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

/// A row as stored, with the texts as maps by language code. The QR code style is in the
/// same row, but `db::qr` reads and writes it.
pub(super) struct Row {
    pub(super) id: Uuid,
    pub(super) title: Json<Texts>,
    pub(super) description: Json<Texts>,
    pub(super) location: String,
    pub(super) starts_at: DateTime<Utc>,
    pub(super) time_zone: String,
    pub(super) rsvp_deadline: Option<DateTime<Utc>>,
}

impl From<Row> for Event {
    fn from(row: Row) -> Self {
        // Only valid zones are stored; an unknown one shows as UTC rather than failing.
        let tz: Tz = row.time_zone.parse().unwrap_or(Tz::UTC);
        Self {
            id: row.id,
            details: Details {
                title: row.title.0,
                description: row.description.0,
                location: row.location,
                starts_at: row.starts_at.with_timezone(&tz),
                rsvp_deadline: row.rsvp_deadline.map(|at| at.with_timezone(&tz)),
            },
        }
    }
}

pub async fn create(db: impl PgExecutor<'_>, details: &Details) -> sqlx::Result<Event> {
    sqlx::query_as!(
        Row,
        r#"insert into event (title, description, location, starts_at, time_zone, rsvp_deadline)
         values ($1, $2, $3, $4, $5, $6)
         returning id, title as "title: Json<Texts>", description as "description: Json<Texts>",
            location, starts_at, time_zone, rsvp_deadline"#,
        Json(&details.title) as _,
        Json(&details.description) as _,
        details.location,
        details.starts_at.to_utc(),
        details.starts_at.timezone().name(),
        details.rsvp_deadline.map(|at| at.to_utc()),
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
    sqlx::query_as!(
        Row,
        r#"update event set title = $2, description = $3, location = $4, starts_at = $5,
            time_zone = $6, rsvp_deadline = $7
         where id = $1
         returning id, title as "title: Json<Texts>", description as "description: Json<Texts>",
            location, starts_at, time_zone, rsvp_deadline"#,
        id,
        Json(&details.title) as _,
        Json(&details.description) as _,
        details.location,
        details.starts_at.to_utc(),
        details.starts_at.timezone().name(),
        details.rsvp_deadline.map(|at| at.to_utc()),
    )
    .fetch_optional(db)
    .await
    .map(|row| row.map(Event::from))
}

pub async fn find(db: impl PgExecutor<'_>, id: Uuid) -> sqlx::Result<Option<Event>> {
    sqlx::query_as!(
        Row,
        r#"select id, title as "title: Json<Texts>", description as "description: Json<Texts>",
            location, starts_at, time_zone, rsvp_deadline from event where id = $1"#,
        id
    )
    .fetch_optional(db)
    .await
    .map(|row| row.map(Event::from))
}

/// Every event, the soonest first.
pub async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Event>> {
    sqlx::query_as!(
        Row,
        r#"select id, title as "title: Json<Texts>", description as "description: Json<Texts>",
            location, starts_at, time_zone, rsvp_deadline from event order by starts_at, id"#
    )
    .fetch_all(db)
    .await
    .map(|rows| rows.into_iter().map(Event::from).collect())
}

/// The events the user has a role for, the soonest first: all they see without a role
/// of their own.
pub async fn list_given(db: impl PgExecutor<'_>, user: &str) -> sqlx::Result<Vec<Event>> {
    sqlx::query_as!(
        Row,
        r#"select e.id, e.title as "title: Json<Texts>", e.description as "description: Json<Texts>",
            e.location, e.starts_at, e.time_zone, e.rsvp_deadline
         from event e join event_access a on a.event_id = e.id
         where a.user_name = $1 order by e.starts_at, e.id"#,
        user
    )
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
    use sqlx::PgPool;

    use super::*;

    /// The tests' event: the Christmas party at Storgata 1 on 12 December 2054. Years
    /// ahead, so its answers stay open whenever the tests run, and in a year with 2026's
    /// calendar, so the weekdays are those the tests were written with.
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
                .with_ymd_and_hms(2054, 12, 12, 19, 0, 0)
                .unwrap(),
            rsvp_deadline: None,
        }
    }

    /// `details()`, answering until noon on 1 December.
    pub fn details_with_deadline() -> Details {
        Details {
            rsvp_deadline: Some(
                chrono_tz::Europe::Oslo
                    .with_ymd_and_hms(2054, 12, 1, 12, 0, 0)
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
    async fn the_database_refuses_what_the_form_would(db: PgPool) {
        let event = event(&db).await;
        for (column, value) in [
            ("title", r#"{"nb": ""}"#),
            ("title", r#"{"en": "Christmas party"}"#),
            ("title", r#"{"nb": "Julebord", "de": "Weihnachtsfeier"}"#),
            ("title", "{\"nb\": \"Jule\\u0001bord\"}"),
            ("title", "{\"nb\": \"Jule\\nbord\"}"),
            ("description", "{\"nb\": \"Velkommen\\u0007\"}"),
            ("description", r#"{"nb": 5}"#),
        ] {
            let result = sqlx::query(sqlx::AssertSqlSafe(format!(
                // The column names are the test's own.
                "update event set {column} = $1::jsonb where id = $2"
            )))
            .bind(value)
            .bind(event.id)
            .execute(&db)
            .await;
            assert!(result.is_err(), "{column} = {value}");
        }
        // A description keeps its line breaks and tabs.
        sqlx::query("update event set description = $1::jsonb where id = $2")
            .bind("{\"nb\": \"Velkommen!\\r\\n\\tKl. 19\"}")
            .bind(event.id)
            .execute(&db)
            .await
            .unwrap();
        let place = sqlx::query("update event set location = $1 where id = $2")
            .bind("Storgata\n1")
            .bind(event.id)
            .execute(&db)
            .await;
        assert!(place.is_err());
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
            "2054-12-12T19:00:00+01:00"
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
        assert!(details.accepts_answers(at("2054-12-12T17:59:00Z")));
        assert!(!details.accepts_answers(at("2054-12-12T18:00:00Z")));
        let details = details_with_deadline();
        assert!(details.accepts_answers(at("2054-12-01T10:59:00Z")));
        assert!(!details.accepts_answers(at("2054-12-01T11:00:00Z")));
    }

    #[sqlx::test]
    async fn events_go_with_everything_in_them(db: PgPool) {
        let mut past = details();
        past.starts_at = chrono_tz::Europe::Oslo
            .with_ymd_and_hms(2054, 1, 1, 19, 0, 0)
            .unwrap();
        let past = create(&db, &past).await.unwrap();
        let future = create(&db, &details()).await.unwrap();
        crate::db::rsvp::create_invite(&db, past.id, "Kari", 1)
            .await
            .unwrap();
        crate::db::group::create(&db, past.id, "Climbers", 2)
            .await
            .unwrap();

        let cutoff = at_utc("2054-06-01T00:00:00Z");
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
