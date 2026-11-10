//! Panel güncelleme uçları. Yalnızca yöneticiler.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::error::AppResult;
use crate::state::AppState;
use crate::update::{self, View};

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(show)).route("/check", post(check)).route("/apply", post(apply))
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(update::view(&state)))
}

async fn check(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(update::check(&state).await?))
}

async fn apply(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Value>> {
    user.require_admin()?;
    auth::audit(&state.db, &user, "panel.update", update::VERSION, "");
    let job_state = state.clone();
    let job_id = state.jobs.start("update", "Panel güncellemesi", Some(user.id), move |ctx| async move {
        // Paket kurulumu gibi sistem işleriyle aynı anda yürümesin.
        let _guard = job_state.apt_lock.lock().await;
        update::apply(&job_state, &ctx, &job_state.config_path).await
    })?;
    Ok(Json(json!({ "job_id": job_id })))
}
