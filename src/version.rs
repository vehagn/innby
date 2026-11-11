//! Which build of Innby this is: its version, and the commit, its date and the release tag
//! `build.rs` stamped it with, for `innby --version` and the pages' footers. A build
//! without git, as from a source archive, has the version alone.

/// The version in Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The commit it was built from.
const COMMIT: Option<&str> = option_env!("INNBY_COMMIT");
/// That commit's date, as 2026-11-11.
pub const DATE: Option<&str> = option_env!("INNBY_COMMIT_DATE");
/// The release tag, as v0.7.0, when the commit is exactly a release.
const TAG: Option<&str> = option_env!("INNBY_TAG");

/// The release, as `v0.7.0`, or for a build between releases the version with its
/// commit, as `0.7.0+67e1386`, so it's never taken for the release itself.
pub fn release() -> String {
    match (TAG, short_commit()) {
        (Some(tag), _) => tag.to_owned(),
        (None, Some(commit)) => format!("{VERSION}+{commit}"),
        (None, None) => VERSION.to_owned(),
    }
}

/// The commit, shortened as GitHub shows it.
pub fn short_commit() -> Option<&'static str> {
    COMMIT.map(|commit| &commit[..commit.len().min(7)])
}

/// The commit's page in the repository, if both are known.
pub fn commit_url() -> Option<String> {
    let repository = option_env!("CARGO_PKG_REPOSITORY").filter(|url| !url.is_empty())?;
    Some(format!(
        "{}/commit/{}",
        repository.trim_end_matches('/'),
        COMMIT?
    ))
}

/// Everything known, for `innby --version`: `0.7.0 (v0.7.0, 67e1386e1157…, 2026-11-11)`.
pub fn long() -> &'static str {
    static LONG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let known: Vec<&str> = [TAG, COMMIT, DATE].into_iter().flatten().collect();
        match known.is_empty() {
            true => VERSION.to_owned(),
            false => format!("{VERSION} ({})", known.join(", ")),
        }
    });
    &LONG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_which_build_this_is() {
        // Tests are built in a checkout, so git stamped them.
        let commit = short_commit().expect("a commit");
        assert_eq!(commit.len(), 7);
        let release = release();
        assert!(release == TAG.unwrap_or_default() || release == format!("{VERSION}+{commit}"));
        assert!(long().starts_with(VERSION) && long().contains(COMMIT.unwrap()));
        assert!(DATE.is_some_and(|date| date.len() == 10));
    }
}
