//! Which languages a page is offered in. The site has a file for each of its languages,
//! but offers only those its admin picks, and an event may narrow them further:
//!
//! - `LangSet`: some of the languages, and matching a browser's wishes against them.
//! - `Offer`: the languages a page is offered in, and the one it's in when the browser
//!   asks for none of them. It's kept for the request being handled, so the language
//!   menu and the `Lang` a handler asks for both go by it.
//! - `Asked`: what a request asks for, with `?lang=` or `Accept-Language`.
//!
//! Matching is BCP 47's lookup (RFC 4647): a tag loses a part at a time from the end
//! until a language answers it, as each file's `serves` says. Should none answer, a
//! language of the same primary language does, so `en-AU` gets English (Traditional)
//! where plain English isn't offered.

use std::cell::Cell;

use axum::http::{header, request::Parts};

use super::{Lang, language};

/// Some of the site's languages.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct LangSet(u64);

impl LangSet {
    /// Every language the site has a file for.
    pub fn all() -> Self {
        Lang::all().iter().copied().collect()
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, lang: Lang) -> bool {
        self.0 & (1 << lang.0) != 0
    }

    pub fn insert(&mut self, lang: Lang) {
        self.0 |= 1 << lang.0;
    }

    /// The languages in both.
    pub fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Its languages, as a language menu lists them.
    pub fn iter(self) -> impl Iterator<Item = Lang> {
        Lang::all()
            .iter()
            .copied()
            .filter(move |lang| self.contains(*lang))
    }

    /// The languages with these codes, as the database keeps a set. A code the site has
    /// no file for any more is left out.
    pub fn from_codes<S: AsRef<str>>(codes: &[S]) -> Self {
        codes
            .iter()
            .filter_map(|code| Lang::by_code(code.as_ref()))
            .collect()
    }

    pub fn codes(self) -> Vec<String> {
        self.iter().map(|lang| lang.code().to_owned()).collect()
    }

    /// The language among these that answers a `?lang=` value or a language tag such as
    /// "nn-NO": "zh-Hant-TW" tries "zh-hant-tw", "zh-hant" and "zh".
    pub fn lookup(self, tag: &str) -> Option<Lang> {
        let mut tag = tag.trim().to_ascii_lowercase().replace('_', "-");
        loop {
            let found = self
                .iter()
                .find(|lang| language::of(*lang).serves.contains(&tag));
            if found.is_some() {
                return found;
            }
            tag.truncate(tag.rfind('-')?);
        }
    }

    /// The language among these of the same primary language as a tag, for one that
    /// `lookup` finds nothing for.
    fn related(self, tag: &str) -> Option<Lang> {
        let primary = |tag: &str| tag.split(['-', '_']).next().map(str::to_ascii_lowercase);
        let wanted = primary(tag.trim()).filter(|primary| !primary.is_empty())?;
        self.iter()
            .find(|lang| primary(lang.tag()).as_deref() == Some(&wanted))
    }

    /// The most preferred of these languages in an `Accept-Language` header.
    pub fn negotiate(self, accept_language: &str) -> Option<Lang> {
        let mut best: Option<(f32, Lang)> = None;
        for item in accept_language.split(',') {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or_default();
            let quality = parts
                .find_map(|param| param.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.parse::<f32>().ok());
            let lang = self.lookup(tag).or_else(|| self.related(tag));
            if let (Some(lang), Some(quality)) = (lang, quality)
                && quality > 0.0
                && best.is_none_or(|(best_quality, _)| quality > best_quality)
            {
                best = Some((quality, lang));
            }
        }
        best.map(|(_, lang)| lang)
    }
}

impl FromIterator<Lang> for LangSet {
    fn from_iter<I: IntoIterator<Item = Lang>>(langs: I) -> Self {
        let mut set = Self::default();
        for lang in langs {
            set.insert(lang);
        }
        set
    }
}

impl std::fmt::Debug for LangSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// The languages a page is offered in, and the one it's in for a browser that asks for
/// none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    pub languages: LangSet,
    pub default: Lang,
}

tokio::task_local! {
    static OFFER: Cell<Offer>;
}

impl Offer {
    /// Every language, and the base by default: what's offered where nobody has chosen,
    /// as on the admin's pages and outside a request.
    pub fn everything() -> Self {
        Self {
            languages: LangSet::all(),
            default: Lang::base(),
        }
    }

    /// These languages and this default, made to hold: a set without its default gets
    /// it, so there's always a language to answer in.
    pub fn new(mut languages: LangSet, default: Lang) -> Self {
        languages.insert(default);
        Self { languages, default }
    }

    /// What the request being handled is offered.
    pub fn current() -> Self {
        OFFER
            .try_with(Cell::get)
            .unwrap_or_else(|_| Self::everything())
    }

    /// Handles a request with this on offer.
    pub async fn scope<T>(self, work: impl Future<Output = T>) -> T {
        OFFER.scope(Cell::new(self), work).await
    }

    /// Narrows what the request being handled is offered to these languages of it, with
    /// `default` in any case: an event's, on its pages, once the event is known.
    /// Returns what's now offered.
    pub fn narrow(chosen: LangSet, default: Lang) -> Self {
        let site = Self::current();
        let languages = match chosen.is_empty() {
            true => site.languages,
            false => site.languages.and(chosen),
        };
        let narrowed = Self::new(languages, default);
        // Outside a request there's nothing to narrow for.
        let _ = OFFER.try_with(|offer| offer.set(narrowed));
        narrowed
    }
}

/// What a request asks for in the way of language: `?lang=`, which wins, and the
/// browser's `Accept-Language`.
#[derive(Debug, Clone, Default)]
pub struct Asked {
    query: Option<String>,
    accept: Option<String>,
}

impl Asked {
    pub fn from_parts(parts: &Parts) -> Self {
        let query = parts
            .uri
            .query()
            .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("lang=")));
        let accept = parts
            .headers
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok());
        Self {
            query: query.map(str::to_owned),
            accept: accept.map(str::to_owned),
        }
    }

    /// The language of the page, of those on offer.
    pub fn among(&self, offer: Offer) -> Lang {
        let from_query = || offer.languages.lookup(self.query.as_deref()?);
        let from_header = || offer.languages.negotiate(self.accept.as_deref()?);
        from_query().or_else(from_header).unwrap_or(offer.default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(langs: &[Lang]) -> LangSet {
        langs.iter().copied().collect()
    }

    fn asked(query: Option<&str>, accept: Option<&str>) -> Asked {
        Asked {
            query: query.map(str::to_owned),
            accept: accept.map(str::to_owned),
        }
    }

    #[test]
    fn a_set_keeps_the_menus_order_and_reads_codes() {
        let some = set(&[Lang::ZH_HANS, Lang::NB, Lang::EN]);
        assert_eq!(some.codes(), ["nb", "en", "zh-hans"]);
        assert!(some.contains(Lang::NB) && !some.contains(Lang::NN));
        // A code whose file has gone is left out.
        assert_eq!(
            LangSet::from_codes(&["nb", "xx", "en"]),
            set(&[Lang::NB, Lang::EN])
        );
        assert!(LangSet::default().is_empty());
        assert_eq!(LangSet::all().iter().count(), Lang::all().len());
    }

    #[test]
    fn picks_only_among_what_is_offered() {
        // Bokmål and plain English, as a host who wants neither variant offers.
        let offer = Offer::new(set(&[Lang::NB, Lang::EN]), Lang::NB);
        // A British browser gets plain English, the nearest that's offered.
        assert_eq!(asked(None, Some("en-GB,en;q=0.9")).among(offer), Lang::EN);
        // A language that isn't offered isn't to be had by asking, either way.
        assert_eq!(asked(Some("en-gb"), None).among(offer), Lang::EN);
        assert_eq!(asked(Some("zh-hans"), Some("zh-CN")).among(offer), Lang::NB);
        assert_eq!(
            asked(Some("zh-hans"), Some("zh-CN, en;q=0.5")).among(offer),
            Lang::EN
        );
        // `?lang=` wins over the browser, and nothing at all gets the default.
        assert_eq!(asked(Some("en"), Some("nb")).among(offer), Lang::EN);
        assert_eq!(asked(None, None).among(offer), Lang::NB);
    }

    #[test]
    fn a_related_language_answers_where_the_plain_one_is_not_offered() {
        let offer = Offer::new(set(&[Lang::NB, Lang::EN_GB]), Lang::NB);
        assert_eq!(asked(None, Some("en-AU")).among(offer), Lang::EN_GB);
        assert_eq!(asked(None, Some("en")).among(offer), Lang::EN_GB);
        assert_eq!(
            asked(None, Some("zh-SG, en;q=0.2")).among(offer),
            Lang::EN_GB
        );
        // But one that's offered and answers the tag itself comes first.
        let both = Offer::new(set(&[Lang::EN, Lang::EN_GB]), Lang::EN);
        assert_eq!(asked(None, Some("en-AU")).among(both), Lang::EN);
        assert_eq!(asked(None, Some("en-GB")).among(both), Lang::EN_GB);
    }

    #[test]
    fn an_offer_always_has_its_default() {
        let offer = Offer::new(set(&[Lang::NN]), Lang::NB);
        assert_eq!(offer.languages, set(&[Lang::NB, Lang::NN]));
        assert_eq!(Offer::everything().default, Lang::base());
    }

    #[tokio::test]
    async fn an_event_narrows_what_its_request_is_offered() {
        // Outside a request everything is offered, and narrowing changes nothing.
        assert_eq!(Offer::current(), Offer::everything());
        let site = Offer::new(set(&[Lang::NB, Lang::NN, Lang::EN]), Lang::NB);
        site.scope(async {
            assert_eq!(Offer::current(), site);
            // An event in English, offered in English and Chinese: of those the site
            // has English alone, which is also its default.
            let event = Offer::narrow(set(&[Lang::EN, Lang::ZH_HANS]), Lang::EN);
            assert_eq!(event, Offer::new(set(&[Lang::EN]), Lang::EN));
            assert_eq!(Offer::current(), event);
            // One that picks none is offered in all the site has, and always its own.
            let open = Offer::narrow(LangSet::default(), Lang::ZH_HANT);
            assert_eq!(open.languages, set(&[Lang::EN, Lang::ZH_HANT]));
        })
        .await;
    }
}
