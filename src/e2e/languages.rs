//! Journeys in other languages: each language falls back as its file says, for the
//! texts the host writes too.

use sqlx::PgPool;

use super::{World, admin};

#[sqlx::test]
async fn an_event_title_falls_back_through_each_languages_chain(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    // The tests' event is titled in Bokmål and British English only.
    let link = world.invite("Kari Nordmann", 1).await;
    let mut kari = world.browser();
    let title = |page: &super::browser::Page, shown: &str, hidden: &str| {
        page.expect(shown).expect_not(hidden);
    };
    for (lang, shown, hidden) in [
        // Nynorsk falls back to Bokmål.
        ("nn", "Julebord", "Christmas party"),
        // American English to British, and both Chinese scripts to British too.
        ("en-us", "Christmas party", "Julebord"),
        ("zh-hans", "Christmas party", "Julebord"),
        ("zh-hant", "Christmas party", "Julebord"),
    ] {
        title(
            kari.open(&format!("{link}?lang={lang}")).await,
            shown,
            hidden,
        );
    }

    // Once the host writes it in Nynorsk, Nynorsk readers get that.
    let mut host = world.host().await;
    host.click(&a.section_settings).await;
    host.fill(&format!("{} · Nynorsk", a.event_title), "Juleselskap");
    host.press(&a.save).await.expect(&a.saved);
    title(
        kari.open(&format!("{link}?lang=nn")).await,
        "Juleselskap",
        "Julebord",
    );
    title(
        kari.open(&format!("{link}?lang=nb")).await,
        "Julebord",
        "Juleselskap",
    );
    // And the page itself is in the language asked for.
    let page = kari.open(&format!("{link}?lang=zh-hant")).await;
    assert!(page.body.contains(r#"<html lang="zh-Hant""#));
}
