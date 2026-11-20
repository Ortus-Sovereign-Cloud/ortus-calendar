//! The people app's JSON API for the web client: the organisation's
//! directory and groups (read only, from the accounts), and one's own
//! contacts (vCards in one's first address book, the same ones CardDAV
//! apps sync).

use std::sync::Arc;

use ortus_core::api::{answer, body, ok, ApiError, Method, Route};
use ortus_core::auth::Access;
use ortus_core::instance::Instance;
use ortus_core::storage::BoxFuture;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::contacts;
use crate::schedule::{split, unfold};

fn query(request: &Request, name: &str) -> String {
    let Some((_, q)) = request.request_uri.split_once('?') else {
        return String::new();
    };
    q.split('&')
        .find_map(|p| {
            let (k, v) = p.split_once('=')?;
            (k == name).then(|| {
                let bytes = v.replace('+', " ");
                let mut out = Vec::new();
                let b = bytes.as_bytes();
                let mut i = 0;
                while i < b.len() {
                    if b[i] == b'%' && i + 2 < b.len() {
                        if let Ok(x) = u8::from_str_radix(&bytes[i + 1..i + 3], 16) {
                            out.push(x);
                            i += 3;
                            continue;
                        }
                    }
                    out.push(b[i]);
                    i += 1;
                }
                String::from_utf8_lossy(&out).into_owned()
            })
        })
        .unwrap_or_default()
}

fn like(q: &str) -> String {
    format!(
        "%{}%",
        q.trim().to_lowercase().replace(['%', '_', '\\'], "")
    )
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

/// Colleagues: ?q= (name or address), ?group= (a group id). Those not
/// yet signed in count; suspended and unverified accounts do not.
async fn directory(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    signed(request, instance).await?;
    let s = instance.store.as_ref();
    let q = query(request, "q");
    let group = query(request, "group");
    if q.len() > 100 || group.len() > 100 {
        return Err(ApiError::bad_request("The search is too long."));
    }
    let rows = if group.is_empty() {
        s.query("SELECT id, name, email FROM core_users WHERE state IN ('active', 'invited') AND (LOWER(name) LIKE $1 OR LOWER(email) LIKE $1) ORDER BY name LIMIT 500", &[like(&q).as_str().into()]).await?
    } else {
        s.query("SELECT u.id, u.name, u.email FROM core_users u JOIN core_group_members m ON m.user_id = u.id WHERE m.group_id = $2 AND u.state IN ('active', 'invited') AND (LOWER(u.name) LIKE $1 OR LOWER(u.email) LIKE $1) ORDER BY u.name LIMIT 500", &[like(&q).as_str().into(), group.as_str().into()]).await?
    };
    let memberships = s.query("SELECT m.user_id, g.name FROM core_group_members m JOIN core_groups g ON g.id = m.group_id ORDER BY g.name", &[]).await?;
    let people: Vec<Value> = rows
        .iter()
        .map(|r| {
            let id = r.text(0).unwrap_or_default();
            let groups: Vec<&str> = memberships.iter().filter(|m| m.text(0) == Some(id)).filter_map(|m| m.text(1)).collect();
            json!({"name": r.text(1).unwrap_or_default(), "email": r.text(2).unwrap_or_default(), "groups": groups})
        })
        .collect();
    let total = s
        .query(
            "SELECT COUNT(*) FROM core_users WHERE state IN ('active', 'invited')",
            &[],
        )
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    Ok(ok(&json!({ "people": people, "total": total })))
}

/// The organisation's groups, with how many are in each.
async fn groups(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    signed(request, instance).await?;
    let rows = instance
        .store
        .query("SELECT g.id, g.name, COUNT(u.id) FROM core_groups g LEFT JOIN core_group_members m ON m.group_id = g.id LEFT JOIN core_users u ON u.id = m.user_id AND u.state IN ('active', 'invited') GROUP BY g.id, g.name ORDER BY g.name", &[])
        .await?;
    let list: Vec<Value> = rows.iter().map(|r| json!({"id": r.text(0).unwrap_or_default(), "name": r.text(1).unwrap_or_default(), "members": r.int(2).unwrap_or(0)})).collect();
    Ok(ok(&json!({ "groups": list })))
}

/// A vCard property's value, unescaped.
fn value(text: &str, name: &str) -> String {
    unfold(text)
        .iter()
        .map(|l| split(l))
        .find(|(n, _, _)| n == name)
        .map(|(_, _, v)| {
            v.replace("\\n", "\n")
                .replace("\\N", "\n")
                .replace("\\,", ",")
                .replace("\\;", ";")
                .replace("\\\\", "\\")
        })
        .unwrap_or_default()
}

fn contact_json(card: &contacts::Card) -> Value {
    json!({
        "id": card.href,
        "name": card.name,
        "emails": card.emails,
        "phones": card.phones,
        "org": value(&card.vcard, "ORG").trim_end_matches(';').to_string(),
        "title": value(&card.vcard, "TITLE"),
        "note": value(&card.vcard, "NOTE"),
    })
}

async fn my_book(instance: &Instance, user: &str) -> Result<contacts::Book, ApiError> {
    contacts::books(instance.store.as_ref(), user)
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::internal)
}

/// One's contacts, ?q= by name, address or organisation.
async fn list(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let book = my_book(instance, &me.id).await?;
    let q = query(request, "q").trim().to_lowercase();
    let cards = contacts::cards(instance.store.as_ref(), &book.id).await?;
    let found: Vec<Value> = cards
        .iter()
        .filter(|c| {
            q.is_empty()
                || c.name.to_lowercase().contains(&q)
                || c.emails.iter().any(|e| e.contains(&q))
                || c.org
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&q)
        })
        .map(contact_json)
        .collect();
    Ok(ok(&json!({ "contacts": found, "total": cards.len() })))
}

#[derive(Deserialize)]
struct ContactInput {
    name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    org: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    note: String,
}

fn escape(text: &str) -> String {
    text.trim()
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

impl ContactInput {
    fn check(&self) -> Result<(), ApiError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(ApiError::bad_request(
                "A contact needs a name of up to 200 characters.",
            ));
        }
        let email = self.email.trim();
        if !email.is_empty()
            && (!email.contains('@') || email.len() > 254 || email.contains(char::is_whitespace))
        {
            return Err(ApiError::bad_request("That is not an address."));
        }
        if self.phone.len() > 50
            || self.org.len() > 200
            || self.title.len() > 200
            || self.note.len() > 5000
        {
            return Err(ApiError::bad_request("The contact is too big."));
        }
        Ok(())
    }

    /// The card, keeping what the form does not set (photos, more
    /// addresses from a phone) when changing one.
    fn vcard(&self, uid: &str, before: Option<&str>) -> String {
        let set = [
            "FN", "N", "EMAIL", "TEL", "ORG", "TITLE", "NOTE", "UID", "VERSION", "BEGIN", "END",
            "REV",
        ];
        let mut lines = vec![
            "BEGIN:VCARD".to_string(),
            "VERSION:4.0".into(),
            format!("UID:{uid}"),
            format!("FN:{}", escape(&self.name)),
        ];
        let parts: Vec<&str> = self.name.split_whitespace().collect();
        let (given, family) = match parts.as_slice() {
            [] => (String::new(), String::new()),
            [one] => (one.to_string(), String::new()),
            [first @ .., last] => (first.join(" "), last.to_string()),
        };
        lines.push(format!("N:{};{};;;", escape(&family), escape(&given)));
        if !self.email.trim().is_empty() {
            lines.push(format!("EMAIL:{}", escape(&self.email.to_lowercase())));
        }
        if !self.phone.trim().is_empty() {
            lines.push(format!("TEL:{}", escape(&self.phone)));
        }
        if !self.org.trim().is_empty() {
            lines.push(format!("ORG:{}", escape(&self.org)));
        }
        if !self.title.trim().is_empty() {
            lines.push(format!("TITLE:{}", escape(&self.title)));
        }
        if !self.note.trim().is_empty() {
            lines.push(format!("NOTE:{}", escape(&self.note)));
        }
        if let Some(before) = before {
            // Keep the other emails and phones too, after the first.
            let mut emails = 0;
            let mut phones = 0;
            for line in unfold(before) {
                let (name, _, _) = split(&line);
                let upper = line.to_ascii_uppercase();
                if upper.starts_with("BEGIN:") && upper != "BEGIN:VCARD"
                    || upper.starts_with("END:") && upper != "END:VCARD"
                {
                    lines.push(line);
                    continue;
                }
                match name.as_str() {
                    "EMAIL" => {
                        emails += 1;
                        if emails > 1 {
                            lines.push(line);
                        }
                    }
                    "TEL" => {
                        phones += 1;
                        if phones > 1 {
                            lines.push(line);
                        }
                    }
                    n if set.contains(&n) => {}
                    _ => lines.push(line),
                }
            }
        }
        lines.push("END:VCARD".into());
        crate::schedule::fold(&lines)
    }
}

fn put_error(e: crate::store::PutError) -> ApiError {
    match e {
        crate::store::PutError::NotFound => ApiError::not_found("There is no such contact."),
        crate::store::PutError::Precondition => {
            ApiError::conflict("The contact changed meanwhile.")
        }
        crate::store::PutError::UidTaken => ApiError::conflict("That contact is already there."),
        _ => ApiError::bad_request("The contact is not valid."),
    }
}

async fn add(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: ContactInput = body(request)?;
    input.check()?;
    let book = my_book(instance, &me.id).await?;
    let uid = ortus_core::auth::random_hex(16);
    let href = format!("{uid}.vcf");
    let s = instance.store.as_ref();
    contacts::put_card(
        s,
        &me.id,
        &book.id,
        &href,
        &input.vcard(&uid, None),
        None,
        true,
    )
    .await
    .map_err(put_error)?;
    let card = contacts::card(s, &book.id, &href)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(ok(&contact_json(&card)))
}

async fn change(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let input: ContactInput = body(request)?;
    input.check()?;
    let book = my_book(instance, &me.id).await?;
    let s = instance.store.as_ref();
    let href = params.get("contact").unwrap_or_default();
    let before = contacts::card(s, &book.id, href)
        .await?
        .ok_or_else(|| ApiError::not_found("There is no such contact."))?;
    contacts::put_card(
        s,
        &me.id,
        &book.id,
        href,
        &input.vcard(&before.uid, Some(&before.vcard)),
        Some(&before.etag),
        false,
    )
    .await
    .map_err(put_error)?;
    let card = contacts::card(s, &book.id, href)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(ok(&contact_json(&card)))
}

async fn remove(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let me = signed(request, instance).await?;
    let book = my_book(instance, &me.id).await?;
    if !contacts::delete_card(
        instance.store.as_ref(),
        &me.id,
        &book.id,
        params.get("contact").unwrap_or_default(),
        None,
    )
    .await
    .map_err(put_error)?
    {
        return Err(ApiError::not_found("There is no such contact."));
    }
    Ok(ok(&json!({})))
}

fn h_directory(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, directory(&request, &instance).await) })
}
fn h_groups(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, groups(&request, &instance).await) })
}
fn h_list(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, list(&request, &instance).await) })
}
fn h_add(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, add(&request, &instance).await) })
}
fn h_change(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, change(&request, &params, &instance).await) })
}
fn h_remove(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, remove(&request, &params, &instance).await) })
}

fn object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    let route = |method, path, id, summary, handler| Route {
        method,
        path,
        id,
        tag: "people",
        summary,
        access: Access::Signed,
        response: object,
        handler,
    };
    vec![
        route(
            Method::Get,
            "/people/directory",
            "directory",
            "Colleagues, by ?q= (name or address) and ?group=.",
            h_directory as ortus_core::api::Handler,
        ),
        route(
            Method::Get,
            "/people/groups",
            "groups",
            "The organisation's groups, with their member counts.",
            h_groups,
        ),
        route(
            Method::Get,
            "/people/contacts",
            "contacts",
            "One's own contacts, by ?q=.",
            h_list,
        ),
        route(
            Method::Post,
            "/people/contacts",
            "addContact",
            "A new contact: {name, email, phone, org, title, note}.",
            h_add,
        ),
        route(
            Method::Put,
            "/people/contacts/:contact",
            "changeContact",
            "Change a contact (as addContact); what the form does not show is kept.",
            h_change,
        ),
        route(
            Method::Delete,
            "/people/contacts/:contact",
            "removeContact",
            "Remove a contact.",
            h_remove,
        ),
    ]
}
