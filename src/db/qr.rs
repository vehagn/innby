//! The event's QR code style, one row the host edits in the admin UI.

use serde::Deserialize;
use sqlx::PgExecutor;

/// How the modules of a QR code are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "qr_shape", rename_all = "lowercase")]
pub enum Shape {
    Square,
    /// Modules that run together, with rounded ends.
    Rounded,
    Dots,
}

impl Shape {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "qr_icon", rename_all = "lowercase")]
pub enum Icon {
    None,
    Heart,
    Star,
    /// The style's `emoji`.
    Emoji,
}

impl Icon {
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

pub async fn style(db: impl PgExecutor<'_>) -> sqlx::Result<Style> {
    sqlx::query_as!(
        Style,
        r#"select shape as "shape: Shape", color, icon as "icon: Icon", emoji from qr_style"#
    )
    .fetch_one(db)
    .await
}

pub async fn save(db: impl PgExecutor<'_>, style: &Style) -> sqlx::Result<()> {
    sqlx::query!(
        "update qr_style set shape = $1, color = $2, icon = $3, emoji = $4",
        style.shape as Shape,
        style.color,
        style.icon as Icon,
        style.emoji,
    )
    .execute(db)
    .await?;
    Ok(())
}
