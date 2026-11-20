//! Events and contacts in the search index, on both shapes.

mod common;

use ortus_calendar::{contacts, store};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing_auth::signed_in;
use ortus_core::runtime::block_on;
use ortus_core::search::{self, Query};

#[test]
fn events_and_contacts_are_found() {
    for (name, instance, _keep) in common::installations() {
        signed_in(&instance, "olena@ministry.example", Role::Member);
        let s = instance.store.as_ref();
        let olena = block_on(ortus_core::auth::accounts::by_email(
            s,
            "olena@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        let cal = block_on(store::calendars(s, &olena)).unwrap()[0].id.clone();
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:r@x\r\nDTSTAMP:20261120T090000Z\r\nDTSTART:20261126T140000Z\r\nDTEND:20261126T143000Z\r\nSUMMARY:Shelter agreement review with legal\r\nLOCATION:Room 4\\, Park Street\r\nATTENDEE;CN=Anna:mailto:anna@regional.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        block_on(store::put_event(s, &olena, &cal, "r.ics", text, None, true)).unwrap();
        let book = block_on(contacts::books(s, &olena)).unwrap()[0].id.clone();
        block_on(contacts::put_card(s, &olena, &book, "a.vcf", "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:a\r\nFN:Anna Kowalska\r\nEMAIL:anna@regional.example\r\nORG:Regional office\r\nEND:VCARD\r\n", None, true)).unwrap();
        let find = |q: &str| -> Vec<(String, String)> {
            block_on(search::search(&instance, &olena, &Query::parse(q), 10))
                .unwrap()
                .into_iter()
                .map(|h| (h.app, h.title))
                .collect()
        };
        assert_eq!(
            vec![(
                "calendar".to_string(),
                "Shelter agreement review with legal".to_string()
            )],
            find("park"),
            "{name}: by place"
        );
        let anna = find("anna");
        assert!(
            anna.contains(&("people".into(), "Anna Kowalska".into()))
                && anna.iter().any(|(a, _)| a == "calendar"),
            "{name}: {anna:?}"
        );
        block_on(store::delete_event(s, &olena, &cal, "r.ics", None)).unwrap();
        assert!(find("park").is_empty(), "{name}: gone with the event");
        block_on(contacts::delete_card(s, &olena, &book, "a.vcf", None)).unwrap();
        assert!(find("kowalska").is_empty(), "{name}");
    }
}
