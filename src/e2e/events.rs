//! Journeys across events: the host makes and runs several side by side, each keeps its
//! guests to itself, one guest and one device can belong to several, and each goes a
//! month after it starts.

use axum::http::StatusCode;
use sqlx::PgPool;

use super::{World, admin, en, link_in};
use crate::{
    db::rsvp::Response,
    i18n::{count, filled},
};

#[sqlx::test]
async fn the_host_creates_an_event_invites_to_it_and_deletes_it(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());

    let mut host = world.admin().await;
    host.page().expect(&a.no_events);
    // The admin reads English, so the new event starts out written in English, with
    // Bokmål among the other languages.
    let form = host.click(&a.new_event).await;
    assert!(
        form.body.contains(r#"<option value="en" selected>"#),
        "{}",
        form.body
    );
    host.fill(&a.event_title, "Summer party")
        .fill(&format!("{} · Bokmål", a.event_title), "Sommerfest")
        .fill(&a.event_location, "Bygdøy")
        .fill(&a.event_starts, "2054-12-20T15:00");
    host.press(&a.create_event)
        .await
        .expect(&a.event_created)
        .expect("Summer party");

    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann");
    host.press(&a.create_invitations).await;
    let link = host.value(&filled(&a.link_for, "name", "Kari Nordmann"));

    // Kari's page is about the summer party, in her language.
    let mut kari = world.browser();
    kari.open(&link)
        .await
        .expect("Summer party")
        .expect("Bygdøy")
        .expect(&t.when);
    kari.press(&t.attend)
        .await
        .expect(&t.answer(Response::Attending, 1));

    // The host moves it; Kari's page follows.
    host.click(&a.details).await;
    host.fill(&a.event_location, "Huk");
    host.press(&a.save).await.expect(&a.saved);
    kari.open(&link).await.expect("Huk").expect_not("Bygdøy");

    // The list of events shows the headcount, and deleting takes everything with it.
    host.click(&format!("‹ {}", a.all_events))
        .await
        .expect("Summer party");
    host.click("Summer party").await;
    host.click(&a.details).await;
    host.check(&filled(&a.delete_event, "title", "Summer party"));
    host.press(&a.delete)
        .await
        .expect(&a.event_deleted)
        .expect(&a.no_events);
    let gone = kari.open(&link).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    gone.expect(&t.gone_title);
}

#[sqlx::test]
async fn one_device_remembers_invitations_to_two_events(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let christmas = world.invite("Kari Nordmann", 1).await;

    let mut host = world.admin().await;
    host.click(&a.new_event).await;
    host.fill(&a.event_title, "Sommerfest")
        .fill(&a.event_starts, "2055-06-20T15:00");
    host.press(&a.create_event).await;
    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Kari Nordmann");
    host.press(&a.create_invitations).await;
    let summer = host.value(&filled(&a.link_for, "name", "Kari Nordmann"));

    // Kari has her phone remember both invitations.
    let mut phone = world.browser();
    for link in [&christmas, &summer] {
        phone.open(link).await;
        phone.check(&t.remember_device);
        phone.press(&t.attend).await.expect(&t.saved);
    }
    assert_eq!(phone.cookie_names().len(), 2);

    // The front page is about Innby, and leads back to both, the soonest first.
    let front = phone.open(&world.site("/")).await;
    front.expect(&t.site_intro).expect(&t.your_invitations);
    let text = &front.doc.text;
    let (christmas_at, summer_at) = (
        text.find("Christmas party").unwrap(),
        text.find("Sommerfest").unwrap(),
    );
    assert!(christmas_at < summer_at, "{text}");
    phone.click(&t.to_your_page).await.expect("Christmas party");

    // Forgetting one leaves the other.
    phone.open(&world.site("/")).await;
    phone
        .press(&t.forget_here)
        .await
        .expect("Sommerfest")
        .expect_not("Christmas party");
    assert_eq!(phone.cookie_names().len(), 1);
}

#[sqlx::test]
async fn the_host_runs_two_events_side_by_side(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let summer = world.summer_party().await;
    let kari = world.invite("Kari Nordmann", 2).await;
    let per = world.invite_to(summer.id, "Per Hansen", 1).await;

    // Kari is coming to Christmas with one more; Per hasn't answered for the summer.
    let mut kari_phone = world.browser();
    kari_phone.open(&kari).await.expect("Christmas party");
    kari_phone.select(&t.how_many, &t.people_count(2));
    kari_phone.press(&t.attend).await;

    // Each event's guest list has only its own guests.
    kari_phone
        .click(&t.tab_guests)
        .await
        .expect("Kari Nordmann")
        .expect_not("Per H");
    let mut per_phone = world.browser();
    per_phone
        .open(&per)
        .await
        .expect("Summer party")
        .expect_not("Christmas party");
    per_phone
        .click(&t.tab_guests)
        .await
        .expect("Per H.")
        .expect_not("Kari");

    // The host sees both events, the soonest first, each with its own headcount,
    // guests and export.
    let mut host = world.admin().await;
    let text = &host.page().doc.text;
    assert!(
        text.find("Christmas party").unwrap() < text.find("Summer party").unwrap(),
        "{text}"
    );
    host.click("Christmas party")
        .await
        .expect(&count(2, &a.coming_one, &a.coming_other))
        .expect("Kari Nordmann")
        .expect_not("Per Hansen");
    let csv = host.download(&a.download_csv).await;
    assert!(csv.body.contains("Kari Nordmann") && !csv.body.contains("Per Hansen"));

    host.click(&format!("‹ {}", a.all_events)).await;
    host.click("Summer party")
        .await
        .expect(&count(0, &a.coming_one, &a.coming_other))
        .expect("Per Hansen")
        .expect_not("Kari Nordmann");
    let csv = host.download(&a.download_csv).await;
    assert!(csv.body.contains("Per Hansen") && !csv.body.contains("Kari Nordmann"));

    // Invitations made on one event's tab belong to it alone.
    host.click(&a.invitation_links).await;
    host.fill(&a.names, "Ola Nordmann");
    host.press(&a.create_invitations).await;
    host.click(&a.guests).await.expect("Ola Nordmann");
    host.click(&format!("‹ {}", a.all_events)).await;
    host.click("Christmas party")
        .await
        .expect_not("Ola Nordmann");
}

#[sqlx::test]
async fn one_address_belongs_to_a_guest_of_two_events(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let summer = world.summer_party().await;
    let links = [
        world.invite("Kari Nordmann", 1).await,
        world.invite_to(summer.id, "Kari Nordmann", 1).await,
    ];

    // Kari confirms the same address on both invitations.
    let mut kari = world.browser();
    for link in &links {
        kari.open(link).await;
        kari.click(&t.tab_you).await;
        kari.fill(&t.email_label, "kari@example.com")
            .check(t.email_consent.trim());
        kari.press(&t.email_add).await.expect(&t.email_sent_notice);
        let confirmation = world.receive("kari@example.com").await;
        kari.open(link_in(&confirmation)).await;
        kari.press(&t.confirm_button)
            .await
            .expect(&t.email_confirmed_notice);
        let mail = world.receive("kari@example.com").await;
        assert!(link_in(&mail).starts_with(link.as_str()), "{}", mail.body);
    }

    // A mislaid link brings both back, one mail each.
    let mut phone = world.browser();
    phone.open(&world.site("/lost")).await;
    phone.fill(&t.email_label, "kari@example.com");
    phone.press(&t.lost_button).await.expect(&t.lost_sent_title);
    let mut mailed: Vec<String> = Vec::new();
    for _ in &links {
        let mail = world.receive("kari@example.com").await;
        mailed.push(link_in(&mail).split('?').next().unwrap().to_owned());
    }
    mailed.sort();
    let mut expected = links.to_vec();
    expected.sort();
    assert_eq!(mailed, expected);

    // Reminders for the summer party remind her of that one only.
    let mut host = world.admin().await;
    host.click("Summer party").await;
    host.press(&a.send_reminders).await.expect(&count(
        1,
        &a.reminders_queued_one,
        &a.reminders_queued_other,
    ));
    let reminder = world.receive("kari@example.com").await;
    assert!(reminder.body.contains("Summer party"), "{}", reminder.body);
    assert!(
        !reminder.body.contains("Christmas party"),
        "{}",
        reminder.body
    );
    world.deliver_mail().await;
    assert_eq!(world.inbox("kari@example.com").len(), 7);
}

#[sqlx::test]
async fn each_event_goes_a_month_after_it_starts(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let summer = world.summer_party().await;
    let christmas = world.invite("Kari Nordmann", 1).await;
    let summer = world.invite_to(summer.id, "Kari Nordmann", 1).await;
    let mut phone = world.browser();
    for link in [&christmas, &summer] {
        phone.open(link).await;
        phone.check(&t.remember_device);
        phone.press(&t.attend).await;
    }

    // A month after Christmas, the janitor takes the Christmas party and everything in
    // it, and leaves the summer party be.
    world.run_janitor("2055-01-13T12:00:00Z").await;
    let gone = phone.open(&christmas).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    gone.expect(&t.gone_title);
    phone.open(&summer).await.expect("Summer party");

    // The front page leads back to the summer party only, and drops the other cookie.
    phone
        .open(&world.site("/"))
        .await
        .expect("Summer party")
        .expect_not("Christmas party");
    assert_eq!(phone.cookie_names().len(), 1);
    world
        .admin()
        .await
        .page()
        .expect("Summer party")
        .expect_not("Christmas party");

    // A month after midsummer, it goes too.
    world.run_janitor("2055-07-21T12:00:00Z").await;
    assert_eq!(phone.open(&summer).await.status, StatusCode::NOT_FOUND);
    world.admin().await.page().expect(&a.no_events);
}

#[sqlx::test]
async fn the_host_carries_an_event_into_next_year(db: PgPool) {
    let world = World::new(db);
    let a = admin();
    world.invite("Kari Nordmann", 2).await;

    // This year's party has a group link for the climbing club.
    let mut host = world.host().await;
    host.click(&a.group_links).await;
    host.fill(&a.names, "Climbing club, 3");
    host.press(&a.create_groups).await;
    let old_link = host.value(&filled(&a.link_for, "name", "Climbing club"));
    // Its QR codes are dots with a glass in the middle.
    host.click(&a.qr_heading).await;
    host.check(&a.qr_dots)
        .check(&a.qr_emoji)
        .fill(&a.qr_emoji, "🥂");
    host.press(&a.qr_save).await.expect(&a.qr_saved);

    // Its file has the details, the group and the QR style, versioned, and none of
    // the guests.
    host.click(&a.details).await;
    let file = host.download(&a.export_event).await;
    assert_eq!(
        file.headers["content-disposition"],
        r#"attachment; filename="julebord-2054.toml""#
    );
    let file = file.body;
    for expected in [
        "apiVersion = \"innby/v1\"",
        "kind = \"Event\"",
        "starts = \"2054-12-12T19:00\"",
        "en = \"Christmas party\"",
        "name = \"Climbing club\"",
        "shape = \"dots\"",
        "emoji = \"🥂\"",
    ] {
        assert!(file.contains(expected), "{expected} in {file}");
    }
    assert!(!file.contains("Kari"), "{file}");

    // A file of another kind, or from a later Innby, is refused by name.
    host.click(&format!("‹ {}", a.all_events)).await;
    host.click(&a.new_event).await;
    host.fill(&a.event_file, &file.replace("innby/v1", "innby/v9"));
    let version = crate::i18n::fill(
        &a.error_file_version,
        &[("found", &"innby/v9"), ("api", &"innby/v1")],
    );
    host.press(&a.read_file).await.expect(&version);

    // A file kept from long ago reads in, but its date is refused until it's changed:
    // the janitor would delete the event at once, with everything made in it.
    let past = filled(
        &a.error_event_past,
        "days",
        crate::janitor::KEEP_AFTER_EVENT.num_days(),
    );
    host.fill(
        &a.event_file,
        &file.replace("2054-12-12T19:00", "2020-12-12T19:00"),
    );
    host.press(&a.read_file).await.expect(&past);
    host.press(&a.create_event).await.expect(&past);

    // Next year's: the file, uploaded with a new date, fills in the new event's form.
    let next_year = file.replace("2054-12-12T19:00", "2055-12-11T19:00");
    host.fill(&a.event_file, "")
        .attach(&a.event_file_upload, "julebord-2054.toml", &next_year);
    host.press(&a.read_file)
        .await
        .expect(&filled(&a.imported_notice, "title", "Julebord"))
        .expect(&a.qr_carried);
    assert_eq!(host.value(&a.event_starts), "2055-12-11T19:00");
    assert_eq!(host.value(&a.event_groups), "Climbing club, 3");
    host.press(&a.create_event).await.expect(&a.event_created);

    // It has the climbing club, with a link of its own, and last year's QR codes.
    host.click(&a.qr_heading).await;
    assert!(host.page().body.contains("🥂</text>"));
    host.click(&a.group_links).await.expect("Climbing club");
    let new_link = host.value(&filled(&a.link_for, "name", "Climbing club"));
    assert_ne!(new_link, old_link);
    host.click(&format!("‹ {}", a.all_events)).await;
    let text = &host.page().doc.text;
    assert_eq!(text.matches("Christmas party").count(), 2, "{text}");

    // Copying skips the file: the form fills in from the event itself.
    host.click("Christmas party").await;
    host.click(&a.details).await;
    host.click(&a.copy_event)
        .await
        .expect(&filled(&a.copied_notice, "title", "Christmas party"))
        .expect(&a.qr_carried);
    assert_eq!(host.value(&a.event_groups), "Climbing club, 3");
}
