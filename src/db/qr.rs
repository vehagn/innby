//! Each event's QR code style, which the host sets on the event's QR codes tab: the
//! modules' shape and colour, and what sits in the middle. It holds no guest data.

use serde::{Deserialize, Serialize};
use sqlx::PgExecutor;
use uuid::Uuid;

/// How the modules of a QR code are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "qr_shape", rename_all = "lowercase")]
pub enum Shape {
    Square,
    /// Modules that run together, with rounded ends.
    Rounded,
    Dots,
}

impl Shape {
    /// The one with this form value.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.code() == code)
    }

    pub const ALL: [Self; 3] = [Self::Square, Self::Rounded, Self::Dots];

    /// The form value.
    pub fn code(self) -> &'static str {
        match self {
            Self::Square => "square",
            Self::Rounded => "rounded",
            Self::Dots => "dots",
        }
    }
}

/// What sits in the middle of the code.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "qr_icon", rename_all = "lowercase")]
pub enum Icon {
    #[default]
    None,
    Heart,
    Star,
    /// The style's `emoji`.
    Emoji,
}

impl Icon {
    /// The one with this form value.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.code() == code)
    }

    pub const ALL: [Self; 4] = [Self::None, Self::Heart, Self::Star, Self::Emoji];

    /// The form value.
    pub fn code(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Heart => "heart",
            Self::Star => "star",
            Self::Emoji => "emoji",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub shape: Shape,
    /// `#rrggbb`, dark enough to scan on white.
    pub color: String,
    pub icon: Icon,
    pub emoji: String,
}

/// The event's style.
pub async fn style(db: impl PgExecutor<'_>, event_id: Uuid) -> sqlx::Result<Style> {
    sqlx::query_as!(
        Style,
        r#"select qr_shape as "shape: Shape", qr_color as color, qr_icon as "icon: Icon",
               qr_emoji as emoji
           from event where id = $1"#,
        event_id,
    )
    .fetch_one(db)
    .await
}

/// Gives the event this style; false if there's no such event.
pub async fn save(db: impl PgExecutor<'_>, event_id: Uuid, style: &Style) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "update event set qr_shape = $2, qr_color = $3, qr_icon = $4, qr_emoji = $5 \
         where id = $1",
        event_id,
        style.shape as Shape,
        style.color,
        style.icon as Icon,
        style.emoji,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}
