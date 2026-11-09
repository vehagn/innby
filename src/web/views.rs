//! What the personal and group link pages show alike: the event, the party size and
//! guest list choices, and the guest list itself.

use askama::Template;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    db::{
        event::Details,
        rsvp::{self, NameDisplay, Response as Answer, Rsvp},
    },
    i18n::{Lang, Messages, fill},
};

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
            title: event.title.get(lang),
            description: event.description.get(lang),
            location: &event.location,
            when: lang.format_when(event.starts_at, event.ends_at),
            starts_at_iso: event.starts_at.to_rfc3339(),
            rsvp_deadline: event
                .rsvp_deadline
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
        let (_, choices) = event.question(Lang::Nb).ok_or(())?;
        let pick: i16 = sent.parse().map_err(drop)?;
        let within = usize::try_from(pick).is_ok_and(|pick| pick < choices.len());
        if within { Ok(Some(pick)) } else { Err(()) }
    }
}

/// The tabs of a guest's pages, each a page of its own under their link.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GuestTab {
    Reply,
    Guests,
    Games,
    You,
}

pub struct TabLink {
    pub href: String,
    pub label: &'static str,
    /// Inline SVG, hidden from screen readers, which read the label.
    pub icon: &'static str,
    pub current: bool,
}

/// The guest's tabs, with Games only when the event offers any (`_guest_tabs.html`).
pub fn guest_tabs(
    t: &'static Messages,
    links: &crate::paths::GuestLinks,
    games: bool,
    current: GuestTab,
) -> Vec<TabLink> {
    use crate::icons;
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
