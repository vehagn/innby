//! The Languages tab, for admins: which of the languages Innby has a file for the site
//! offers guests, and which one a guest gets whose browser asks for none of them. A
//! language's file may be there without the site using it. Each is listed with what it
//! falls back to and how far it's translated. The choice is kept in the database
//! (`db::languages`), where the site reads it; an event narrows it further in its form.

use askama::Template;
use axum::{
    Form,
    extract::{Query, State},
    response::Response,
};

use super::{
    forms::SavedQuery,
    lang,
    page::{AdminState, PageError, redirect},
};
use crate::{
    db::languages,
    http::render_form,
    i18n::{Lang, LangSet, Offer, fill},
    paths,
};

/// A language as the page lists it.
struct Row {
    code: &'static str,
    name: &'static str,
    /// The languages its missing texts come from, nearest first; empty for the base.
    fallbacks: String,
    /// How far it's translated, as "49% (186 of 373)".
    translated: String,
    offered: bool,
    default: bool,
}

#[derive(Template)]
#[template(path = "admin/languages.html")]
struct LanguagesPage {
    rows: Vec<Row>,
    notice: Option<String>,
    error: Option<String>,
}

/// The page with `offer` ticked: what's saved, or what was sent and needs fixing.
fn page(
    offer: Offer,
    default: Option<Lang>,
    notice: Option<String>,
    error: Option<String>,
) -> Response {
    let text = lang::text();
    let rows = Lang::all()
        .iter()
        .map(|&lang| {
            let fallbacks: Vec<&str> = lang.fallbacks().iter().map(|lang| lang.name()).collect();
            let (own, total) = lang.translated();
            let share = [
                ("percent", &lang.progress() as &dyn std::fmt::Display),
                ("own", &own),
                ("total", &total),
            ];
            Row {
                code: lang.code(),
                name: lang.name(),
                fallbacks: fallbacks.join(" → "),
                translated: fill(&text.translated_share, &share),
                offered: offer.languages.contains(lang),
                default: default == Some(lang),
            }
        })
        .collect();
    let needs_fixing = error.is_some();
    let page = LanguagesPage {
        rows,
        notice,
        error,
    };
    render_form(page, needs_fixing)
}

/// The languages, with those the site offers ticked.
pub(super) async fn show(
    State(state): State<AdminState>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let offer = languages::site(&state.db).await?;
    let notice = query.saved.map(|_| lang::text().saved.clone());
    Ok(page(offer, Some(offer.default), notice, None))
}

/// Offers the languages ticked, and no others: an `offer` field for each, and `default`
/// for the one a guest gets whose browser asks for none of them.
pub(super) async fn edit(
    State(state): State<AdminState>,
    Form(form): Form<Vec<(String, String)>>,
) -> Result<Response, PageError> {
    let text = lang::text();
    // Every language sent under a name: a code Innby has no file for is passed over.
    let sent = |name: &str| -> Vec<Lang> {
        let named = form.iter().filter(|(key, _)| key == name);
        named.filter_map(|(_, code)| Lang::by_code(code)).collect()
    };
    let offered: LangSet = sent("offer").into_iter().collect();
    let default = sent("default").first().copied();
    let error = match default {
        _ if offered.is_empty() => Some(&text.error_no_languages),
        Some(default) if offered.contains(default) => None,
        _ => Some(&text.error_default_language),
    };
    let (Some(default), None) = (default, error) else {
        // As sent, to fix: `Offer::new` would add the default to the ticks.
        let shown = Offer {
            languages: offered,
            default: Lang::base(),
        };
        return Ok(page(shown, default, None, error.cloned()));
    };
    languages::save_site(&state.db, Offer::new(offered, default)).await?;
    tracing::info!(
        offered = offered.codes().join(","),
        default = default.code(),
        "site languages saved"
    );
    Ok(redirect(&format!("{}?saved", paths::LANGUAGES)))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use sqlx::PgPool;

    use super::*;
    use crate::{
        admin::testing::Admin,
        testing::{body, escape, location},
    };

    /// Whether the page has the box or button of this name and language ticked.
    fn ticked(html: &str, name: &str, code: &str) -> bool {
        let field = format!(r#"name="{name}" value="{code}""#);
        let rest = html.split(&field).nth(1).expect("the field");
        rest.split('>').next().unwrap().ends_with(" checked")
    }

    #[sqlx::test]
    async fn an_admin_picks_the_languages_the_site_offers(db: PgPool) {
        let admin = Admin::new(db);
        // Until someone picks, every language is offered, and English is the default.
        let html = body(admin.get(paths::LANGUAGES).await).await;
        for code in ["nb", "en", "en-gb", "zh-hant"] {
            assert!(ticked(&html, "offer", code), "{code}: {html}");
        }
        assert!(ticked(&html, "default", "en"), "{html}");
        assert!(!ticked(&html, "default", "nb"), "{html}");
        // Each says what it falls back to, and how far it's translated.
        assert!(html.contains("<td>Bokmål → English</td>"), "{html}");
        assert!(html.contains("<td>100% ("), "{html}");
        let base = escape(&Lang::EN.messages().admin.fallback_none);
        assert!(html.contains(&base), "{html}");

        let response = admin
            .post(paths::LANGUAGES, "offer=nb&offer=nn&offer=en&default=nb")
            .await;
        assert_eq!(location(&response), "/languages?saved");
        let picked: LangSet = [Lang::NB, Lang::NN, Lang::EN].into_iter().collect();
        assert_eq!(
            languages::site(&admin.db).await.unwrap(),
            Offer::new(picked, Lang::NB)
        );
        let html = body(admin.get("/languages?saved").await).await;
        assert!(html.contains("Saved."), "{html}");
        assert!(ticked(&html, "offer", "nn"), "{html}");
        assert!(!ticked(&html, "offer", "en-gb"), "{html}");
        assert!(ticked(&html, "default", "nb"), "{html}");
    }

    #[sqlx::test]
    async fn refuses_none_offered_or_a_default_that_is_not(db: PgPool) {
        let admin = Admin::new(db);
        let none = "Offer at least one language.";
        let not_offered = "The default must be one of the languages offered.";
        for (form, error) in [
            ("default=nb", none),
            ("offer=xx&default=nb", none),
            ("offer=nb&offer=en&default=nn", not_offered),
            ("offer=nb", not_offered),
        ] {
            let response = admin.post(paths::LANGUAGES, form).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{form}"
            );
            let html = body(response).await;
            assert!(html.contains(error), "{form}: {html}");
        }
        // What was ticked stays ticked, to fix.
        let html = body(admin.post(paths::LANGUAGES, "offer=nb&default=nn").await).await;
        assert!(ticked(&html, "offer", "nb"), "{html}");
        assert!(!ticked(&html, "offer", "en"), "{html}");
        assert_eq!(
            languages::site(&admin.db).await.unwrap(),
            Offer::everything()
        );
    }
}
