//! Başka panelin yedek dosyasından içe aktarma uçları (yalnızca yönetici).

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::backup_import::{self, Inspection, ImportRequest};
use crate::error::AppResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/backup/inspect", post(inspect))
        .route("/backup/import", post(import))
        .route("/backup/{id}", get(detail).delete(remove))
}

#[derive(Deserialize)]
struct InspectBody {
    /// Yedek dosyasının (ya da açılmış yedek klasörünün) sunucudaki mutlak yolu.
    path: String,
}

/// Yedeği güvenli geçici klasöre açıp içindekileri çıkaran görevi başlatır (arka plan görevi).
async fn inspect(State(state): State<AppState>, user: AuthUser, Json(body): Json<InspectBody>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let (inspect_id, job_id) = backup_import::start_inspect(&state, &user, &body.path)?;
    auth::audit(&state.db, &user, "migrate.backup.inspect", body.path.trim(), &inspect_id);
    Ok(Json(json!({ "inspect_id": inspect_id, "job_id": job_id })))
}

/// İncelemenin durumu ve bitmişse içindekilerin önizlemesi.
async fn detail(State(state): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<Inspection>> {
    user.require_admin()?;
    Ok(Json(backup_import::get_inspection(&state, &id)?))
}

/// Seçilen alan adlarını ve veritabanlarını hedef hesaba içe aktaran görevi başlatır.
async fn import(State(state): State<AppState>, user: AuthUser, Json(body): Json<ImportRequest>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let job_id = backup_import::start_import(&state, &user, body).await?;
    Ok(Json(json!({ "job_id": job_id })))
}

/// Geçici klasörü (açılmış yedek ve önizleme) siler.
async fn remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<String>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    backup_import::delete_inspection(&state, &id)?;
    auth::audit(&state.db, &user, "migrate.backup.delete", &id, "");
    Ok(Json(json!({ "ok": true })))
}
