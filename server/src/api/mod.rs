//! HTTP API yönlendirmeleri. Bütün uçlar `/api/v1` altındadır.

use axum::Router;

use crate::state::AppState;

mod auth;
mod components;
mod cron;
mod databases;
mod dns;
mod docker;
mod files;
mod firewall;
mod ftp;
mod git;
mod jobs;
mod logs;
mod mail;
mod notify;
mod php;
mod redis;
mod sites;
pub mod ssl;
mod system;
mod terminal;
mod users;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/auth", auth::router())
        .nest("/users", users::router())
        .nest("/system", system::router())
        .nest("/jobs", jobs::router())
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
        .nest("/redis", redis::router())
        .nest("/docker", docker::router())
        .nest("/notifications", notify::router())
        .nest("/hooks", git::hooks_router())
        .nest("/sites", sites::router().merge(ssl::router()).merge(git::router()).merge(ftp::router()))
}
