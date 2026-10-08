//! OpenSchool bridge: shared core for the desktop (Rust) and mobile (Kotlin) clients.
//!
//! Target service: "Моя школа" on Gosuslugi (`/api/myschool/*`), authenticated by the ESIA session.

uniffi::setup_scaffolding!();

mod client;
mod datamart;
mod error;
mod models;

pub use client::{Client, SessionCookie, LOGIN_URL};
pub use error::BridgeError;
pub use models::*;

#[uniffi::export]
pub fn bridge_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// URL to open in a WebView/browser to start the ESIA login.
#[uniffi::export]
pub fn login_url() -> String {
    LOGIN_URL.to_string()
}
