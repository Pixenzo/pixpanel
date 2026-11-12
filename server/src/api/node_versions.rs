//! Node.js sürüm güncellemeleri. `/system/node` altında yayınlanır; yalnızca yöneticiler.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::AppResult;
use crate::hosting::node_update::{self, VersionInfo};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/versions", get(list)).route("/versions/{major}/update", post(update))
}

/// Kurulu Node.js ana sürümleri: kurulu ve en son yama sürümü, güncelleme var mı, kaç site kullanıyor.
async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<VersionInfo>>> {
    user.require_admin()?;
    Ok(Json(node_update::versions(&state.db).await?))
}

/// Ana sürümü en son yama sürümüne günceller (görev); kullanan servisler sırayla yeniden başlatılır.
async fn update(State(state): State<AppState>, user: AuthUser, Path(major): Path<String>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let job_id = node_update::start_job(&state, Some(user.id), &major).await?;
    auth::audit(&state.db, &user, "node.update", &major, "");
    Ok(Json(json!({ "job_id": job_id })))
}
