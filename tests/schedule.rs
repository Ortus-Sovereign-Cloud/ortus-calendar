//! Scheduling on both installation shapes: colleagues get invitations in
//! their calendars and their answers reach the organiser; people outside
//! get iMIP by mail (a stand-in mailer here); invitations, answers and
//! cancellations by mail are applied only from who may send them; the
//! agenda and free times.

mod common;

use std::sync::{Arc, Mutex};

use ortus_calendar::{schedule, store};
use ortus_core::app::{App, AppInfo, Group};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, Instance};
use ortus_core::interfaces::Mailer;
use ortus_core::runtime::block_on;
use ortus_core::storage::BoxFuture;

#[derive(Default)]
struct Outbox(Mutex<Vec<(String, Vec<String>, String)>>);

impl Mailer for Outbox {
    fn send_invitation<'a>(
        &'a self,
        user: &'a str,
        to: &'a [String],
        _subject: &'a str,
        _text: &'a str,
        ics: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.0
            .lock()
            .unwrap()
            .push((user.to_string(), to.to_vec(), ics.to_string()));
        Box::pin(async { Ok(()) })
    }
}

struct FakeMail(Arc<Outbox>);

impl App for FakeMail {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "mail",
            group: Group::Communicate,
            path: "/mail",
            order: 1,
        }
    }
    fn provide(&self, _: &Arc<Instance>, services: &ortus_core::services::Services) {
        let mailer: Arc<dyn Mailer> = self.0.clone();
        services.provide("mail", mailer);
    }
}

fn event(organizer: &str, attendees: &[&str]) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//Ortus test//EN".into(),
        "BEGIN:VEVENT".into(),
        "UID:review@ministry.example".into(),
        "DTSTAMP:20261120T090000Z".into(),
        "DTSTART:20261126T140000Z".into(),
        "DTEND:20261126T143000Z".into(),
        "SUMMARY:Shelter agreement review".into(),
        format!("ORGANIZER:mailto:{organizer}"),
    ];
    for a in attendees {
        lines.push(format!(
            "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{a}"
        ));
    }
    lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    lines.join("\r\n") + "\r\n"
}

#[test]
fn invitations_answers_and_cancellations() {
    for (name, store_only, _keep) in common::installations() {
        // The same database, with a stand-in mail app providing Mailer.
        let outbox = Arc::new(Outbox::default());
        let mut apps = ortus_calendar::apps();
        apps.push(Arc::new(FakeMail(Arc::clone(&outbox))));
        let instance =
            ortus_core::instance::testing_store::instance(Arc::clone(&store_only.store), apps);
        for app in instance.apps.clone() {
            app.provide(&instance, &instance.services);
        }
        let s = instance.store.as_ref();
        let id = |email: &str| {
            signed_in(&instance, email, Role::Member);
            block_on(ortus_core::auth::accounts::by_email(s, email))
                .unwrap()
                .unwrap()
                .id
        };
        let (olena, marek) = (id("olena@ministry.example"), id("marek@ministry.example"));
        let olenas = block_on(store::calendars(s, &olena)).unwrap()[0].id.clone();

        // Olena invites Marek (here) and Anna (outside).
        let text = event(
            "olena@ministry.example",
            &["marek@ministry.example", "anna@regional.example"],
        );
        block_on(store::put_event(
            s,
            &olena,
            &olenas,
            "review.ics",
            &text,
            None,
            true,
        ))
        .unwrap();
        block_on(schedule::publish(&instance, &olena, None, &text)).unwrap();
        let mareks = block_on(store::event_by_uid(s, &marek, "review@ministry.example"))
            .unwrap()
            .expect("in Marek's calendar");
        assert!(
            mareks.ical.contains("PARTSTAT=NEEDS-ACTION") && !mareks.ical.contains("METHOD"),
            "{name}: {}",
            mareks.ical
        );
        let sent = outbox.0.lock().unwrap().clone();
        assert_eq!(1, sent.len(), "{name}: one mail, to Anna only");
        assert_eq!(vec!["anna@regional.example".to_string()], sent[0].1);
        assert!(sent[0].2.contains("METHOD:REQUEST"));

        // His copy waits for an answer, at the right time.
        let copy = block_on(schedule::copy(&instance, &marek, "review@ministry.example"))
            .unwrap()
            .expect("his copy");
        let start = 1_795_651_200_000 + 14 * 3_600_000;
        assert_eq!(
            (
                Some("NEEDS-ACTION".to_string()),
                Some(start),
                Some(start + 1_800_000)
            ),
            (copy.answer, copy.start_ms, copy.end_ms),
            "{name}"
        );
        assert!(block_on(schedule::copy(&instance, &marek, "nothing@here"))
            .unwrap()
            .is_none());
        // Marek accepts: Olena's copy shows it.
        let answered = block_on(schedule::respond(
            &instance,
            &marek,
            "review@ministry.example",
            "accepted",
        ))
        .unwrap();
        assert_eq!(Some("ACCEPTED".to_string()), answered.answer);
        let olena_copy = block_on(store::event_by_uid(s, &olena, "review@ministry.example"))
            .unwrap()
            .unwrap();
        assert!(
            olena_copy
                .ical
                .contains("PARTSTAT=ACCEPTED:mailto:marek@ministry.example"),
            "{name}: {}",
            olena_copy.ical
        );

        // Anna answers by mail; someone else cannot answer for her.
        let reply = event("olena@ministry.example", &["anna@regional.example"])
            .replace("PARTSTAT=NEEDS-ACTION;RSVP=TRUE", "PARTSTAT=TENTATIVE")
            .replace("BEGIN:VEVENT", "METHOD:REPLY\r\nBEGIN:VEVENT");
        assert!(
            block_on(schedule::receive(
                &instance,
                &olena,
                "mallory@evil.example",
                &reply
            ))
            .is_err(),
            "{name}"
        );
        block_on(schedule::receive(
            &instance,
            &olena,
            "anna@regional.example",
            &reply,
        ))
        .unwrap();
        let olena_copy = block_on(store::event_by_uid(s, &olena, "review@ministry.example"))
            .unwrap()
            .unwrap();
        assert!(
            olena_copy
                .ical
                .contains("PARTSTAT=TENTATIVE:mailto:anna@regional.example"),
            "{name}: {}",
            olena_copy.ical
        );

        // An invitation from outside lands in Marek's calendar; only its
        // organiser may change or cancel it.
        let outside = event("daniel@hydro.example", &["marek@ministry.example"])
            .replace("review@ministry.example", "briefing@hydro.example")
            .replace("BEGIN:VEVENT", "METHOD:REQUEST\r\nBEGIN:VEVENT");
        let got = block_on(schedule::receive(
            &instance,
            &marek,
            "daniel@hydro.example",
            &outside,
        ))
        .unwrap();
        assert_eq!(
            ("REQUEST", Some("NEEDS-ACTION".to_string())),
            (got.method.as_str(), got.answer.clone()),
            "{name}"
        );
        let spoofed = outside.replace("Shelter agreement review", "Moved to midnight");
        assert!(
            block_on(schedule::receive(
                &instance,
                &marek,
                "mallory@evil.example",
                &spoofed
            ))
            .is_err(),
            "{name}: not the organiser"
        );
        let cancel = outside.replace("METHOD:REQUEST", "METHOD:CANCEL");
        assert!(block_on(schedule::receive(
            &instance,
            &marek,
            "mallory@evil.example",
            &cancel
        ))
        .is_err());
        let cancelled = block_on(schedule::receive(
            &instance,
            &marek,
            "daniel@hydro.example",
            &cancel,
        ))
        .unwrap();
        assert!(cancelled.cancelled, "{name}");

        // Marek answering the outside invitation mails Daniel a REPLY.
        let before = outbox.0.lock().unwrap().len();
        let briefing = event("daniel@hydro.example", &["marek@ministry.example"])
            .replace("review@ministry.example", "brief2@hydro.example")
            .replace("BEGIN:VEVENT", "METHOD:REQUEST\r\nBEGIN:VEVENT");
        block_on(schedule::receive(
            &instance,
            &marek,
            "daniel@hydro.example",
            &briefing,
        ))
        .unwrap();
        block_on(schedule::respond(
            &instance,
            &marek,
            "brief2@hydro.example",
            "declined",
        ))
        .unwrap();
        let sent = outbox.0.lock().unwrap().clone();
        assert_eq!(before + 1, sent.len(), "{name}");
        assert!(
            sent.last().unwrap().2.contains("METHOD:REPLY")
                && sent.last().unwrap().2.contains("PARTSTAT=DECLINED"),
            "{name}"
        );

        // Agenda and free times: the review (26 Nov 14:00-14:30) is busy.
        let day = 1_795_651_200_000; // 26 Nov 2026 00:00 UTC
        let agenda = block_on(schedule::agenda(&instance, &marek, day, day + 86_400_000)).unwrap();
        assert!(
            agenda
                .iter()
                .any(|a| a.summary == "Shelter agreement review"
                    && a.answer.as_deref() == Some("ACCEPTED")),
            "{name}: {agenda:?}"
        );
        let free = block_on(schedule::free_times(
            &instance,
            &olena,
            &["marek@ministry.example".into()],
            day + 13 * 3_600_000,
            day + 16 * 3_600_000,
            30,
            10,
        ))
        .unwrap();
        let hours: Vec<f64> = free
            .iter()
            .map(|t| (t - day) as f64 / 3_600_000.0)
            .collect();
        assert_eq!(vec![13.0, 13.5, 14.5, 15.0, 15.5], hours, "{name}");

        // Olena takes Anna off: Anna is told it is cancelled.
        let fewer = event("olena@ministry.example", &["marek@ministry.example"]);
        block_on(schedule::publish(&instance, &olena, Some(&text), &fewer)).unwrap();
        assert!(
            outbox
                .0
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .2
                .contains("METHOD:CANCEL"),
            "{name}"
        );
    }
}
