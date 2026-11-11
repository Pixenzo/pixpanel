//! Sunucu durumu: genel bakış, ölçümler, süreçler, servisler ve denetim günlüğü.
//! Bu uçların tamamı yalnızca yöneticilere açıktır.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sys::{exec, metrics, procs, services};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/overview", get(overview))
        .route("/metrics", get(metrics_history))
        .route("/metrics/archive", get(metrics_archive))
        .route("/processes", get(list_processes))
        .route("/processes/{pid}/signal", post(signal_process))
        .route("/services", get(list_services))
        .route("/services/{name}/{action}", post(service_action))
        .route("/audit", get(audit_log))
}

#[derive(Serialize)]
struct Overview {
    host: metrics::HostInfo,
    current: metrics::Sample,
    disks: Vec<metrics::Disk>,
}

async fn overview(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    let (host, disks) = tokio::join!(metrics::host_info(), metrics::disks());
    Ok(Json(Overview { host, current: state.metrics.latest(), disks }))
}

#[derive(Deserialize)]
struct HistoryQuery {
    /// İstenen örnek sayısı (2 saniyede bir örnek alınır).
    count: Option<usize>,
}

async fn metrics_history(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<HistoryQuery>,
) -> AppResult<Json<Vec<metrics::Sample>>> {
    user.require_admin()?;
    Ok(Json(state.metrics.history(query.count.unwrap_or(150).clamp(1, 900))))
}

#[derive(Deserialize)]
struct ArchiveQuery {
    /// `day`, `week`, `month` ya da `year`.
    range: String,
}

/// Sunucu yükünün uzun dönem geçmişi (aralığa göre seyreltilmiş ortalamalar).
async fn metrics_archive(State(state): State<AppState>, user: AuthUser, Query(query): Query<ArchiveQuery>) -> AppResult<Json<Vec<metrics::ArchivePoint>>> {
    user.require_admin()?;
    let points = metrics::archive(&state.db, &query.range)?.ok_or_else(|| crate::error::AppError::bad("Geçersiz aralık"))?;
    Ok(Json(points))
}

async fn list_processes(user: AuthUser) -> AppResult<Json<Vec<procs::Process>>> {
    user.require_admin()?;
    Ok(Json(procs::list().await))
}

#[derive(Deserialize)]
struct SignalBody {
    signal: String,
}

async fn signal_process(
    State(state): State<AppState>,
    user: AuthUser,
    Path(pid): Path<u32>,
    Json(body): Json<SignalBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    if !matches!(body.signal.as_str(), "TERM" | "KILL" | "HUP" | "INT" | "STOP" | "CONT") {
        return Err(AppError::bad("Geçersiz sinyal"));
    }
    // pid 1 (init) ve panelin kendisi yanlışlıkla öldürülmemeli.
    if pid <= 1 || pid == std::process::id() {
        return Err(AppError::bad("Bu sürece sinyal gönderilemez"));
    }
    exec::run_ok("kill", &["-s", &body.signal, "--", &pid.to_string()]).await?;
    auth::audit(&state.db, &user, "process.signal", &pid.to_string(), &body.signal);
    Ok(Json(json!({ "ok": true })))
}

async fn list_services(user: AuthUser) -> AppResult<Json<Vec<services::Service>>> {
    user.require_admin()?;
    Ok(Json(services::list().await?))
}

async fn service_action(
    State(state): State<AppState>,
    user: AuthUser,
    Path((name, action)): Path<(String, String)>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    services::action(&name, &action).await?;
    auth::audit(&state.db, &user, "service.action", &name, &action);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<i64>,
    before_id: Option<i64>,
}

#[derive(Serialize)]
struct AuditEntry {
    id: i64,
    username: String,
    action: String,
    target: String,
    detail: String,
    ip: String,
    created_at: i64,
}

async fn audit_log(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<AuditQuery>,
) -> AppResult<Json<Vec<AuditEntry>>> {
    user.require_admin()?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let entries = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, username, action, target, detail, ip, created_at FROM audit_log
             WHERE ?1 IS NULL OR id < ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query.before_id, limit], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                username: row.get(1)?,
                action: row.get(2)?,
                target: row.get(3)?,
                detail: row.get(4)?,
                ip: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect()
    })?;
    Ok(Json(entries))
}
