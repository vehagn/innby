use uuid::Uuid;

use super::*;
use crate::db::event::tests::details;

fn event(details: crate::db::event::Details) -> Event {
    Event {
        id: Uuid::nil(),
        details,
        changed_at: None,
    }
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2054, 11, 1, 12, 0, 0).unwrap()
}

#[test]
fn holds_the_event_alone_in_utc() {
    let mut party = details();
    party
        .description
        .set(Lang::EN, "Welcome!\nDinner at 8; dress: smart, please.");
    let file = ics(&event(party), Lang::EN, "fest.example.com", now());
    let lines: Vec<_> = file.split("\r\n").collect();
    assert_eq!(
        lines,
        [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "PRODID:-//Innby//Innby//EN",
            "CALSCALE:GREGORIAN",
            "METHOD:PUBLISH",
            "BEGIN:VEVENT",
            "UID:00000000-0000-0000-0000-000000000000@fest.example.com",
            "DTSTAMP:20541101T120000Z",
            // 19:00 in Oslo, and with no set end, three hours later.
            "DTSTART:20541212T180000Z",
            "DTEND:20541212T210000Z",
            "SUMMARY:Christmas party",
            r"LOCATION:Storgata 1\, Oslo",
            r"DESCRIPTION:Welcome!\nDinner at 8\; dress: smart\, please.",
            "END:VEVENT",
            "END:VCALENDAR",
            "",
        ]
    );
    // In Bokmål, the event's own title, and its end when it has one.
    let mut details = details();
    details.ends_at = Some(
        chrono_tz::Europe::Oslo
            .with_ymd_and_hms(2054, 12, 13, 1, 0, 0)
            .unwrap(),
    );
    let file = ics(&event(details), Lang::NB, "fest.example.com", now());
    assert!(file.contains("\r\nSUMMARY:Julebord\r\n"), "{file}");
    assert!(file.contains("\r\nDTEND:20541213T000000Z\r\n"), "{file}");
}

#[test]
fn folds_long_lines_between_characters() {
    let line = format!("DESCRIPTION:{}", "ø".repeat(60));
    let folded = fold(&line);
    assert!(folded.ends_with("\r\n"));
    let parts: Vec<_> = folded.trim_end().split("\r\n").collect();
    assert!(parts.len() > 1, "{folded}");
    assert!(parts.iter().all(|part| part.len() <= 75), "{parts:?}");
    assert!(parts[1..].iter().all(|part| part.starts_with(' ')));
    let unfolded: String = parts.join("\r\n").replace("\r\n ", "");
    assert_eq!(unfolded, line);
    assert_eq!(fold("SUMMARY:Fest"), "SUMMARY:Fest\r\n");
}

#[test]
fn finds_the_sites_host() {
    assert_eq!(host("https://fest.example.com"), "fest.example.com");
    assert_eq!(host("http://localhost:8080/sub"), "localhost:8080");
}
