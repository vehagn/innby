use chrono::{DateTime, Locale};
use chrono_tz::Tz;
use serde::Serialize;

use crate::db::rsvp::Response;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "lang", rename_all = "lowercase")]
pub enum Lang {
    Nb,
    En,
}

impl Lang {
    pub const DEFAULT: Lang = Lang::Nb;

    pub fn code(self) -> &'static str {
        match self {
            Lang::Nb => "nb",
            Lang::En => "en",
        }
    }

    /// Parses a primary language subtag. Nynorsk and plain "no" map to bokmål.
    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_ascii_lowercase().as_str() {
            "nb" | "nn" | "no" => Some(Lang::Nb),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// Picks the most preferred supported language from an `Accept-Language` header.
    pub fn negotiate(accept_language: &str) -> Option<Self> {
        let mut best: Option<(f32, Lang)> = None;
        for item in accept_language.split(',') {
            let mut parts = item.split(';');
            let primary = parts.next().unwrap_or_default().trim().split('-').next();
            let quality = parts
                .find_map(|param| param.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.parse::<f32>().ok());
            if let (Some(lang), Some(quality)) = (primary.and_then(Lang::from_code), quality)
                && quality > 0.0
                && best.is_none_or(|(best_quality, _)| quality > best_quality)
            {
                best = Some((quality, lang));
            }
        }
        best.map(|(_, lang)| lang)
    }

    pub fn other(self) -> Lang {
        match self {
            Lang::Nb => Lang::En,
            Lang::En => Lang::Nb,
        }
    }

    pub fn messages(self) -> &'static Messages {
        match self {
            Lang::Nb => &NB,
            Lang::En => &EN,
        }
    }

    fn locale(self) -> Locale {
        match self {
            Lang::Nb => Locale::nb_NO,
            Lang::En => Locale::en_GB,
        }
    }

    /// A date and time as people write it, e.g. "lørdag 12. desember 2026 kl. 19:00".
    pub fn format_date(self, date: DateTime<Tz>) -> String {
        date.format_localized(self.messages().date_format, self.locale())
            .to_string()
    }
}

/// A value that exists in every supported language.
#[derive(Debug, Clone)]
pub struct Localized<T> {
    pub nb: T,
    pub en: T,
}

impl<T> Localized<T> {
    pub fn get(&self, lang: Lang) -> &T {
        match lang {
            Lang::Nb => &self.nb,
            Lang::En => &self.en,
        }
    }
}

/// Every user-facing string. A missing translation is a compile error.
pub struct Messages {
    /// Name of the other language, shown on the language switch.
    pub other_language: &'static str,
    pub privacy_link: &'static str,

    // Event
    /// `chrono` format for dates and times.
    pub date_format: &'static str,
    pub when: &'static str,
    pub place: &'static str,
    pub rsvp_hint: &'static str,

    // RSVP page
    pub greeting: &'static str,
    /// Shown until the guest has answered: what we store and why (GDPR Art. 13/14).
    pub first_visit_notice: &'static str,
    pub keep_link: &'static str,
    pub saved: &'static str,
    pub your_answer: &'static str,
    pub answer_pending: &'static str,
    pub answer_attending: &'static str,
    pub answer_declined: &'static str,
    pub how_many: &'static str,
    pub person: &'static str,
    pub people: &'static str,
    pub attend: &'static str,
    pub decline: &'static str,
    pub reply_by: &'static str,
    pub answers_closed: &'static str,
    pub name_heading: &'static str,
    pub change_name: &'static str,
    pub data_heading: &'static str,
    pub data_text: &'static str,
    pub export: &'static str,
    pub delete_confirm: &'static str,
    pub delete: &'static str,
    pub save_link_title: &'static str,
    pub save_link_text: &'static str,

    // Optional e-mail
    pub email_heading: &'static str,
    pub email_intro: &'static str,
    pub email_label: &'static str,
    /// The consent statement next to the unticked box (GDPR Art. 7).
    pub email_consent: &'static str,
    pub email_add: &'static str,
    pub email_pending: &'static str,
    pub email_pending_hint: &'static str,
    pub email_confirmed: &'static str,
    pub email_remove: &'static str,
    pub email_sent_notice: &'static str,
    pub email_removed_notice: &'static str,
    pub email_confirmed_notice: &'static str,
    pub invalid_email: &'static str,
    pub consent_required: &'static str,
    pub confirm_title: &'static str,
    pub confirm_text: &'static str,
    pub confirm_button: &'static str,
    pub confirm_invalid_text: &'static str,
    pub lost_link: &'static str,
    pub lost_title: &'static str,
    pub lost_text: &'static str,
    pub lost_button: &'static str,
    pub lost_sent_title: &'static str,
    pub lost_sent_text: &'static str,

    // Open RSVP
    pub join_heading: &'static str,
    /// What we store and why (GDPR Art. 13), shown where the data is collected.
    pub join_notice: &'static str,
    pub your_name: &'static str,
    pub join_button: &'static str,

    // Errors and outcomes
    pub invalid_party_size: &'static str,
    pub invalid_name: &'static str,
    pub confirm_delete: &'static str,
    pub gone_title: &'static str,
    pub gone_text: &'static str,
    pub deleted_title: &'static str,
    pub deleted_text: &'static str,
    pub to_event: &'static str,
    pub too_many_attempts: &'static str,
    pub not_found_title: &'static str,
    pub not_found_text: &'static str,
}

impl Messages {
    /// "1 person", "3 people".
    pub fn people_count(&self, n: i32) -> String {
        format!("{n} {}", if n == 1 { self.person } else { self.people })
    }

    /// A guest's answer, e.g. "Coming · 2 people".
    pub fn answer(&self, response: Response, party_size: i32) -> String {
        match response {
            Response::Pending => self.answer_pending.to_owned(),
            Response::Attending => {
                format!(
                    "{} · {}",
                    self.answer_attending,
                    self.people_count(party_size)
                )
            }
            Response::Declined => self.answer_declined.to_owned(),
        }
    }
}

static NB: Messages = Messages {
    other_language: "English",
    privacy_link: "Personvern",

    date_format: "%A %-d. %B %Y kl. %H:%M",
    when: "Når",
    place: "Hvor",
    rsvp_hint: "Har du fått en invitasjon? Bruk lenken du fikk for å svare.",

    greeting: "Hei",
    first_visit_notice: "Du er invitert! For å planlegge festen lagrer vi navnet ditt, svaret \
        ditt og hvor mange dere blir. Ikke noe mer, og alt slettes 30 dager etter festen.",
    keep_link: "Dette er din personlige side. Ta vare på lenken for å endre svaret senere, \
        og ikke del den med andre.",
    saved: "Lagret!",
    your_answer: "Ditt svar",
    answer_pending: "Ikke svart ennå",
    answer_attending: "Kommer",
    answer_declined: "Kommer ikke",
    how_many: "Hvor mange kommer?",
    person: "person",
    people: "personer",
    attend: "Jeg kommer",
    decline: "Jeg kan ikke",
    reply_by: "Svarfrist",
    answers_closed: "Svarfristen har gått ut. Ta kontakt med verten hvis noe har endret seg.",
    name_heading: "Navn",
    change_name: "Endre navn",
    data_heading: "Dine data",
    data_text: "Du kan laste ned alt vi har lagret om deg, eller slette det nå.",
    export: "Last ned (JSON)",
    delete_confirm: "Ja, slett svaret mitt og alt som er lagret om meg",
    delete: "Slett",
    save_link_title: "Ta vare på lenken din!",
    save_link_text: "Dette er den eneste måten å endre eller slette svaret ditt på. Legg til \
        et bokmerke eller kopier lenken et trygt sted.",

    email_heading: "Påminnelse på e-post",
    email_intro: "Vil du få en påminnelse og lenken din på e-post? Det er helt valgfritt.",
    email_label: "E-postadresse",
    email_consent: "Ja, send meg påminnelser og lenken min på e-post. Jeg kan fjerne \
        adressen når som helst.",
    email_add: "Legg til",
    email_pending: "Vi har sendt en bekreftelseslenke til",
    email_pending_hint: "Lenken virker i 48 timer.",
    email_confirmed: "Påminnelser sendes til",
    email_remove: "Fjern e-postadressen",
    email_sent_notice: "Sjekk innboksen din for å bekrefte adressen.",
    email_removed_notice: "E-postadressen din er fjernet.",
    email_confirmed_notice: "E-postadressen din er bekreftet. Vi sender deg lenken din.",
    invalid_email: "Skriv inn en gyldig e-postadresse.",
    consent_required: "Kryss av i boksen for å gi samtykke.",
    confirm_title: "Bekreft e-postadressen",
    confirm_text: "Trykk på knappen for å bekrefte at du vil få påminnelser og lenken din \
        på e-post.",
    confirm_button: "Bekreft",
    confirm_invalid_text: "Bekreftelseslenken har gått ut eller er ikke lenger gyldig. Legg \
        til adressen på nytt fra siden din.",
    lost_link: "Mistet lenken din?",
    lost_title: "Mistet lenken?",
    lost_text: "Har du lagt til og bekreftet e-postadressen din, sender vi lenken på nytt.",
    lost_button: "Send meg lenken",
    lost_sent_title: "Sjekk innboksen din",
    lost_sent_text: "Hvis adressen er registrert og bekreftet, har vi sendt deg lenken din.",

    join_heading: "Si at du kommer",
    join_notice: "Vi lagrer bare navnet ditt og hvor mange dere blir, for å planlegge \
        festen. Alt slettes 30 dager etter festen, og du kan slette det selv når som helst.",
    your_name: "Navnet ditt",
    join_button: "Jeg kommer!",

    invalid_party_size: "Velg et gyldig antall.",
    invalid_name: "Navnet må være mellom 1 og 100 tegn.",
    confirm_delete: "Kryss av i boksen for å bekrefte slettingen.",
    gone_title: "Denne lenken virker ikke",
    gone_text: "Den kan være skrevet feil, erstattet av en ny lenke, eller så er svaret \
        slettet. Spør verten om en ny lenke.",
    deleted_title: "Svaret ditt er slettet",
    deleted_text: "Vi har slettet navnet ditt, svaret ditt og alt annet vi hadde lagret om \
        deg. Ombestemt deg? Spør verten om en ny invitasjon.",
    to_event: "Til forsiden",
    too_many_attempts: "For mange forsøk herfra. Prøv igjen om en time.",
    not_found_title: "Siden finnes ikke",
    not_found_text: "Sjekk at lenken er riktig, eller spør verten.",
};

static EN: Messages = Messages {
    other_language: "Norsk",
    privacy_link: "Privacy",

    date_format: "%A %-d %B %Y, %H:%M",
    when: "When",
    place: "Where",
    rsvp_hint: "Got an invitation? Use the link you received to respond.",

    greeting: "Hi",
    first_visit_notice: "You're invited! To plan the party we store your name, your answer \
        and how many of you are coming. Nothing else, and it's all deleted 30 days after \
        the event.",
    keep_link: "This is your personal page. Keep the link to change your answer later, and \
        don't share it.",
    saved: "Saved!",
    your_answer: "Your answer",
    answer_pending: "Not answered yet",
    answer_attending: "Coming",
    answer_declined: "Not coming",
    how_many: "How many are coming?",
    person: "person",
    people: "people",
    attend: "I'm coming",
    decline: "I can't make it",
    reply_by: "Reply by",
    answers_closed: "The reply deadline has passed. Contact the host if something has changed.",
    name_heading: "Name",
    change_name: "Change name",
    data_heading: "Your data",
    data_text: "You can download everything we store about you, or delete it now.",
    export: "Download (JSON)",
    delete_confirm: "Yes, delete my RSVP and everything stored about me",
    delete: "Delete",
    save_link_title: "Save your link!",
    save_link_text: "It's the only way to change or delete your answer. Bookmark this page \
        or copy the link somewhere safe.",

    email_heading: "Reminders by e-mail",
    email_intro: "Want a reminder and your link by e-mail? It's entirely optional.",
    email_label: "E-mail address",
    email_consent: "Yes, e-mail me reminders and my link. I can remove my address at any \
        time.",
    email_add: "Add",
    email_pending: "We've sent a confirmation link to",
    email_pending_hint: "It works for 48 hours.",
    email_confirmed: "Reminders go to",
    email_remove: "Remove my e-mail address",
    email_sent_notice: "Check your inbox to confirm the address.",
    email_removed_notice: "Your e-mail address has been removed.",
    email_confirmed_notice: "Your e-mail address is confirmed. We'll send you your link.",
    invalid_email: "Enter a valid e-mail address.",
    consent_required: "Tick the box to give your consent.",
    confirm_title: "Confirm your e-mail address",
    confirm_text: "Press the button to confirm that you want reminders and your link by \
        e-mail.",
    confirm_button: "Confirm",
    confirm_invalid_text: "The confirmation link has expired or is no longer valid. Add your \
        address again from your RSVP page.",
    lost_link: "Lost your link?",
    lost_title: "Lost your link?",
    lost_text: "If you added and confirmed your e-mail address, we'll send your link again.",
    lost_button: "Send me my link",
    lost_sent_title: "Check your inbox",
    lost_sent_text: "If the address is registered and confirmed, we've sent you your link.",

    join_heading: "Let us know you're coming",
    join_notice: "We only store your name and how many of you are coming, to plan the \
        party. It's all deleted 30 days after the event, and you can delete it yourself at \
        any time.",
    your_name: "Your name",
    join_button: "I'm coming!",

    invalid_party_size: "Choose a valid number.",
    invalid_name: "The name must be between 1 and 100 characters.",
    confirm_delete: "Tick the box to confirm the deletion.",
    gone_title: "This link doesn't work",
    gone_text: "It may be mistyped, replaced by a new link, or the RSVP has been deleted. \
        Ask the host for a new link.",
    deleted_title: "Your RSVP has been deleted",
    deleted_text: "We've deleted your name, your answer and everything else we stored about \
        you. Changed your mind? Ask the host for a new invitation.",
    to_event: "Back to the event",
    too_many_attempts: "Too many attempts from here. Try again in an hour.",
    not_found_title: "Page not found",
    not_found_text: "Check that the link is right, or ask the host.",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_by_quality() {
        assert_eq!(Lang::negotiate("en-GB,en;q=0.9,nb;q=0.8"), Some(Lang::En));
        assert_eq!(Lang::negotiate("de-DE, nb;q=0.5, en;q=0.7"), Some(Lang::En));
        assert_eq!(Lang::negotiate("nn-NO, en;q=0.3"), Some(Lang::Nb));
    }

    #[test]
    fn ignores_unsupported_zero_and_malformed_entries() {
        assert_eq!(Lang::negotiate("de, fr;q=0.8"), None);
        assert_eq!(Lang::negotiate("en;q=0, nb;q=0.1"), Some(Lang::Nb));
        assert_eq!(Lang::negotiate("en;q=abc"), None);
        assert_eq!(Lang::negotiate(""), None);
    }

    #[test]
    fn counts_people() {
        assert_eq!(EN.people_count(1), "1 person");
        assert_eq!(NB.people_count(3), "3 personer");
        assert_eq!(EN.answer(Response::Attending, 2), "Coming · 2 people");
        assert_eq!(NB.answer(Response::Declined, 2), "Kommer ikke");
    }
}
