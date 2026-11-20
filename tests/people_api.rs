//! The people app's JSON API on both shapes: the directory and groups,
//! and one's own contacts made, found, changed (keeping what the form
//! does not show) and removed; nobody sees another's contacts.

mod common;

use std::sync::Arc;

use ortus_core::auth::accounts::Role;
use ortus_core::instance::testing_auth::signed_in;
use rust_web_server::test_client::TestClient;
use serde_json::{json, Value};

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
fn directory_groups_and_contacts() {
    for (name, instance, _keep) in common::installations() {
        let olena = signed_in(&instance, "olena@ministry.example", Role::Member);
        let marek = signed_in(&instance, "marek@ministry.example", Role::Member);
        let s = instance.store.as_ref();
        ortus_core::runtime::block_on(s.execute(
            "INSERT INTO core_groups (id, name, created_at) VALUES ('g1', 'flood-team', 0)",
            &[],
        ))
        .unwrap();
        let marek_id = ortus_core::runtime::block_on(ortus_core::auth::accounts::by_email(
            s,
            "marek@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        ortus_core::runtime::block_on(s.execute(
            "INSERT INTO core_group_members (group_id, user_id) VALUES ('g1', $1)",
            &[marek_id.as_str().into()],
        ))
        .unwrap();
        let client = TestClient::new(ortus_core::app(Arc::clone(&instance)));

        let (_, all) = call(&client, &olena, "GET", "/people/directory", None);
        assert_eq!(2, all["people"].as_array().unwrap().len(), "{name}: {all}");
        let (_, found) = call(&client, &olena, "GET", "/people/directory?q=mar", None);
        assert_eq!(
            json!(["flood-team"]),
            found["people"][0]["groups"],
            "{name}: {found}"
        );
        let (_, groups) = call(&client, &olena, "GET", "/people/groups", None);
        assert_eq!(
            (json!("flood-team"), json!(1)),
            (
                groups["groups"][0]["name"].clone(),
                groups["groups"][0]["members"].clone()
            ),
            "{name}"
        );
        let (_, team) = call(&client, &olena, "GET", "/people/directory?group=g1", None);
        assert_eq!(1, team["people"].as_array().unwrap().len());

        let (status, anna) = call(
            &client,
            &olena,
            "POST",
            "/people/contacts",
            Some(
                json!({"name": "Anna Kowalska", "email": "Anna@Regional.example", "phone": "+00 100 300 117", "org": "Regional office; north", "title": "Regional coordinator"}),
            ),
        );
        assert_eq!(200, status, "{name}: {anna}");
        assert_eq!(json!(["anna@regional.example"]), anna["emails"]);
        assert_eq!(json!("Regional office; north"), anna["org"], "{name}");
        let id = anna["id"].as_str().unwrap().to_string();
        let (_, mine) = call(&client, &olena, "GET", "/people/contacts?q=regional", None);
        assert_eq!(1, mine["contacts"].as_array().unwrap().len(), "{name}");
        let (_, his) = call(&client, &marek, "GET", "/people/contacts", None);
        assert_eq!(
            0,
            his["contacts"].as_array().unwrap().len(),
            "{name}: not his"
        );
        let (status, _) = call(
            &client,
            &marek,
            "DELETE",
            &format!("/people/contacts/{id}"),
            None,
        );
        assert_eq!(404, status);

        // A phone added a second address and a birthday; the form keeps them.
        let book = ortus_core::runtime::block_on(ortus_calendar::contacts::books(
            s,
            &ortus_core::runtime::block_on(ortus_core::auth::accounts::by_email(
                s,
                "olena@ministry.example",
            ))
            .unwrap()
            .unwrap()
            .id,
        ))
        .unwrap()[0]
            .id
            .clone();
        let card = ortus_core::runtime::block_on(ortus_calendar::contacts::card(s, &book, &id))
            .unwrap()
            .unwrap();
        let richer = card.vcard.replace(
            "END:VCARD",
            "EMAIL:anna.k@home.example\r\nBDAY:19850412\r\nEND:VCARD",
        );
        let user = ortus_core::runtime::block_on(ortus_core::auth::accounts::by_email(
            s,
            "olena@ministry.example",
        ))
        .unwrap()
        .unwrap()
        .id;
        ortus_core::runtime::block_on(ortus_calendar::contacts::put_card(
            s,
            &user,
            &book,
            &id,
            &richer,
            Some(&card.etag),
            false,
        ))
        .unwrap();
        let (status, changed) = call(
            &client,
            &olena,
            "PUT",
            &format!("/people/contacts/{id}"),
            Some(
                json!({"name": "Anna Kowalska", "email": "anna@regional.example", "phone": "+00 100 300 118"}),
            ),
        );
        assert_eq!(200, status, "{name}: {changed}");
        assert_eq!(json!(["+00 100 300 118"]), changed["phones"]);
        assert_eq!(
            json!(["anna@regional.example", "anna.k@home.example"]),
            changed["emails"],
            "{name}"
        );
        let card = ortus_core::runtime::block_on(ortus_calendar::contacts::card(s, &book, &id))
            .unwrap()
            .unwrap();
        assert!(card.vcard.contains("BDAY:19850412"), "{name}");

        let (status, _) = call(
            &client,
            &olena,
            "POST",
            "/people/contacts",
            Some(json!({"name": " "})),
        );
        assert_eq!(400, status);
        let (status, _) = call(
            &client,
            &olena,
            "DELETE",
            &format!("/people/contacts/{id}"),
            None,
        );
        assert_eq!(200, status);
    }
}
