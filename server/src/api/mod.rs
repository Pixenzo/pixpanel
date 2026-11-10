//! HTTP API yönlendirmeleri. Bütün uçlar `/api/v1` altındadır.

use axum::Router;

use crate::state::AppState;

mod auth;
mod components;
mod jobs;
mod sites;
mod system;
mod users;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/auth", auth::router())
        .nest("/users", users::router())
        .nest("/system", system::router())
        .nest("/jobs", jobs::router())
        .nest("/components", components::router())
        .nest("/sites", sites::router())
}
