//! Journeys with the optional e-mail address: confirming it, a mislaid link, and
//! reminders.

use sqlx::PgPool;

use super::{World, admin, en, link_in};
use crate::i18n::{count, filled};

#[sqlx::test]
async fn a_guest_confirms_their_address_and_gets_a_mislaid_link_again(db: PgPool) {
    let world = World::new(db);
    let t = en();
    let link = world.invite("Kari Nordmann", 1).await;
    let mut kari = world.browser();
    kari.open(&link).await;

    // Kari adds her address; the confirmation mail has no name in it.
    kari.click(&t.tab_you).await;
    kari.fill(&t.email_label, "kari@example.com")
        .check(t.email_consent.trim());
    kari.press(&t.email_add)
        .await
        .expect(&t.email_sent_notice)
        .expect(&filled(&t.email_pending, "email", "kari@example.com"));
    let confirmation = world.receive("kari@example.com").await;
    assert!(!confirmation.body.contains("Kari"), "{}", confirmation.body);

    // The link shows a page, and only the button confirms, so mail scanners can't.
    kari.open(link_in(&confirmation))
        .await
        .expect(&t.confirm_title);
    kari.press(&t.confirm_button)
        .await
        .expect(&t.email_confirmed_notice)
        .expect(&filled(&t.email_confirmed, "email", "kari@example.com"));
    // Then her link comes by mail, in the language she used.
    let mail = world.receive("kari@example.com").await;
    assert_eq!(link_in(&mail), format!("{link}?lang=en"));

    // On a new phone she asks for her link from the front page.
    let mut phone = world.browser();
    phone.open(&world.site("/")).await;
    phone.click(&t.lost_link).await;
    phone.fill(&t.email_label, "kari@example.com");
    phone.press(&t.lost_button).await.expect(&t.lost_sent_title);
    let mail = world.receive("kari@example.com").await;
    phone
        .open(link_in(&mail))
        .await
        .expect(&filled(&t.greeting, "name", "Kari Nordmann"));

    // Nobody else gets anything for an address that isn't confirmed.
    phone.open(&world.site("/lost")).await;
    phone.fill(&t.email_label, "ola@example.com");
    phone.press(&t.lost_button).await.expect(&t.lost_sent_title);
    world.deliver_mail().await;
    assert!(world.inbox("ola@example.com").is_empty());
}

#[sqlx::test]
async fn the_host_sends_reminders_to_confirmed_addresses(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    for (name, address) in [("Kari", "kari@example.com"), ("Ola", "ola@example.com")] {
        let mut guest = world.browser();
        guest.open(&world.invite(name, 1).await).await;
        guest.click(&t.tab_you).await;
        guest
            .fill(&t.email_label, address)
            .check(t.email_consent.trim());
        guest.press(&t.email_add).await;
        // Kari confirms; Ola never does.
        let confirmation = world.receive(address).await;
        if name == "Kari" {
            guest.open(link_in(&confirmation)).await;
            guest.press(&t.confirm_button).await;
            world.receive(address).await;
        }
    }

    let mut host = world.host().await;
    host.page()
        .expect(&count(1, &a.subscribers_one, &a.subscribers_other));
    host.press(&a.send_reminders).await.expect(&count(
        1,
        &a.reminders_queued_one,
        &a.reminders_queued_other,
    ));
    let reminder = world.receive("kari@example.com").await;
    assert!(reminder.body.contains("Kari"), "{}", reminder.body);
    world.deliver_mail().await;
    assert_eq!(world.inbox("ola@example.com").len(), 1);

    // Each guest gets one reminder, however often the host presses.
    host.press(&a.send_reminders).await.expect(&count(
        0,
        &a.reminders_queued_one,
        &a.reminders_queued_other,
    ));
}

#[sqlx::test]
async fn a_guest_hears_when_the_time_or_place_changes(db: PgPool) {
    let world = World::new(db);
    let (t, a) = (en(), admin());
    let link = world.invite("Kari", 1).await;
    let mut kari = world.browser();
    kari.open(&link).await;
    kari.click(&t.tab_you).await;
    kari.fill(&t.email_label, "kari@example.com")
        .check(t.email_consent.trim());
    kari.press(&t.email_add).await;
    let confirmation = world.receive("kari@example.com").await;
    kari.open(link_in(&confirmation)).await;
    kari.press(&t.confirm_button).await;
    world.receive("kari@example.com").await;
    kari.open(&link).await;
    kari.press(&t.attend).await.expect_not(&t.changed_notice);

    // The host moves the party.
    let mut host = world.host().await;
    host.click(&a.details).await.expect(&a.event_change_hint);
    host.fill(&a.event_location, "Bygdøy");
    host.press(&a.save)
        .await
        .expect(&count(1, &a.change_notices_one, &a.change_notices_other));
    let notice = world.receive("kari@example.com").await;
    assert!(notice.subject.contains("A change to"), "{}", notice.subject);
    assert!(notice.body.contains("Bygdøy"), "{}", notice.body);
    // She's coming, so the event comes for her calendar again.
    assert!(notice.calendar.unwrap().contains("LOCATION:Bygdøy"));

    // Her page says so until she answers again.
    kari.open(&link).await.expect(&t.changed_notice);
    kari.press(&t.attend).await.expect_not(&t.changed_notice);

    // A new title alone is no change of time or place: no mail, no notice.
    host.fill(&format!("{} · Bokmål", a.event_title), "Julefest");
    host.press(&a.save).await.expect(&a.saved);
    world.deliver_mail().await;
    assert_eq!(world.inbox("kari@example.com").len(), 3);
    kari.open(&link).await.expect_not(&t.changed_notice);
}
