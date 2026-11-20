//! The calendar store: calendars per person, events kept as their
//! iCalendar text (the source of truth CalDAV serves back), with the facts
//! we search by (start, end, recurrence) beside them, an ETag per event and
//! a sync counter per calendar for sync-collection.

use calcard::common::timezone::Tz;
use calcard::icalendar::dates::TimeOrDelta;
use calcard::icalendar::{ICalendar, ICalendarComponentType, ICalendarProperty};
use ortus_core::storage::{AsyncStore, AsyncTx, Migration, StoreError, StoreResult};
use serde::Serialize;

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "calendars and events",
        sql: "CREATE TABLE cal_calendars (
        id TEXT PRIMARY KEY,
        user_id TEXT NOT NULL,
        name TEXT NOT NULL,
        color TEXT NOT NULL,
        sync BIGINT NOT NULL,
        created_at BIGINT NOT NULL,
        UNIQUE (user_id, name)
    );
    CREATE TABLE cal_events (
        id TEXT PRIMARY KEY,
        calendar_id TEXT NOT NULL REFERENCES cal_calendars (id),
        href TEXT NOT NULL,
        uid TEXT NOT NULL,
        ical TEXT NOT NULL,
        etag TEXT NOT NULL,
        summary TEXT NOT NULL,
        starts_at BIGINT,
        ends_at BIGINT,
        recurring BIGINT NOT NULL,
        all_day BIGINT NOT NULL,
        busy BIGINT NOT NULL,
        organizer TEXT,
        sync BIGINT NOT NULL,
        updated_at BIGINT NOT NULL,
        UNIQUE (calendar_id, href),
        UNIQUE (calendar_id, uid)
    );
    CREATE INDEX cal_events_range ON cal_events (calendar_id, starts_at);
    CREATE TABLE cal_changes (
        calendar_id TEXT NOT NULL,
        sync BIGINT NOT NULL,
        href TEXT NOT NULL,
        deleted BIGINT NOT NULL,
        PRIMARY KEY (calendar_id, sync)
    )",
    },
    Migration {
        version: 2,
        name: "calendars named by their apps",
        sql: "ALTER TABLE cal_calendars ADD COLUMN path TEXT;
    UPDATE cal_calendars SET path = id;
    CREATE UNIQUE INDEX cal_calendars_path ON cal_calendars (user_id, path)",
    },
    Migration {
        version: 3,
        name: "times offered by mail",
        sql: "CREATE TABLE cal_polls (
        token TEXT PRIMARY KEY,
        user_id TEXT NOT NULL,
        summary TEXT NOT NULL,
        minutes BIGINT NOT NULL,
        times TEXT NOT NULL,
        people TEXT NOT NULL,
        picked BIGINT,
        event_id TEXT,
        created_at BIGINT NOT NULL,
        expires_at BIGINT NOT NULL
    );
    CREATE INDEX cal_polls_user ON cal_polls (user_id)",
    },
];

/// Occurrences beyond this many are not expanded (a daily event for more
/// than a century).
const MAX_OCCURRENCES: usize = 50_000;
/// An event's text is at most this big.
pub const MAX_ICAL: usize = 1024 * 1024;

fn now() -> i64 {
    ortus_core::program::now_ms()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", ortus_core::auth::random_hex(12))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Calendar {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub color: String,
    pub sync: i64,
    /// Its name in CalDAV paths (`/dav/calendar/home/<path>/`).
    pub path: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Attendee {
    pub email: String,
    pub name: Option<String>,
    /// NEEDS-ACTION, ACCEPTED, DECLINED, TENTATIVE, DELEGATED
    pub partstat: String,
}

/// What we read out of an event's iCalendar text.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Facts {
    pub uid: String,
    pub summary: String,
    pub organizer: Option<String>,
    pub attendees: Vec<Attendee>,
    /// The first occurrence, Unix ms.
    pub starts_at: Option<i64>,
    /// The end of the last occurrence; `None` for one that never ends.
    pub ends_at: Option<i64>,
    pub recurring: bool,
    pub all_day: bool,
    /// Whether it makes one busy (TRANSP is not TRANSPARENT, not cancelled).
    pub busy: bool,
}

fn mailto(value: &str) -> String {
    calcard::icalendar::utils::strip_mailto_scheme(value)
        .trim()
        .to_ascii_lowercase()
}

/// Reads an event: it must be one VEVENT (with its overrides), one UID.
pub fn facts(text: &str) -> Result<Facts, String> {
    if text.len() > MAX_ICAL {
        return Err("too_big".into());
    }
    let ical = ICalendar::parse(text).map_err(|_| "not_icalendar".to_string())?;
    let events: Vec<_> = ical
        .components
        .iter()
        .filter(|c| c.component_type == ICalendarComponentType::VEvent)
        .collect();
    let main = events
        .iter()
        .find(|c| !c.is_recurrence_override())
        .or(events.first())
        .ok_or("no_event")?;
    let uid = main
        .uid()
        .filter(|u| !u.trim().is_empty() && u.len() <= 255)
        .ok_or("no_uid")?
        .to_string();
    if events.iter().any(|c| c.uid() != Some(uid.as_str())) {
        return Err("several_uids".into());
    }
    let text_of = |prop: ICalendarProperty| {
        main.property(&prop)
            .and_then(|e| e.values.first())
            .and_then(|v| v.as_text())
            .map(str::to_string)
    };
    let summary: String = text_of(ICalendarProperty::Summary)
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect();
    let organizer = text_of(ICalendarProperty::Organizer).map(|o| mailto(&o));
    let attendees = main
        .properties(&ICalendarProperty::Attendee)
        .filter_map(|e| {
            let email = mailto(e.values.first()?.as_text()?);
            let param = |name: &str| {
                e.params
                    .iter()
                    .find(|p| p.name.as_str().eq_ignore_ascii_case(name))
                    .and_then(|p| p.value.as_text())
                    .map(str::to_string)
            };
            Some(Attendee {
                email,
                name: param("CN"),
                partstat: param("PARTSTAT")
                    .unwrap_or_else(|| "NEEDS-ACTION".into())
                    .to_ascii_uppercase(),
            })
        })
        .collect();
    let transparent =
        text_of(ICalendarProperty::Transp).is_some_and(|t| t.eq_ignore_ascii_case("TRANSPARENT"));
    let cancelled =
        text_of(ICalendarProperty::Status).is_some_and(|t| t.eq_ignore_ascii_case("CANCELLED"));
    let recurring = main.is_recurrent();
    // An all-day event's DTSTART is a date (VALUE=DATE).
    let all_day = main.property(&ICalendarProperty::Dtstart).is_some_and(|e| {
        e.params.iter().any(|p| p.name.as_str().eq_ignore_ascii_case("VALUE") && p.value.as_text().is_some_and(|v| v.eq_ignore_ascii_case("DATE")))
            || matches!(e.values.first(), Some(calcard::icalendar::ICalendarValue::PartialDateTime(d)) if d.hour.is_none())
    });
    let occurrences = expand(&ical);
    let starts_at = occurrences.iter().map(|o| o.0).min();
    let ends_at = if recurring && !finite(main) {
        None
    } else {
        occurrences.iter().map(|o| o.1).max()
    };
    Ok(Facts {
        uid,
        summary,
        organizer,
        attendees,
        starts_at,
        ends_at,
        recurring,
        all_day,
        busy: !transparent && !cancelled,
    })
}

/// Whether a recurring event ends (COUNT or UNTIL).
fn finite(event: &calcard::icalendar::ICalendarComponent) -> bool {
    event
        .properties(&ICalendarProperty::Rrule)
        .all(|e| match e.values.first() {
            Some(calcard::icalendar::ICalendarValue::RecurrenceRule(rule)) => {
                rule.count.is_some() || rule.until.is_some()
            }
            _ => true,
        })
}

/// Every occurrence (start, end) in Unix ms, floating times as UTC.
fn expand(ical: &ICalendar) -> Vec<(i64, i64)> {
    ical.expand_dates(Tz::UTC, MAX_OCCURRENCES)
        .events
        .into_iter()
        .filter(|e| {
            ical.component_by_id(e.comp_id)
                .is_some_and(|c| c.component_type == ICalendarComponentType::VEvent)
        })
        .map(|e| {
            let start = e.start.timestamp().as_millisecond();
            let end = match e.end {
                TimeOrDelta::Time(t) => t.timestamp().as_millisecond(),
                TimeOrDelta::Delta(d) => start + i64::try_from(d.as_millis()).unwrap_or(0),
            };
            (start, end.max(start))
        })
        .collect()
}

/// The text with every endless RRULE ending a day after `to`, so that
/// expanding it stops near the window asked for.
fn bounded(text: &str, to: i64) -> String {
    let lines = crate::schedule::unfold(text);
    let date_only = lines.iter().any(|l| {
        let (name, params, value) = crate::schedule::split(l);
        name == "DTSTART"
            && (params
                .iter()
                .any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE"))
                || value.trim().len() == 8)
    });
    let until = to.saturating_add(86_400_000);
    let stamp = if date_only {
        crate::api::date(until)
    } else {
        crate::api::utc(until)
    };
    let out: Vec<String> = lines
        .into_iter()
        .map(|l| {
            let upper = l.to_ascii_uppercase();
            if upper.starts_with("RRULE") && !upper.contains("COUNT=") && !upper.contains("UNTIL=")
            {
                format!("{l};UNTIL={stamp}")
            } else {
                l
            }
        })
        .collect();
    crate::schedule::fold(&out)
}

/// The occurrences of an event between `from` and `to` (Unix ms).
pub fn occurrences(text: &str, from: i64, to: i64) -> Vec<(i64, i64)> {
    let text = bounded(text, to);
    let Ok(ical) = ICalendar::parse(&text) else {
        return Vec::new();
    };
    let mut all: Vec<(i64, i64)> = expand(&ical)
        .into_iter()
        .filter(|(s, e)| *s < to && (*e > from || (*e == *s && *s >= from)))
        .collect();
    all.sort();
    all
}

fn calendar_of(r: &ortus_core::storage::Row) -> Calendar {
    Calendar {
        id: r.text(0).unwrap_or_default().into(),
        user_id: r.text(1).unwrap_or_default().into(),
        name: r.text(2).unwrap_or_default().into(),
        color: r.text(3).unwrap_or_default().into(),
        sync: r.int(4).unwrap_or(0),
        path: r.text(5).or(r.text(0)).unwrap_or_default().into(),
    }
}

const CALENDAR: &str = "SELECT id, user_id, name, color, sync, path FROM cal_calendars";

/// A person's calendars; the first visit makes "Calendar".
pub async fn calendars(store: &dyn AsyncStore, user_id: &str) -> StoreResult<Vec<Calendar>> {
    let found: Vec<Calendar> = store
        .query(
            &format!("{CALENDAR} WHERE user_id = $1 ORDER BY created_at, name"),
            &[user_id.into()],
        )
        .await?
        .iter()
        .map(calendar_of)
        .collect();
    if !found.is_empty() {
        return Ok(found);
    }
    store
        .execute(
            "INSERT INTO cal_calendars (id, user_id, name, color, sync, created_at, path) VALUES ($1, $2, 'Calendar', 'accent', 1, $3, $1) ON CONFLICT (user_id, name) DO NOTHING",
            &[new_id("cal_").into(), user_id.into(), now().into()],
        )
        .await?;
    Ok(store
        .query(
            &format!("{CALENDAR} WHERE user_id = $1 ORDER BY created_at, name"),
            &[user_id.into()],
        )
        .await?
        .iter()
        .map(calendar_of)
        .collect())
}

/// One of a person's calendars.
pub async fn calendar(
    store: &dyn AsyncStore,
    user_id: &str,
    id: &str,
) -> StoreResult<Option<Calendar>> {
    Ok(store
        .query(
            &format!("{CALENDAR} WHERE id = $1 AND user_id = $2"),
            &[id.into(), user_id.into()],
        )
        .await?
        .first()
        .map(calendar_of))
}

/// One of a person's calendars by its CalDAV path name.
pub async fn calendar_at(
    store: &dyn AsyncStore,
    user_id: &str,
    path: &str,
) -> StoreResult<Option<Calendar>> {
    Ok(store
        .query(
            &format!("{CALENDAR} WHERE user_id = $1 AND path = $2"),
            &[user_id.into(), path.into()],
        )
        .await?
        .first()
        .map(calendar_of))
}

/// Whether a name may be a calendar's path segment.
pub fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 100
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !path.starts_with('.')
}

/// Makes a calendar at the CalDAV path an app chose (MKCALENDAR).
pub async fn create_calendar_at(
    store: &dyn AsyncStore,
    user_id: &str,
    path: &str,
    name: &str,
    color: &str,
) -> StoreResult<Option<Calendar>> {
    if !path_ok(path) || calendar_at(store, user_id, path).await?.is_some() {
        return Ok(None);
    }
    let Some(made) = create_calendar(store, user_id, name, color).await? else {
        return Ok(None);
    };
    store
        .execute(
            "UPDATE cal_calendars SET path = $1 WHERE id = $2",
            &[path.into(), made.id.as_str().into()],
        )
        .await?;
    calendar(store, user_id, &made.id).await
}

/// Renames or recolours a calendar.
pub async fn update_calendar(
    store: &dyn AsyncStore,
    user_id: &str,
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
) -> StoreResult<bool> {
    let mut changed = false;
    if let Some(name) = name
        .map(str::trim)
        .filter(|n| !n.is_empty() && n.chars().count() <= 100 && !n.chars().any(char::is_control))
    {
        changed |= store
            .execute(
                "UPDATE cal_calendars SET name = $1 WHERE id = $2 AND user_id = $3",
                &[name.into(), id.into(), user_id.into()],
            )
            .await?
            == 1;
    }
    if let Some(color) =
        color.filter(|c| ["accent", "teal", "orange", "lime", "red", "violet"].contains(c))
    {
        changed |= store
            .execute(
                "UPDATE cal_calendars SET color = $1 WHERE id = $2 AND user_id = $3",
                &[color.into(), id.into(), user_id.into()],
            )
            .await?
            == 1;
    }
    Ok(changed)
}

pub async fn create_calendar(
    store: &dyn AsyncStore,
    user_id: &str,
    name: &str,
    color: &str,
) -> StoreResult<Option<Calendar>> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Ok(None);
    }
    let color = if ["accent", "teal", "orange", "lime", "red", "violet"].contains(&color) {
        color
    } else {
        "accent"
    };
    let id = new_id("cal_");
    let added = store
        .execute(
            "INSERT INTO cal_calendars (id, user_id, name, color, sync, created_at, path) VALUES ($1, $2, $3, $4, 1, $5, $1) ON CONFLICT (user_id, name) DO NOTHING",
            &[id.as_str().into(), user_id.into(), name.into(), color.into(), now().into()],
        )
        .await?;
    if added == 0 {
        return Ok(None);
    }
    calendar(store, user_id, &id).await
}

/// Deletes a calendar of a person's and its events (not their last one).
pub async fn delete_calendar(store: &dyn AsyncStore, user_id: &str, id: &str) -> StoreResult<bool> {
    if calendars(store, user_id).await?.len() < 2 || calendar(store, user_id, id).await?.is_none() {
        return Ok(false);
    }
    let mut tx = store.begin().await?;
    for sql in [
        "DELETE FROM cal_changes WHERE calendar_id = $1",
        "DELETE FROM cal_events WHERE calendar_id = $1",
        "DELETE FROM cal_calendars WHERE id = $1",
    ] {
        tx.execute(sql, &[id.into()]).await?;
    }
    tx.commit().await?;
    Ok(true)
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub calendar_id: String,
    pub href: String,
    pub uid: String,
    pub ical: String,
    pub etag: String,
    pub summary: String,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
    pub recurring: bool,
    pub all_day: bool,
    pub busy: bool,
    pub organizer: Option<String>,
    pub sync: i64,
}

const EVENT: &str = "SELECT id, calendar_id, href, uid, ical, etag, summary, starts_at, ends_at, recurring, all_day, busy, organizer, sync FROM cal_events";

fn event_of(r: &ortus_core::storage::Row) -> Event {
    Event {
        id: r.text(0).unwrap_or_default().into(),
        calendar_id: r.text(1).unwrap_or_default().into(),
        href: r.text(2).unwrap_or_default().into(),
        uid: r.text(3).unwrap_or_default().into(),
        ical: r.text(4).unwrap_or_default().into(),
        etag: r.text(5).unwrap_or_default().into(),
        summary: r.text(6).unwrap_or_default().into(),
        starts_at: r.int(7),
        ends_at: r.int(8),
        recurring: r.int(9).unwrap_or(0) != 0,
        all_day: r.int(10).unwrap_or(0) != 0,
        busy: r.int(11).unwrap_or(0) != 0,
        organizer: r.text(12).map(str::to_string),
        sync: r.int(13).unwrap_or(0),
    }
}

/// An event by its name in a calendar (CalDAV's resource name).
pub async fn event(
    store: &dyn AsyncStore,
    calendar_id: &str,
    href: &str,
) -> StoreResult<Option<Event>> {
    Ok(store
        .query(
            &format!("{EVENT} WHERE calendar_id = $1 AND href = $2"),
            &[calendar_id.into(), href.into()],
        )
        .await?
        .first()
        .map(event_of))
}

/// An event by its UID, in any of the person's calendars.
pub async fn event_by_uid(
    store: &dyn AsyncStore,
    user_id: &str,
    uid: &str,
) -> StoreResult<Option<Event>> {
    Ok(store
        .query(&format!("{EVENT} WHERE uid = $1 AND calendar_id IN (SELECT id FROM cal_calendars WHERE user_id = $2)"), &[uid.into(), user_id.into()])
        .await?
        .first()
        .map(event_of))
}

/// Every event of a calendar.
pub async fn events(store: &dyn AsyncStore, calendar_id: &str) -> StoreResult<Vec<Event>> {
    Ok(store
        .query(
            &format!("{EVENT} WHERE calendar_id = $1 ORDER BY starts_at, href"),
            &[calendar_id.into()],
        )
        .await?
        .iter()
        .map(event_of)
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
pub enum PutError {
    /// No such calendar of the person's.
    NotFound,
    /// If-Match or If-None-Match did not hold.
    Precondition,
    /// Another event in the calendar has this UID.
    UidTaken,
    Invalid(String),
}

impl From<StoreError> for PutError {
    fn from(e: StoreError) -> Self {
        PutError::Invalid(format!("store: {e}"))
    }
}

/// The ETag of a text: a short hash, quoted as HTTP has them.
pub fn etag_of(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("\"{:016x}{:x}\"", h.finish(), text.len())
}

/// Records a change for sync-collection and bumps the calendar's counter.
async fn changed(
    tx: &mut dyn AsyncTx,
    calendar_id: &str,
    href: &str,
    deleted: bool,
) -> StoreResult<i64> {
    tx.execute(
        "UPDATE cal_calendars SET sync = sync + 1 WHERE id = $1",
        &[calendar_id.into()],
    )
    .await?;
    let sync = tx
        .query(
            "SELECT sync FROM cal_calendars WHERE id = $1",
            &[calendar_id.into()],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    tx.execute(
        "INSERT INTO cal_changes (calendar_id, sync, href, deleted) VALUES ($1, $2, $3, $4)",
        &[
            calendar_id.into(),
            sync.into(),
            href.into(),
            i64::from(deleted).into(),
        ],
    )
    .await?;
    Ok(sync)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Put {
    pub etag: String,
    pub created: bool,
}

/// Stores an event under `href` in a calendar of the person's.
/// `if_match` is the ETag the client last saw; `if_none_match` asks that
/// it be new.
pub async fn put_event(
    store: &dyn AsyncStore,
    user_id: &str,
    calendar_id: &str,
    href: &str,
    text: &str,
    if_match: Option<&str>,
    if_none_match: bool,
) -> Result<Put, PutError> {
    if href.is_empty()
        || href.len() > 255
        || href.contains(['/', '\\'])
        || href.starts_with('.')
        || href.chars().any(char::is_control)
    {
        return Err(PutError::Invalid("bad_name".into()));
    }
    let facts = facts(text).map_err(PutError::Invalid)?;
    if calendar(store, user_id, calendar_id).await?.is_none() {
        return Err(PutError::NotFound);
    }
    let etag = etag_of(text);
    let mut tx = store.begin().await?;
    // Writers of one calendar take turns (the row lock lasts until commit),
    // so If-Match is judged against what is really there.
    tx.execute(
        "UPDATE cal_calendars SET sync = sync WHERE id = $1",
        &[calendar_id.into()],
    )
    .await?;
    let existing = tx
        .query(
            "SELECT id, etag, uid FROM cal_events WHERE calendar_id = $1 AND href = $2",
            &[calendar_id.into(), href.into()],
        )
        .await?;
    let existing = existing.first().map(|r| {
        (
            r.text(0).unwrap_or_default().to_string(),
            r.text(1).unwrap_or_default().to_string(),
            r.text(2).unwrap_or_default().to_string(),
        )
    });
    let refuse = match (&existing, if_match, if_none_match) {
        (Some(_), _, true) => true,
        (Some((_, current, _)), Some(wanted), _) => wanted != "*" && wanted != current,
        (None, Some(_), _) => true,
        _ => false,
    };
    if refuse {
        tx.rollback().await?;
        return Err(PutError::Precondition);
    }
    let other = tx
        .query(
            "SELECT href FROM cal_events WHERE calendar_id = $1 AND uid = $2 AND href <> $3",
            &[calendar_id.into(), facts.uid.as_str().into(), href.into()],
        )
        .await?;
    if !other.is_empty() {
        tx.rollback().await?;
        return Err(PutError::UidTaken);
    }
    let sync = changed(&mut *tx, calendar_id, href, false).await?;
    let values = |id: &str| -> Vec<ortus_core::storage::Value> {
        vec![
            id.into(),
            calendar_id.into(),
            href.into(),
            facts.uid.as_str().into(),
            text.into(),
            etag.as_str().into(),
            facts.summary.as_str().into(),
            facts.starts_at.into(),
            facts.ends_at.into(),
            i64::from(facts.recurring).into(),
            i64::from(facts.all_day).into(),
            i64::from(facts.busy).into(),
            facts.organizer.clone().into(),
            sync.into(),
            now().into(),
        ]
    };
    let fresh_id = new_id("ev_");
    match &existing {
        Some((id, _, _)) => {
            tx.execute(
                "UPDATE cal_events SET calendar_id = $2, href = $3, uid = $4, ical = $5, etag = $6, summary = $7, starts_at = $8, ends_at = $9, recurring = $10, all_day = $11, busy = $12, organizer = $13, sync = $14, updated_at = $15 WHERE id = $1",
                &values(id),
            )
            .await?;
        }
        None => {
            tx.execute(
                "INSERT INTO cal_events (id, calendar_id, href, uid, ical, etag, summary, starts_at, ends_at, recurring, all_day, busy, organizer, sync, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
                &values(&fresh_id),
            )
            .await?;
        }
    }
    tx.commit().await?;
    let event_id = existing
        .as_ref()
        .map_or(fresh_id.clone(), |(id, _, _)| id.clone());
    crate::index::event(
        store,
        user_id,
        &event_id,
        text,
        &facts.summary,
        facts.starts_at,
    )
    .await;
    Ok(Put {
        etag,
        created: existing.is_none(),
    })
}

/// Deletes an event of a person's calendar; `if_match` as for put.
pub async fn delete_event(
    store: &dyn AsyncStore,
    user_id: &str,
    calendar_id: &str,
    href: &str,
    if_match: Option<&str>,
) -> Result<bool, PutError> {
    if calendar(store, user_id, calendar_id).await?.is_none() {
        return Err(PutError::NotFound);
    }
    let mut tx = store.begin().await?;
    tx.execute(
        "UPDATE cal_calendars SET sync = sync WHERE id = $1",
        &[calendar_id.into()],
    )
    .await?;
    let rows = tx
        .query(
            "SELECT etag, id FROM cal_events WHERE calendar_id = $1 AND href = $2",
            &[calendar_id.into(), href.into()],
        )
        .await?;
    let event_id = rows
        .first()
        .and_then(|r| r.text(1))
        .unwrap_or_default()
        .to_string();
    let Some(current) = rows.first().and_then(|r| r.text(0)).map(str::to_string) else {
        tx.rollback().await?;
        return Ok(false);
    };
    if if_match.is_some_and(|m| m != "*" && m != current) {
        tx.rollback().await?;
        return Err(PutError::Precondition);
    }
    tx.execute(
        "DELETE FROM cal_events WHERE calendar_id = $1 AND href = $2",
        &[calendar_id.into(), href.into()],
    )
    .await?;
    changed(&mut *tx, calendar_id, href, true).await?;
    tx.commit().await?;
    crate::index::remove(store, "calendar", &event_id).await;
    Ok(true)
}

/// What changed in a calendar after sync counter `since`: (href, deleted),
/// the latest change of each. `None` when `since` is unknown.
pub async fn changes_since(
    store: &dyn AsyncStore,
    calendar_id: &str,
    since: i64,
) -> StoreResult<Option<(Vec<(String, bool)>, i64)>> {
    let Some(calendar) = store
        .query(&format!("{CALENDAR} WHERE id = $1"), &[calendar_id.into()])
        .await?
        .first()
        .map(calendar_of)
    else {
        return Ok(None);
    };
    if since > calendar.sync || since < 0 {
        return Ok(None);
    }
    let rows = store.query("SELECT href, deleted, sync FROM cal_changes WHERE calendar_id = $1 AND sync > $2 ORDER BY sync", &[calendar_id.into(), since.into()]).await?;
    let mut latest: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    for r in rows {
        latest.insert(
            r.text(0).unwrap_or_default().to_string(),
            r.int(1).unwrap_or(0) != 0,
        );
    }
    Ok(Some((latest.into_iter().collect(), calendar.sync)))
}

/// One occurrence of an event, for views and free/busy.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Occurrence {
    pub event: String,
    pub calendar_id: String,
    pub href: String,
    pub uid: String,
    pub summary: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub busy: bool,
}

/// The occurrences in [from, to) of the events in `calendar_ids`.
pub async fn occurrences_in(
    store: &dyn AsyncStore,
    calendar_ids: &[String],
    from: i64,
    to: i64,
) -> StoreResult<Vec<Occurrence>> {
    let mut out = Vec::new();
    for calendar_id in calendar_ids {
        let rows = store
            .query(
                &format!("{EVENT} WHERE calendar_id = $1 AND starts_at < $2 AND (ends_at IS NULL OR ends_at > $3 OR (ends_at = starts_at AND starts_at >= $3))"),
                &[calendar_id.as_str().into(), to.into(), from.into()],
            )
            .await?;
        for e in rows.iter().map(event_of) {
            let times = if e.recurring {
                occurrences(&e.ical, from, to)
            } else {
                vec![(e.starts_at.unwrap_or(0), e.ends_at.unwrap_or(0))]
            };
            for (start, end) in times {
                out.push(Occurrence {
                    event: e.id.clone(),
                    calendar_id: e.calendar_id.clone(),
                    href: e.href.clone(),
                    uid: e.uid.clone(),
                    summary: e.summary.clone(),
                    start,
                    end,
                    all_day: e.all_day,
                    busy: e.busy,
                });
            }
        }
    }
    out.sort_by_key(|o| (o.start, o.end));
    Ok(out)
}

/// Everything of a person's, for forgetting them.
pub async fn forget(tx: &mut dyn AsyncTx, user_id: &str) -> StoreResult<()> {
    for sql in [
        "DELETE FROM cal_changes WHERE calendar_id IN (SELECT id FROM cal_calendars WHERE user_id = $1)",
        "DELETE FROM cal_events WHERE calendar_id IN (SELECT id FROM cal_calendars WHERE user_id = $1)",
        "DELETE FROM cal_calendars WHERE user_id = $1",
        "DELETE FROM cal_polls WHERE user_id = $1",
    ] {
        tx.execute(sql, &[user_id.into()]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:standup@ministry.example\r\nDTSTAMP:20261101T000000Z\r\nDTSTART:20261102T093000Z\r\nDTEND:20261102T094500Z\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Standup\r\nORGANIZER;CN=Olena:mailto:olena@ministry.example\r\nATTENDEE;CN=Marek;PARTSTAT=ACCEPTED:mailto:Marek@ministry.example\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn facts_and_occurrences() {
        let f = facts(WEEKLY).unwrap();
        assert_eq!("standup@ministry.example", f.uid);
        assert_eq!("Standup", f.summary);
        assert_eq!(Some("olena@ministry.example".into()), f.organizer);
        assert_eq!("marek@ministry.example", f.attendees[0].email);
        assert_eq!("ACCEPTED", f.attendees[0].partstat);
        assert!(f.recurring && !f.all_day && f.busy);
        // 2 Nov 09:30 UTC and four weeks of it, the last ending 23 Nov 09:45.
        assert_eq!(Some(1_793_611_800_000), f.starts_at);
        assert_eq!(
            Some(1_793_611_800_000 + 3 * 7 * 86_400_000 + 15 * 60_000),
            f.ends_at
        );
        let week2 = occurrences(
            WEEKLY,
            1_793_611_800_000 + 86_400_000,
            1_793_611_800_000 + 8 * 86_400_000,
        );
        assert_eq!(
            vec![(
                1_793_611_800_000 + 7 * 86_400_000,
                1_793_611_800_000 + 7 * 86_400_000 + 15 * 60_000
            )],
            week2
        );
    }

    #[test]
    fn endless_and_all_day() {
        let endless = WEEKLY.replace("RRULE:FREQ=WEEKLY;COUNT=4", "RRULE:FREQ=DAILY");
        assert_eq!(None, facts(&endless).unwrap().ends_at);
        let day = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART;VALUE=DATE:20261119\r\nDTEND;VALUE=DATE:20261120\r\nSUMMARY:Briefing day\r\nTRANSP:TRANSPARENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let f = facts(day).unwrap();
        assert!(f.all_day && !f.busy, "{f:?}");
        assert_eq!(
            Err("no_uid".into()),
            facts(&WEEKLY.replace("UID:standup@ministry.example\r\n", ""))
        );
        assert_eq!(Err("not_icalendar".into()), facts("hello"));
    }
}
