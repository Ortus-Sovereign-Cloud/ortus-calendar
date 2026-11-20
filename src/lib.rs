//! Calendar and contacts: CalDAV, CardDAV, invitations, free and busy, the directory.
//!
//! An Ortus app crate: it depends on `ortus-core` only and hands its apps
//! to a program through [`apps`].

use std::sync::Arc;

use ortus_core::app::{App, AppInfo, Group};

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
}

/// The apps of this crate, for a program to run.
pub fn apps() -> Vec<Arc<dyn App>> {
    vec![Arc::new(Calendar), Arc::new(People)]
}
