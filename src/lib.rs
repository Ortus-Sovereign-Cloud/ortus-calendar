//! Calendar and contacts: CalDAV, CardDAV, invitations, free and busy, the directory.
//!
//! An Ortus app crate: it depends on `ortus-core` only and hands its apps
//! to a program through [`apps`].

pub mod api;
pub mod contacts;
pub mod dav;
pub mod index;
pub mod listener;
pub mod people_api;
pub mod polls;
pub mod schedule;
pub mod store;

use std::borrow::Cow;
use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};
use ortus_core::storage::Migration;

/// The built web client (web/dist): one bundle for both apps.
#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist"]
struct Web;

/// The calendar app.
pub struct Calendar;

impl App for Calendar {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "calendar",
            group: Group::Organise,
            path: "/calendar",
            order: 1,
        }
    }

    fn routes(&self) -> Vec<ortus_core::api::Route> {
        let mut all = api::routes();
        all.extend(polls::routes());
        all
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn dav(&self) -> Option<ortus_core::dav::DavHandler> {
        Some(dav::calendar)
    }

    fn well_known(&self) -> Vec<(&'static str, &'static str)> {
        vec![("caldav", "/dav/calendar/")]
    }

    fn provide(
        &self,
        instance: &Arc<ortus_core::instance::Instance>,
        services: &ortus_core::services::Services,
    ) {
        let service: Arc<dyn ortus_core::interfaces::Calendar> = Arc::new(schedule::Service {
            instance: Arc::downgrade(instance),
        });
        services.provide("calendar", service);
    }

    fn role_tasks(&self) -> Vec<ortus_core::app::RoleTask> {
        // CalDAV and CardDAV on their own port: see listener.rs.
        vec![ortus_core::app::RoleTask {
            role: ortus_core::config::Role::Web,
            name: "dav",
            run: listener::run,
        }]
    }

    fn web_file(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        Web::get(path).map(|file| file.data)
    }
}

/// The people app.
pub struct People;

impl App for People {
    fn info(&self) -> AppInfo {
        AppInfo {
            id: "people",
            group: Group::Organise,
            path: "/people",
            order: 3,
        }
    }

    fn routes(&self) -> Vec<ortus_core::api::Route> {
        people_api::routes()
    }

    fn migrations(&self) -> &'static [Migration] {
        contacts::MIGRATIONS
    }

    fn dav(&self) -> Option<ortus_core::dav::DavHandler> {
        Some(dav::people)
    }

    fn well_known(&self) -> Vec<(&'static str, &'static str)> {
        vec![("carddav", "/dav/people/")]
    }

    fn web_file(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        Web::get(path).map(|file| file.data)
    }
}

/// The apps of this crate, for a program to run.
pub fn apps() -> Vec<Arc<dyn App>> {
    vec![Arc::new(Calendar), Arc::new(People)]
}
