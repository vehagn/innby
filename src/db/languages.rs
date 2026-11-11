//! The languages the site offers guests, as an admin picks them on the admin UI's
//! Languages tab: some of those it has files for, and the one a guest gets whose browser
//! asks for none of them. Nothing is stored until an admin saves a choice; until then
//! the site offers every language it has, and English by default.

use sqlx::PgExecutor;

use crate::i18n::{Lang, LangSet, Offer};

/// What the site offers. A stored language whose file has since gone is left out, and
/// if the default's has, the first that's left stands in.
pub async fn site(db: impl PgExecutor<'_>) -> sqlx::Result<Offer> {
    let row = sqlx::query!("select offered, default_lang from site_languages")
        .fetch_optional(db)
        .await?;
    let Some(row) = row else {
        return Ok(Offer::everything());
    };
    let offered = LangSet::from_codes(&row.offered);
    let default = Lang::by_code(&row.default_lang).or_else(|| offered.iter().next());
    Ok(match default {
        Some(default) => Offer::new(offered, default),
        // Every language picked has gone: the same as none picked.
        None => Offer::everything(),
    })
}

/// Keeps the admin's choice in place of any before it.
pub async fn save_site(db: impl PgExecutor<'_>, offer: Offer) -> sqlx::Result<()> {
    sqlx::query!(
        "insert into site_languages (offered, default_lang) values ($1, $2)
         on conflict (only_row) do update
         set offered = excluded.offered, default_lang = excluded.default_lang",
        &offer.languages.codes(),
        offer.default.code(),
    )
    .execute(db)
    .await
    .map(drop)
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test]
    async fn offers_everything_until_an_admin_picks(db: PgPool) {
        assert_eq!(site(&db).await.unwrap(), Offer::everything());

        let picked: LangSet = [Lang::NB, Lang::EN].into_iter().collect();
        save_site(&db, Offer::new(picked, Lang::NB)).await.unwrap();
        assert_eq!(site(&db).await.unwrap(), Offer::new(picked, Lang::NB));
        // A new choice takes the place of the old.
        let only: LangSet = [Lang::NN].into_iter().collect();
        save_site(&db, Offer::new(only, Lang::NN)).await.unwrap();
        assert_eq!(site(&db).await.unwrap(), Offer::new(only, Lang::NN));
    }

    #[sqlx::test]
    async fn a_language_whose_file_has_gone_is_left_out(db: PgPool) {
        sqlx::query!(
            "insert into site_languages (offered, default_lang) values ($1, 'xx')",
            &["xx".to_owned(), "nb".to_owned()],
        )
        .execute(&db)
        .await
        .unwrap();
        let only: LangSet = [Lang::NB].into_iter().collect();
        assert_eq!(site(&db).await.unwrap(), Offer::new(only, Lang::NB));
        // The database refuses what the form would: none offered, or a default that isn't.
        for (offered, default) in [
            (vec![], "nb"),
            (vec!["nb".to_owned()], "en"),
            (vec!["NB!".to_owned()], "NB!"),
        ] {
            let result = sqlx::query!(
                "update site_languages set offered = $1, default_lang = $2",
                &offered,
                default,
            )
            .execute(&db)
            .await;
            assert!(result.is_err(), "{offered:?} {default}");
        }
    }
}
