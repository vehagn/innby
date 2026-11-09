//! Admin forms that made something, by the random token each form carries, for a day: the
//! same form sent again, as when the answer to the first was lost on the way, goes where
//! the first went rather than making it all again. Nothing in it is personal: a token,
//! and the admin page the form led to.

use sqlx::PgExecutor;

/// Where the form with this token led, if it was sent before.
pub async fn sent(db: impl PgExecutor<'_>, token: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar!(
        "select page from admin_form_sent where token = $1 and sent_at > now() - interval '1 day'",
        token,
    )
    .fetch_optional(db)
    .await
}

/// Notes that the form with this token led to `page`, and forgets those from more than a
/// day ago, which nobody sends again. Returns false if another request with the same
/// token got there first.
pub async fn note(db: impl PgExecutor<'_>, token: &str, page: &str) -> sqlx::Result<bool> {
    let noted = sqlx::query!(
        r#"
        with old as (delete from admin_form_sent where sent_at <= now() - interval '1 day')
        insert into admin_form_sent (token, page) values ($1, $2)
        on conflict (token) do nothing
        "#,
        token,
        page,
    )
    .execute(db)
    .await?;
    Ok(noted.rows_affected() == 1)
}
