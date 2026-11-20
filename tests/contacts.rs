//! The contacts store on both installation shapes: the default address
//! book, cards put and replaced with ETags, one UID per book, sync changes,
//! another person's book refused, and the directory of colleagues.

mod common;

use ortus_calendar::contacts::{self, PutError};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing_auth::signed_in;
use ortus_core::runtime::block_on;

fn card(uid: &str, name: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:4.0\r\nUID:{uid}\r\nFN:{name}\r\nEMAIL:{}@regional.example\r\nEND:VCARD\r\n", name.split(' ').next().unwrap_or("x").to_lowercase())
}

#[test]
fn address_books_and_cards() {
    for (name, instance, _keep) in common::installations() {
        let s = instance.store.as_ref();
        let book = block_on(contacts::books(s, "u_olena")).unwrap();
        assert_eq!(
            ("Contacts", 1),
            (book[0].name.as_str(), book.len()),
            "{name}"
        );
        let book = book[0].id.clone();
        let theirs = block_on(contacts::books(s, "u_marek")).unwrap()[0]
            .id
            .clone();

        let put = block_on(contacts::put_card(
            s,
            "u_olena",
            &book,
            "anna.vcf",
            &card("anna-1", "Anna Kowalska"),
            None,
            true,
        ))
        .unwrap();
        assert!(put.created, "{name}");
        assert_eq!(
            Err(PutError::Precondition),
            block_on(contacts::put_card(
                s,
                "u_olena",
                &book,
                "anna.vcf",
                &card("anna-1", "Anna K"),
                Some("\"old\""),
                false
            ))
        );
        block_on(contacts::put_card(
            s,
            "u_olena",
            &book,
            "anna.vcf",
            &card("anna-1", "Anna Kowalska-Nowak"),
            Some(&put.etag),
            false,
        ))
        .unwrap();
        assert_eq!(
            Err(PutError::UidTaken),
            block_on(contacts::put_card(
                s,
                "u_olena",
                &book,
                "other.vcf",
                &card("anna-1", "X"),
                None,
                false
            ))
        );
        assert_eq!(
            Err(PutError::NotFound),
            block_on(contacts::put_card(
                s,
                "u_olena",
                &theirs,
                "x.vcf",
                &card("x", "X"),
                None,
                false
            )),
            "{name}: not her book"
        );
        let got = block_on(contacts::card(s, &book, "anna.vcf"))
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                "Anna Kowalska-Nowak",
                vec!["anna@regional.example".to_string()]
            ),
            (got.name.as_str(), got.emails.clone()),
            "{name}"
        );

        let token = block_on(contacts::book(s, "u_olena", &book))
            .unwrap()
            .unwrap()
            .sync;
        block_on(contacts::put_card(
            s,
            "u_olena",
            &book,
            "daniel.vcf",
            &card("daniel-1", "Daniel Price"),
            None,
            true,
        ))
        .unwrap();
        assert!(block_on(contacts::delete_card(s, "u_olena", &book, "anna.vcf", None)).unwrap());
        let (changes, _) = block_on(contacts::changes_since(s, &book, token))
            .unwrap()
            .unwrap();
        assert_eq!(
            vec![
                ("anna.vcf".to_string(), true),
                ("daniel.vcf".to_string(), false)
            ],
            changes,
            "{name}"
        );

        signed_in(&instance, "marek@ministry.example", Role::Member);
        signed_in(&instance, "olena@ministry.example", Role::Member);
        let found = block_on(contacts::directory(s, "mar", 10)).unwrap();
        assert_eq!(
            vec!["marek@ministry.example"],
            found.iter().map(|c| c.email.as_str()).collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(2, block_on(contacts::directory(s, "", 10)).unwrap().len());
    }
}
