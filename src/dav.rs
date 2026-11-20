//! CalDAV (RFC 4791) and CardDAV (RFC 6352) for apps (Thunderbird, Apple
//! Calendar and Contacts, DAVx5), with sync-collection (RFC 6578). Signed
//! in by `Authorization` (an app password, or a session token).
//!
//! Layout, the same for both (`/dav/calendar`, `/dav/people`):
//! `/` the root, `/principal/` the person, `/home/` their collections,
//! `/home/<collection>/` one calendar or address book, and
//! `/home/<collection>/<name>` one event or card. The people app also
//! has `/home/directory/`: the installation's people, read-only.

use std::sync::Arc;

use ortus_core::auth::accounts::User;
use ortus_core::instance::Instance;
use ortus_core::storage::BoxFuture;
use quick_xml::events::Event;
use rust_web_server::header::Header;
use rust_web_server::prelude::*;

use crate::{contacts, store};

const DAV: &str = "DAV:";
const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
const CS: &str = "http://calendarserver.org/ns/";
const APPLE: &str = "http://apple.com/ns/ical/";
const MAX_BODY: usize = 1024 * 1024;
const DIRECTORY: &str = "directory";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Calendar,
    People,
}

impl Kind {
    fn base(self) -> &'static str {
        match self {
            Kind::Calendar => "/dav/calendar",
            Kind::People => "/dav/people",
        }
    }
    fn item_type(self) -> &'static str {
        match self {
            Kind::Calendar => "text/calendar; charset=utf-8",
            Kind::People => "text/vcard; charset=utf-8",
        }
    }
}

/// What a path names.
enum Target {
    Root,
    Principal,
    Home,
    Collection(String),
    Item(String, String),
}

fn target(rest: &str) -> Option<Target> {
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => Some(Target::Root),
        ["principal"] => Some(Target::Principal),
        ["home"] => Some(Target::Home),
        ["home", collection] => Some(Target::Collection(decode(collection))),
        ["home", collection, item] => Some(Target::Item(decode(collection), decode(item))),
        _ => None,
    }
}

fn decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                16,
            ) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~@".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.to_string().into(),
        value: value.to_string().into(),
    }
}

fn status(
    code: &rust_web_server::response::StatusCodeReasonPhrase,
    body: &str,
    headers: Vec<Header>,
) -> Response {
    let mut response = Response::text(code, body);
    response.headers.extend(headers);
    response
}

fn multistatus(responses: &str, sync_token: Option<String>) -> Response {
    let token = sync_token
        .map(|t| format!("<D:sync-token>{}</D:sync-token>", escape(&t)))
        .unwrap_or_default();
    let body = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"{CALDAV}\" xmlns:CR=\"{CARDDAV}\" xmlns:CS=\"{CS}\" xmlns:A=\"{APPLE}\">{responses}{token}</D:multistatus>");
    let mut response = Response::get_response(
        STATUS_CODE_REASON_PHRASE.n207_multi_status,
        Some(vec![header(
            "Content-Type",
            "application/xml; charset=utf-8",
        )]),
        Some(vec![rust_web_server::range::Range::get_content_range(
            body.into_bytes(),
            "application/xml; charset=utf-8".to_string(),
        )]),
    );
    response
        .headers
        .retain(|h| !h.name.eq_ignore_ascii_case("Accept-Ranges"));
    response
}

fn sync_uri(n: i64) -> String {
    format!("https://ortus.invalid/sync/{n}")
}

fn sync_number(token: &str) -> Option<i64> {
    token
        .trim()
        .strip_prefix("https://ortus.invalid/sync/")?
        .parse()
        .ok()
}

/// What a request body asks for.
#[derive(Default)]
struct Ask {
    /// The root element: propfind, calendar-multiget, sync-collection...
    root: String,
    /// Properties as "namespace|name"; empty with `all` for allprop.
    props: Vec<String>,
    all: bool,
    hrefs: Vec<String>,
    sync_token: Option<String>,
    /// A time-range in a calendar-query (start, end) as given.
    range: (Option<String>, Option<String>),
    /// Values set by PROPPATCH ("namespace|name", text).
    set: Vec<(String, String)>,
}

fn parse(body: &[u8]) -> Result<Ask, String> {
    let mut ask = Ask::default();
    if body.iter().all(u8::is_ascii_whitespace) {
        ask.all = true;
        return Ok(ask);
    }
    let mut reader = quick_xml::NsReader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    let mut text = String::new();
    loop {
        let (resolved, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|e| e.to_string())?;
        let ns = match resolved {
            quick_xml::name::ResolveResult::Bound(n) => n.as_ref().to_string(),
            _ => String::new(),
        };
        let open =
            |e: &quick_xml::events::BytesStart<'_>, stack: &Vec<String>, ask: &mut Ask| -> String {
                let local = e.local_name().as_ref().to_string();
                let key = format!("{ns}|{local}");
                let parent = stack.last().cloned().unwrap_or_default();
                if stack.is_empty() {
                    ask.root = key.clone();
                }
                if parent.ends_with("|prop")
                    && !ask.root.ends_with("|propertyupdate")
                    && !ask.root.ends_with("|mkcalendar")
                {
                    ask.props.push(key.clone());
                }
                if key == format!("{DAV}|allprop") || key == format!("{DAV}|propname") {
                    ask.all = true;
                }
                if local == "time-range" {
                    for a in e.attributes().flatten() {
                        let value = a.value.to_string();
                        match a.key.local_name().as_ref() {
                            "start" => ask.range.0 = Some(value),
                            "end" => ask.range.1 = Some(value),
                            _ => {}
                        }
                    }
                }
                key
            };
        match event {
            Event::Start(e) => {
                if stack.len() >= 64 {
                    return Err("nested too deep".into());
                }
                let key = open(&e, &stack, &mut ask);
                stack.push(key);
                text.clear();
            }
            Event::Empty(e) => {
                let key = open(&e, &stack, &mut ask);
                let parent = stack.last().cloned().unwrap_or_default();
                if parent.ends_with("|prop")
                    && (ask.root.ends_with("|propertyupdate") || ask.root.ends_with("|mkcalendar"))
                {
                    ask.set.push((key, String::new()));
                }
            }
            Event::End(_) => {
                let key = stack.pop().unwrap_or_default();
                let parent = stack.last().cloned().unwrap_or_default();
                if key == format!("{DAV}|href") {
                    ask.hrefs.push(text.trim().to_string());
                }
                if key == format!("{DAV}|sync-token") {
                    ask.sync_token = Some(text.trim().to_string());
                }
                if parent.ends_with("|prop")
                    && (ask.root.ends_with("|propertyupdate") || ask.root.ends_with("|mkcalendar"))
                {
                    ask.set.push((key, text.trim().to_string()));
                }
                text.clear();
            }
            Event::Text(t) => text.push_str(t.as_ref()),
            Event::GeneralRef(r) => text.push_str(match r.as_ref() {
                "amp" => "&",
                "lt" => "<",
                "gt" => ">",
                "quot" => "\"",
                "apos" => "'",
                _ => "",
            }),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(ask)
}

/// One resource's properties, answered as asked: (found, not found).
fn propstat(href: &str, found: &[(String, String)], missing: &[String]) -> String {
    let mut out = format!("<D:response><D:href>{}</D:href>", escape(href));
    if !found.is_empty() {
        out.push_str("<D:propstat><D:prop>");
        for (_, xml) in found {
            out.push_str(xml);
        }
        out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    }
    if !missing.is_empty() {
        out.push_str("<D:propstat><D:prop>");
        for key in missing {
            let (ns, local) = key.split_once('|').unwrap_or(("", key));
            out.push_str(&format!("<X:{local} xmlns:X=\"{}\"/>", escape(ns)));
        }
        out.push_str("</D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>");
    }
    out.push_str("</D:response>");
    out
}

/// Every property a resource has, by key, as XML.
type Props = Vec<(String, String)>;

fn key(ns: &str, local: &str) -> String {
    format!("{ns}|{local}")
}

fn answer(href: &str, have: &Props, ask: &Ask) -> String {
    if ask.all || ask.props.is_empty() {
        let found: Props = have
            .iter()
            .filter(|(k, _)| !k.ends_with("|calendar-data") && !k.ends_with("|address-data"))
            .cloned()
            .collect();
        return propstat(href, &found, &[]);
    }
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for wanted in &ask.props {
        match have.iter().find(|(k, _)| k == wanted) {
            Some(p) => found.push(p.clone()),
            None => missing.push(wanted.clone()),
        }
    }
    propstat(href, &found, &missing)
}

fn color_hex(color: &str) -> &'static str {
    match color {
        "teal" => "#4FD1C5FF",
        "orange" => "#F5A35CFF",
        "lime" => "#C6F06BFF",
        "red" => "#FF7A6EFF",
        "violet" => "#C9A7FFFF",
        _ => "#8EA2FFFF",
    }
}

fn color_name(hex: &str) -> &'static str {
    let hex = hex.trim().to_ascii_uppercase();
    for name in ["teal", "orange", "lime", "red", "violet", "accent"] {
        if color_hex(name).starts_with(&hex[..hex.len().min(7)]) {
            return name;
        }
    }
    "accent"
}

struct Ctx<'a> {
    instance: &'a Arc<Instance>,
    user: &'a User,
    kind: Kind,
}

impl Ctx<'_> {
    fn store(&self) -> &dyn ortus_core::storage::AsyncStore {
        self.instance.store.as_ref()
    }
    fn principal(&self) -> String {
        format!("{}/principal/", self.kind.base())
    }
    fn home(&self) -> String {
        format!("{}/home/", self.kind.base())
    }

    fn person_props(&self, target: &Target) -> Props {
        let mut p = vec![
            (key(DAV, "current-user-principal"), format!("<D:current-user-principal><D:href>{}</D:href></D:current-user-principal>", self.principal())),
            (key(DAV, "principal-URL"), format!("<D:principal-URL><D:href>{}</D:href></D:principal-URL>", self.principal())),
            (key(DAV, "displayname"), format!("<D:displayname>{}</D:displayname>", escape(&self.user.name))),
            (key(DAV, "owner"), format!("<D:owner><D:href>{}</D:href></D:owner>", self.principal())),
            (key(DAV, "current-user-privilege-set"), "<D:current-user-privilege-set><D:privilege><D:read/></D:privilege></D:current-user-privilege-set>".into()),
        ];
        let resourcetype = match target {
            Target::Principal => "<D:resourcetype><D:principal/></D:resourcetype>",
            _ => "<D:resourcetype><D:collection/></D:resourcetype>",
        };
        p.push((key(DAV, "resourcetype"), resourcetype.into()));
        // Both homes, whichever way an app came in: one principal.
        p.push((
            key(CALDAV, "calendar-home-set"),
            format!(
                "<C:calendar-home-set><D:href>{}/home/</D:href></C:calendar-home-set>",
                Kind::Calendar.base()
            ),
        ));
        p.push((key(CALDAV, "calendar-user-address-set"), format!("<C:calendar-user-address-set><D:href>mailto:{}</D:href></C:calendar-user-address-set>", escape(&self.user.email))));
        p.push((
            key(CARDDAV, "addressbook-home-set"),
            format!(
                "<CR:addressbook-home-set><D:href>{}/home/</D:href></CR:addressbook-home-set>",
                Kind::People.base()
            ),
        ));
        p
    }

    fn calendar_props(&self, c: &store::Calendar) -> Props {
        vec![
            (key(DAV, "resourcetype"), "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>".into()),
            (key(DAV, "displayname"), format!("<D:displayname>{}</D:displayname>", escape(&c.name))),
            (key(CS, "getctag"), format!("<CS:getctag>\"{}\"</CS:getctag>", c.sync)),
            (key(DAV, "sync-token"), format!("<D:sync-token>{}</D:sync-token>", sync_uri(c.sync))),
            (key(CALDAV, "supported-calendar-component-set"), "<C:supported-calendar-component-set><C:comp name=\"VEVENT\"/></C:supported-calendar-component-set>".into()),
            (key(APPLE, "calendar-color"), format!("<A:calendar-color>{}</A:calendar-color>", color_hex(&c.color))),
            (key(DAV, "owner"), format!("<D:owner><D:href>{}</D:href></D:owner>", self.principal())),
            (key(DAV, "current-user-privilege-set"), "<D:current-user-privilege-set><D:privilege><D:all/></D:privilege><D:privilege><D:read/></D:privilege><D:privilege><D:write/></D:privilege></D:current-user-privilege-set>".into()),
            (key(DAV, "supported-report-set"), "<D:supported-report-set><D:supported-report><D:report><C:calendar-multiget/></D:report></D:supported-report><D:supported-report><D:report><C:calendar-query/></D:report></D:supported-report><D:supported-report><D:report><D:sync-collection/></D:report></D:supported-report></D:supported-report-set>".into()),
        ]
    }

    fn book_props(&self, id: &str, name: &str, sync: i64, writable: bool) -> Props {
        let privileges = if writable {
            "<D:privilege><D:all/></D:privilege><D:privilege><D:read/></D:privilege><D:privilege><D:write/></D:privilege>"
        } else {
            "<D:privilege><D:read/></D:privilege>"
        };
        let _ = id;
        vec![
            (key(DAV, "resourcetype"), "<D:resourcetype><D:collection/><CR:addressbook/></D:resourcetype>".into()),
            (key(DAV, "displayname"), format!("<D:displayname>{}</D:displayname>", escape(name))),
            (key(CS, "getctag"), format!("<CS:getctag>\"{sync}\"</CS:getctag>")),
            (key(DAV, "sync-token"), format!("<D:sync-token>{}</D:sync-token>", sync_uri(sync))),
            (key(CARDDAV, "supported-address-data"), "<CR:supported-address-data><CR:address-data-type content-type=\"text/vcard\" version=\"3.0\"/><CR:address-data-type content-type=\"text/vcard\" version=\"4.0\"/></CR:supported-address-data>".into()),
            (key(DAV, "owner"), format!("<D:owner><D:href>{}</D:href></D:owner>", self.principal())),
            (key(DAV, "current-user-privilege-set"), format!("<D:current-user-privilege-set>{privileges}</D:current-user-privilege-set>")),
            (key(DAV, "supported-report-set"), "<D:supported-report-set><D:supported-report><D:report><CR:addressbook-multiget/></D:report></D:supported-report><D:supported-report><D:report><CR:addressbook-query/></D:report></D:supported-report><D:supported-report><D:report><D:sync-collection/></D:report></D:supported-report></D:supported-report-set>".into()),
        ]
    }

    fn item_props(&self, etag: &str, data: &str) -> Props {
        let data_key = match self.kind {
            Kind::Calendar => (
                key(CALDAV, "calendar-data"),
                format!("<C:calendar-data>{}</C:calendar-data>", escape(data)),
            ),
            Kind::People => (
                key(CARDDAV, "address-data"),
                format!("<CR:address-data>{}</CR:address-data>", escape(data)),
            ),
        };
        vec![
            (key(DAV, "resourcetype"), "<D:resourcetype/>".into()),
            (
                key(DAV, "getetag"),
                format!("<D:getetag>{}</D:getetag>", escape(etag)),
            ),
            (
                key(DAV, "getcontenttype"),
                format!(
                    "<D:getcontenttype>{}</D:getcontenttype>",
                    self.kind.item_type()
                ),
            ),
            data_key,
        ]
    }

    /// The person's collections: (path, props, items as (name, etag, data)).
    async fn collections(&self) -> Result<Vec<(String, Props)>, String> {
        let s = self.store();
        Ok(match self.kind {
            Kind::Calendar => store::calendars(s, &self.user.id)
                .await
                .map_err(|e| e.to_string())?
                .iter()
                .map(|c| (c.path.clone(), self.calendar_props(c)))
                .collect(),
            Kind::People => {
                let mut out: Vec<(String, Props)> = contacts::books(s, &self.user.id)
                    .await
                    .map_err(|e| e.to_string())?
                    .iter()
                    .map(|b| (b.id.clone(), self.book_props(&b.id, &b.name, b.sync, true)))
                    .collect();
                let count = contacts::directory(s, "", 1000)
                    .await
                    .map_err(|e| e.to_string())?
                    .len() as i64;
                out.push((
                    DIRECTORY.into(),
                    self.book_props(DIRECTORY, "Directory", count, false),
                ));
                out
            }
        })
    }

    async fn collection(&self, path: &str) -> Result<Option<Props>, String> {
        Ok(self
            .collections()
            .await?
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, props)| props))
    }

    /// Items of a collection: (name, etag, data).
    async fn items(&self, path: &str) -> Result<Vec<(String, String, String)>, String> {
        let s = self.store();
        match self.kind {
            Kind::Calendar => {
                let Some(c) = store::calendar_at(s, &self.user.id, path)
                    .await
                    .map_err(|e| e.to_string())?
                else {
                    return Ok(Vec::new());
                };
                Ok(store::events(s, &c.id)
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|e| (e.href, e.etag, e.ical))
                    .collect())
            }
            Kind::People if path == DIRECTORY => Ok(contacts::directory(s, "", 1000)
                .await
                .map_err(|e| e.to_string())?
                .iter()
                .map(|c| {
                    let card = contacts::colleague_vcard(c);
                    (format!("{}.vcf", c.id), contacts::etag_of(&card), card)
                })
                .collect()),
            Kind::People => {
                if contacts::book(s, &self.user.id, path)
                    .await
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    return Ok(Vec::new());
                }
                Ok(contacts::cards(s, path)
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|c| (c.href, c.etag, c.vcard))
                    .collect())
            }
        }
    }

    async fn item(&self, path: &str, name: &str) -> Result<Option<(String, String)>, String> {
        let s = self.store();
        match self.kind {
            Kind::Calendar => {
                let Some(c) = store::calendar_at(s, &self.user.id, path)
                    .await
                    .map_err(|e| e.to_string())?
                else {
                    return Ok(None);
                };
                Ok(store::event(s, &c.id, name)
                    .await
                    .map_err(|e| e.to_string())?
                    .map(|e| (e.etag, e.ical)))
            }
            Kind::People if path == DIRECTORY => Ok(self
                .items(path)
                .await?
                .into_iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, e, d)| (e, d))),
            Kind::People => {
                if contacts::book(s, &self.user.id, path)
                    .await
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(contacts::card(s, path, name)
                    .await
                    .map_err(|e| e.to_string())?
                    .map(|c| (c.etag, c.vcard)))
            }
        }
    }

    fn collection_href(&self, path: &str) -> String {
        format!("{}{}/", self.home(), encode(path))
    }

    fn item_href(&self, path: &str, name: &str) -> String {
        format!("{}{}/{}", self.home(), encode(path), encode(name))
    }
}

fn depth(request: &Request) -> u8 {
    match request
        .header("depth")
        .map(|h| h.value.trim().to_string())
        .as_deref()
    {
        Some("0") => 0,
        Some("1") => 1,
        // infinity is answered as 1.
        Some(_) => 1,
        None => 1,
    }
}

async fn propfind(
    ctx: &Ctx<'_>,
    request: &Request,
    target: &Target,
    ask: &Ask,
) -> Result<Response, String> {
    let mut out = String::new();
    let deep = depth(request) >= 1;
    match target {
        Target::Root | Target::Principal => {
            let href = if matches!(target, Target::Root) {
                format!("{}/", ctx.kind.base())
            } else {
                ctx.principal()
            };
            out.push_str(&answer(&href, &ctx.person_props(target), ask));
        }
        Target::Home => {
            out.push_str(&answer(&ctx.home(), &ctx.person_props(target), ask));
            if deep {
                for (path, props) in ctx.collections().await? {
                    out.push_str(&answer(&ctx.collection_href(&path), &props, ask));
                }
            }
        }
        Target::Collection(path) => {
            let Some(props) = ctx.collection(path).await? else {
                return Ok(status(
                    STATUS_CODE_REASON_PHRASE.n404_not_found,
                    "Not found",
                    vec![],
                ));
            };
            out.push_str(&answer(&ctx.collection_href(path), &props, ask));
            if deep {
                for (name, etag, data) in ctx.items(path).await? {
                    out.push_str(&answer(
                        &ctx.item_href(path, &name),
                        &ctx.item_props(&etag, &data),
                        ask,
                    ));
                }
            }
        }
        Target::Item(path, name) => {
            let Some((etag, data)) = ctx.item(path, name).await? else {
                return Ok(status(
                    STATUS_CODE_REASON_PHRASE.n404_not_found,
                    "Not found",
                    vec![],
                ));
            };
            out.push_str(&answer(
                &ctx.item_href(path, name),
                &ctx.item_props(&etag, &data),
                ask,
            ));
        }
    }
    Ok(multistatus(&out, None))
}

/// An iCalendar UTC time (20261101T000000Z) as Unix ms.
fn ical_time(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.len() < 15 {
        return None;
    }
    let n = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (
        n(0, 4)?,
        n(4, 6)?,
        n(6, 8)?,
        n(9, 11)?,
        n(11, 13)?,
        n(13, 15)?,
    );
    // Days from civil (Howard Hinnant).
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + s * 1000)
}

async fn report(ctx: &Ctx<'_>, target: &Target, ask: &Ask) -> Result<Response, String> {
    let Target::Collection(path) = target else {
        return Ok(status(
            STATUS_CODE_REASON_PHRASE.n403_forbidden,
            "Reports are on collections",
            vec![],
        ));
    };
    if ctx.collection(path).await?.is_none() {
        return Ok(status(
            STATUS_CODE_REASON_PHRASE.n404_not_found,
            "Not found",
            vec![],
        ));
    }
    let root = ask.root.rsplit('|').next().unwrap_or_default().to_string();
    let mut out = String::new();
    match root.as_str() {
        "calendar-multiget" | "addressbook-multiget" => {
            for href in &ask.hrefs {
                let name = decode(href.rsplit('/').next().unwrap_or_default());
                match ctx.item(path, &name).await? {
                    Some((etag, data)) => out.push_str(&answer(href, &ctx.item_props(&etag, &data), ask)),
                    None => out.push_str(&format!("<D:response><D:href>{}</D:href><D:status>HTTP/1.1 404 Not Found</D:status></D:response>", escape(href))),
                }
            }
            Ok(multistatus(&out, None))
        }
        "calendar-query" | "addressbook-query" => {
            let wanted: Option<std::collections::HashSet<String>> = match (ctx.kind, &ask.range) {
                (Kind::Calendar, (Some(start), end)) => {
                    let from = ical_time(start).unwrap_or(i64::MIN / 2);
                    let to = end.as_deref().and_then(ical_time).unwrap_or(i64::MAX / 2);
                    let calendar = store::calendar_at(ctx.store(), &ctx.user.id, path)
                        .await
                        .map_err(|e| e.to_string())?
                        .map(|c| c.id);
                    let ids: Vec<String> = calendar.into_iter().collect();
                    Some(
                        store::occurrences_in(ctx.store(), &ids, from, to)
                            .await
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .map(|o| o.href)
                            .collect(),
                    )
                }
                _ => None,
            };
            for (name, etag, data) in ctx.items(path).await? {
                if wanted.as_ref().is_none_or(|w| w.contains(&name)) {
                    out.push_str(&answer(
                        &ctx.item_href(path, &name),
                        &ctx.item_props(&etag, &data),
                        ask,
                    ));
                }
            }
            Ok(multistatus(&out, None))
        }
        "sync-collection" => {
            let since = match ask.sync_token.as_deref().filter(|t| !t.is_empty()) {
                None => 0,
                Some(t) => match sync_number(t) {
                    Some(n) => n,
                    None => return Ok(sync_refused()),
                },
            };
            let (changes, now): (Vec<(String, bool)>, i64) = match ctx.kind {
                Kind::People if path == DIRECTORY => {
                    // The directory is small: every card, every time.
                    let items = ctx.items(path).await?;
                    let n = items.len() as i64;
                    (
                        items
                            .into_iter()
                            .map(|(name, _, _)| (name, false))
                            .collect(),
                        n,
                    )
                }
                Kind::People => match contacts::changes_since(ctx.store(), path, since)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    Some(found) => found,
                    None => return Ok(sync_refused()),
                },
                Kind::Calendar => {
                    let Some(c) = store::calendar_at(ctx.store(), &ctx.user.id, path)
                        .await
                        .map_err(|e| e.to_string())?
                    else {
                        return Ok(status(
                            STATUS_CODE_REASON_PHRASE.n404_not_found,
                            "Not found",
                            vec![],
                        ));
                    };
                    match store::changes_since(ctx.store(), &c.id, since)
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        Some(found) => found,
                        None => return Ok(sync_refused()),
                    }
                }
            };
            for (name, deleted) in changes {
                let href = ctx.item_href(path, &name);
                match (deleted, ctx.item(path, &name).await?) {
                    (false, Some((etag, data))) => out.push_str(&answer(&href, &ctx.item_props(&etag, &data), ask)),
                    _ => out.push_str(&format!("<D:response><D:href>{}</D:href><D:status>HTTP/1.1 404 Not Found</D:status></D:response>", escape(&href))),
                }
            }
            Ok(multistatus(&out, Some(sync_uri(now))))
        }
        _ => Ok(status(
            STATUS_CODE_REASON_PHRASE.n403_forbidden,
            "That report is not supported",
            vec![],
        )),
    }
}

fn sync_refused() -> Response {
    let body = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:error xmlns:D=\"DAV:\"><D:valid-sync-token/></D:error>";
    status(
        STATUS_CODE_REASON_PHRASE.n403_forbidden,
        body,
        vec![header("Content-Type", "application/xml; charset=utf-8")],
    )
}

fn precondition(
    code: &rust_web_server::response::StatusCodeReasonPhrase,
    ns: &str,
    name: &str,
) -> Response {
    let body = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:error xmlns:D=\"DAV:\" xmlns:X=\"{ns}\"><X:{name}/></D:error>");
    status(
        code,
        &body,
        vec![header("Content-Type", "application/xml; charset=utf-8")],
    )
}

async fn put(ctx: &Ctx<'_>, request: &Request, target: &Target) -> Result<Response, String> {
    let Target::Item(path, name) = target else {
        return Ok(status(
            STATUS_CODE_REASON_PHRASE.n405_method_not_allowed,
            "PUT goes to an item",
            vec![],
        ));
    };
    let body = String::from_utf8_lossy(&request.body[..]).into_owned();
    let if_match = request
        .header("if-match")
        .map(|h| h.value.trim().to_string());
    let if_none_match = request
        .header("if-none-match")
        .is_some_and(|h| h.value.trim() == "*");
    let s = ctx.store();
    let result = match ctx.kind {
        Kind::Calendar => {
            let Some(c) = store::calendar_at(s, &ctx.user.id, path)
                .await
                .map_err(|e| e.to_string())?
            else {
                return Ok(status(
                    STATUS_CODE_REASON_PHRASE.n404_not_found,
                    "No such calendar",
                    vec![],
                ));
            };
            store::put_event(
                s,
                &ctx.user.id,
                &c.id,
                name,
                &body,
                if_match.as_deref(),
                if_none_match,
            )
            .await
        }
        Kind::People if path == DIRECTORY => {
            return Ok(status(
                STATUS_CODE_REASON_PHRASE.n403_forbidden,
                "The directory is read-only",
                vec![],
            ))
        }
        Kind::People => {
            contacts::put_card(
                s,
                &ctx.user.id,
                path,
                name,
                &body,
                if_match.as_deref(),
                if_none_match,
            )
            .await
        }
    };
    Ok(match result {
        Ok(put) => status(
            if put.created {
                STATUS_CODE_REASON_PHRASE.n201_created
            } else {
                STATUS_CODE_REASON_PHRASE.n204_no_content
            },
            "",
            vec![header("ETag", &put.etag)],
        ),
        Err(store::PutError::Precondition) => status(
            STATUS_CODE_REASON_PHRASE.n412_precondition_failed,
            "It changed since",
            vec![],
        ),
        Err(store::PutError::NotFound) => status(
            STATUS_CODE_REASON_PHRASE.n404_not_found,
            "No such collection",
            vec![],
        ),
        Err(store::PutError::UidTaken) => match ctx.kind {
            Kind::Calendar => precondition(
                STATUS_CODE_REASON_PHRASE.n409_conflict,
                CALDAV,
                "no-uid-conflict",
            ),
            Kind::People => precondition(
                STATUS_CODE_REASON_PHRASE.n409_conflict,
                CARDDAV,
                "no-uid-conflict",
            ),
        },
        Err(store::PutError::Invalid(_)) => match ctx.kind {
            Kind::Calendar => precondition(
                STATUS_CODE_REASON_PHRASE.n403_forbidden,
                CALDAV,
                "valid-calendar-data",
            ),
            Kind::People => precondition(
                STATUS_CODE_REASON_PHRASE.n403_forbidden,
                CARDDAV,
                "valid-address-data",
            ),
        },
    })
}

async fn delete(ctx: &Ctx<'_>, request: &Request, target: &Target) -> Result<Response, String> {
    let if_match = request
        .header("if-match")
        .map(|h| h.value.trim().to_string());
    let s = ctx.store();
    let no_content = || status(STATUS_CODE_REASON_PHRASE.n204_no_content, "", vec![]);
    let not_found = || {
        status(
            STATUS_CODE_REASON_PHRASE.n404_not_found,
            "Not found",
            vec![],
        )
    };
    match (ctx.kind, target) {
        (Kind::People, Target::Item(path, _) | Target::Collection(path)) if path == DIRECTORY => {
            Ok(status(
                STATUS_CODE_REASON_PHRASE.n403_forbidden,
                "The directory is read-only",
                vec![],
            ))
        }
        (Kind::Calendar, Target::Item(path, name)) => {
            let Some(c) = store::calendar_at(s, &ctx.user.id, path)
                .await
                .map_err(|e| e.to_string())?
            else {
                return Ok(not_found());
            };
            Ok(
                match store::delete_event(s, &ctx.user.id, &c.id, name, if_match.as_deref()).await {
                    Ok(true) => no_content(),
                    Ok(false) => not_found(),
                    Err(store::PutError::Precondition) => status(
                        STATUS_CODE_REASON_PHRASE.n412_precondition_failed,
                        "It changed since",
                        vec![],
                    ),
                    Err(_) => not_found(),
                },
            )
        }
        (Kind::People, Target::Item(path, name)) => Ok(
            match contacts::delete_card(s, &ctx.user.id, path, name, if_match.as_deref()).await {
                Ok(true) => no_content(),
                Ok(false) => not_found(),
                Err(store::PutError::Precondition) => status(
                    STATUS_CODE_REASON_PHRASE.n412_precondition_failed,
                    "It changed since",
                    vec![],
                ),
                Err(_) => not_found(),
            },
        ),
        (Kind::Calendar, Target::Collection(path)) => {
            let Some(c) = store::calendar_at(s, &ctx.user.id, path)
                .await
                .map_err(|e| e.to_string())?
            else {
                return Ok(not_found());
            };
            Ok(
                if store::delete_calendar(s, &ctx.user.id, &c.id)
                    .await
                    .map_err(|e| e.to_string())?
                {
                    no_content()
                } else {
                    status(
                        STATUS_CODE_REASON_PHRASE.n403_forbidden,
                        "The last calendar stays",
                        vec![],
                    )
                },
            )
        }
        _ => Ok(status(
            STATUS_CODE_REASON_PHRASE.n403_forbidden,
            "That cannot be deleted",
            vec![],
        )),
    }
}

async fn mkcalendar(ctx: &Ctx<'_>, target: &Target, ask: &Ask) -> Result<Response, String> {
    let (Kind::Calendar, Target::Collection(path)) = (ctx.kind, target) else {
        return Ok(status(
            STATUS_CODE_REASON_PHRASE.n403_forbidden,
            "Calendars are made in the calendar home",
            vec![],
        ));
    };
    let name = ask
        .set
        .iter()
        .find(|(k, _)| k.ends_with("|displayname"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| path.clone());
    let color = ask
        .set
        .iter()
        .find(|(k, _)| k.ends_with("|calendar-color"))
        .map(|(_, v)| color_name(v))
        .unwrap_or("accent");
    Ok(
        match store::create_calendar_at(ctx.store(), &ctx.user.id, path, &name, color)
            .await
            .map_err(|e| e.to_string())?
        {
            Some(_) => status(STATUS_CODE_REASON_PHRASE.n201_created, "", vec![]),
            None => status(
                STATUS_CODE_REASON_PHRASE.n405_method_not_allowed,
                "It exists already, or the name is not allowed",
                vec![],
            ),
        },
    )
}

async fn proppatch(ctx: &Ctx<'_>, target: &Target, ask: &Ask) -> Result<Response, String> {
    let href = match target {
        Target::Collection(path) => ctx.collection_href(path),
        _ => {
            return Ok(status(
                STATUS_CODE_REASON_PHRASE.n403_forbidden,
                "Only collections change here",
                vec![],
            ))
        }
    };
    if let (Kind::Calendar, Target::Collection(path)) = (ctx.kind, target) {
        if let Some(c) = store::calendar_at(ctx.store(), &ctx.user.id, path)
            .await
            .map_err(|e| e.to_string())?
        {
            let name = ask
                .set
                .iter()
                .find(|(k, _)| k.ends_with("|displayname"))
                .map(|(_, v)| v.as_str());
            let color = ask
                .set
                .iter()
                .find(|(k, _)| k.ends_with("|calendar-color"))
                .map(|(_, v)| color_name(v));
            store::update_calendar(ctx.store(), &ctx.user.id, &c.id, name, color)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    let found: Props = ask
        .set
        .iter()
        .map(|(k, _)| {
            let (ns, local) = k.split_once('|').unwrap_or(("", k));
            (
                k.clone(),
                format!("<X:{local} xmlns:X=\"{}\"/>", escape(ns)),
            )
        })
        .collect();
    Ok(multistatus(&propstat(&href, &found, &[]), None))
}

async fn get(ctx: &Ctx<'_>, target: &Target) -> Result<Response, String> {
    let Target::Item(path, name) = target else {
        return Ok(status(
            STATUS_CODE_REASON_PHRASE.n200_ok,
            "Ortus CalDAV and CardDAV: use a calendar or contacts app with this address.",
            vec![],
        ));
    };
    let Some((etag, data)) = ctx.item(path, name).await? else {
        return Ok(status(
            STATUS_CODE_REASON_PHRASE.n404_not_found,
            "Not found",
            vec![],
        ));
    };
    let mut response = Response::get_response(
        STATUS_CODE_REASON_PHRASE.n200_ok,
        Some(vec![
            header("ETag", &etag),
            header("X-Content-Type-Options", "nosniff"),
            header("Content-Security-Policy", "sandbox; default-src 'none'"),
        ]),
        Some(vec![rust_web_server::range::Range::get_content_range(
            data.into_bytes(),
            ctx.kind.item_type().to_string(),
        )]),
    );
    response
        .headers
        .retain(|h| !h.name.eq_ignore_ascii_case("Accept-Ranges"));
    Ok(response)
}

fn unauthorized() -> Response {
    // The challenge is only ever sent under /dav/, where apps need it.
    status(
        STATUS_CODE_REASON_PHRASE.n401_unauthorized,
        "Sign in with your address and an app password",
        vec![header(
            "WWW-Authenticate",
            "Basic realm=\"Ortus\", charset=\"UTF-8\"",
        )],
    )
}

async fn handle(
    kind: Kind,
    request: Request,
    connection: ConnectionInfo,
    instance: Arc<Instance>,
    rest: String,
) -> Response {
    if request.method == "OPTIONS" {
        return status(
            STATUS_CODE_REASON_PHRASE.n200_ok,
            "",
            vec![
                header("DAV", "1, 3, calendar-access, addressbook"),
                header(
                    "Allow",
                    "OPTIONS, GET, PUT, DELETE, PROPFIND, PROPPATCH, REPORT, MKCALENDAR",
                ),
            ],
        );
    }
    let client =
        ortus_core::auth::client_address(&request, &connection, &instance.config.trusted_proxies);
    let Some(user) = ortus_core::auth::by_header(&request, &instance, &client).await else {
        return unauthorized();
    };
    let Some(target) = target(&rest) else {
        return status(
            STATUS_CODE_REASON_PHRASE.n404_not_found,
            "Not found",
            vec![],
        );
    };
    if request.body.len() > MAX_BODY {
        return status(
            STATUS_CODE_REASON_PHRASE.n413_payload_too_large,
            "Too big",
            vec![],
        );
    }
    let ctx = Ctx {
        instance: &instance,
        user: &user,
        kind,
    };
    let ask = || parse(&request.body[..]);
    let result = match request.method.as_str() {
        "PROPFIND" => match ask() {
            Ok(a) => propfind(&ctx, &request, &target, &a).await,
            Err(_) => Ok(status(
                STATUS_CODE_REASON_PHRASE.n400_bad_request,
                "That is not XML",
                vec![],
            )),
        },
        "REPORT" => match ask() {
            Ok(a) => report(&ctx, &target, &a).await,
            Err(_) => Ok(status(
                STATUS_CODE_REASON_PHRASE.n400_bad_request,
                "That is not XML",
                vec![],
            )),
        },
        "PROPPATCH" => match ask() {
            Ok(a) => proppatch(&ctx, &target, &a).await,
            Err(_) => Ok(status(
                STATUS_CODE_REASON_PHRASE.n400_bad_request,
                "That is not XML",
                vec![],
            )),
        },
        "MKCALENDAR" | "MKCOL" => match ask() {
            Ok(a) => mkcalendar(&ctx, &target, &a).await,
            Err(_) => Ok(status(
                STATUS_CODE_REASON_PHRASE.n400_bad_request,
                "That is not XML",
                vec![],
            )),
        },
        "GET" => get(&ctx, &target).await,
        "PUT" => put(&ctx, &request, &target).await,
        "DELETE" => delete(&ctx, &request, &target).await,
        _ => Ok(status(
            STATUS_CODE_REASON_PHRASE.n405_method_not_allowed,
            "Not here",
            vec![],
        )),
    };
    result.unwrap_or_else(|e| {
        eprintln!("ortus: calendar: dav: {e}");
        status(
            STATUS_CODE_REASON_PHRASE.n500_internal_server_error,
            "Something went wrong",
            vec![],
        )
    })
}

pub fn calendar(
    request: Request,
    connection: ConnectionInfo,
    instance: Arc<Instance>,
    rest: String,
) -> BoxFuture<'static, Response> {
    Box::pin(handle(Kind::Calendar, request, connection, instance, rest))
}

pub fn people(
    request: Request,
    connection: ConnectionInfo,
    instance: Arc<Instance>,
    rest: String,
) -> BoxFuture<'static, Response> {
    Box::pin(handle(Kind::People, request, connection, instance, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read() {
        let body = br#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:displayname/><c:calendar-home-set/></d:prop></d:propfind>"#;
        let ask = parse(body).unwrap();
        assert_eq!(
            vec![
                "DAV:|displayname".to_string(),
                format!("{CALDAV}|calendar-home-set")
            ],
            ask.props
        );
        let body = br#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="20261101T000000Z" end="20261201T000000Z"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#;
        let ask = parse(body).unwrap();
        assert_eq!(
            (
                Some("20261101T000000Z".to_string()),
                Some("20261201T000000Z".to_string())
            ),
            ask.range
        );
        assert_eq!(Some(1_793_491_200_000), ical_time("20261101T000000Z"));
        let body = br#"<D:sync-collection xmlns:D="DAV:"><D:sync-token>https://ortus.invalid/sync/7</D:sync-token><D:prop><D:getetag/></D:prop></D:sync-collection>"#;
        assert_eq!(
            Some(7),
            parse(body)
                .unwrap()
                .sync_token
                .as_deref()
                .and_then(sync_number)
        );
        assert!(parse(b"").unwrap().all);
        assert_eq!("a%20b", encode("a b"));
        assert_eq!("a b", decode("a%20b"));
    }
}
