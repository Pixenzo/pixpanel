//! HTTP API yönlendirmeleri. Bütün uçlar `/api/v1` altındadır.

use axum::Router;

use crate::state::AppState;

mod access;
mod addresses;
mod auth;
mod backup;
mod backup_import;
mod backup_mine;
mod branding;
mod components;
pub mod cron;
mod databases;
mod dns;
mod docker;
mod events;
mod fail2ban;
mod files;
mod firewall;
mod ftp;
mod git;
mod health;
mod jobs;
mod logs;
mod mail;
mod node_versions;
mod mail_extras;
mod notify;
mod packages;
mod panelcert;
mod php;
mod redis;
pub(crate) mod sites;
pub mod ssl;
mod system;
mod terminal;
mod update;
mod users;
mod webhooks;
mod wordpress;

/// Hesap adının biçim denetimi (hesap adı Linux kullanıcı adı ve klasör adıdır).
pub use users::validate_username as validate_account_name;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/auth", auth::router())
        .nest("/users", users::router().merge(access::router()))
        .nest("/system", system::router().nest("/node", node_versions::router()).nest("/ips", addresses::router()).nest("/certificate", panelcert::router()).nest("/update", update::router()))
        .nest("/branding", branding::router())
        .nest("/jobs", jobs::router())
        .nest("/events", events::router())
        .nest("/health", health::router())
        .nest("/packages", packages::router())
        .nest("/components", components::router())
        .nest("/databases", databases::router())
        .nest("/files", files::router())
        .nest("/terminal", terminal::router())
        .nest("/cron", cron::router())
        .nest("/logs", logs::router())
        .nest("/php", php::router())
        .nest("/mail", mail::router())
        .nest("/dns", dns::router())
        .nest("/firewall", firewall::router())
        .nest("/fail2ban", fail2ban::router())
        .nest("/redis", redis::router())
        .nest("/migrate", backup_import::router())
        .nest("/backup", backup::router().nest("/mine", backup_mine::router()))
        .nest("/docker", docker::router())
        .nest("/notifications", notify::router())
        .nest("/hooks", git::hooks_router())
        .nest("/webhooks", webhooks::router())
        .nest("/sites", sites::router().merge(ssl::router()).merge(git::router()).merge(ftp::router()).merge(wordpress::router()))
}
