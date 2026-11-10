//! Sağlık merkezi uçları: bulgular, tek tıkla düzeltme ve kendi kendine bakım
//! ayarları. Yalnızca yöneticiler.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::error::AppResult;
use crate::health::{self, Action, Report, Settings};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(show))
        .route("/check", post(check))
        .route("/fix", post(fix))
        .route("/settings", axum::routing::put(save_settings))
}

#[derive(Serialize)]
struct View {
    report: Report,
    settings: Settings,
    /// Son yapılan düzeltmeler (yeniden eskiye).
    actions: Vec<Action>,
    /// Denetimler bu aralıkla yinelenir (saniye).
    interval_secs: u64,
}

fn view(state: &AppState) -> AppResult<View> {
    Ok(View {
        report: state.health.report(),
        settings: health::load_settings(state),
        actions: health::recent_actions(state, 30)?,
        interval_secs: health::CHECK_INTERVAL.as_secs(),
    })
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(view(&state)?))
}

/// Denetimleri beklemeden hemen yeniden çalıştırır.
async fn check(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    health::run_checks(&state).await;
    Ok(Json(view(&state)?))
}

#[derive(Deserialize)]
struct FixBody {
    key: String,
}

async fn fix(State(state): State<AppState>, user: AuthUser, Json(body): Json<FixBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let message = health::fix_now(&state, &body.key, user.id).await?;
    auth::audit(&state.db, &user, "health.fix", &body.key, "");
    // Düzeltmenin sonucu bir sonraki turu beklemeden ekrana yansısın.
    let recheck = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        health::run_checks(&recheck).await;
    });
    Ok(Json(json!({ "message": message })))
}

async fn save_settings(State(state): State<AppState>, user: AuthUser, Json(settings): Json<Settings>) -> AppResult<Json<View>> {
    user.require_admin()?;
    health::save_settings(&state, &settings)?;
    auth::audit(&state.db, &user, "health.settings", "", "");
    Ok(Json(view(&state)?))
}
