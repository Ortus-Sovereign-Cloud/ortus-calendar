//! Events and contacts in the core's search index.

use ortus_core::search::{self, Entry};
use ortus_core::storage::AsyncStore;
use serde_json::json;

use crate::schedule::{split, unfold};

fn values(text: &str, names: &[&str]) -> String {
    unfold(text)
        .iter()
        .map(|l| split(l))
        .filter(|(n, _, _)| names.contains(&n.as_str()))
        .map(|(_, _, v)| {
            v.replace("\\n", " ")
                .replace("\\N", " ")
                .replace("\\,", ",")
                .replace(r"\;", ";")
                .replace("\\\\", "\\")
                .replace("mailto:", "")
                .replace("MAILTO:", "")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// An event of `user`'s (by its id) as the index keeps it.
pub async fn event(
    store: &dyn AsyncStore,
    user: &str,
    id: &str,
    text: &str,
    summary: &str,
    start: Option<i64>,
) {
    let body = values(text, &["LOCATION", "DESCRIPTION", "ATTENDEE", "ORGANIZER"]);
    let entry = Entry {
        app: "calendar",
        id,
        owner: user,
        title: summary,
        body: &body,
        meta: json!({"path": format!("/calendar?event={id}"), "start": start}),
        at: start.unwrap_or(0),
    };
    if let Err(e) = search::index(store, &entry).await {
        eprintln!("ortus: calendar: indexing an event: {e}");
    }
}

/// A contact of `user`'s.
pub async fn card(store: &dyn AsyncStore, user: &str, id: &str, text: &str, name: &str) {
    let body = values(text, &["EMAIL", "TEL", "ORG", "TITLE", "NOTE"]);
    let email = values(text, &["EMAIL"])
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let entry = Entry {
        app: "people",
        id,
        owner: user,
        title: name,
        body: &body,
        meta: json!({"path": "/people?section=contacts", "email": email}),
        at: 0,
    };
    if let Err(e) = search::index(store, &entry).await {
        eprintln!("ortus: people: indexing a contact: {e}");
    }
}

pub async fn remove(store: &dyn AsyncStore, app: &str, id: &str) {
    let _ = search::remove(store, app, id).await;
}
