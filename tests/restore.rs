//! Bringing a calendar back as it was, on both shapes: a new calendar with
//! its events as they were, under new UIDs (so the copies never stand in
//! for the events they came from); only the person's calendars are offered.

mod common;

use ortus_calendar::store;
use ortus_core::auth::accounts::Role;
use ortus_core::files::Files;
use ortus_core::instance::testing_auth::signed_in;
use ortus_core::points::{self, Snapshot};
use ortus_core::runtime::block_on;

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ortus//EN\r\nBEGIN:VEVENT\r\nUID:shelter@ministry.example\r\nDTSTAMP:20261117T090000Z\r\nDTSTART:20261126T140000Z\r\nDTEND:20261126T143000Z\r\nSUMMARY:Shelter walk-through\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn a_calendar_comes_back_as_a_new_calendar() {
    for (name, instance, _keep) in common::installations() {
        let s = instance.store.as_ref();
        signed_in(&instance, "olena@ministry.example", Role::Member);
        signed_in(&instance, "marek@ministry.example", Role::Member);
        let id = |e: &str| {
            block_on(ortus_core::auth::accounts::by_email(s, e))
                .unwrap()
                .unwrap()
                .id
        };
        let (olena, marek) = (id("olena@ministry.example"), id("marek@ministry.example"));
        let field = block_on(store::create_calendar(s, &olena, "Field visits", "teal"))
            .unwrap()
            .unwrap();
        block_on(store::put_event(
            s, &olena, &field.id, "walk.ics", EVENT, None, true,
        ))
        .unwrap();
        let dir = common::dir();
        let dest = Files::local(&dir.0.join("backups")).unwrap();
        let at = ortus_core::program::now_ms();
        block_on(points::take(&instance, &dest, at, true)).unwrap();
        // Since then the event was removed.
        block_on(s.execute(
            "DELETE FROM cal_events WHERE calendar_id = $1",
            &[field.id.as_str().into()],
        ))
        .unwrap();

        let point = block_on(Snapshot::open(dest, &points::name_at(at))).unwrap();
        let calendar = ortus_calendar::apps().remove(0);
        let hers = block_on(calendar.restore_choices(&point, &olena, "calendar")).unwrap();
        assert!(
            hers.iter()
                .any(|(i, n)| *i == field.id && n == "Field visits"),
            "{name}: {hers:?}"
        );
        let his = block_on(calendar.restore_choices(&point, &marek, "calendar")).unwrap();
        assert!(!his.iter().any(|(i, _)| *i == field.id), "{name}");

        block_on(calendar.restore(
            &instance,
            &point,
            &olena,
            "calendar",
            &field.id,
            "Field visits (2026-11-17 18:00)",
        ))
        .unwrap();
        let made = block_on(store::calendars(s, &olena))
            .unwrap()
            .into_iter()
            .find(|c| c.name == "Field visits (2026-11-17 18:00)")
            .expect("the new calendar");
        let events = block_on(store::events(s, &made.id)).unwrap();
        assert_eq!(1, events.len(), "{name}");
        assert_eq!("Shelter walk-through", events[0].summary, "{name}");
        assert!(
            events[0].uid.starts_with("restored-")
                && events[0].uid.ends_with("shelter@ministry.example"),
            "{name}: {}",
            events[0].uid
        );
    }
}
