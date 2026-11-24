//! What the personal and group link pages show alike: the event, the party size and
//! guest list choices, the guest list itself, a guest's tabs, and a form shown again
//! with what to fix (`FormPage`).

use askama::Template;
use axum::{http::StatusCode, response::Response};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    db::{
        event::{Details, MAX_OPEN_QUESTIONS},
        rsvp::{self, NameDisplay, Response as Answer, Rsvp},
    },
    http::render_with,
    i18n::{Lang, Messages, fill},
    icons::{self, TabLink},
};

/// A page with a form on it, which can be shown again saying what to fix.
pub trait FormPage: Template + Sized {
    /// The page's texts, and where it keeps what to fix.
    fn error_slot(&mut self) -> (&'static Messages, &mut Option<&'static str>);

    /// The page again with what's wrong, picked from its texts: 422 for a form to fix,
    /// or the status that says why it was refused.
    fn render_error(
        mut self,
        status: StatusCode,
        pick: impl Fn(&'static Messages) -> &'static str,
    ) -> Response {
        let (t, error) = self.error_slot();
        *error = Some(pick(t));
        render_with(status, self)
    }
}

/// The event details shown at the top of the public pages.
pub struct EventView<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub location: &'a str,
    /// When it starts, and ends if it has a set end.
    pub when: String,
    pub starts_at_iso: String,
    pub rsvp_deadline: Option<String>,
}

impl<'a> EventView<'a> {
    pub fn new(event: &'a Details, lang: Lang) -> Self {
        Self {
            title: event.title(lang),
            description: event.description(lang),
            location: &event.location,
            when: lang.format_when(&event.starts_at, event.ends_at.as_ref()),
            // As RFC 3339 has it, for `<time datetime>`.
            starts_at_iso: event.starts_at.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            rsvp_deadline: event
                .rsvp_deadline
                .as_ref()
                .map(|deadline| lang.format_date(deadline)),
        }
    }
}

/// One choice in the "How many are coming?" list (`_party_size.html`).
pub struct PartySize {
    pub value: i32,
    pub label: String,
    pub selected: bool,
}

impl PartySize {
    /// The choices from 1 to `max`, with `selected` preselected.
    pub fn choices(t: &Messages, max: i32, selected: i32) -> Vec<Self> {
        (1..=max)
            .map(|value| Self {
                value,
                label: t.people_count(value),
                selected: value == selected,
            })
            .collect()
    }
}

/// One choice for how a name shows in the guest list (`_name_display.html`).
pub struct DisplayChoice {
    pub value: &'static str,
    pub label: String,
    pub selected: bool,
}

impl DisplayChoice {
    /// The choices, with `selected` preselected. Given the guest's name, each label shows
    /// what others would see, e.g. "First name and initial (Kari N.)".
    pub fn choices(t: &Messages, name: Option<&str>, selected: NameDisplay) -> Vec<Self> {
        NameDisplay::ALL
            .into_iter()
            .map(|display| {
                let label = t.name_display(display);
                Self {
                    value: display.code(),
                    label: match name.and_then(|name| display.apply(name)) {
                        Some(shown) => {
                            fill(&t.display_choice, &[("label", &label), ("name", &shown)])
                        }
                        None => label.to_owned(),
                    },
                    selected: display == selected,
                }
            })
            .collect()
    }
}

/// The guest list that every personal and group link shows (`_guest_list.html`), with
/// each name as its guest chose to show it.
#[derive(Default)]
pub struct GuestList {
    pub coming: Vec<ListedGuest>,
    pub pending: Vec<ListedGuest>,
    /// How many aren't listed: those coming who chose not to be, with everyone they
    /// bring, and the invitations not answered yet that chose not to be.
    pub coming_hidden: usize,
    pub pending_hidden: usize,
    /// Everyone coming, with those they bring, listed or not.
    pub coming_people: i32,
}

pub struct ListedGuest {
    pub name: String,
    /// How many a coming guest brings besides themselves, shown as "+1".
    pub plus: i32,
    /// Whether this is the guest looking at the list.
    pub you: bool,
}

impl GuestList {
    /// The event's list as the guest `you` sees it, or as someone who hasn't signed up
    /// yet.
    pub async fn load(db: &PgPool, event_id: Uuid, you: Option<Uuid>) -> sqlx::Result<Self> {
        let mut list = Self::default();
        for guest in rsvp::guest_list(db, event_id).await? {
            let (names, hidden, plus) = match guest.response {
                Answer::Attending => {
                    list.coming_people += guest.party_size;
                    (
                        &mut list.coming,
                        &mut list.coming_hidden,
                        guest.party_size - 1,
                    )
                }
                Answer::Pending | Answer::Declined => {
                    (&mut list.pending, &mut list.pending_hidden, 0)
                }
            };
            match NameDisplay::listed(guest.response, guest.name_display).apply(&guest.name) {
                Some(name) => names.push(ListedGuest {
                    name,
                    plus,
                    you: Some(guest.id) == you,
                }),
                None => *hidden += 1 + plus as usize,
            }
        }
        for names in [&mut list.coming, &mut list.pending] {
            names.sort_by_cached_key(|guest| guest.name.to_lowercase());
        }
        Ok(list)
    }

    pub fn has_coming(&self) -> bool {
        !self.coming.is_empty() || self.coming_hidden > 0
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty() || self.pending_hidden > 0
    }

    /// How many invitations have yet to be answered, listed or not.
    pub fn pending_count(&self) -> usize {
        self.pending.len() + self.pending_hidden
    }
}

/// The guest list on a page of its own. Link previews in chat apps and mail scanners
/// fetch only the link that was shared, so they never see a name; a guest gets here
/// with a button.
#[derive(Template)]
#[template(path = "guests.html")]
pub struct GuestsPage<'a> {
    pub lang: Lang,
    pub t: &'static Messages,
    pub event: EventView<'a>,
    pub guests: GuestList,
    /// For a guest: how they show up, and the other ways they could.
    pub you: Option<YourListing>,
    pub flash: Option<&'static str>,
    /// A guest's tabs; none for someone deciding whether to sign up.
    pub tabs: Vec<TabLink>,
    /// For someone deciding whether to sign up: back to the form, and what it says.
    pub back: Option<(String, &'static str)>,
}

pub struct YourListing {
    /// The guest's name as the list shows it, if it does.
    pub name: Option<String>,
    pub declined: bool,
    pub others: Vec<DisplayChoice>,
    /// Where the buttons for the others post.
    pub action: String,
}

impl YourListing {
    pub fn new(t: &Messages, rsvp: &Rsvp, action: String) -> Self {
        Self {
            name: rsvp.listed().apply(&rsvp.name),
            declined: rsvp.response == Answer::Declined,
            others: DisplayChoice::choices(t, None, rsvp.listed())
                .into_iter()
                .filter(|choice| !choice.selected)
                .collect(),
            action,
        }
    }
}

/// The host's question, as the answer and sign-up forms ask it (`_question.html`).
pub struct Question {
    pub text: String,
    pub choices: Vec<Choice>,
}

/// One choice, or none, in the question's list.
pub struct Choice {
    /// The choice's place in the list, or empty for none.
    pub value: String,
    pub label: String,
    pub selected: bool,
}

impl Question {
    /// The event's question in `lang`, with `picked` selected, if the event asks one.
    pub fn new(event: &crate::db::event::Details, lang: Lang, picked: Option<i16>) -> Option<Self> {
        let (text, choices) = event.question(lang)?;
        let none = Choice {
            value: String::new(),
            label: lang.messages().choice_none.clone(),
            selected: picked.is_none(),
        };
        let choices = choices.into_iter().enumerate().map(|(i, label)| Choice {
            value: i.to_string(),
            label: label.to_owned(),
            selected: picked == i16::try_from(i).ok(),
        });
        Some(Self {
            text: text.to_owned(),
            choices: std::iter::once(none).chain(choices).collect(),
        })
    }

    /// A pick as the form sends it: none, or a place in the event's list.
    pub fn parse(event: &crate::db::event::Details, sent: Option<&str>) -> Result<Option<i16>, ()> {
        let Some(sent) = sent.filter(|sent| !sent.is_empty()) else {
            return Ok(None);
        };
        let (_, choices) = event.question(event.lang).ok_or(())?;
        let pick: i16 = sent.parse().map_err(drop)?;
        let within = usize::try_from(pick).is_ok_and(|pick| pick < choices.len());
        if within { Ok(Some(pick)) } else { Err(()) }
    }
}

/// One of the event's open questions with the guest's answer, as the answer and
/// sign-up forms ask it (`_open_questions.html`).
pub struct OpenQuestion {
    /// The form field the answer comes back in: `answer_0` for the first, and so on.
    pub field: String,
    pub text: String,
    pub answer: String,
}

/// The longest answer to an open question.
pub const MAX_ANSWER: usize = 500;

/// The answers to the open questions as a form sends them, a field each. The forms
/// list the fields themselves rather than flatten a struct of them in, since flattening
/// has serde_urlencoded read every field as text, and a party size then won't parse.
pub type SentAnswers<'a> = [&'a str; MAX_OPEN_QUESTIONS];

impl OpenQuestion {
    /// The event's open questions in `lang`, with these answers filled in.
    pub fn all(event: &Details, lang: Lang, answers: &[String]) -> Vec<Self> {
        event
            .open_questions(lang)
            .into_iter()
            .enumerate()
            .map(|(i, text)| Self {
                field: format!("answer_{i}"),
                text: text.to_owned(),
                answer: answers.get(i).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// The answers as kept: one for each question, trimmed, with a browser's `\r\n`
    /// as `\n`, and none at all if every one is empty. A field past the last question
    /// is ignored. An answer too long, or with control characters other than line
    /// breaks and tabs, which only a crafted request sends, is refused.
    pub fn parse(event: &Details, sent: SentAnswers) -> Result<Vec<String>, ()> {
        let asked = event.own_open_questions().len();
        let answers: Vec<String> = sent
            .iter()
            .take(asked)
            .map(|answer| answer.replace("\r\n", "\n").trim().to_owned())
            .collect();
        let fits = answers.iter().all(|answer| {
            answer.chars().count() <= MAX_ANSWER
                && !answer
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        });
        if !fits {
            return Err(());
        }
        if answers.iter().all(String::is_empty) {
            return Ok(Vec::new());
        }
        Ok(answers)
    }
}

/// The tabs of a guest's pages, each a page of its own under their link.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GuestTab {
    Reply,
    Guests,
    Games,
    Board,
    You,
}

/// The guest's tabs, with Games only when the event offers any, and Board only when its
/// board is for guests (`_guest_tabs.html`).
pub fn guest_tabs(
    t: &'static Messages,
    links: &crate::paths::GuestLinks,
    games: bool,
    board: bool,
    current: GuestTab,
) -> Vec<TabLink> {
    [
        (
            GuestTab::Reply,
            &links.page,
            &t.tab_reply,
            icons::ENVELOPE,
            true,
        ),
        (
            GuestTab::Guests,
            &links.guests,
            &t.tab_guests,
            icons::PEOPLE,
            true,
        ),
        (
            GuestTab::Games,
            &links.games,
            &t.games_heading,
            icons::GAMEPAD,
            games,
        ),
        (
            GuestTab::Board,
            &links.board,
            &t.board_heading,
            icons::SPEECH,
            board,
        ),
        (GuestTab::You, &links.you, &t.tab_you, icons::PERSON, true),
    ]
    .into_iter()
    .filter(|(.., shown)| *shown)
    .map(|(tab, href, label, icon, _)| TabLink {
        href: href.clone(),
        label,
        icon,
        current: tab == current,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_answers_to_the_open_questions() {
        let mut event = crate::db::event::tests::details();
        let none = ["  ", "", "", "", ""];
        // An event that asks none keeps none, whatever is sent.
        assert_eq!(
            OpenQuestion::parse(&event, ["Hei", "", "", "", ""]),
            Ok(vec![])
        );

        event
            .open_questions
            .set(Lang::NB, "Hvem kjører?\nEn hilsen?");
        // Trimmed, with a browser's line breaks as `\n`, and a field past the last
        // question ignored.
        assert_eq!(
            OpenQuestion::parse(&event, [" Vi kjører,\r\nto plasser ", "", "Hei", "", ""]),
            Ok(vec!["Vi kjører,\nto plasser".to_owned(), String::new()])
        );
        // Nothing answered is no answers at all.
        assert_eq!(OpenQuestion::parse(&event, none), Ok(vec![]));
        let long = "x".repeat(MAX_ANSWER + 1);
        assert!(OpenQuestion::parse(&event, [&long, "", "", "", ""]).is_err());
        assert!(OpenQuestion::parse(&event, ["Vi\u{7}", "", "", "", ""]).is_err());
        // An answer as long as allowed counts line breaks once, as browsers send them
        // twice.
        let lines = format!("{}\r\n{}", "x".repeat(249), "x".repeat(250));
        assert!(OpenQuestion::parse(&event, [&lines, "", "", "", ""]).is_ok());

        // The form asks them in the guest's language, with what they answered.
        let asked = OpenQuestion::all(&event, Lang::EN, &["Vi kjører".to_owned()]);
        let shown: Vec<_> = asked
            .iter()
            .map(|q| (q.field.as_str(), q.text.as_str(), q.answer.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("answer_0", "Hvem kjører?", "Vi kjører"),
                ("answer_1", "En hilsen?", "")
            ]
        );
    }
}
