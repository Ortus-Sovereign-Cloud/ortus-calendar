//! The contacts store (the People app): address books per person, cards
//! kept as their vCard text (what CardDAV serves back) with the facts we
//! search by beside them; and the installation's people as a read-only
//! directory.

use calcard::vcard::{VCard, VCardProperty};
use ortus_core::storage::{AsyncStore, AsyncTx, Migration, StoreResult};
use serde::Serialize;

pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "address books and cards",
    sql: "CREATE TABLE ab_books (
        id TEXT PRIMARY KEY,
        user_id TEXT NOT NULL,
        name TEXT NOT NULL,
        sync BIGINT NOT NULL,
        created_at BIGINT NOT NULL,
        UNIQUE (user_id, name)
    );
    CREATE TABLE ab_cards (
        id TEXT PRIMARY KEY,
        book_id TEXT NOT NULL REFERENCES ab_books (id),
        href TEXT NOT NULL,
        uid TEXT NOT NULL,
        vcard TEXT NOT NULL,
        etag TEXT NOT NULL,
        name TEXT NOT NULL,
        emails TEXT NOT NULL,
        phones TEXT NOT NULL,
        org TEXT,
        sync BIGINT NOT NULL,
        updated_at BIGINT NOT NULL,
        UNIQUE (book_id, href),
        UNIQUE (book_id, uid)
    );
    CREATE TABLE ab_changes (
        book_id TEXT NOT NULL,
        sync BIGINT NOT NULL,
        href TEXT NOT NULL,
        deleted BIGINT NOT NULL,
        PRIMARY KEY (book_id, sync)
    )",
}];

pub const MAX_VCARD: usize = 512 * 1024;

fn now() -> i64 {
    ortus_core::program::now_ms()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", ortus_core::auth::random_hex(12))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Book {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub sync: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CardFacts {
    pub uid: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub org: Option<String>,
}

/// Reads a card: one vCard with a UID.
pub fn facts(text: &str) -> Result<CardFacts, String> {
    if text.len() > MAX_VCARD {
        return Err("too_big".into());
    }
    let card = VCard::parse(text).map_err(|_| "not_vcard".to_string())?;
    let uid = card
        .uid()
        .filter(|u| !u.trim().is_empty() && u.len() <= 255)
        .ok_or("no_uid")?
        .to_string();
    let texts = |prop: VCardProperty| -> Vec<String> {
        card.properties(&prop)
            .filter_map(|e| {
                let parts: Vec<&str> = e
                    .values
                    .iter()
                    .filter_map(|v| v.as_text())
                    .filter(|t| !t.is_empty())
                    .collect();
                (!parts.is_empty()).then(|| parts.join(" "))
            })
            .collect()
    };
    let name = texts(VCardProperty::Fn).into_iter().next().or_else(|| {
        // N is family;given;additional;prefix;suffix.
        card.property(&VCardProperty::N).map(|e| {
            let v: Vec<&str> = e.values.iter().filter_map(|v| v.as_text()).collect();
            [v.get(1).copied(), v.first().copied()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
    });
    let emails: Vec<String> = texts(VCardProperty::Email)
        .into_iter()
        .map(|e| e.trim().trim_start_matches("mailto:").to_ascii_lowercase())
        .collect();
    let phones = texts(VCardProperty::Tel)
        .into_iter()
        .map(|t| t.trim_start_matches("tel:").to_string())
        .collect();
    let org = texts(VCardProperty::Org).into_iter().next();
    let name: String = name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| emails.first().cloned())
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect();
    Ok(CardFacts {
        uid,
        name,
        emails,
        phones,
        org,
    })
}

fn book_of(r: &ortus_core::storage::Row) -> Book {
    Book {
        id: r.text(0).unwrap_or_default().into(),
        user_id: r.text(1).unwrap_or_default().into(),
        name: r.text(2).unwrap_or_default().into(),
        sync: r.int(3).unwrap_or(0),
    }
}

const BOOK: &str = "SELECT id, user_id, name, sync FROM ab_books";

/// A person's address books; the first visit makes "Contacts".
pub async fn books(store: &dyn AsyncStore, user_id: &str) -> StoreResult<Vec<Book>> {
    let found: Vec<Book> = store
        .query(
            &format!("{BOOK} WHERE user_id = $1 ORDER BY created_at, name"),
            &[user_id.into()],
        )
        .await?
        .iter()
        .map(book_of)
        .collect();
    if !found.is_empty() {
        return Ok(found);
    }
    store
        .execute("INSERT INTO ab_books (id, user_id, name, sync, created_at) VALUES ($1, $2, 'Contacts', 1, $3) ON CONFLICT (user_id, name) DO NOTHING", &[new_id("ab_").into(), user_id.into(), now().into()])
        .await?;
    Ok(store
        .query(
            &format!("{BOOK} WHERE user_id = $1 ORDER BY created_at, name"),
            &[user_id.into()],
        )
        .await?
        .iter()
        .map(book_of)
        .collect())
}

pub async fn book(store: &dyn AsyncStore, user_id: &str, id: &str) -> StoreResult<Option<Book>> {
    Ok(store
        .query(
            &format!("{BOOK} WHERE id = $1 AND user_id = $2"),
            &[id.into(), user_id.into()],
        )
        .await?
        .first()
        .map(book_of))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Card {
    pub id: String,
    pub book_id: String,
    pub href: String,
    pub uid: String,
    pub vcard: String,
    pub etag: String,
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub org: Option<String>,
    pub sync: i64,
}

const CARD: &str =
    "SELECT id, book_id, href, uid, vcard, etag, name, emails, phones, org, sync FROM ab_cards";

fn card_of(r: &ortus_core::storage::Row) -> Card {
    let list = |i: usize| {
        serde_json::from_str::<Vec<String>>(r.text(i).unwrap_or("[]")).unwrap_or_default()
    };
    Card {
        id: r.text(0).unwrap_or_default().into(),
        book_id: r.text(1).unwrap_or_default().into(),
        href: r.text(2).unwrap_or_default().into(),
        uid: r.text(3).unwrap_or_default().into(),
        vcard: r.text(4).unwrap_or_default().into(),
        etag: r.text(5).unwrap_or_default().into(),
        name: r.text(6).unwrap_or_default().into(),
        emails: list(7),
        phones: list(8),
        org: r.text(9).map(str::to_string),
        sync: r.int(10).unwrap_or(0),
    }
}

pub async fn card(store: &dyn AsyncStore, book_id: &str, href: &str) -> StoreResult<Option<Card>> {
    Ok(store
        .query(
            &format!("{CARD} WHERE book_id = $1 AND href = $2"),
            &[book_id.into(), href.into()],
        )
        .await?
        .first()
        .map(card_of))
}

pub async fn cards(store: &dyn AsyncStore, book_id: &str) -> StoreResult<Vec<Card>> {
    Ok(store
        .query(
            &format!("{CARD} WHERE book_id = $1 ORDER BY name, href"),
            &[book_id.into()],
        )
        .await?
        .iter()
        .map(card_of)
        .collect())
}

pub use crate::store::{etag_of, Put, PutError};

async fn changed(
    tx: &mut dyn AsyncTx,
    book_id: &str,
    href: &str,
    deleted: bool,
) -> StoreResult<i64> {
    tx.execute(
        "UPDATE ab_books SET sync = sync + 1 WHERE id = $1",
        &[book_id.into()],
    )
    .await?;
    let sync = tx
        .query("SELECT sync FROM ab_books WHERE id = $1", &[book_id.into()])
        .await?
        .first()
        .and_then(|r| r.int(0))
        .unwrap_or(0);
    tx.execute(
        "INSERT INTO ab_changes (book_id, sync, href, deleted) VALUES ($1, $2, $3, $4)",
        &[
            book_id.into(),
            sync.into(),
            href.into(),
            i64::from(deleted).into(),
        ],
    )
    .await?;
    Ok(sync)
}

/// Stores a card under `href` in an address book of the person's.
pub async fn put_card(
    store: &dyn AsyncStore,
    user_id: &str,
    book_id: &str,
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
    if book(store, user_id, book_id).await?.is_none() {
        return Err(PutError::NotFound);
    }
    let etag = etag_of(text);
    let mut tx = store.begin().await?;
    // Writers of one address book take turns (see store::put_event).
    tx.execute(
        "UPDATE ab_books SET sync = sync WHERE id = $1",
        &[book_id.into()],
    )
    .await?;
    let existing = tx
        .query(
            "SELECT id, etag FROM ab_cards WHERE book_id = $1 AND href = $2",
            &[book_id.into(), href.into()],
        )
        .await?;
    let existing = existing.first().map(|r| {
        (
            r.text(0).unwrap_or_default().to_string(),
            r.text(1).unwrap_or_default().to_string(),
        )
    });
    let refuse = match (&existing, if_match, if_none_match) {
        (Some(_), _, true) => true,
        (Some((_, current)), Some(wanted), _) => wanted != "*" && wanted != current,
        (None, Some(_), _) => true,
        _ => false,
    };
    if refuse {
        tx.rollback().await?;
        return Err(PutError::Precondition);
    }
    if !tx
        .query(
            "SELECT href FROM ab_cards WHERE book_id = $1 AND uid = $2 AND href <> $3",
            &[book_id.into(), facts.uid.as_str().into(), href.into()],
        )
        .await?
        .is_empty()
    {
        tx.rollback().await?;
        return Err(PutError::UidTaken);
    }
    let sync = changed(&mut *tx, book_id, href, false).await?;
    let emails = serde_json::to_string(&facts.emails).unwrap_or_default();
    let phones = serde_json::to_string(&facts.phones).unwrap_or_default();
    let values = |id: &str| -> Vec<ortus_core::storage::Value> {
        vec![
            id.into(),
            book_id.into(),
            href.into(),
            facts.uid.as_str().into(),
            text.into(),
            etag.as_str().into(),
            facts.name.as_str().into(),
            emails.as_str().into(),
            phones.as_str().into(),
            facts.org.clone().into(),
            sync.into(),
            now().into(),
        ]
    };
    match &existing {
        Some((id, _)) => {
            tx.execute("UPDATE ab_cards SET book_id = $2, href = $3, uid = $4, vcard = $5, etag = $6, name = $7, emails = $8, phones = $9, org = $10, sync = $11, updated_at = $12 WHERE id = $1", &values(id)).await?;
        }
        None => {
            tx.execute("INSERT INTO ab_cards (id, book_id, href, uid, vcard, etag, name, emails, phones, org, sync, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)", &values(&new_id("vc_"))).await?;
        }
    }
    tx.commit().await?;
    Ok(Put {
        etag,
        created: existing.is_none(),
    })
}

pub async fn delete_card(
    store: &dyn AsyncStore,
    user_id: &str,
    book_id: &str,
    href: &str,
    if_match: Option<&str>,
) -> Result<bool, PutError> {
    if book(store, user_id, book_id).await?.is_none() {
        return Err(PutError::NotFound);
    }
    let mut tx = store.begin().await?;
    // Writers of one address book take turns (see store::put_event).
    tx.execute(
        "UPDATE ab_books SET sync = sync WHERE id = $1",
        &[book_id.into()],
    )
    .await?;
    let rows = tx
        .query(
            "SELECT etag FROM ab_cards WHERE book_id = $1 AND href = $2",
            &[book_id.into(), href.into()],
        )
        .await?;
    let Some(current) = rows.first().and_then(|r| r.text(0)).map(str::to_string) else {
        tx.rollback().await?;
        return Ok(false);
    };
    if if_match.is_some_and(|m| m != "*" && m != current) {
        tx.rollback().await?;
        return Err(PutError::Precondition);
    }
    tx.execute(
        "DELETE FROM ab_cards WHERE book_id = $1 AND href = $2",
        &[book_id.into(), href.into()],
    )
    .await?;
    changed(&mut *tx, book_id, href, true).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn changes_since(
    store: &dyn AsyncStore,
    book_id: &str,
    since: i64,
) -> StoreResult<Option<(Vec<(String, bool)>, i64)>> {
    let Some(book) = store
        .query(&format!("{BOOK} WHERE id = $1"), &[book_id.into()])
        .await?
        .first()
        .map(book_of)
    else {
        return Ok(None);
    };
    if since > book.sync || since < 0 {
        return Ok(None);
    }
    let mut latest: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    for r in store
        .query(
            "SELECT href, deleted FROM ab_changes WHERE book_id = $1 AND sync > $2 ORDER BY sync",
            &[book_id.into(), since.into()],
        )
        .await?
    {
        latest.insert(
            r.text(0).unwrap_or_default().to_string(),
            r.int(1).unwrap_or(0) != 0,
        );
    }
    Ok(Some((latest.into_iter().collect(), book.sync)))
}

/// Someone in this installation, from the directory.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Colleague {
    pub id: String,
    pub name: String,
    pub email: String,
}

/// The installation's people (active accounts), matching `q` when given.
pub async fn directory(store: &dyn AsyncStore, q: &str, limit: i64) -> StoreResult<Vec<Colleague>> {
    let like = format!(
        "%{}%",
        q.trim().to_lowercase().replace(['%', '_', '\\'], "")
    );
    Ok(store
        .query(
            "SELECT id, name, email FROM core_users WHERE state = 'active' AND (LOWER(name) LIKE $1 OR LOWER(email) LIKE $1) ORDER BY name, email LIMIT $2",
            &[like.as_str().into(), limit.clamp(1, 1000).into()],
        )
        .await?
        .iter()
        .map(|r| Colleague { id: r.text(0).unwrap_or_default().into(), name: r.text(1).unwrap_or_default().into(), email: r.text(2).unwrap_or_default().into() })
        .collect())
}

/// A directory entry as a vCard (for CardDAV's directory book).
pub fn colleague_vcard(c: &Colleague) -> String {
    let escape = |s: &str| {
        s.replace('\\', "\\\\")
            .replace(',', "\\,")
            .replace(';', "\\;")
            .replace(['\r', '\n'], " ")
    };
    format!("BEGIN:VCARD\r\nVERSION:4.0\r\nUID:urn:ortus:person:{}\r\nFN:{}\r\nEMAIL:{}\r\nKIND:individual\r\nEND:VCARD\r\n", c.id, escape(&c.name), escape(&c.email))
}

pub async fn forget(tx: &mut dyn AsyncTx, user_id: &str) -> StoreResult<()> {
    for sql in [
        "DELETE FROM ab_changes WHERE book_id IN (SELECT id FROM ab_books WHERE user_id = $1)",
        "DELETE FROM ab_cards WHERE book_id IN (SELECT id FROM ab_books WHERE user_id = $1)",
        "DELETE FROM ab_books WHERE user_id = $1",
    ] {
        tx.execute(sql, &[user_id.into()]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_are_read() {
        let f = facts("BEGIN:VCARD\r\nVERSION:4.0\r\nUID:anna-1\r\nFN:Anna Kowalska\r\nEMAIL;TYPE=work:Anna@Regional.example\r\nTEL:+48 71 555 0101\r\nORG:Regional office\r\nEND:VCARD\r\n").unwrap();
        assert_eq!(
            ("anna-1", "Anna Kowalska"),
            (f.uid.as_str(), f.name.as_str())
        );
        assert_eq!(vec!["anna@regional.example"], f.emails);
        assert_eq!(vec!["+48 71 555 0101"], f.phones);
        assert_eq!(Some("Regional office".into()), f.org);
        let n = facts("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:x\r\nN:Price;Daniel;;;\r\nEND:VCARD\r\n")
            .unwrap();
        assert_eq!("Daniel Price", n.name);
        assert_eq!(
            Err("no_uid".into()),
            facts("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:x\r\nEND:VCARD\r\n")
        );
        let c = Colleague {
            id: "u1".into(),
            name: "Marek, Nowak".into(),
            email: "marek@ministry.example".into(),
        };
        assert!(colleague_vcard(&c).contains("FN:Marek\\, Nowak"));
    }
}
