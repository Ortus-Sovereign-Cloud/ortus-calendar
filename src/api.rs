//! The calendar's JSON API for the web client (and anyone with a token):
//! calendars, events in a range, making and changing events (which sends
//! the invitations), answers, and times that work.

use std::sync::Arc;

use ortus_core::api::{answer, body, ok, ApiError, Method, Route};
use ortus_core::auth::Access;
use ortus_core::instance::Instance;
use ortus_core::storage::BoxFuture;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::schedule::{self, fold, mailto, split, unfold};
use crate::store;

const DAY: i64 = 86_400_000;

fn query(request: &Request, name: &str) -> Option<String> {
    let q = request.request_uri.split_once('?')?.1;
    q.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k == name).then(|| percent(v))
    })
}

fn percent(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&v[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn number(request: &Request, name: &str) -> Result<i64, ApiError> {
    query(request, name)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| ApiError::bad_request(format!("?{name}= is a time in Unix ms.")))
}

async fn signed(
    request: &Request,
    instance: &Arc<Instance>,
) -> Result<ortus_core::auth::accounts::User, ApiError> {
    Ok(ortus_core::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user)
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Year, month, day of a day number since 1970 (proleptic Gregorian).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn utc(ms: i64) -> String {
    let (y, m, d) = civil(ms.div_euclid(DAY));
    let s = ms.rem_euclid(DAY) / 1000;
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn date(ms: i64) -> String {
    let (y, m, d) = civil(ms.div_euclid(DAY));
    format!("{y:04}{m:02}{d:02}")
}

/// The first VEVENT's properties, by name (unescaped value).
/// A content line: name, parameters, value.
type Line = (String, Vec<(String, String)>, String);

fn props(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut seen = false;
    for line in unfold(text) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            if upper == "BEGIN:VEVENT" {
                if seen {
                    break;
                }
                seen = true;
            }
            depth += 1;
            continue;
        }
        if upper.starts_with("END:") {
            depth -= 1;
            continue;
        }
        if seen && depth == 2 {
            out.push(split(&line));
        }
    }
    out
}

fn prop(props: &[Line], name: &str) -> Option<String> {
    props
        .iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, _, v)| unescape(v))
}

/// An event as the web client sees it.
pub(crate) fn event_json(event: &store::Event, me: &str) -> Value {
    let p = props(&event.ical);
    let facts = store::facts(&event.ical).ok();
    let attendees: Vec<Value> = facts
        .as_ref()
        .map(|f| {
            f.attendees
                .iter()
                .map(|a| json!({"email": a.email, "name": a.name, "answer": a.partstat}))
                .collect()
        })
        .unwrap_or_default();
    let organizer = event.organizer.clone();
    let mine = organizer.as_deref().is_none_or(|o| o == me);
    let answer = if mine {
        None
    } else {
        facts.as_ref().and_then(|f| {
            f.attendees
                .iter()
                .find(|a| a.email == me)
                .map(|a| a.partstat.clone())
        })
    };
    let rrule = prop(&p, "RRULE").unwrap_or_default();
    let repeat = if rrule.is_empty() {
        "none"
    } else if rrule.contains("BYDAY=MO,TU,WE,TH,FR") {
        "weekdays"
    } else if rrule.contains("FREQ=DAILY") {
        "daily"
    } else if rrule.contains("FREQ=WEEKLY") {
        "weekly"
    } else if rrule.contains("FREQ=MONTHLY") {
        "monthly"
    } else {
        "other"
    };
    json!({
        "id": event.id,
        "calendar": event.calendar_id,
        "uid": event.uid,
        "summary": event.summary,
        "start": event.starts_at,
        "end": facts.as_ref().and_then(|f| f.starts_at.zip(f.ends_at).map(|_| ())).and(single_end(&event.ical, event.starts_at)),
        "allDay": event.all_day,
        "location": prop(&p, "LOCATION").unwrap_or_default(),
        "description": prop(&p, "DESCRIPTION").unwrap_or_default(),
        "organizer": organizer,
        "attendees": attendees,
        "mine": mine,
        "answer": answer,
        "repeat": repeat,
        "focus": prop(&p, "X-ORTUS-KIND").is_some_and(|k| k.eq_ignore_ascii_case("focus")),
        "private": prop(&p, "CLASS").is_some_and(|k| !k.eq_ignore_ascii_case("PUBLIC")),
        "cancelled": prop(&p, "STATUS").is_some_and(|k| k.eq_ignore_ascii_case("CANCELLED")),
    })
}

/// The end of the first occurrence.
fn single_end(text: &str, start: Option<i64>) -> Option<i64> {
    let start = start?;
    store::occurrences(text, start, start + 1)
        .first()
        .map(|(_, e)| *e)
}

#[derive(Deserialize)]
struct EventInput {
    calendar: Option<String>,
    summary: String,
    start: i64,
    end: i64,
    #[serde(default, rename = "allDay")]
    all_day: bool,
    #[serde(default)]
    location: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    attendees: Vec<String>,
    #[serde(default)]
    repeat: Option<String>,
    #[serde(default)]
    focus: bool,
    #[serde(default)]
    private: bool,
}

impl EventInput {
    fn check(&self) -> Result<(), ApiError> {
        let summary = self.summary.trim();
        if summary.is_empty() || summary.chars().count() > 300 {
            return Err(ApiError::bad_request(
                "An event needs a title of up to 300 characters.",
            ));
        }
        if self.end <= self.start || self.end - self.start > 60 * DAY {
            return Err(ApiError::bad_request(
                "The end is after the start, within 60 days.",
            ));
        }
        if self.location.len() > 1000
            || self.description.len() > 20_000
            || self.attendees.len() > 200
        {
            return Err(ApiError::bad_request("The event is too big."));
        }
        if self.attendees.iter().any(|a| {
            !a.contains('@')
                || a.len() > 254
                || a.contains(['\r', '\n', ':', ';', ',', '<', '>', ' '])
        }) {
            return Err(ApiError::bad_request("Each attendee is an address."));
        }
        Ok(())
    }

    /// The event's own lines (inside VEVENT), replacing what it sets.
    fn lines(&self, me: &str, name: &str, sequence: i64) -> Vec<String> {
        let mut out = vec![format!("DTSTAMP:{}", utc(ortus_core::program::now_ms()))];
        if self.all_day {
            out.push(format!("DTSTART;VALUE=DATE:{}", date(self.start)));
            out.push(format!(
                "DTEND;VALUE=DATE:{}",
                date(self.end.max(self.start + DAY))
            ));
        } else {
            out.push(format!("DTSTART:{}", utc(self.start)));
            out.push(format!("DTEND:{}", utc(self.end)));
        }
        out.push(format!("SUMMARY:{}", escape(self.summary.trim())));
        if !self.location.trim().is_empty() {
            out.push(format!("LOCATION:{}", escape(self.location.trim())));
        }
        if !self.description.trim().is_empty() {
            out.push(format!("DESCRIPTION:{}", escape(self.description.trim())));
        }
        match self.repeat.as_deref().unwrap_or("none") {
            "daily" => out.push("RRULE:FREQ=DAILY".into()),
            "weekdays" => out.push("RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into()),
            "weekly" => out.push("RRULE:FREQ=WEEKLY".into()),
            "monthly" => out.push("RRULE:FREQ=MONTHLY".into()),
            _ => {}
        }
        if self.focus {
            out.push("X-ORTUS-KIND:focus".into());
        }
        if self.private {
            out.push("CLASS:PRIVATE".into());
        }
        out.push(format!("SEQUENCE:{sequence}"));
        let people: Vec<String> = self
            .attendees
            .iter()
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| a != me)
            .collect();
        if !people.is_empty() {
            out.push(format!(
                "ORGANIZER;CN=\"{}\":mailto:{me}",
                name.replace('"', "'")
            ));
            out.push(format!("ATTENDEE;PARTSTAT=ACCEPTED;ROLE=CHAIR:mailto:{me}"));
            for a in people {
                out.push(format!(
                    "ATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{a}"
                ));
            }
        }
        out
    }
}

/// A new event's text with attendees, as the organiser makes it.
pub(crate) fn event_text(
    uid: &str,
    me: &str,
    name: &str,
    summary: &str,
    start: i64,
    end: i64,
    people: &[String],
) -> String {
    let input = EventInput {
        calendar: None,
        summary: summary.to_string(),
        start,
        end,
        all_day: false,
        location: String::new(),
        description: String::new(),
        attendees: people.to_vec(),
        repeat: None,
        focus: false,
        private: false,
    };
    new_text(uid, input.lines(me, name, 0))
}

/// The names `EventInput::lines` sets; they are replaced on a change.
const SET: &[&str] = &[
    "DTSTAMP",
    "DTSTART",
    "DTEND",
    "DURATION",
    "SUMMARY",
    "LOCATION",
    "DESCRIPTION",
    "RRULE",
    "X-ORTUS-KIND",
    "CLASS",
    "SEQUENCE",
    "ORGANIZER",
    "ATTENDEE",
];

fn new_text(uid: &str, lines: Vec<String>) -> String {
    let mut all = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//Ortus//Calendar//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}"),
    ];
    all.extend(lines);
    all.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    fold(&all)
}

/// The text with the first VEVENT's settable lines replaced; attendees
/// keep their answers.
fn changed_text(before: &str, mut lines: Vec<String>) -> String {
    let answers: Vec<(String, String)> = props(before)
        .into_iter()
        .filter(|(n, _, _)| n == "ATTENDEE")
        .filter_map(|(_, params, v)| {
            params
                .iter()
                .find(|(k, _)| k == "PARTSTAT")
                .map(|(_, p)| (mailto(&v), p.clone()))
        })
        .collect();
    for line in lines.iter_mut() {
        let (name, mut params, value) = split(line);
        if name == "ATTENDEE" && !params.iter().any(|(k, _)| k == "ROLE") {
            if let Some((_, p)) = answers.iter().find(|(e, _)| *e == mailto(&value)) {
                params.retain(|(k, _)| k != "PARTSTAT" && k != "RSVP");
                params.insert(0, ("PARTSTAT".into(), p.clone()));
                let mut out = name.clone();
                for (k, v) in &params {
                    out.push_str(&format!(";{k}={v}"));
                }
                *line = format!("{out}:{value}");
            }
        }
    }
    let mut out = Vec::new();
    let mut depth = 0;
    let mut seen = false;
    let mut main = false;
    for line in unfold(before) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:") {
            depth += 1;
            if upper == "BEGIN:VEVENT" && !seen {
                seen = true;
                main = true;
                out.push(line);
                out.append(&mut lines);
                continue;
            }
        } else if upper.starts_with("END:") {
            if upper == "END:VEVENT" {
                main = false;
            }
            depth -= 1;
        } else if main && depth == 2 && SET.contains(&split(&line).0.as_str()) {
            continue;
        }
        out.push(line);
    }
    fold(&out)
}

async fn mine(instance: &Instance, user: &str, id: &str) -> Result<store::Event, ApiError> {
    let s = instance.store.as_ref();
    let rows = s.query("SELECT e.calendar_id, e.href FROM cal_events e JOIN cal_calendars c ON c.id = e.calendar_id WHERE e.id = $1 AND c.user_id = $2", &[id.into(), user.into()]).await?;
    let row = rows
        .first()
        .ok_or_else(|| ApiError::not_found("There is no such event."))?;
    let (cal, href) = (
        row.text(0).unwrap_or_default().to_string(),
        row.text(1).unwrap_or_default().to_string(),
    );
    store::event(s, &cal, &href)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such event."))
}

fn scheduling(e: String) -> ApiError {
    match e.as_str() {
        "mail_not_installed" => ApiError::not_installed("mail"),
        _ => ApiError {
            status: 502,
            code: "invitations",
            message: format!("The event is saved, but the invitations could not all go: {e}"),
        },
    }
}

fn put_error(e: store::PutError) -> ApiError {
    match e {
        store::PutError::NotFound => ApiError::not_found("There is no such calendar."),
        store::PutError::Precondition => ApiError::conflict("The event changed meanwhile."),
        store::PutError::UidTaken => ApiError::conflict("That event is already in the calendar."),
        _ => ApiError::bad_request("The event is not valid."),
    }
}

async fn list_calendars(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let list: Vec<Value> = store::calendars(instance.store.as_ref(), &me.id)
        .await?
        .into_iter()
        .map(|c| json!({"id": c.id, "name": c.name, "color": c.color}))
        .collect();
    Ok(ok(&json!({ "calendars": list })))
}

#[derive(Deserialize)]
struct CalendarInput {
    name: Option<String>,
    color: Option<String>,
}

async fn add_calendar(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: CalendarInput = body(request)?;
    let c = store::create_calendar(
        instance.store.as_ref(),
        &me.id,
        input.name.as_deref().unwrap_or_default(),
        input.color.as_deref().unwrap_or("accent"),
    )
    .await?
    .ok_or_else(|| ApiError::bad_request("A calendar needs a name of up to 100 characters."))?;
    Ok(ok(&json!({"id": c.id, "name": c.name, "color": c.color})))
}

async fn change_calendar(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: CalendarInput = body(request)?;
    let id = params.get("calendar").unwrap_or_default();
    store::calendar(instance.store.as_ref(), &me.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such calendar."))?;
    store::update_calendar(
        instance.store.as_ref(),
        &me.id,
        id,
        input.name.as_deref(),
        input.color.as_deref(),
    )
    .await?;
    Ok(ok(&json!({})))
}

async fn remove_calendar(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let id = params.get("calendar").unwrap_or_default();
    if !store::delete_calendar(instance.store.as_ref(), &me.id, id).await? {
        return Err(ApiError::conflict(
            "That calendar is not there, or it is the last one.",
        ));
    }
    Ok(ok(&json!({})))
}

/// Occurrences between ?from= and ?to= (Unix ms, at most 62 days).
async fn list_events(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (from, to) = (number(request, "from")?, number(request, "to")?);
    if to <= from || to - from > 62 * DAY {
        return Err(ApiError::bad_request(
            "?from= and ?to= span at most 62 days.",
        ));
    }
    let s = instance.store.as_ref();
    let email = me.email.to_ascii_lowercase();
    let ids: Vec<String> = store::calendars(s, &me.id)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let mut out = Vec::new();
    let mut cache: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for o in store::occurrences_in(s, &ids, from, to).await? {
        let base = match cache.get(&o.event) {
            Some(v) => v.clone(),
            None => {
                let Some(e) = store::event(s, &o.calendar_id, &o.href).await? else {
                    continue;
                };
                let v = event_json(&e, &email);
                cache.insert(o.event.clone(), v.clone());
                v
            }
        };
        let mut v = base;
        v["start"] = json!(o.start);
        v["end"] = json!(o.end);
        if v["cancelled"] == json!(true) {
            continue;
        }
        out.push(v);
    }
    Ok(ok(&json!({ "events": out })))
}

async fn get_event(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let e = mine(instance, &me.id, params.get("event").unwrap_or_default()).await?;
    Ok(ok(&event_json(&e, &me.email.to_ascii_lowercase())))
}

async fn add_event(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: EventInput = body(request)?;
    input.check()?;
    let s = instance.store.as_ref();
    let calendar = match &input.calendar {
        Some(id) => store::calendar(s, &me.id, id)
            .await?
            .ok_or_else(|| ApiError::not_found("There is no such calendar."))?,
        None => store::calendars(s, &me.id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(ApiError::internal)?,
    };
    let email = me.email.to_ascii_lowercase();
    let domain = email.rsplit_once('@').map_or("ortus.local", |(_, d)| d);
    let uid = format!("{}@{domain}", ortus_core::auth::random_hex(12));
    let text = new_text(&uid, input.lines(&email, &me.name, 0));
    let href = format!("{}.ics", ortus_core::auth::random_hex(12));
    store::put_event(s, &me.id, &calendar.id, &href, &text, None, true)
        .await
        .map_err(put_error)?;
    let event = store::event(s, &calendar.id, &href)
        .await?
        .ok_or_else(ApiError::internal)?;
    if !input.attendees.is_empty() {
        schedule::publish(instance, &me.id, None, &text)
            .await
            .map_err(scheduling)?;
    }
    Ok(ok(&event_json(&event, &email)))
}

async fn change_event(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: EventInput = body(request)?;
    input.check()?;
    let email = me.email.to_ascii_lowercase();
    let before = mine(instance, &me.id, params.get("event").unwrap_or_default()).await?;
    if before.organizer.as_deref().is_some_and(|o| o != email) {
        return Err(ApiError {
            status: 403,
            code: "not_organizer",
            message: "Only its organiser changes an event; answer it instead.".into(),
        });
    }
    let sequence = prop(&props(&before.ical), "SEQUENCE")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
        + 1;
    let text = changed_text(&before.ical, input.lines(&email, &me.name, sequence));
    let s = instance.store.as_ref();
    let target = input
        .calendar
        .clone()
        .unwrap_or_else(|| before.calendar_id.clone());
    if target != before.calendar_id {
        store::calendar(s, &me.id, &target)
            .await?
            .ok_or_else(|| ApiError::not_found("There is no such calendar."))?;
        store::delete_event(s, &me.id, &before.calendar_id, &before.href, None)
            .await
            .map_err(put_error)?;
        store::put_event(s, &me.id, &target, &before.href, &text, None, true)
            .await
            .map_err(put_error)?;
    } else {
        store::put_event(
            s,
            &me.id,
            &target,
            &before.href,
            &text,
            Some(&before.etag),
            false,
        )
        .await
        .map_err(put_error)?;
    }
    let event = store::event(s, &target, &before.href)
        .await?
        .ok_or_else(ApiError::internal)?;
    if !input.attendees.is_empty() || before.ical.contains("ATTENDEE") {
        schedule::publish(instance, &me.id, Some(&before.ical), &text)
            .await
            .map_err(scheduling)?;
    }
    Ok(ok(&event_json(&event, &email)))
}

async fn remove_event(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let email = me.email.to_ascii_lowercase();
    let e = mine(instance, &me.id, params.get("event").unwrap_or_default()).await?;
    let s = instance.store.as_ref();
    let organiser = e.organizer.as_deref().is_none_or(|o| o == email);
    if organiser {
        let people: Vec<String> = store::facts(&e.ical)
            .map(|f| {
                f.attendees
                    .into_iter()
                    .map(|a| a.email)
                    .filter(|a| *a != email)
                    .collect()
            })
            .unwrap_or_default();
        if !people.is_empty() {
            schedule::cancel_for(instance, &me.id, &people, &e.ical)
                .await
                .map_err(scheduling)?;
        }
    } else if !e.ical.contains("STATUS:CANCELLED") {
        // Removing an invitation is declining it.
        let _ = schedule::respond(instance, &me.id, &e.uid, "DECLINED").await;
    }
    store::delete_event(s, &me.id, &e.calendar_id, &e.href, None)
        .await
        .map_err(put_error)?;
    Ok(ok(&json!({})))
}

#[derive(Deserialize)]
struct Rsvp {
    answer: String,
}

async fn rsvp(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: Rsvp = body(request)?;
    let e = mine(instance, &me.id, params.get("event").unwrap_or_default()).await?;
    schedule::respond(instance, &me.id, &e.uid, &input.answer)
        .await
        .map_err(|e| match e.as_str() {
            "bad_answer" => ApiError::bad_request("The answer is accepted, tentative or declined."),
            "not_invited" | "no_organizer" => {
                ApiError::conflict("You are not invited to that event.")
            }
            other => scheduling(other.to_string()),
        })?;
    let e = mine(instance, &me.id, &e.id).await?;
    Ok(ok(&event_json(&e, &me.email.to_ascii_lowercase())))
}

/// Times that work: ?people=a,b&from=&to=&minutes=30.
async fn free(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let (from, to) = (number(request, "from")?, number(request, "to")?);
    if to <= from || to - from > 31 * DAY {
        return Err(ApiError::bad_request(
            "?from= and ?to= span at most 31 days.",
        ));
    }
    let minutes = query(request, "minutes")
        .and_then(|m| m.parse().ok())
        .unwrap_or(30);
    let people: Vec<String> = query(request, "people")
        .unwrap_or_default()
        .split(',')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| p.contains('@'))
        .take(50)
        .collect();
    let limit = query(request, "limit")
        .and_then(|m| m.parse().ok())
        .unwrap_or(20usize)
        .min(200);
    let times = schedule::free_times(instance, &me.id, &people, from, to, minutes, limit)
        .await
        .map_err(|_| ApiError::internal())?;
    let mut known = Vec::new();
    for p in &people {
        known.push(json!({"email": p, "here": schedule::local_user(instance, p).await.is_some()}));
    }
    Ok(ok(&json!({ "times": times, "people": known })))
}

macro_rules! plain {
    ($name:ident, $f:ident) => {
        fn $name(
            request: Request,
            _: PathParams,
            _: ConnectionInfo,
            instance: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $f(&request, &instance).await) })
        }
    };
}
macro_rules! with_params {
    ($name:ident, $f:ident) => {
        fn $name(
            request: Request,
            params: PathParams,
            _: ConnectionInfo,
            instance: Arc<Instance>,
        ) -> BoxFuture<'static, Response> {
            Box::pin(async move { answer(&request, $f(&request, &params, &instance).await) })
        }
    };
}
plain!(h_list_calendars, list_calendars);
plain!(h_add_calendar, add_calendar);
with_params!(h_change_calendar, change_calendar);
with_params!(h_remove_calendar, remove_calendar);
plain!(h_list_events, list_events);
plain!(h_add_event, add_event);
with_params!(h_get_event, get_event);
with_params!(h_change_event, change_event);
with_params!(h_remove_event, remove_event);
with_params!(h_rsvp, rsvp);
plain!(h_free, free);

fn object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let route = |method, path, id, summary, handler| Route {
        method,
        path,
        id,
        tag: "calendar",
        summary,
        access: Access::Signed,
        response: object,
        handler,
    };
    vec![
        route(Method::Get, "/calendar/calendars", "calendars", "One's calendars.", h_list_calendars as ortus_core::api::Handler),
        route(Method::Post, "/calendar/calendars", "addCalendar", "A new calendar: {name, color}.", h_add_calendar),
        route(Method::Patch, "/calendar/calendars/:calendar", "changeCalendar", "Rename or recolour a calendar: {name?, color?}.", h_change_calendar),
        route(Method::Delete, "/calendar/calendars/:calendar", "removeCalendar", "Remove a calendar with its events (not the last one).", h_remove_calendar),
        route(Method::Get, "/calendar/events", "events", "One's events between ?from= and ?to= (Unix ms, at most 62 days), repeats expanded.", h_list_events),
        route(Method::Post, "/calendar/events", "addEvent", "A new event: {calendar?, summary, start, end, allDay, location, description, attendees, repeat (none, daily, weekdays, weekly, monthly), focus, private}. Attendees are invited.", h_add_event),
        route(Method::Get, "/calendar/events/:event", "event", "One event.", h_get_event),
        route(Method::Put, "/calendar/events/:event", "changeEvent", "Change one's event (as addEvent); attendees are told, and those taken off get a cancellation.", h_change_event),
        route(Method::Delete, "/calendar/events/:event", "removeEvent", "Remove an event: its attendees get a cancellation; removing an invitation declines it.", h_remove_event),
        route(Method::Post, "/calendar/events/:event/rsvp", "answerEvent", "Answer an invitation: {answer: accepted, tentative or declined}.", h_rsvp),
        route(Method::Get, "/calendar/free", "freeTimes", "Times that work for one and ?people= (addresses, comma separated) between ?from= and ?to=, for ?minutes=.", h_free),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_keep_what_is_not_set_and_answers() {
        let before = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:x\r\nDTSTART:20261126T140000Z\r\nDTEND:20261126T143000Z\r\nSUMMARY:Old\r\nORGANIZER:mailto:o@m.example\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:a@m.example\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let input = EventInput {
            calendar: None,
            summary: "New, better".into(),
            start: 1_795_672_800_000,
            end: 1_795_674_600_000,
            all_day: false,
            location: String::new(),
            description: String::new(),
            attendees: vec!["a@m.example".into(), "b@m.example".into()],
            repeat: Some("weekly".into()),
            focus: false,
            private: false,
        };
        let text = changed_text(before, input.lines("o@m.example", "O", 1));
        assert!(
            text.contains("SUMMARY:New\\, better") && !text.contains("SUMMARY:Old"),
            "{text}"
        );
        assert!(
            text.contains("PARTSTAT=ACCEPTED:mailto:a@m.example"),
            "{text}"
        );
        assert!(text.contains("PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:b@m.example"));
        assert!(
            text.contains("TRIGGER:-PT5M")
                && text.contains("RRULE:FREQ=WEEKLY")
                && text.contains("SEQUENCE:1")
        );
        assert!(store::facts(&text).is_ok());
        assert_eq!("a b", unescape("a\\ b").replace('\\', ""));
        assert_eq!("x y", percent("x%20y"));
        assert_eq!("20261126T140000Z", utc(1_795_651_200_000 + 14 * 3_600_000));
        assert_eq!("20240229", date(1_709_164_800_000));
    }
}
