//! Regressions of the slice 4 review, on both installation shapes.

mod common;

use std::sync::{Arc, Mutex};

use ortus_calendar::{schedule, store};
use ortus_core::app::{App, AppInfo, Group};
use ortus_core::auth::accounts::Role;
use ortus_core::instance::{testing_auth::signed_in, Instance};
use ortus_core::interfaces::Mailer;
use ortus_core::runtime::block_on;
use ortus_core::storage::BoxFuture;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

#[derive(Default)]
struct Outbox(Mutex<Vec<String>>);

impl Mailer for Outbox {
    fn send_invitation<'a>(
        &'a self,
        _: &'a str,
        _: &'a [String],
        _: &'a str,
        _: &'a str,
        ics: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.0.lock().unwrap().push(ics.to_string());
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

fn with_mail(base: &Arc<Instance>) -> Arc<Instance> {
    let mut apps = ortus_calendar::apps();
    apps.push(Arc::new(FakeMail(Arc::new(Outbox::default()))));
    let instance = ortus_core::instance::testing_store::instance(Arc::clone(&base.store), apps);
    for app in instance.apps.clone() {
        app.provide(&instance, &instance.services);
    }
    instance
}

fn call<A: rust_web_server::application::Application>(
    client: &TestClient<A>,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (i16, Value) {
    let url = format!("/api/v1{path}");
    let mut r = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        _ => client.delete(&url),
    };
    r = r
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Requested-With", "ortus");
    if let Some(b) = body {
        r = r
            .header("Content-Type", "application/json")
            .body_text(&b.to_string());
    }
    let response = r.send();
    (
        response.status(),
        serde_json::from_str(response.body_text()).unwrap_or(Value::Null),
    )
}

const H: i64 = 3_600_000;

fn ics(uid: &str, organizer: Option<&str>, extra: &[&str]) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//t//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}"),
        "DTSTAMP:20261120T090000Z".into(),
        "DTSTART:20261126T140000Z".into(),
        "DTEND:20261126T143000Z".into(),
        "SUMMARY:Original".into(),
    ];
    if let Some(o) = organizer {
        lines.push(format!("ORGANIZER:mailto:{o}"));
    }
    lines.extend(extra.iter().map(|s| s.to_string()));
    lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    lines.join("\r\n") + "\r\n"
}

/// H1: an attendee who makes himself organiser of his copy cannot change
/// or cancel the real organiser's event or others' copies.
#[test]
fn an_attendee_cannot_take_over_an_event() {
    for (name, base, _keep) in common::installations() {
        let instance = with_mail(&base);
        let alice = signed_in(&instance, "alice@ministry.example", Role::Member);
        let bob = signed_in(&instance, "bob@ministry.example", Role::Member);
        let _carol = signed_in(&instance, "carol@ministry.example", Role::Member);
        let id = |e: &str| {
            block_on(ortus_core::auth::accounts::by_email(
                instance.store.as_ref(),
                e,
            ))
            .unwrap()
            .unwrap()
            .id
        };
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
        let soon = (ortus_core::program::now_ms() / H + 48) * H;
        let (_, made) = call(
            &client,
            &alice,
            "POST",
            "/calendar/events",
            Some(
                json!({"summary": "Alice's meeting", "start": soon, "end": soon + H, "attendees": ["bob@ministry.example", "carol@ministry.example"]}),
            ),
        );
        let uid = made["uid"].as_str().unwrap().to_string();
        // Bob rewrites his copy as his own (as a CalDAV app could).
        let s = instance.store.as_ref();
        let his = block_on(store::event_by_uid(s, &id("bob@ministry.example"), &uid))
            .unwrap()
            .unwrap();
        let mine = his
            .ical
            .replace("ORGANIZER;CN=", "X-WAS;CN=")
            .replace(
                "ATTENDEE;PARTSTAT=ACCEPTED;ROLE=CHAIR:mailto:alice",
                "ATTENDEE:mailto:alice",
            )
            .replace(
                "BEGIN:VEVENT\r\n",
                "BEGIN:VEVENT\r\nORGANIZER:mailto:bob@ministry.example\r\n",
            );
        block_on(store::put_event(
            s,
            &id("bob@ministry.example"),
            &his.calendar_id,
            &his.href,
            &mine,
            Some(&his.etag),
            false,
        ))
        .unwrap();
        let (status, _) = call(
            &client,
            &bob,
            "PUT",
            &format!("/calendar/events/{}", his.id),
            Some(
                json!({"summary": "Hijacked", "start": soon + 5 * H, "end": soon + 6 * H, "attendees": ["alice@ministry.example"]}),
            ),
        );
        assert_eq!(200, status, "{name}: his own copy he may change");
        let alices = block_on(store::event_by_uid(s, &id("alice@ministry.example"), &uid))
            .unwrap()
            .unwrap();
        assert_eq!(
            "Alice's meeting", alices.summary,
            "{name}: Alice's event is untouched"
        );
        let carols = block_on(store::event_by_uid(s, &id("carol@ministry.example"), &uid))
            .unwrap()
            .unwrap();
        assert_eq!("Alice's meeting", carols.summary, "{name}");
        assert!(
            !carols.ical.contains("STATUS:CANCELLED"),
            "{name}: not cancelled by Bob"
        );
    }
}

/// M1: an invitation by mail must come from its organiser; an event with
/// no organiser is not changed by mail; an older invitation does not undo
/// a newer one.
#[test]
fn invitations_by_mail_come_from_their_organiser() {
    for (name, base, _keep) in common::installations() {
        let instance = with_mail(&base);
        signed_in(&instance, "olena@ministry.example", Role::Member);
        let olena = block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            "olena@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        let s = instance.store.as_ref();
        let request = |uid: &str, organizer: &str, extra: &[&str]| {
            ics(uid, Some(organizer), extra)
                .replace("BEGIN:VEVENT", "METHOD:REQUEST\r\nBEGIN:VEVENT")
        };
        // Pretending to be the boss.
        let fake = request(
            "a@x",
            "ceo@ministry.example",
            &["ATTENDEE:mailto:olena@ministry.example"],
        );
        assert_eq!(
            Err("not_the_organizer".to_string()),
            block_on(schedule::receive(
                &instance,
                &olena,
                "mallory@evil.example",
                &fake
            ))
            .map(|_| ()),
            "{name}"
        );
        // An event of hers with no organiser is not changed by mail.
        let cal = block_on(store::calendars(s, &olena)).unwrap()[0].id.clone();
        block_on(store::put_event(
            s,
            &olena,
            &cal,
            "own.ics",
            &ics("own@x", None, &[]),
            None,
            true,
        ))
        .unwrap();
        let over = request(
            "own@x",
            "mallory@evil.example",
            &["ATTENDEE:mailto:olena@ministry.example"],
        );
        assert!(
            block_on(schedule::receive(
                &instance,
                &olena,
                "mallory@evil.example",
                &over
            ))
            .is_err(),
            "{name}"
        );
        assert_eq!(
            "Original",
            block_on(store::event_by_uid(s, &olena, "own@x"))
                .unwrap()
                .unwrap()
                .summary
        );
        // Sequence 2 then sequence 1: the second is stale.
        let v2 = request(
            "seq@x",
            "daniel@hydro.example",
            &["SEQUENCE:2", "ATTENDEE:mailto:olena@ministry.example"],
        )
        .replace("SUMMARY:Original", "SUMMARY:Newer");
        block_on(schedule::receive(
            &instance,
            &olena,
            "daniel@hydro.example",
            &v2,
        ))
        .unwrap();
        let v1 = request(
            "seq@x",
            "daniel@hydro.example",
            &["SEQUENCE:1", "ATTENDEE:mailto:olena@ministry.example"],
        )
        .replace("SUMMARY:Original", "SUMMARY:Older");
        assert_eq!(
            Err("stale".to_string()),
            block_on(schedule::receive(
                &instance,
                &olena,
                "daniel@hydro.example",
                &v1
            ))
            .map(|_| ()),
            "{name}"
        );
        assert_eq!(
            "Newer",
            block_on(store::event_by_uid(s, &olena, "seq@x"))
                .unwrap()
                .unwrap()
                .summary
        );
    }
}

/// M2: one occurrence changed or cancelled by mail leaves the series.
#[test]
fn one_occurrence_by_mail_leaves_the_series() {
    for (name, base, _keep) in common::installations() {
        let instance = with_mail(&base);
        signed_in(&instance, "olena@ministry.example", Role::Member);
        let olena = block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            "olena@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        let s = instance.store.as_ref();
        let series = ics(
            "weekly@x",
            Some("daniel@hydro.example"),
            &[
                "RRULE:FREQ=WEEKLY;COUNT=4",
                "ATTENDEE:mailto:olena@ministry.example",
            ],
        )
        .replace("BEGIN:VEVENT", "METHOD:REQUEST\r\nBEGIN:VEVENT");
        block_on(schedule::receive(
            &instance,
            &olena,
            "daniel@hydro.example",
            &series,
        ))
        .unwrap();
        // The second week moves an hour later.
        let moved = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:weekly@x\r\nRECURRENCE-ID:20261203T140000Z\r\nDTSTAMP:20261120T090000Z\r\nDTSTART:20261203T150000Z\r\nDTEND:20261203T153000Z\r\nSUMMARY:Original\r\nORGANIZER:mailto:daniel@hydro.example\r\nATTENDEE:mailto:olena@ministry.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        block_on(schedule::receive(
            &instance,
            &olena,
            "daniel@hydro.example",
            moved,
        ))
        .unwrap();
        let nov26 = 1_795_651_200_000;
        let event = block_on(store::event_by_uid(s, &olena, "weekly@x"))
            .unwrap()
            .unwrap();
        let times = store::occurrences(&event.ical, nov26, nov26 + 30 * 86_400_000);
        assert_eq!(4, times.len(), "{name}: still four: {}", event.ical);
        assert!(
            times.contains(&(
                nov26 + 7 * 86_400_000 + 15 * H,
                nov26 + 7 * 86_400_000 + 15 * H + H / 2
            )),
            "{name}: {times:?}"
        );
        // The third week is cancelled: three remain.
        let off = moved
            .replace("METHOD:REQUEST", "METHOD:CANCEL")
            .replace("20261203T140000Z", "20261210T140000Z")
            .replace("DTSTART:20261203T150000Z", "DTSTART:20261210T140000Z")
            .replace("DTEND:20261203T153000Z", "DTEND:20261210T143000Z");
        block_on(schedule::receive(
            &instance,
            &olena,
            "daniel@hydro.example",
            &off,
        ))
        .unwrap();
        let event = block_on(store::event_by_uid(s, &olena, "weekly@x"))
            .unwrap()
            .unwrap();
        let times = store::occurrences(&event.ical, nov26, nov26 + 30 * 86_400_000);
        assert_eq!(3, times.len(), "{name}: {}", event.ical);
        assert!(event.busy, "{name}: the series is not cancelled");
    }
}

/// L1, L2, L5: moving into a calendar that has the UID loses nothing;
/// control characters do not reach the text; offered times are on the
/// half hour.
#[test]
fn moves_text_and_offers_are_safe() {
    for (name, instance, _keep) in common::installations() {
        let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
        let id = block_on(ortus_core::auth::accounts::by_email(
            instance.store.as_ref(),
            "olena@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
        let soon = (ortus_core::program::now_ms() / H + 48) * H;
        let (_, made) = call(
            &client,
            &olena,
            "POST",
            "/calendar/events",
            Some(json!({"summary": "Line\rbreak\u{7}", "start": soon, "end": soon + H})),
        );
        let s = instance.store.as_ref();
        let event = block_on(store::event_by_uid(s, &id, made["uid"].as_str().unwrap()))
            .unwrap()
            .unwrap();
        assert!(
            !event.ical.contains('\u{7}') && !event.ical.contains("Line\r"),
            "{name}"
        );
        // Another calendar already holds that UID.
        let (_, other) = call(
            &client,
            &olena,
            "POST",
            "/calendar/calendars",
            Some(json!({"name": "Other"})),
        );
        let other = other["id"].as_str().unwrap().to_string();
        block_on(store::put_event(
            s,
            &id,
            &other,
            "dup.ics",
            &event.ical,
            None,
            true,
        ))
        .unwrap();
        let (status, _) = call(
            &client,
            &olena,
            "PUT",
            &format!("/calendar/events/{}", event.id),
            Some(json!({"calendar": other, "summary": "Moved", "start": soon, "end": soon + H})),
        );
        assert_eq!(409, status, "{name}");
        assert!(
            block_on(store::event(s, &event.calendar_id, &event.href))
                .unwrap()
                .is_some(),
            "{name}: still there"
        );
        let (status, _) = call(
            &client,
            &olena,
            "POST",
            "/calendar/polls",
            Some(json!({"summary": "x", "times": [soon + 15 * 60_000], "people": ["a@b.example"]})),
        );
        assert_eq!(400, status, "{name}");
    }
}

/// H2: a chunk size that would wrap around is refused at once.
#[test]
fn the_dav_listener_refuses_huge_chunks() {
    let (_, instance, _keep) = common::installations().into_iter().next().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (_tx, mut stop) = tokio::sync::watch::channel(false);
        tokio::spawn(async move { ortus_calendar::listener::serve(instance, listener, &mut stop, None).await });
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket.write_all(b"PUT /dav/calendar/x HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\nffffffffffffffff\r\n").await.unwrap();
        let mut buffer = [0u8; 16];
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), socket.read(&mut buffer)).await.expect("closed at once, not waiting for more");
        assert_eq!(0, read.unwrap_or(0), "the connection is closed");
    });
}
