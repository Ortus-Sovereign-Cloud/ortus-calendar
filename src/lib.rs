//! Calendar and contacts: CalDAV, CardDAV, invitations, free and busy, the directory.
//!
//! An Ortus app crate: it depends on `ortus-core` only and hands its apps
//! to a program through [`apps`].

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

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
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

    fn web_file(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        Web::get(path).map(|file| file.data)
    }
}

/// The apps of this crate, for a program to run.
pub fn apps() -> Vec<Arc<dyn App>> {
    vec![Arc::new(Calendar), Arc::new(People)]
}
