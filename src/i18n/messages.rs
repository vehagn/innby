//! Every user-facing string, from `locales/{code}.toml` with the gaps filled in from the
//! language's fallbacks (see `language`), as one struct per language.

use std::{borrow::Borrow, fmt::Display};

use serde::{Deserialize, Serialize};

use super::{count, fill};
use crate::db::{
    access::EventRole,
    qr,
    rsvp::{NameDisplay, Response},
};

/// Every user-facing string. A language's file may leave any of them out, for its
/// fallbacks to fill in, but may not have others. `{name}` and the like in a text are filled in with `fill`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Messages {
    /// What the language switch is, for screen readers.
    pub languages: String,
    pub theme: String,
    pub theme_auto: String,
    pub theme_light: String,
    pub theme_dark: String,
    pub privacy_link: String,
    /// In the privacy notice, who sends the mail: a processor `INNBY_MAIL_PROCESSOR`
    /// names, or the host's own server.
    pub privacy_mail_processor: String,
    pub privacy_mail_own: String,

    // Event
    /// `chrono` format for dates and times.
    pub date_format: String,
    /// The same for a time alone, when an event ends on the day it starts.
    pub time_format: String,
    /// When an event starts and ends: `{start}`, and `{end}` as a time alone or as a date
    /// and time.
    pub when_until: String,
    pub when: String,
    pub place: String,
    pub site_intro: String,
    pub your_invitations: String,
    /// Remembered invitations to events that are over.
    pub past_invitations: String,
    pub rsvp_hint: String,

    // RSVP page
    pub greeting: String,
    /// Shown until the guest has answered: what we store and why (GDPR Art. 13/14).
    pub first_visit_notice: String,
    pub keep_link: String,
    /// The tabs of a guest's pages, and the bar's name for screen readers.
    pub tab_reply: String,
    pub tab_guests: String,
    pub tab_you: String,
    pub tabs_label: String,
    pub device_heading: String,
    /// On the page of a guest who answered before the time or place changed.
    pub changed_notice: String,
    /// The empty pick for the host's question.
    pub choice_none: String,
    pub calendar_hint: String,
    pub add_to_calendar: String,
    pub saved: String,
    pub your_answer: String,
    pub answer_pending: String,
    pub answer_attending: String,
    pub answer_declined: String,
    pub how_many: String,
    /// "{n} person".
    pub people_one: String,
    /// "{n} people".
    pub people_other: String,
    pub attend: String,
    pub decline: String,
    pub reply_by: String,
    pub answers_closed: String,
    pub name_heading: String,
    pub change_name: String,
    pub data_heading: String,
    pub data_text: String,
    pub export: String,
    pub delete_confirm: String,
    pub delete: String,
    pub save_link_title: String,
    pub save_link_text: String,

    // Remembering the guest on this device
    pub remember_device: String,
    pub remember_hint: String,
    pub remembered_text: String,
    pub forget_device: String,
    pub remembered_notice: String,
    pub forgotten_notice: String,
    /// On the front page, for a guest this device remembers.
    pub welcome_back: String,
    pub to_your_page: String,
    pub not_you: String,
    /// Forgets one invitation on this device, from the front page.
    pub forget_here: String,
    /// On each front-page invitation to an event this device remembers more than once,
    /// as when two people signed up through a group's link on one phone.
    pub remembered_twice: String,

    // Optional e-mail
    pub email_heading: String,
    pub email_intro: String,
    pub email_label: String,
    /// The consent statement next to the unticked box (GDPR Art. 7).
    pub email_consent: String,
    pub email_add: String,
    pub email_pending: String,
    pub email_pending_hint: String,
    pub email_confirmed: String,
    pub email_remove: String,
    pub email_sent_notice: String,
    pub email_removed_notice: String,
    pub email_confirmed_notice: String,
    pub invalid_email: String,
    pub consent_required: String,
    pub confirm_title: String,
    pub confirm_text: String,
    pub confirm_button: String,
    pub confirm_invalid_text: String,
    pub lost_link: String,
    pub lost_title: String,
    pub lost_text: String,
    pub lost_button: String,
    pub lost_sent_title: String,
    pub lost_sent_text: String,

    // Guest list
    pub guest_list_heading: String,
    /// The button to the guest list, which is on a page of its own.
    pub see_guest_list: String,
    pub back_to_sign_up: String,
    pub guest_list_coming: String,
    pub guest_list_pending: String,
    /// "+ {n} more", for guests who'd rather not be listed.
    pub guest_list_more: String,
    /// "{n} invitation", for how many have yet to reply.
    pub invitations_one: String,
    /// "{n} invitations".
    pub invitations_other: String,
    pub guest_list_empty: String,
    pub guest_list_note: String,
    /// After your own name on the guest list: "(you)".
    pub you: String,
    // Minigames on the guest's page
    pub games_heading: String,
    pub games_hint: String,
    pub play: String,
    /// In a high score table, for a guest who isn't on the guest list.
    pub a_guest: String,
    pub no_scores: String,
    pub col_player: String,
    pub col_score: String,
    /// Between whole seconds and hundredths: "12.34".
    pub decimal_mark: String,
    pub seconds: String,
    pub listed_as: String,
    pub listed_hidden: String,
    pub listed_declined: String,
    pub show_instead: String,
    pub name_display_label: String,
    pub display_full: String,
    pub display_partial: String,
    pub display_hidden: String,
    /// A choice with what others would see: "{label} ({name})".
    pub display_choice: String,

    // Group sign-up
    pub join_group: String,
    pub join_heading: String,
    /// What we store and why (GDPR Art. 13), shown where the data is collected.
    pub join_notice: String,
    pub your_name: String,
    pub join_button: String,

    // Errors and outcomes
    pub invalid_party_size: String,
    pub invalid_choice: String,
    pub invalid_name: String,
    pub confirm_delete: String,
    pub gone_title: String,
    pub gone_text: String,
    pub deleted_title: String,
    pub deleted_text: String,
    pub to_home: String,
    pub too_many_attempts: String,
    pub not_found_title: String,
    pub not_found_text: String,

    pub admin: AdminMessages,
    pub mail: Mails,
}

/// The host's admin pages. Pairs ending in `_one` and `_other` are for one of something
/// and for any other number, picked with `count`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMessages {
    /// After "Innby" in the header and the page titles.
    pub admin: String,
    pub log_out: String,
    pub log_in: String,
    pub log_in_text: String,
    pub logged_out: String,
    pub user_name: String,
    pub password: String,
    pub wrong_login: String,
    pub too_many_logins: String,
    pub log_in_with: String,
    pub log_in_or: String,
    pub login_expired: String,
    pub login_cancelled: String,
    pub login_failed: String,
    pub no_role: String,
    pub forbidden: String,
    pub not_found: String,
    pub saved: String,
    pub events: String,
    /// What screen readers call the trail above a page's tabs.
    pub trail: String,
    pub no_events: String,
    pub new_event: String,
    pub create_event: String,
    pub event_created: String,
    pub event_deleted: String,
    pub details: String,
    /// The language an event is written in, chosen on its form.
    pub event_language: String,
    pub event_language_hint: String,
    pub event_title: String,
    pub event_description: String,
    pub event_location: String,
    pub event_starts: String,
    /// What a screen reader calls the time field beside a date's, as "Starts: time".
    pub time_of: String,
    /// How to write a time: on the 24-hour clock, though am and pm are understood.
    pub time_hint: String,
    pub event_ends: String,
    pub event_ends_hint: String,
    /// Under the Details tab's form: who's told of a new time or place.
    pub event_change_hint: String,
    pub event_question: String,
    pub event_choices: String,
    pub event_question_hint: String,
    pub event_time_zone: String,
    pub event_deadline: String,
    pub event_deadline_hint: String,
    pub event_languages: String,
    pub event_languages_hint: String,
    /// The languages an event's pages are offered in, ticked in its form.
    pub event_offered: String,
    pub event_offered_hint: String,
    pub event_retention_hint: String,
    pub delete_event: String,
    pub error_event_title: String,
    pub error_event_text: String,
    pub error_event_control: String,
    pub error_event_time: String,
    pub error_event_deadline: String,
    pub error_event_ends: String,
    pub error_event_question: String,
    pub error_event_past: String,
    pub error_time_zone: String,
    pub col_event: String,
    pub col_when: String,
    pub event_groups: String,
    pub event_groups_hint: String,
    pub import_event: String,
    pub import_hint: String,
    pub event_file: String,
    pub event_file_upload: String,
    pub read_file: String,
    pub imported_notice: String,
    pub copied_notice: String,
    pub export_event: String,
    pub export_hint: String,
    pub copy_event: String,
    pub error_event_groups: String,
    pub error_event_file: String,
    pub error_file_language: String,
    pub error_file_too_large: String,
    pub error_file_not_text: String,
    pub error_no_file: String,
    pub error_file_unversioned: String,
    pub error_file_version: String,
    pub error_file_kind: String,

    // The headcount
    pub coming_one: String,
    pub coming_other: String,
    pub parties_one: String,
    pub parties_other: String,
    pub not_coming: String,
    pub no_answer: String,

    // Invitations
    pub new_invitations: String,
    pub new_invitations_hint: String,
    pub names: String,
    /// Example lines in the empty box, one per line.
    pub names_placeholder: String,
    pub max_per_invitation: String,
    pub create_invitations: String,
    pub all_invitation_links: String,
    pub invitation_links: String,
    pub invitation_links_hint: String,
    pub no_invitations: String,
    pub invitations_created_one: String,
    pub invitations_created_other: String,

    // Group links
    pub group_links: String,
    pub group_links_hint: String,
    pub closed: String,
    pub no_groups: String,
    /// The label of a group's link field, for screen readers.
    pub link_for: String,
    /// A link to a guest's or group's page, beside its field. Browsers offer to copy a
    /// link that's held or right-clicked, which a button can't do without JavaScript.
    pub open_link: String,
    /// The same in a table, for screen readers; it starts with `open_link`.
    pub open_link_for: String,
    /// How to copy a link with `open_link`.
    pub copy_link_hint: String,
    pub new_groups: String,
    /// Example lines in the empty box, one per line.
    pub groups_placeholder: String,
    pub max_per_sign_up: String,
    pub create_groups: String,
    pub groups_created_one: String,
    pub groups_created_other: String,
    pub sign_up_link: String,
    pub group_link_hint_one: String,
    pub group_link_hint_other: String,
    pub group_link: String,
    pub group_new_link_hint: String,
    pub no_sign_ups: String,
    pub group_name: String,
    pub max_for_new_sign_ups: String,
    pub delete_group: String,
    pub group_in_use_hint: String,

    // QR codes
    pub qr_heading: String,
    pub qr_hint: String,
    pub qr_shape: String,
    pub qr_square: String,
    pub qr_rounded: String,
    pub qr_dots: String,
    pub qr_color: String,
    /// What goes in the middle of the code.
    pub qr_icon: String,
    pub qr_none: String,
    pub qr_heart: String,
    pub qr_star: String,
    pub qr_emoji: String,
    pub qr_save: String,
    pub qr_saved: String,
    pub qr_carried: String,
    /// Under the example code on the index page.
    pub qr_preview: String,
    pub qr_download: String,

    // Minigames
    pub games: String,
    pub games_hint: String,
    pub games_none: String,
    pub save_games: String,

    // Access: roles for one event
    pub access: String,
    pub access_hint: String,
    pub no_access: String,
    pub col_role: String,
    pub role_viewer: String,
    pub role_editor: String,
    pub roles_hint: String,
    pub give_access: String,
    pub remove_access: String,
    pub error_user_name: String,
    pub no_events_given: String,

    // Users: password users an admin makes
    pub users: String,
    pub users_hint: String,
    pub no_users: String,
    pub new_user: String,
    pub create_user: String,
    pub role_admin: String,
    pub role_creator: String,
    pub role_none: String,
    pub users_roles_hint: String,
    pub password_hint: String,
    pub new_password: String,
    pub new_password_hint: String,
    pub set_password: String,
    pub save_role: String,
    pub delete_user: String,
    pub users_config: String,
    pub users_config_hint: String,
    pub error_role: String,
    pub error_password: String,
    pub error_new_user_name: String,
    pub error_user_taken: String,
    pub error_yourself: String,

    // Languages: which of them the site offers guests, for admins
    pub languages: String,
    pub languages_hint: String,
    pub languages_default_hint: String,
    pub languages_translated_hint: String,
    pub col_language: String,
    pub col_offered: String,
    pub col_default: String,
    pub col_fallback: String,
    pub col_translated: String,
    /// What the base language falls back to.
    pub fallback_none: String,
    /// How far a language is translated: `{percent}`, `{own}` texts of `{total}`.
    pub translated_share: String,
    /// What screen readers call a language's tick and its default button.
    pub offer_language: String,
    pub default_language: String,
    pub error_no_languages: String,
    pub error_default_language: String,

    // Guests
    pub guests: String,
    /// Under the guests' heading when the table shows one answer only.
    pub show_all: String,
    /// When the table shows one answer and nobody has given it.
    pub no_guests_shown: String,
    pub no_guests: String,
    pub download_csv: String,
    pub link: String,
    pub guest_link: String,
    pub guest_link_hint: String,
    pub new_link: String,
    pub new_link_hint: String,
    pub edit: String,
    pub name_or_household: String,
    pub max_people: String,
    pub save: String,
    pub delete: String,
    pub delete_guest: String,

    // Reminders
    pub reminders: String,
    pub subscribers_one: String,
    pub subscribers_other: String,
    pub reminders_hint: String,
    pub send_reminders: String,
    pub reminders_queued_one: String,
    pub reminders_queued_other: String,
    pub change_notices_one: String,
    pub change_notices_other: String,
    /// After the tally of picks for the host's question.
    pub tally_hint: String,

    // Table columns and facts, also the CSV's header
    pub col_name: String,
    pub col_answer: String,
    pub col_people: String,
    pub col_max_people: String,
    pub col_source: String,
    pub col_guest_list: String,
    pub col_email: String,
    pub col_answered: String,
    pub col_created: String,
    pub col_group: String,
    pub col_signed_up: String,
    pub col_people_each: String,
    pub col_link: String,

    // A guest's row
    pub answer_pending: String,
    pub answer_attending: String,
    pub answer_declined: String,
    pub source_invitation: String,
    /// A sign-up through a group's link.
    pub source_group: String,
    pub email_unconfirmed: String,
    pub email_confirmed: String,
    pub listed_full: String,
    pub listed_partial: String,
    pub listed_hidden: String,

    // Errors
    pub error_party_size: String,
    pub error_name: String,
    pub error_group_name: String,
    /// A batch line that needs fixing: "Line {n}: {error}".
    pub error_line: String,
    pub error_no_names: String,
    pub error_confirm_delete: String,
    pub error_group_in_use: String,
    pub error_qr_color: String,
    pub error_qr_emoji: String,
}

impl AdminMessages {
    /// Takes references too, as templates pass them.
    pub fn qr_shape_name(&self, shape: impl Borrow<qr::Shape>) -> &str {
        match shape.borrow() {
            qr::Shape::Square => &self.qr_square,
            qr::Shape::Rounded => &self.qr_rounded,
            qr::Shape::Dots => &self.qr_dots,
        }
    }

    pub fn event_role_name(&self, role: impl Borrow<EventRole>) -> &str {
        match role.borrow() {
            EventRole::Viewer => &self.role_viewer,
            EventRole::Editor => &self.role_editor,
        }
    }

    pub fn qr_icon_name(&self, icon: impl Borrow<qr::Icon>) -> &str {
        match icon.borrow() {
            qr::Icon::None => &self.qr_none,
            qr::Icon::Heart => &self.qr_heart,
            qr::Icon::Star => &self.qr_star,
            qr::Icon::Emoji => &self.qr_emoji,
        }
    }
}

/// The mails, in plain text.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mails {
    /// Under every mail.
    pub footer: String,
    /// After a reminder's text, when the event's calendar file is attached.
    pub calendar_attached: String,
    pub confirm_email: MailText,
    pub email_in_use: MailText,
    pub rsvp_link: MailText,
    pub reminder: MailText,
    /// The event's start, end or place changed.
    pub event_changed: MailText,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailText {
    pub subject: String,
    pub body: String,
}

impl Messages {
    /// "1 person", "3 people".
    pub fn people_count(&self, n: i32) -> String {
        let text = if n == 1 {
            &self.people_one
        } else {
            &self.people_other
        };
        fill(text, &[("n", &n)])
    }

    /// "1 invitation", "3 invitations".
    pub fn invitation_count(&self, n: impl Display) -> String {
        count(n, &self.invitations_one, &self.invitations_other)
    }

    /// "+ 3 more".
    pub fn more_guests(&self, n: impl Display) -> String {
        fill(&self.guest_list_more, &[("n", &n)])
    }

    /// The label for a guest list choice.
    pub fn name_display(&self, display: NameDisplay) -> &str {
        match display {
            NameDisplay::Full => &self.display_full,
            NameDisplay::Partial => &self.display_partial,
            NameDisplay::Hidden => &self.display_hidden,
        }
    }

    /// A guest's answer, e.g. "Coming · 2 people".
    pub fn answer(&self, response: Response, party_size: i32) -> String {
        match response {
            Response::Pending => self.answer_pending.clone(),
            Response::Attending => {
                format!(
                    "{} · {}",
                    self.answer_attending,
                    self.people_count(party_size)
                )
            }
            Response::Declined => self.answer_declined.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;
    use crate::i18n::{Lang, bold_count};

    /// The `{…}` placeholders in a text.
    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(key, _)| key))
            .collect()
    }

    /// Every text of `lang` next to the base language's, as `(key, base, text)`.
    fn next_to_base(lang: Lang) -> Vec<(String, String, String)> {
        fn walk(path: String, nb: &Value, other: &Value, out: &mut Vec<(String, String, String)>) {
            match (nb, other) {
                (Value::Object(nb), Value::Object(other)) => {
                    for (key, nb) in nb {
                        walk(format!("{path}{key}."), nb, &other[key], out);
                    }
                }
                (Value::String(nb), Value::String(other)) => {
                    out.push((path, nb.clone(), other.clone()));
                }
                mismatch => panic!("{path}: {mismatch:?}"),
            }
        }
        let [nb, other] =
            [Lang::base(), lang].map(|lang| serde_json::to_value(lang.messages()).unwrap());
        let mut out = Vec::new();
        walk(String::new(), &nb, &other, &mut out);
        out
    }

    #[test]
    fn languages_use_the_same_placeholders() {
        for &lang in Lang::all() {
            let texts = next_to_base(lang);
            assert!(texts.len() > 90, "{}", texts.len());
            for (key, base, text) in texts {
                assert_eq!(placeholders(&base), placeholders(&text), "{lang:?} {key}");
            }
        }
    }

    #[test]
    fn counts_people() {
        let [nb, nn, en, us, zh] =
            [Lang::NB, Lang::NN, Lang::EN, Lang::EN_US, Lang::ZH_HANS].map(Lang::messages);
        let gb = Lang::EN_GB.messages();
        assert_eq!(en.people_count(1), "1 person");
        assert_eq!(gb.people_count(1), "1 guest");
        assert_eq!(nb.people_count(3), "3 personer");
        assert_eq!(nn.people_count(3), "3 personar");
        assert_eq!(us.people_count(2), "2 people");
        assert_eq!(zh.answer(Response::Attending, 3), "参加 · 3 人");
        assert_eq!(en.answer(Response::Attending, 2), "Coming · 2 people");
        assert_eq!(gb.answer(Response::Attending, 2), "Attending · 2 guests");
        assert_eq!(us.answer(Response::Attending, 2), "Going · 2 people");
        assert_eq!(nb.answer(Response::Declined, 2), "Kommer ikke");
        assert_eq!(en.more_guests(3), "+ 3 more");
        assert_eq!(en.invitation_count(1), "1 invitation");
        assert_eq!(nn.invitation_count(4), "4 invitasjonar");
        assert_eq!(nb.more_guests(1), "+ 1 til");
        assert_eq!(
            count(1, &us.admin.coming_one, &us.admin.coming_other),
            "1 person going"
        );
        assert_eq!(
            bold_count(3, &nb.admin.coming_one, &nb.admin.coming_other),
            "<strong>3</strong> personer kommer"
        );
    }
}
