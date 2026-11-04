//! What the admin's forms send, where several pages share it: a name with a party size
//! (a guest's, or a group's), a batch of them a line each (invitations, group links, and
//! a new event's group links), and a delete the host has confirmed. Also `?saved`,
//! which a page gets after a change so it can say so.
//!
//! Every check answers in the page's language, ready to show beside the form.

use serde::Deserialize;

use super::lang;
use crate::{
    db::rsvp::{self, MAX_PARTY_SIZE, NameKind},
    i18n::fill,
};

/// A name and how many people it covers: a guest's invitation, or a group's for each
/// sign-up.
#[derive(Deserialize)]
pub(super) struct NameForm {
    pub(super) name: String,
    pub(super) max_party_size: i32,
}

impl NameForm {
    /// The name, cleaned up, or what's wrong.
    pub(super) fn validate(&self, kind: NameKind) -> Result<String, String> {
        validate(&self.name, self.max_party_size, kind)
    }
}

/// A name and a party size, checked: the name cleaned up, or what's wrong.
pub(super) fn validate(name: &str, max_party_size: i32, kind: NameKind) -> Result<String, String> {
    let text = lang::text();
    if !(1..=MAX_PARTY_SIZE).contains(&max_party_size) {
        return Err(fill(&text.error_party_size, &[("max", &MAX_PARTY_SIZE)]));
    }
    let error = match kind {
        NameKind::Person => &text.error_name,
        NameKind::Group => &text.error_group_name,
    };
    rsvp::normalize_name(name, kind).ok_or_else(|| error.clone())
}

/// Several names at once, one a line.
#[derive(Deserialize)]
pub(super) struct BatchForm {
    pub(super) names: String,
    /// For lines that don't give their own.
    pub(super) max_party_size: i32,
}

impl BatchForm {
    /// Each line's name and party size. A line can end in a comma and its own party size;
    /// blank lines are skipped. One invalid line fails the whole batch.
    pub(super) fn parse(&self, kind: NameKind) -> Result<Vec<(String, i32)>, String> {
        let mut entries = Vec::new();
        for (index, line) in self.names.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (name, max_party_size) = line
                .rsplit_once(',')
                .and_then(|(name, size)| Some((name, size.trim().parse().ok()?)))
                .unwrap_or((line, self.max_party_size));
            let name = validate(name, max_party_size, kind).map_err(|error| {
                let line = index + 1;
                fill(&lang::text().error_line, &[("n", &line), ("error", &error)])
            })?;
            entries.push((name, max_party_size));
        }
        if entries.is_empty() {
            return Err(lang::text().error_no_names.clone());
        }
        Ok(entries)
    }
}

/// Deleting something, which the host confirms with a tick.
#[derive(Deserialize)]
pub(super) struct DeleteForm {
    pub(super) confirm: Option<String>,
}

/// `?saved`, after a change, for the page to say it was saved.
#[derive(Deserialize)]
pub(super) struct SavedQuery {
    pub(super) saved: Option<String>,
}
