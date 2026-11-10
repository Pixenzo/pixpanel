//! Sunucu bileşenlerinin listelenmesi ve kurulması. Yalnızca yöneticiler.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::components::{self, ComponentStatus};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list)).route("/{id}/install", post(install))
}

async fn list(user: AuthUser) -> AppResult<Json<Vec<ComponentStatus>>> {
    user.require_admin()?;
    Ok(Json(components::status_all().await))
}

async fn install(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let component = components::find(&id)?;
    if components::status(&component).await.installed {
        return Err(AppError::conflict(format!("{} zaten kurulu", component.name)));
    }
    let apt_lock = state.apt_lock.clone();
    let title = format!("{} kurulumu", component.name);
    let job_id = state.jobs.start("component.install", &title, Some(user.id), move |ctx| async move {
        // Aynı anda yalnızca bir paket işlemi yürür; diğerleri sırasını bekler.
        if apt_lock.try_lock().is_err() {
            ctx.log("Başka bir kurulumun bitmesi bekleniyor");
        }
        let _guard = apt_lock.lock().await;
        components::install(&ctx, &component).await
    })?;
    auth::audit(&state.db, &user, "component.install", &id, "");
    Ok(Json(json!({ "job_id": job_id })))
}
