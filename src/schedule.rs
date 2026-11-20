//! Scheduling (iTIP, RFC 5546; iMIP, RFC 6047): invitations, answers and
//! cancellations. Colleagues' calendars are written directly; people
//! outside get mail through the mail app (`Mailer`), which applies the
//! label policy. Invitations arriving by mail come in through
//! [`receive`].

use std::sync::Arc;

use ortus_core::instance::Instance;
use ortus_core::interfaces::{AgendaItem, Invitation, ItemRef, Mailer};

use crate::store;

// --- iCalendar text, line by line ---

/// Content lines, unfolded.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            let last = lines.last_mut().expect("a line");
            last.push_str(&raw[1..]);
        } else if !raw.is_empty() {
            lines.push(raw.to_string());
        }
    }
    lines
}

/// Lines folded at 75 octets (on character boundaries), CRLF ended.
fn fold(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        let mut used = 0;
        for c in line.chars() {
            let len = c.len_utf8();
            if used + len > 75 {
                out.push_str("\r\n ");
                used = 1;
            }
            out.push(c);
            used += len;
        }
        out.push_str("\r\n");
    }
    out
}

/// A content line's name, its parameters and its value.
fn split(line: &str) -> (String, Vec<(String, String)>, String) {
    // The value starts at the first colon outside quotes.
    let mut quoted = false;
    let mut cut = line.len();
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                cut = i;
                break;
            }
            _ => {}
        }
    }
    let (head, value) = (&line[..cut], line.get(cut + 1..).unwrap_or_default());
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    quoted = false;
    for c in head.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ';' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    let name = parts.remove(0).to_ascii_uppercase();
    let params = parts
        .into_iter()
        .filter_map(|p| {
            p.split_once('=')
                .map(|(k, v)| (k.to_ascii_uppercase(), v.to_string()))
        })
        .collect();
    (name, params, value.to_string())
}

fn join(name: &str, params: &[(String, String)], value: &str) -> String {
    let mut out = name.to_string();
    for (k, v) in params {
        out.push(';');
        out.push_str(k);
        out.push('=');
        out.push_str(v);
    }
    out.push(':');
    out.push_str(value);
    out
}

fn mailto(value: &str) -> String {
    let v = value.trim();
    v.strip_prefix("mailto:")
        .or_else(|| v.strip_prefix("MAILTO:"))
        .unwrap_or(v)
        .to_ascii_lowercase()
}

/// An event's text, to change and write back.
struct Ics {
    lines: Vec<String>,
}

impl Ics {
    fn parse(text: &str) -> Self {
        Self {
            lines: unfold(text),
        }
    }

    fn text(&self) -> String {
        fold(&self.lines)
    }

    /// The METHOD of the object, if any.
    fn method(&self) -> Option<String> {
        self.lines
            .iter()
            .map(|l| split(l))
            .find(|(n, _, _)| n == "METHOD")
            .map(|(_, _, v)| v.trim().to_ascii_uppercase())
    }

    /// Sets (or removes) the METHOD, in the VCALENDAR header.
    fn set_method(&mut self, method: Option<&str>) {
        self.lines.retain(|l| split(l).0 != "METHOD");
        if let Some(method) = method {
            let at = self
                .lines
                .iter()
                .position(|l| l.eq_ignore_ascii_case("BEGIN:VCALENDAR"))
                .map_or(0, |i| i + 1);
            self.lines.insert(at, format!("METHOD:{method}"));
        }
    }

    fn uid(&self) -> Option<String> {
        self.lines
            .iter()
            .map(|l| split(l))
            .find(|(n, _, _)| n == "UID")
            .map(|(_, _, v)| v.trim().to_string())
    }

    fn organizer(&self) -> Option<String> {
        self.lines
            .iter()
            .map(|l| split(l))
            .find(|(n, _, _)| n == "ORGANIZER")
            .map(|(_, _, v)| mailto(&v))
    }

    fn summary(&self) -> String {
        self.lines
            .iter()
            .map(|l| split(l))
            .find(|(n, _, _)| n == "SUMMARY")
            .map(|(_, _, v)| {
                v.replace("\\,", ",")
                    .replace("\\;", ";")
                    .replace("\\n", " ")
            })
            .unwrap_or_default()
    }

    /// Attendees and their answers (first VEVENT that names them).
    fn attendees(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, params, value) in self.lines.iter().map(|l| split(l)) {
            if name == "ATTENDEE" {
                let email = mailto(&value);
                let partstat = params
                    .iter()
                    .find(|(k, _)| k == "PARTSTAT")
                    .map(|(_, v)| v.trim_matches('"').to_ascii_uppercase())
                    .unwrap_or_else(|| "NEEDS-ACTION".into());
                if !out.iter().any(|(e, _)| *e == email) {
                    out.push((email, partstat));
                }
            }
        }
        out
    }

    fn partstat(&self, email: &str) -> Option<String> {
        self.attendees()
            .into_iter()
            .find(|(e, _)| e == email)
            .map(|(_, p)| p)
    }

    /// Sets one attendee's answer everywhere it appears. Answers whether
    /// they are an attendee at all.
    fn set_partstat(&mut self, email: &str, partstat: &str) -> bool {
        let mut found = false;
        for line in self.lines.iter_mut() {
            let (name, mut params, value) = split(line);
            if name == "ATTENDEE" && mailto(&value) == email {
                params.retain(|(k, _)| k != "PARTSTAT" && k != "RSVP");
                params.push(("PARTSTAT".into(), partstat.to_string()));
                *line = join(&name, &params, &value);
                found = true;
            }
        }
        found
    }

    /// Marks every VEVENT cancelled (its own STATUS replaced).
    fn cancel(&mut self) {
        let mut out = Vec::new();
        let mut in_event = false;
        let mut in_alarm = false;
        for line in &self.lines {
            let upper = line.to_ascii_uppercase();
            match upper.as_str() {
                "BEGIN:VEVENT" => {
                    in_event = true;
                    out.push(line.clone());
                    out.push("STATUS:CANCELLED".into());
                    continue;
                }
                "END:VEVENT" => in_event = false,
                "BEGIN:VALARM" => in_alarm = true,
                "END:VALARM" => in_alarm = false,
                _ => {}
            }
            if in_event && !in_alarm && split(line).0 == "STATUS" {
                continue;
            }
            out.push(line.clone());
        }
        self.lines = out;
    }

    /// Only the main VEVENT, only `attendee` among the attendees: a REPLY.
    fn reply_for(&self, attendee: &str) -> Ics {
        let mut out = Vec::new();
        let mut depth_event = false;
        let mut skipping = false;
        let mut seen_event = false;
        for line in &self.lines {
            let upper = line.to_ascii_uppercase();
            if upper == "BEGIN:VEVENT" {
                if seen_event {
                    skipping = true;
                    continue;
                }
                seen_event = true;
                depth_event = true;
            }
            if skipping {
                if upper == "END:VEVENT" {
                    skipping = false;
                }
                continue;
            }
            if upper == "END:VEVENT" {
                depth_event = false;
            }
            if depth_event && upper.starts_with("BEGIN:VALARM") {
                skipping = true;
                continue;
            }
            let (name, _, value) = split(line);
            if depth_event && name == "ATTENDEE" && mailto(&value) != attendee {
                continue;
            }
            out.push(line.clone());
        }
        let mut ics = Ics { lines: out };
        ics.set_method(Some("REPLY"));
        ics
    }
}

// --- Who is here ---

/// The account of an address in this installation, if any.
async fn local_user(instance: &Instance, email: &str) -> Option<String> {
    ortus_core::auth::accounts::by_email(instance.store.as_ref(), email)
        .await
        .ok()
        .flatten()
        .filter(|u| u.state == ortus_core::auth::accounts::State::Active)
        .map(|u| u.id)
}

async fn email_of(instance: &Instance, user: &str) -> Option<String> {
    ortus_core::auth::accounts::by_id(instance.store.as_ref(), user)
        .await
        .ok()
        .flatten()
        .map(|u| u.email.to_ascii_lowercase())
}

async fn mailer(instance: &Instance) -> Result<Arc<dyn Mailer>, String> {
    instance
        .services
        .get::<dyn Mailer>(&instance.switches)
        .await
        .map_err(|_| "mail_not_installed".to_string())
}

fn item_ref(event: &store::Event) -> ItemRef {
    ItemRef {
        app: "calendar".into(),
        id: event.id.clone(),
        path: format!("/calendar?event={}", event.id),
    }
}

/// Puts `text` (no METHOD) as the event with its UID into the person's
/// calendar: replacing their copy if they have one, else into their first
/// calendar. Answers the stored event.
async fn upsert(instance: &Instance, user: &str, text: &str) -> Result<store::Event, String> {
    let s = instance.store.as_ref();
    let uid = Ics::parse(text).uid().ok_or("no_uid")?;
    let (calendar, href, etag) = match store::event_by_uid(s, user, &uid)
        .await
        .map_err(|e| e.to_string())?
    {
        Some(e) => (e.calendar_id.clone(), e.href.clone(), Some(e.etag.clone())),
        None => {
            let calendar = store::calendars(s, user)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .next()
                .ok_or("no_calendar")?;
            let safe: String = uid
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || "-_.@".contains(*c))
                .take(100)
                .collect();
            (
                calendar.id,
                format!(
                    "{}.ics",
                    if safe.is_empty() {
                        ortus_core::auth::random_hex(8)
                    } else {
                        safe
                    }
                ),
                None,
            )
        }
    };
    store::put_event(
        s,
        user,
        &calendar,
        &href,
        text,
        etag.as_deref(),
        etag.is_none(),
    )
    .await
    .map_err(|e| format!("{e:?}"))?;
    store::event(s, &calendar, &href)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "lost".to_string())
}

fn invitation(
    method: &str,
    ics: &Ics,
    answer: Option<String>,
    event: Option<&store::Event>,
) -> Invitation {
    let facts = store::facts(&ics.text()).ok();
    Invitation {
        uid: ics.uid().unwrap_or_default(),
        method: method.to_string(),
        summary: ics.summary(),
        start_ms: facts.as_ref().and_then(|f| f.starts_at),
        end_ms: facts.as_ref().and_then(|f| f.ends_at),
        organizer: ics.organizer(),
        answer,
        cancelled: ics
            .lines
            .iter()
            .any(|l| l.eq_ignore_ascii_case("STATUS:CANCELLED")),
        item: event.map(item_ref),
    }
}

/// Sends the organiser's event to everyone it invites: colleagues get it
/// in their calendars (keeping their answer), people outside by mail.
/// `before` is the event as it was (to tell who was taken off it).
pub async fn publish(
    instance: &Instance,
    organizer: &str,
    before: Option<&str>,
    after: &str,
) -> Result<(), String> {
    let me = email_of(instance, organizer).await.ok_or("no_account")?;
    let event = Ics::parse(after);
    if event.organizer().is_some_and(|o| o != me) {
        // Someone else's event in this calendar: not ours to send.
        return Ok(());
    }
    let now: Vec<String> = event
        .attendees()
        .into_iter()
        .map(|(e, _)| e)
        .filter(|e| *e != me)
        .collect();
    let gone: Vec<String> = before
        .map(|b| {
            Ics::parse(b)
                .attendees()
                .into_iter()
                .map(|(e, _)| e)
                .filter(|e| *e != me && !now.contains(e))
                .collect()
        })
        .unwrap_or_default();
    let mut outside = Vec::new();
    for email in &now {
        match local_user(instance, email).await {
            Some(user) => {
                // Their own answer stays.
                let mut copy = Ics::parse(after);
                copy.set_method(None);
                if let Ok(Some(theirs)) = store::event_by_uid(
                    instance.store.as_ref(),
                    &user,
                    &event.uid().unwrap_or_default(),
                )
                .await
                {
                    if let Some(answer) = Ics::parse(&theirs.ical).partstat(email) {
                        copy.set_partstat(email, &answer);
                    }
                }
                upsert(instance, &user, &copy.text()).await?;
            }
            None => outside.push(email.clone()),
        }
    }
    let subject = event.summary();
    if !outside.is_empty() {
        let mut request = Ics::parse(after);
        request.set_method(Some("REQUEST"));
        let text = format!("You are invited: {subject}");
        mailer(instance)
            .await?
            .send_invitation(
                organizer,
                &outside,
                &format!("Invitation: {subject}"),
                &text,
                &request.text(),
            )
            .await?;
    }
    if !gone.is_empty() {
        cancel_for(instance, organizer, &gone, after).await?;
    }
    Ok(())
}

/// Tells `people` the event is off: colleagues' copies are cancelled,
/// people outside get a CANCEL.
pub async fn cancel_for(
    instance: &Instance,
    organizer: &str,
    people: &[String],
    text: &str,
) -> Result<(), String> {
    let mut outside = Vec::new();
    let mut ics = Ics::parse(text);
    ics.cancel();
    for email in people {
        match local_user(instance, email).await {
            Some(user) => {
                ics.set_method(None);
                if let Ok(Some(_)) = store::event_by_uid(
                    instance.store.as_ref(),
                    &user,
                    &ics.uid().unwrap_or_default(),
                )
                .await
                {
                    upsert(instance, &user, &ics.text()).await?;
                }
            }
            None => outside.push(email.clone()),
        }
    }
    if !outside.is_empty() {
        ics.set_method(Some("CANCEL"));
        let subject = ics.summary();
        mailer(instance)
            .await?
            .send_invitation(
                organizer,
                &outside,
                &format!("Cancelled: {subject}"),
                &format!("This event is cancelled: {subject}"),
                &ics.text(),
            )
            .await?;
    }
    Ok(())
}

/// The person answers an invitation in their calendar.
pub async fn respond(
    instance: &Instance,
    user: &str,
    uid: &str,
    answer: &str,
) -> Result<Invitation, String> {
    let answer = answer.to_ascii_uppercase();
    if !["ACCEPTED", "TENTATIVE", "DECLINED"].contains(&answer.as_str()) {
        return Err("bad_answer".into());
    }
    let me = email_of(instance, user).await.ok_or("no_account")?;
    let s = instance.store.as_ref();
    let mine = store::event_by_uid(s, user, uid)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("not_found")?;
    let mut ics = Ics::parse(&mine.ical);
    if !ics.set_partstat(&me, &answer) {
        return Err("not_invited".into());
    }
    let stored = upsert(instance, user, &ics.text()).await?;
    let organizer = ics.organizer().ok_or("no_organizer")?;
    match local_user(instance, &organizer).await {
        Some(organizer_user) => {
            // The organiser's copy shows the answer.
            if let Ok(Some(theirs)) = store::event_by_uid(s, &organizer_user, uid).await {
                let mut copy = Ics::parse(&theirs.ical);
                if copy.set_partstat(&me, &answer) {
                    upsert(instance, &organizer_user, &copy.text()).await?;
                }
            }
        }
        None => {
            let reply = ics.reply_for(&me);
            let verb = match answer.as_str() {
                "ACCEPTED" => "Accepted",
                "TENTATIVE" => "Tentative",
                _ => "Declined",
            };
            mailer(instance)
                .await?
                .send_invitation(
                    user,
                    std::slice::from_ref(&organizer),
                    &format!("{verb}: {}", ics.summary()),
                    &format!("{verb}: {}", ics.summary()),
                    &reply.text(),
                )
                .await?;
        }
    }
    Ok(invitation("REQUEST", &ics, Some(answer), Some(&stored)))
}

/// An iMIP message for the person from `sender` (as mail verified it).
pub async fn receive(
    instance: &Instance,
    user: &str,
    sender: &str,
    text: &str,
) -> Result<Invitation, String> {
    store::facts(&{
        let mut plain = Ics::parse(text);
        plain.set_method(None);
        plain.text()
    })?;
    let ics = Ics::parse(text);
    let method = ics.method().unwrap_or_else(|| "PUBLISH".into());
    let sender = sender.trim().to_ascii_lowercase();
    let me = email_of(instance, user).await.ok_or("no_account")?;
    let uid = ics.uid().ok_or("no_uid")?;
    let s = instance.store.as_ref();
    let existing = store::event_by_uid(s, user, &uid)
        .await
        .map_err(|e| e.to_string())?;
    match method.as_str() {
        "REQUEST" | "PUBLISH" => {
            // Only the organiser changes an event already in the calendar.
            if let Some(e) = &existing {
                if Ics::parse(&e.ical).organizer().is_some_and(|o| o != sender) {
                    return Err("not_the_organizer".into());
                }
            }
            let mut copy = Ics::parse(text);
            copy.set_method(None);
            let answer = existing
                .as_ref()
                .and_then(|e| Ics::parse(&e.ical).partstat(&me))
                .or_else(|| copy.partstat(&me));
            if let Some(answer) = &answer {
                copy.set_partstat(&me, answer);
            }
            let stored = upsert(instance, user, &copy.text()).await?;
            Ok(invitation(&method, &copy, answer, Some(&stored)))
        }
        "REPLY" => {
            let Some(e) = existing else {
                return Err("not_found".into());
            };
            let mut mine = Ics::parse(&e.ical);
            if mine.organizer().is_some_and(|o| o != me) {
                return Err("not_mine".into());
            }
            // The answer counts only for the one who sent it.
            let answer = ics.partstat(&sender).ok_or("not_an_attendee")?;
            if !mine.set_partstat(&sender, &answer) {
                return Err("not_an_attendee".into());
            }
            let stored = upsert(instance, user, &mine.text()).await?;
            Ok(invitation("REPLY", &mine, Some(answer), Some(&stored)))
        }
        "CANCEL" => {
            let Some(e) = existing else {
                return Err("not_found".into());
            };
            let mut mine = Ics::parse(&e.ical);
            if mine.organizer().is_none_or(|o| o != sender) {
                return Err("not_the_organizer".into());
            }
            mine.cancel();
            let stored = upsert(instance, user, &mine.text()).await?;
            let answer = mine.partstat(&me);
            Ok(invitation("CANCEL", &mine, answer, Some(&stored)))
        }
        _ => Err("unsupported_method".into()),
    }
}

/// The person's events in a range, for the agenda.
pub async fn agenda(
    instance: &Instance,
    user: &str,
    from: i64,
    to: i64,
) -> Result<Vec<AgendaItem>, String> {
    let s = instance.store.as_ref();
    let me = email_of(instance, user).await.unwrap_or_default();
    let ids: Vec<String> = store::calendars(s, user)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let mut out = Vec::new();
    for o in store::occurrences_in(s, &ids, from, to)
        .await
        .map_err(|e| e.to_string())?
    {
        let event = store::event(s, &o.calendar_id, &o.href)
            .await
            .map_err(|e| e.to_string())?;
        let answer = event.as_ref().and_then(|e| {
            let ics = Ics::parse(&e.ical);
            if ics.organizer().is_some_and(|org| org != me) {
                ics.partstat(&me)
            } else {
                None
            }
        });
        if event
            .as_ref()
            .is_some_and(|e| e.ical.contains("STATUS:CANCELLED"))
        {
            continue;
        }
        out.push(AgendaItem {
            uid: o.uid.clone(),
            summary: o.summary.clone(),
            start_ms: o.start,
            end_ms: o.end,
            all_day: o.all_day,
            answer,
            item: ItemRef {
                app: "calendar".into(),
                id: o.event.clone(),
                path: format!("/calendar?event={}", o.event),
            },
        });
    }
    Ok(out)
}

/// When everyone is free: starts (Unix ms) every half hour between `from`
/// and `to` where `minutes` fit for the person and `people` (addresses of
/// colleagues; people outside are not known and do not count).
pub async fn free_times(
    instance: &Instance,
    user: &str,
    people: &[String],
    from: i64,
    to: i64,
    minutes: i64,
    limit: usize,
) -> Result<Vec<i64>, String> {
    let s = instance.store.as_ref();
    let mut users = vec![user.to_string()];
    for email in people.iter().take(50) {
        if let Some(u) = local_user(instance, &email.to_ascii_lowercase()).await {
            if !users.contains(&u) {
                users.push(u);
            }
        }
    }
    let mut busy: Vec<(i64, i64)> = Vec::new();
    for u in &users {
        let ids: Vec<String> = store::calendars(s, u)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|c| c.id)
            .collect();
        for o in store::occurrences_in(s, &ids, from, to)
            .await
            .map_err(|e| e.to_string())?
        {
            if o.busy && !o.all_day {
                busy.push((o.start, o.end));
            }
        }
    }
    let length = minutes.clamp(5, 24 * 60) * 60_000;
    let step = 30 * 60_000;
    let mut start = from.div_euclid(step) * step;
    if start < from {
        start += step;
    }
    let mut out = Vec::new();
    while start + length <= to && out.len() < limit.min(200) {
        let end = start + length;
        if !busy.iter().any(|(b, e)| *b < end && *e > start) {
            out.push(start);
        }
        start += step;
    }
    Ok(out)
}

/// The calendar as other apps see it (`interfaces::Calendar`).
pub struct Service {
    pub instance: std::sync::Weak<Instance>,
}

impl Service {
    fn instance(&self) -> Result<Arc<Instance>, String> {
        self.instance
            .upgrade()
            .ok_or_else(|| "stopping".to_string())
    }
}

impl ortus_core::interfaces::Calendar for Service {
    fn receive<'a>(
        &'a self,
        user: &'a str,
        sender: &'a str,
        ics: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<Invitation, String>> {
        Box::pin(async move {
            let instance = self.instance()?;
            receive(&instance, user, sender, ics).await
        })
    }
    fn respond<'a>(
        &'a self,
        user: &'a str,
        uid: &'a str,
        answer: &'a str,
    ) -> ortus_core::storage::BoxFuture<'a, Result<Invitation, String>> {
        Box::pin(async move {
            let instance = self.instance()?;
            respond(&instance, user, uid, answer).await
        })
    }
    fn agenda<'a>(
        &'a self,
        user: &'a str,
        from_ms: i64,
        to_ms: i64,
    ) -> ortus_core::storage::BoxFuture<'a, Result<Vec<AgendaItem>, String>> {
        Box::pin(async move {
            let instance = self.instance()?;
            agenda(&instance, user, from_ms, to_ms).await
        })
    }
    fn free_times<'a>(
        &'a self,
        user: &'a str,
        people: &'a [String],
        from_ms: i64,
        to_ms: i64,
        minutes: i64,
        limit: usize,
    ) -> ortus_core::storage::BoxFuture<'a, Result<Vec<i64>, String>> {
        Box::pin(async move {
            let instance = self.instance()?;
            free_times(&instance, user, people, from_ms, to_ms, minutes, limit).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Ortus//EN\r\nBEGIN:VEVENT\r\nUID:review@ministry.example\r\nDTSTAMP:20261120T090000Z\r\nDTSTART:20261126T140000Z\r\nDTEND:20261126T143000Z\r\nSUMMARY:Shelter agreement review\\, legal\r\nORGANIZER;CN=Olena:mailto:olena@ministry.example\r\nATTENDEE;CN=\"Kowalska, Anna\";PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:anna@regional.example\r\nATTENDEE;PARTSTAT=ACCEPTED:mailto:olena@ministry.example\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn lines_are_read_and_changed() {
        let mut ics = Ics::parse(EVENT);
        assert_eq!(Some("review@ministry.example".into()), ics.uid());
        assert_eq!(Some("olena@ministry.example".into()), ics.organizer());
        assert_eq!("Shelter agreement review, legal", ics.summary());
        assert_eq!(
            Some("NEEDS-ACTION".into()),
            ics.partstat("anna@regional.example")
        );
        assert!(ics.set_partstat("anna@regional.example", "ACCEPTED"));
        assert_eq!(
            Some("ACCEPTED".into()),
            ics.partstat("anna@regional.example")
        );
        assert!(
            ics.text().contains("CN=\"Kowalska, Anna\""),
            "{}",
            ics.text()
        );
        ics.set_method(Some("REQUEST"));
        assert_eq!(Some("REQUEST".into()), ics.method());
        let reply = ics.reply_for("anna@regional.example");
        assert_eq!(Some("REPLY".into()), reply.method());
        assert_eq!(1, reply.attendees().len());
        assert!(!reply.text().contains("VALARM"));
        let mut gone = Ics::parse(EVENT);
        gone.cancel();
        assert!(gone.text().contains("STATUS:CANCELLED"));
        assert!(store::facts(&{
            let mut plain = Ics::parse(&ics.text());
            plain.set_method(None);
            plain.text()
        })
        .is_ok());
        let long = format!("SUMMARY:{}", "é".repeat(100));
        assert!(fold(&[long]).lines().all(|l| l.len() <= 76));
    }
}
