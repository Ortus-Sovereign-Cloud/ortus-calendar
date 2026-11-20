//! Times offered by mail ("Times that work" in Compose): the sender offers
//! a few free times; anyone with the link picks one, no account needed,
//! and the event lands in the sender's calendar with everyone invited.
//! The link is the only key (128 random bits); a poll is picked once and
//! ends after 14 days.

use std::sync::Arc;

use ortus_core::api::{answer, body, ok, ApiError, Method, Route};
use ortus_core::auth::Access;
use ortus_core::instance::Instance;
use ortus_core::storage::BoxFuture;
use rust_web_server::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{schedule, store};

const LIFE: i64 = 14 * 86_400_000;

#[derive(Deserialize)]
struct Offer {
    summary: String,
    #[serde(default = "thirty")]
    minutes: i64,
    times: Vec<i64>,
    people: Vec<String>,
}

fn thirty() -> i64 {
    30
}

/// Offers times: answers the link to send.
async fn offer(request: &Request, instance: &Arc<Instance>) -> Result<Response, ApiError> {
    let me = ortus_core::auth::current(request, instance)
        .await
        .ok_or_else(ApiError::sign_in)?
        .user;
    let input: Offer = body(request)?;
    let now = ortus_core::program::now_ms();
    let summary = input.summary.trim();
    if summary.is_empty() || summary.chars().count() > 300 {
        return Err(ApiError::bad_request(
            "The meeting needs a title of up to 300 characters.",
        ));
    }
    if !(5..=480).contains(&input.minutes)
        || input.times.is_empty()
        || input.times.len() > 8
        || input
            .times
            .iter()
            .any(|t| *t < now || *t > now + 90 * 86_400_000)
    {
        return Err(ApiError::bad_request(
            "Offer one to eight future times of 5 to 480 minutes.",
        ));
    }
    let people: Vec<String> = input
        .people
        .iter()
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    if people.is_empty()
        || people.len() > 50
        || people.iter().any(|p| {
            !p.contains('@')
                || p.len() > 254
                || p.contains(['\r', '\n', ':', ';', ',', '<', '>', ' '])
        })
    {
        return Err(ApiError::bad_request("Name the people, by address."));
    }
    let token = ortus_core::auth::random_hex(16);
    instance
        .store
        .execute(
            "INSERT INTO cal_polls (token, user_id, summary, minutes, times, people, created_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[token.as_str().into(), me.id.as_str().into(), summary.into(), input.minutes.into(), json!(input.times).to_string().into(), json!(people).to_string().into(), now.into(), (now + LIFE).into()],
        )
        .await?;
    let url = format!(
        "{}/calendar/public/pick/{token}",
        instance.config.public_url.trim_end_matches('/')
    );
    Ok(ok(&json!({ "token": token, "url": url })))
}

struct Poll {
    user_id: String,
    summary: String,
    minutes: i64,
    times: Vec<i64>,
    people: Vec<String>,
    picked: Option<i64>,
}

async fn poll(instance: &Instance, token: &str) -> Result<Poll, ApiError> {
    let gone = || ApiError::not_found("This link has ended.");
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(gone());
    }
    let rows = instance
        .store
        .query("SELECT user_id, summary, minutes, times, people, picked FROM cal_polls WHERE token = $1 AND expires_at > $2", &[token.into(), ortus_core::program::now_ms().into()])
        .await?;
    let r = rows.first().ok_or_else(gone)?;
    Ok(Poll {
        user_id: r.text(0).unwrap_or_default().into(),
        summary: r.text(1).unwrap_or_default().into(),
        minutes: r.int(2).unwrap_or(30),
        times: serde_json::from_str(r.text(3).unwrap_or("[]")).unwrap_or_default(),
        people: serde_json::from_str(r.text(4).unwrap_or("[]")).unwrap_or_default(),
        picked: r.int(5),
    })
}

/// The times still open: offered, in the future, and still free for the
/// sender and their colleagues among the people.
async fn open_times(instance: &Instance, p: &Poll) -> Vec<i64> {
    let now = ortus_core::program::now_ms();
    let mut out = Vec::new();
    for t in &p.times {
        if *t <= now {
            continue;
        }
        let free = schedule::free_times(
            instance,
            &p.user_id,
            &p.people,
            *t,
            *t + p.minutes * 60_000,
            p.minutes,
            1,
        )
        .await
        .unwrap_or_default();
        if free.first() == Some(t) {
            out.push(*t);
        }
    }
    out
}

/// What the link shows.
async fn show(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let _ = request;
    let p = poll(instance, params.get("token").unwrap_or_default()).await?;
    let organizer = ortus_core::auth::accounts::by_id(instance.store.as_ref(), &p.user_id)
        .await?
        .map(|u| u.name)
        .unwrap_or_default();
    let times = if p.picked.is_some() {
        Vec::new()
    } else {
        open_times(instance, &p).await
    };
    Ok(ok(
        &json!({ "summary": p.summary, "organizer": organizer, "minutes": p.minutes, "times": times, "picked": p.picked }),
    ))
}

#[derive(Deserialize)]
struct Pick {
    time: i64,
}

/// Someone with the link picks a time: the event is made and everyone invited.
async fn pick(
    request: &Request,
    params: &PathParams,
    instance: &Arc<Instance>,
) -> Result<Response, ApiError> {
    let token = params.get("token").unwrap_or_default();
    let input: Pick = body(request)?;
    let p = poll(instance, token).await?;
    if p.picked.is_some() {
        return Err(ApiError::conflict("A time is already chosen."));
    }
    if !open_times(instance, &p).await.contains(&input.time) {
        return Err(ApiError::conflict(
            "That time is no longer free; choose another.",
        ));
    }
    // Only the first pick counts.
    let claimed = instance
        .store
        .execute(
            "UPDATE cal_polls SET picked = $1 WHERE token = $2 AND picked IS NULL",
            &[input.time.into(), token.into()],
        )
        .await?;
    if claimed != 1 {
        return Err(ApiError::conflict("A time is already chosen."));
    }
    let s = instance.store.as_ref();
    let account = ortus_core::auth::accounts::by_id(s, &p.user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("This link has ended."))?;
    let email = account.email.to_ascii_lowercase();
    let calendar = store::calendars(s, &p.user_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::internal)?;
    let domain = email.rsplit_once('@').map_or("ortus.local", |(_, d)| d);
    let uid = format!("{}@{domain}", ortus_core::auth::random_hex(12));
    let text = crate::api::event_text(
        &uid,
        &email,
        &account.name,
        &p.summary,
        input.time,
        input.time + p.minutes * 60_000,
        &p.people,
    );
    let href = format!("{}.ics", ortus_core::auth::random_hex(12));
    store::put_event(s, &p.user_id, &calendar.id, &href, &text, None, true)
        .await
        .map_err(|_| ApiError::internal())?;
    if let Some(event) = store::event(s, &calendar.id, &href).await? {
        let _ = instance
            .store
            .execute(
                "UPDATE cal_polls SET event_id = $1 WHERE token = $2",
                &[event.id.as_str().into(), token.into()],
            )
            .await;
    }
    if let Err(e) = schedule::publish(instance, &p.user_id, None, &text).await {
        eprintln!("ortus: calendar: invitations for a picked time: {e}");
    }
    Ok(ok(&json!({ "picked": input.time })))
}

fn h_offer(
    request: Request,
    _: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, offer(&request, &instance).await) })
}
fn h_show(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, show(&request, &params, &instance).await) })
}
fn h_pick(
    request: Request,
    params: PathParams,
    _: ConnectionInfo,
    instance: Arc<Instance>,
) -> BoxFuture<'static, Response> {
    Box::pin(async move { answer(&request, pick(&request, &params, &instance).await) })
}

fn object() -> Value {
    json!({"type": "object"})
}

pub fn routes() -> Vec<Route> {
    vec![
        Route { method: Method::Post, path: "/calendar/polls", id: "offerTimes", tag: "calendar", summary: "Offer times by mail: {summary, minutes, times, people}. Answers the link anyone may use to pick one.", access: Access::Signed, response: object, handler: h_offer },
        Route { method: Method::Get, path: "/calendar/polls/:token", id: "offeredTimes", tag: "calendar", summary: "The times a link offers that are still free (no account needed).", access: Access::Public, response: object, handler: h_show },
        Route { method: Method::Post, path: "/calendar/polls/:token/pick", id: "pickTime", tag: "calendar", summary: "Pick one of the offered times: {time}. The event is made and everyone invited; only the first pick counts.", access: Access::Public, response: object, handler: h_pick },
    ]
}
