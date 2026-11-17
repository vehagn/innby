//! What went wrong, as the logs may say it. An error's own words can quote what it was
//! about: Postgres names the value it couldn't read, a provider describes the request it
//! turned down. So the logs get a [`Cause`] instead, which is made only from what holds
//! nothing a guest or a host typed: words written in the code, and what the database
//! says of an error in the schema's terms. `metrics::Label` does the same for metrics.

use std::{borrow::Cow, fmt};

/// What failed: from the code, or from what kind of error it was, never from its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cause(Cow<'static, str>);

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&'static str> for Cause {
    fn from(cause: &'static str) -> Self {
        Self(Cow::Borrowed(cause))
    }
}

impl From<&sqlx::Error> for Cause {
    fn from(err: &sqlx::Error) -> Self {
        use sqlx::Error;
        let cause = match err {
            // What Postgres refused: its SQLSTATE code, and the constraint or table it
            // names, which are the schema's words. Not its message, which quotes a value
            // in some, as `invalid input syntax for type uuid: "…"` does.
            Error::Database(err) => {
                let code = err.code().unwrap_or(Cow::Borrowed("without a code"));
                let on = err.constraint().or(err.table());
                return Self(Cow::Owned(match on {
                    Some(on) => format!("database error {code} on {on}"),
                    None => format!("database error {code}"),
                }));
            }
            // Which way the connection broke, as `ConnectionRefused`.
            Error::Io(err) => {
                return Self(Cow::Owned(format!("database connection {:?}", err.kind())));
            }
            Error::Tls(_) => "database connection: TLS failed",
            Error::Protocol(_) => "database connection: unexpected data",
            Error::PoolTimedOut => "no database connection free in time",
            Error::PoolClosed => "the database pool is closed",
            Error::RowNotFound => "a query found no row",
            // The column's name or number, which is the query's.
            Error::ColumnDecode { index, .. } => {
                return Self(Cow::Owned(format!("unreadable column {index} of a query")));
            }
            Error::ColumnNotFound(_) | Error::ColumnIndexOutOfBounds { .. } => {
                "a query lacks a column"
            }
            Error::Decode(_) | Error::Encode(_) | Error::TypeNotFound { .. } => {
                "a value and its database type don't match"
            }
            _ => "database error",
        };
        Self(Cow::Borrowed(cause))
    }
}

impl From<sqlx::Error> for Cause {
    fn from(err: sqlx::Error) -> Self {
        Self::from(&err)
    }
}

/// A page that couldn't be rendered: which way, not what a value said of itself.
impl From<askama::Error> for Cause {
    fn from(err: askama::Error) -> Self {
        Self(Cow::Borrowed(match err {
            askama::Error::Fmt => "rendering a page: formatting failed",
            askama::Error::ValueMissing => "rendering a page: a value is missing",
            askama::Error::ValueType => "rendering a page: a value has the wrong type",
            _ => "rendering a page failed",
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use sqlx::PgPool;

    use super::*;
    use crate::db::admin_user;

    const PRIVATE: &str = "host-private@example.com";

    #[sqlx::test]
    async fn a_database_error_is_its_code_and_what_the_schema_names(db: PgPool) {
        // The same address twice: Postgres names the constraint, and quotes the address
        // in the error's detail.
        admin_user::create(&db, "kari", None, None, Some(PRIVATE))
            .await
            .unwrap()
            .unwrap();
        let err = sqlx::query("insert into admin_user (name, email) values ('ola', $1)")
            .bind(PRIVATE)
            .execute(&db)
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains(PRIVATE), "{err:?}");
        assert_eq!(
            Cause::from(&err).to_string(),
            "database error 23505 on admin_user_email_key"
        );

        // A value Postgres can't read is quoted in its message, and left out here.
        let err = sqlx::query("select $1::uuid")
            .bind(PRIVATE)
            .execute(&db)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(PRIVATE), "{err}");
        assert_eq!(Cause::from(err).to_string(), "database error 22P02");
    }

    #[test]
    fn other_errors_are_their_kind_alone() {
        let cause = |err: sqlx::Error| Cause::from(err).to_string();
        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, PRIVATE);
        assert_eq!(
            cause(refused.into()),
            "database connection ConnectionRefused"
        );
        assert_eq!(
            cause(sqlx::Error::Protocol(PRIVATE.into())),
            "database connection: unexpected data"
        );
        assert_eq!(
            cause(sqlx::Error::ColumnDecode {
                index: "\"email\"".into(),
                source: PRIVATE.into(),
            }),
            "unreadable column \"email\" of a query"
        );
        assert_eq!(
            cause(sqlx::Error::PoolTimedOut),
            "no database connection free in time"
        );
        let custom = askama::Error::Custom(PRIVATE.into());
        assert_eq!(Cause::from(custom).to_string(), "rendering a page failed");
        assert_eq!(
            Cause::from("as the code says").to_string(),
            "as the code says"
        );
    }
}
