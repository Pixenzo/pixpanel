//! HTTP API yönlendirmeleri. Bütün uçlar `/api/v1` altındadır.

use axum::Router;

use crate::state::AppState;

mod auth;
mod components;
mod databases;
mod files;
mod jobs;
mod sites;
pub mod ssl;
mod system;
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
        .nest("/sites", sites::router().merge(ssl::router()))
}
