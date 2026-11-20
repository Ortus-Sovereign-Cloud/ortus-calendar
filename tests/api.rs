//! The calendar's JSON API on both installation shapes: events made,
//! listed with repeats, answered, changed and removed; colleagues'
//! calendars follow; times that work; nobody reaches another's events.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing_auth::signed_in;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

const DAY: i64 = 86_400_000;
const NOV26: i64 = 1_795_651_200_000;
const H: i64 = 3_600_000;

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
        "PATCH" => client.patch(&url),
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

#[test]
fn events_through_the_api() {
    for (name, instance, _keep) in common::installations() {
        let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
        let marek = signed_in(&instance, "marek@ministry.example", Role::Member);
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));

        let (status, made) = call(
            &client,
            &olena,
            "POST",
            "/calendar/events",
            Some(
                json!({"summary": "Plan review", "start": NOV26 + 13 * H, "end": NOV26 + 14 * H + H / 2, "attendees": ["marek@ministry.example"], "location": "Room 4; floor 2"}),
            ),
        );
        assert_eq!(200, status, "{name}: {made}");
        let id = made["id"].as_str().unwrap().to_string();
        let (_, standup) = call(
            &client,
            &olena,
            "POST",
            "/calendar/events",
            Some(
                json!({"summary": "Standup", "start": NOV26 - 3 * DAY + 9 * H, "end": NOV26 - 3 * DAY + 9 * H + H / 4, "repeat": "weekdays"}),
            ),
        );
        assert_eq!("weekdays", standup["repeat"], "{name}");

        // The week: five standups and the review.
        let (_, week) = call(
            &client,
            &olena,
            "GET",
            &format!(
                "/calendar/events?from={}&to={}",
                NOV26 - 3 * DAY,
                NOV26 + 2 * DAY
            ),
            None,
        );
        let events = week["events"].as_array().unwrap();
        assert_eq!(
            5,
            events.iter().filter(|e| e["summary"] == "Standup").count(),
            "{name}: {week}"
        );
        let review = events
            .iter()
            .find(|e| e["summary"] == "Plan review")
            .unwrap();
        assert_eq!(json!("Room 4; floor 2"), review["location"]);
        assert_eq!(json!(true), review["mine"]);

        // Marek has it, waiting for his answer; he accepts.
        let (_, his) = call(
            &client,
            &marek,
            "GET",
            &format!("/calendar/events?from={}&to={}", NOV26, NOV26 + DAY),
            None,
        );
        let invite = his["events"][0].clone();
        assert_eq!(json!("NEEDS-ACTION"), invite["answer"], "{name}: {his}");
        let (status, _) = call(
            &client,
            &marek,
            "PUT",
            &format!("/calendar/events/{}", invite["id"].as_str().unwrap()),
            Some(json!({"summary": "Mine now", "start": NOV26, "end": NOV26 + H})),
        );
        assert_eq!(403, status, "{name}: only the organiser changes it");
        let (status, answered) = call(
            &client,
            &marek,
            "POST",
            &format!("/calendar/events/{}/rsvp", invite["id"].as_str().unwrap()),
            Some(json!({"answer": "accepted"})),
        );
        assert_eq!(
            (200, json!("ACCEPTED")),
            (status, answered["answer"].clone()),
            "{name}"
        );
        let (_, theirs) = call(
            &client,
            &olena,
            "GET",
            &format!("/calendar/events/{id}"),
            None,
        );
        assert!(
            theirs["attendees"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["email"] == "marek@ministry.example" && a["answer"] == "ACCEPTED"),
            "{name}: {theirs}"
        );

        // Olena cannot see Marek's events by id.
        let (status, _) = call(
            &client,
            &olena,
            "GET",
            &format!("/calendar/events/{}", invite["id"].as_str().unwrap()),
            None,
        );
        assert_eq!(404, status);

        // Times that work 13:00-16:00 for both: not during the review.
        let (_, free) = call(
            &client,
            &olena,
            "GET",
            &format!(
                "/calendar/free?people=marek@ministry.example&from={}&to={}&minutes=30",
                NOV26 + 13 * H,
                NOV26 + 16 * H
            ),
            None,
        );
        let hours: Vec<f64> = free["times"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t.as_i64().unwrap() - NOV26) as f64 / H as f64)
            .collect();
        assert_eq!(vec![14.5, 15.0, 15.5], hours, "{name}");

        // Moved: Marek's copy moves and keeps his answer.
        let (status, moved) = call(
            &client,
            &olena,
            "PUT",
            &format!("/calendar/events/{id}"),
            Some(
                json!({"summary": "Plan review", "start": NOV26 + 15 * H, "end": NOV26 + 16 * H, "attendees": ["marek@ministry.example"]}),
            ),
        );
        assert_eq!(200, status, "{name}: {moved}");
        let (_, his) = call(
            &client,
            &marek,
            "GET",
            &format!("/calendar/events?from={}&to={}", NOV26, NOV26 + DAY),
            None,
        );
        assert_eq!(
            (json!(NOV26 + 15 * H), json!("ACCEPTED")),
            (
                his["events"][0]["start"].clone(),
                his["events"][0]["answer"].clone()
            ),
            "{name}: {his}"
        );

        // Removed: Marek's copy is cancelled and leaves his week.
        let (status, _) = call(
            &client,
            &olena,
            "DELETE",
            &format!("/calendar/events/{id}"),
            None,
        );
        assert_eq!(200, status);
        let (_, his) = call(
            &client,
            &marek,
            "GET",
            &format!("/calendar/events?from={}&to={}", NOV26, NOV26 + DAY),
            None,
        );
        assert_eq!(0, his["events"].as_array().unwrap().len(), "{name}: {his}");

        // Bad input.
        let (status, _) = call(
            &client,
            &olena,
            "POST",
            "/calendar/events",
            Some(json!({"summary": "", "start": NOV26, "end": NOV26 + H})),
        );
        assert_eq!(400, status);
        let (status, _) = call(
            &client,
            &olena,
            "GET",
            &format!("/calendar/events?from=0&to={}", 100 * DAY),
            None,
        );
        assert_eq!(400, status);
    }
}

#[test]
fn times_offered_by_mail_are_picked_once() {
    for (name, instance, _keep) in common::installations() {
        let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
        let marek = signed_in(&instance, "marek@ministry.example", Role::Member);
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));
        let soon = (ortus_core::program::now_ms() / H + 48) * H;
        // Marek is busy at the second time.
        call(
            &client,
            &marek,
            "POST",
            "/calendar/events",
            Some(json!({"summary": "Busy", "start": soon + H, "end": soon + 2 * H})),
        );
        let (status, offer) = call(
            &client,
            &olena,
            "POST",
            "/calendar/polls",
            Some(
                json!({"summary": "Shelter agreement review", "minutes": 30, "times": [soon, soon + H, soon + 2 * H], "people": ["marek@ministry.example", "anna@regional.example"]}),
            ),
        );
        assert_eq!(200, status, "{name}: {offer}");
        let token = offer["token"].as_str().unwrap().to_string();
        assert!(offer["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/calendar/public/pick/{token}")));

        // Anyone with the link sees the times still free, without signing in.
        let shown = client
            .get(&format!("/api/v1/calendar/polls/{token}"))
            .send();
        assert_eq!(200, shown.status(), "{name}");
        let shown: Value = serde_json::from_str(shown.body_text()).unwrap();
        assert_eq!(
            json!([soon, soon + 2 * H]),
            shown["times"],
            "{name}: {shown}"
        );
        assert_eq!(json!("Shelter agreement review"), shown["summary"]);

        let pick = |time: i64| {
            client
                .post(&format!("/api/v1/calendar/polls/{token}/pick"))
                .header("X-Requested-With", "ortus")
                .header("Content-Type", "application/json")
                .body_text(&json!({ "time": time }).to_string())
                .send()
                .status()
        };
        assert_eq!(409, pick(soon + H), "{name}: not a free time");
        // The outside attendee needs mail, which this program lacks: the
        // event is still made; the invitation to Anna cannot go.
        assert_eq!(200, pick(soon + 2 * H), "{name}");
        assert_eq!(409, pick(soon), "{name}: picked once");
        let (_, mine) = call(
            &client,
            &olena,
            "GET",
            &format!("/calendar/events?from={}&to={}", soon, soon + 3 * H),
            None,
        );
        assert!(
            mine["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["summary"] == "Shelter agreement review"
                    && e["start"] == json!(soon + 2 * H)),
            "{name}: {mine}"
        );
        let (_, his) = call(
            &client,
            &marek,
            "GET",
            &format!("/calendar/events?from={}&to={}", soon, soon + 3 * H),
            None,
        );
        assert!(
            his["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |e| e["summary"] == "Shelter agreement review" && e["answer"] == "NEEDS-ACTION"
                ),
            "{name}: {his}"
        );
        assert_eq!(
            404,
            client
                .get("/api/v1/calendar/polls/00000000000000000000000000000000")
                .send()
                .status()
        );
    }
}
