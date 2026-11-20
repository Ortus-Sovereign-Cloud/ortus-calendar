//! Bringing back a calendar as it was at a restore point: a new calendar
//! of the person's with its events as they were. The copies get new UIDs,
//! so they never stand in for the events they came from (scheduling looks
//! events up by UID).

use std::sync::Arc;

use ortus_core::instance::Instance;
use ortus_core::points::{text, Snapshot};

use crate::store;

/// The person's calendars at the point: (id, name).
pub async fn choices(point: &Snapshot, user: &str) -> Result<Vec<(String, String)>, String> {
    let mut list: Vec<(String, String)> = point
        .rows("cal_calendars", |r| text(r, "user_id") == user)
        .await?
        .iter()
        .map(|r| (text(r, "id").to_string(), text(r, "name").to_string()))
        .collect();
    list.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(list)
}

/// Restores calendar `id` of `user` as a new calendar `into`.
pub async fn calendar(
    instance: &Arc<Instance>,
    point: &Snapshot,
    user: &str,
    id: &str,
    into: &str,
) -> Result<String, String> {
    let events = point
        .rows("cal_events", |r| text(r, "calendar_id") == id)
        .await?;
    let s = instance.store.as_ref();
    let made = store::create_calendar(s, user, into, "accent")
        .await
        .map_err(|e| e.to_string())?
        .ok_or("a calendar of that name is there already")?;
    for e in &events {
        let uid = text(e, "uid");
        let fresh = format!("restored-{}-{uid}", ortus_core::auth::random_hex(6));
        let ical = text(e, "ical")
            .lines()
            .map(|l| {
                if l.starts_with("UID:") {
                    format!("UID:{fresh}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n");
        let href = format!("{}.ics", ortus_core::auth::random_hex(12));
        store::put_event(s, user, &made.id, &href, &ical, None, true)
            .await
            .map_err(|e| format!("{e:?}"))?;
    }
    Ok("/calendar".into())
}
