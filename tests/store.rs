//! The calendar store on both installation shapes: the default calendar,
//! events put and replaced with ETags, UIDs unique per calendar, sync
//! changes, occurrences in a range, another person's calendar refused.

mod common;

use ortus_calendar::store::{self, PutError};
use ortus_core::runtime::block_on;

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:UIDX\r\nDTSTAMP:20261101T000000Z\r\nDTSTART:20261119T140000Z\r\nDTEND:20261119T143000Z\r\nSUMMARY:SUMMARYX\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn event(uid: &str, summary: &str) -> String {
    EVENT.replace("UIDX", uid).replace("SUMMARYX", summary)
}

#[test]
fn calendars_and_events() {
    for (name, instance, _keep) in common::installations() {
        let s = instance.store.as_ref();
        let mine = block_on(store::calendars(s, "u_olena")).unwrap();
        assert_eq!(1, mine.len(), "{name}");
        assert_eq!("Calendar", mine[0].name);
        let cal = mine[0].id.clone();
        let theirs = block_on(store::calendars(s, "u_marek")).unwrap()[0]
            .id
            .clone();

        let put = block_on(store::put_event(
            s,
            "u_olena",
            &cal,
            "review.ics",
            &event("review@ministry.example", "Shelter review"),
            None,
            true,
        ))
        .unwrap();
        assert!(put.created, "{name}");
        assert_eq!(
            Err(PutError::Precondition),
            block_on(store::put_event(
                s,
                "u_olena",
                &cal,
                "review.ics",
                &event("review@ministry.example", "x"),
                None,
                true
            )),
            "{name}: If-None-Match on an existing one"
        );
        assert_eq!(
            Err(PutError::Precondition),
            block_on(store::put_event(
                s,
                "u_olena",
                &cal,
                "review.ics",
                &event("review@ministry.example", "x"),
                Some("\"stale\""),
                false
            ))
        );
        let changed = block_on(store::put_event(
            s,
            "u_olena",
            &cal,
            "review.ics",
            &event("review@ministry.example", "Shelter review with legal"),
            Some(&put.etag),
            false,
        ))
        .unwrap();
        assert!(!changed.created && changed.etag != put.etag, "{name}");
        assert_eq!(
            Err(PutError::UidTaken),
            block_on(store::put_event(
                s,
                "u_olena",
                &cal,
                "other.ics",
                &event("review@ministry.example", "x"),
                None,
                false
            )),
            "{name}: one UID per calendar"
        );
        assert_eq!(
            Err(PutError::NotFound),
            block_on(store::put_event(
                s,
                "u_olena",
                &theirs,
                "x.ics",
                &event("x", "x"),
                None,
                false
            )),
            "{name}: not her calendar"
        );
        assert!(matches!(
            block_on(store::put_event(
                s,
                "u_olena",
                &cal,
                "../x.ics",
                &event("y", "y"),
                None,
                false
            )),
            Err(PutError::Invalid(_))
        ));
        assert!(matches!(
            block_on(store::put_event(
                s,
                "u_olena",
                &cal,
                "bad.ics",
                "not a calendar",
                None,
                false
            )),
            Err(PutError::Invalid(_))
        ));

        let got = block_on(store::event(s, &cal, "review.ics"))
            .unwrap()
            .unwrap();
        assert_eq!("Shelter review with legal", got.summary);
        assert_eq!(got.etag, changed.etag);

        // Sync: what changed after a token.
        let token = block_on(store::calendar(s, "u_olena", &cal))
            .unwrap()
            .unwrap()
            .sync;
        block_on(store::put_event(
            s,
            "u_olena",
            &cal,
            "standup.ics",
            &event("standup@ministry.example", "Standup"),
            None,
            true,
        ))
        .unwrap();
        assert!(block_on(store::delete_event(s, "u_olena", &cal, "review.ics", None)).unwrap());
        let (changes, next) = block_on(store::changes_since(s, &cal, token))
            .unwrap()
            .unwrap();
        assert_eq!(
            vec![
                ("review.ics".to_string(), true),
                ("standup.ics".to_string(), false)
            ],
            changes,
            "{name}"
        );
        assert_eq!(
            Some((vec![], next)),
            block_on(store::changes_since(s, &cal, next)).unwrap()
        );
        assert_eq!(
            None,
            block_on(store::changes_since(s, &cal, next + 10)).unwrap(),
            "{name}: an unknown token"
        );

        // Occurrences in a range: a weekly one, and the single one.
        let weekly = event("w@ministry.example", "Weekly").replace(
            "DTEND:20261119T143000Z",
            "DTEND:20261119T143000Z\r\nRRULE:FREQ=WEEKLY",
        );
        block_on(store::put_event(
            s,
            "u_olena",
            &cal,
            "weekly.ics",
            &weekly,
            None,
            true,
        ))
        .unwrap();
        let day = 86_400_000;
        let from = 1_795_132_800_000; // 20 Nov 2026 00:00 UTC
        let found = block_on(store::occurrences_in(
            s,
            std::slice::from_ref(&cal),
            from,
            from + 14 * day,
        ))
        .unwrap();
        let summaries: Vec<(&str, i64)> = found
            .iter()
            .map(|o| (o.summary.as_str(), (o.start - from) / day))
            .collect();
        assert_eq!(
            vec![("Weekly", 6), ("Weekly", 13)],
            summaries,
            "{name}: {found:?}"
        );
        let thursday = block_on(store::occurrences_in(
            s,
            std::slice::from_ref(&cal),
            from - day,
            from,
        ))
        .unwrap();
        assert_eq!(
            2,
            thursday.len(),
            "{name}: the standup and the weekly on the 19th: {thursday:?}"
        );

        assert!(
            !block_on(store::delete_calendar(s, "u_olena", &cal)).unwrap(),
            "{name}: not the last calendar"
        );
        let second = block_on(store::create_calendar(s, "u_olena", "Flood", "teal"))
            .unwrap()
            .unwrap();
        assert!(
            block_on(store::create_calendar(s, "u_olena", "Flood", "teal"))
                .unwrap()
                .is_none()
        );
        assert!(block_on(store::delete_calendar(s, "u_olena", &second.id)).unwrap());
    }
}
