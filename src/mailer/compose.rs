//! The mails, in the guest's language. Plain text only: nothing to track, nothing to
//! render remotely.

use chrono::{Duration, Utc};

use crate::{
    config::Config,
    db::{
        outbox::Kind,
        rsvp::{EmailStatus, Response, Rsvp},
    },
    i18n::Lang,
};

/// How long a confirmation link works; unconfirmed addresses are cleared after as long.
pub const CONFIRM_WITHIN: Duration = Duration::hours(48);

#[derive(Debug, Clone)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// The mail for a job, or why there's nothing to send any more: the guest may have
/// removed or changed their address since the job was queued.
pub fn compose(config: &Config, kind: Kind, rsvp: &Rsvp) -> Result<Mail, &'static str> {
    let to = rsvp.email.clone().ok_or("address removed")?;
    let needed = match kind {
        Kind::ConfirmEmail => Some(EmailStatus::Unconfirmed),
        Kind::RsvpLink | Kind::Reminder => Some(EmailStatus::Confirmed),
        // Goes to whoever holds the address, confirmed or not.
        Kind::EmailInUse => None,
    };
    if needed.is_some_and(|status| status != rsvp.email_status) {
        return Err("address no longer in the right state");
    }

    let lang = rsvp.lang;
    let title = config.event.title.get(lang);
    let base = &config.base_url;
    let rsvp_url = || {
        let link = config.link_key.rsvp_link(rsvp.id, rsvp.link_nonce);
        format!("{base}/r/{link}?lang={}", lang.code())
    };
    let name = &rsvp.name;
    let nb = lang == Lang::Nb;

    let (subject, text) = match kind {
        Kind::ConfirmEmail => {
            let link = config
                .link_key
                .email_link(rsvp.id, &to, Utc::now() + CONFIRM_WITHIN);
            let url = format!("{base}/e/{link}?lang={}", lang.code());
            if nb {
                (
                    format!("Bekreft e-postadressen din – {title}"),
                    format!(
                        "Hei, {name}!\n\nÅpne lenken og trykk «Bekreft» for å få påminnelser og \
                         lenken din på e-post:\n\n{url}\n\nLenken virker i 48 timer. Var det ikke \
                         deg, kan du se bort fra denne e-posten. Da slettes adressen automatisk."
                    ),
                )
            } else {
                (
                    format!("Confirm your e-mail address – {title}"),
                    format!(
                        "Hi, {name}!\n\nOpen the link and press \"Confirm\" to get reminders and \
                         your link by e-mail:\n\n{url}\n\nThe link works for 48 hours. If this \
                         wasn't you, ignore this e-mail and the address is deleted automatically."
                    ),
                )
            }
        }
        Kind::EmailInUse => {
            let url = format!("{base}/lost?lang={}", lang.code());
            if nb {
                (
                    format!("E-postadressen din er allerede registrert – {title}"),
                    format!(
                        "Hei!\n\nNoen, kanskje deg, prøvde å legge til denne e-postadressen på et \
                         svar til {title}, men den er allerede i bruk. Har du mistet lenken din, \
                         kan du få den tilsendt her:\n\n{url}\n\nVar det ikke deg, kan du se bort \
                         fra denne e-posten."
                    ),
                )
            } else {
                (
                    format!("Your e-mail address is already registered – {title}"),
                    format!(
                        "Hi!\n\nSomeone, perhaps you, tried to add this e-mail address to an RSVP \
                         for {title}, but it's already in use. If you've lost your link, you can \
                         have it sent to you here:\n\n{url}\n\nIf this wasn't you, ignore this \
                         e-mail."
                    ),
                )
            }
        }
        Kind::RsvpLink => {
            let url = rsvp_url();
            if nb {
                (
                    format!("Lenken din – {title}"),
                    format!(
                        "Hei, {name}!\n\nHer er din personlige lenke til {title}. Bruk den for å \
                         endre eller slette svaret ditt:\n\n{url}\n\nIkke del lenken med andre."
                    ),
                )
            } else {
                (
                    format!("Your link – {title}"),
                    format!(
                        "Hi, {name}!\n\nHere's your personal link for {title}. Use it to change \
                         or delete your answer:\n\n{url}\n\nDon't share the link with anyone."
                    ),
                )
            }
        }
        Kind::Reminder => {
            let event = &config.event;
            let t = lang.messages();
            let when = event
                .starts_at
                .format_localized(t.date_format, lang.locale());
            let answer = match rsvp.response {
                Response::Attending => {
                    format!(
                        "{} · {}",
                        t.answer_attending,
                        t.people_count(rsvp.party_size)
                    )
                }
                Response::Declined => t.answer_declined.to_owned(),
                Response::Pending => t.answer_pending.to_owned(),
            };
            let (place, url) = (&event.location, rsvp_url());
            if nb {
                (
                    format!("Påminnelse: {title}"),
                    format!(
                        "Hei, {name}!\n\nEn påminnelse om {title}: {when}, {place}.\n\nSvaret \
                         ditt: {answer}. Endre det her:\n\n{url}"
                    ),
                )
            } else {
                (
                    format!("Reminder: {title}"),
                    format!(
                        "Hi, {name}!\n\nA reminder about {title}: {when}, {place}.\n\nYour \
                         answer: {answer}. Change it here:\n\n{url}"
                    ),
                )
            }
        }
    };

    let footer = if nb {
        format!(
            "Du får denne e-posten fordi adressen ble lagt til på {base}. Du kan fjerne den fra \
             siden din når som helst."
        )
    } else {
        format!(
            "You're getting this e-mail because the address was added on {base}. You can remove \
             it from your RSVP page at any time."
        )
    };
    Ok(Mail {
        to,
        subject,
        body: format!("{text}\n\n-- \n{footer}\n"),
    })
}
