use axum::http::{StatusCode, header};
use chrono::TimeZone;
use sqlx::PgPool;
use uuid::Uuid;

use super::*;
use crate::{
    db::{event::tests::details, rsvp},
    web::testing::{Site, body, en, escape},
};

fn event(details: crate::db::event::Details) -> Event {
    Event {
        id: Uuid::nil(),
        details,
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
        .set(Lang::EnGb, "Welcome!\nDinner at 8; dress: smart, please.");
    let file = ics(&event(party), Lang::EnGb, "fest.example.com", now());
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
    let file = ics(&event(details), Lang::Nb, "fest.example.com", now());
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

#[sqlx::test]
async fn a_guest_who_is_coming_can_add_the_event_to_their_calendar(db: PgPool) {
    let site = Site::new(db);
    let (invite, link) = site.invite("Kari Nordmann", 2).await;
    let t = en();
    let page = format!("/r/{link}?lang=en");
    let calendar = format!("/r/{link}/event.ics?lang=en");

    // Not before they say they're coming.
    let html = body(site.get(&page).await).await;
    assert!(!html.contains(&escape(&t.add_to_calendar)));

    rsvp::respond(
        &site.db,
        invite.id,
        rsvp::Response::Attending,
        2,
        None,
        Lang::EnGb,
        crate::web::PRIVACY_VERSION,
    )
    .await
    .unwrap();
    let html = body(site.get(&page).await).await;
    assert!(html.contains(&escape(&t.add_to_calendar)), "{html}");
    assert!(html.contains(&format!(r#"href="{calendar}""#)), "{html}");

    let response = site.get(&calendar).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "text/calendar; charset=utf-8"
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let file = body(response).await;
    assert!(file.contains("\r\nSUMMARY:Christmas party\r\n"), "{file}");
    // Nothing of the guest's.
    assert!(!file.contains("Kari") && !file.contains(&link), "{file}");

    // A link that doesn't work gets nothing.
    let response = site.get("/r/wrong-code/event.ics").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
