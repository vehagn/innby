//! Reading `INNBY_*` settings through a lookup function: the process environment when
//! Innby runs, and a map in the tests. Blank counts as unset.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow};

pub(super) struct Env<F>(pub(super) F);

/// The real lookup, for `Env`.
pub(super) fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

impl<F: Fn(&str) -> Option<String>> Env<F> {
    pub(super) fn optional(&self, key: &str) -> Option<String> {
        (self.0)(key).filter(|value| !value.trim().is_empty())
    }

    pub(super) fn required(&self, key: &str) -> Result<String> {
        self.optional(key)
            .ok_or_else(|| anyhow!("{key} must be set"))
    }

    pub(super) fn socket_address(&self, key: &str, default: &str) -> Result<SocketAddr> {
        let value = self.optional(key).unwrap_or_else(|| default.into());
        value
            .parse()
            .with_context(|| format!("{key}: invalid socket address {value:?}"))
    }
}
