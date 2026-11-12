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
use crate::sys::{exec, metrics, procs, services, updates};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/overview", get(overview))
        .route("/metrics", get(metrics_history))
        .route("/metrics/archive", get(metrics_archive))
        .route("/processes", get(list_processes))
        .route("/processes/{pid}/signal", post(signal_process))
        .route("/services", get(list_services))
        .route("/services/{name}/history", get(service_history))
        .route("/services/{name}/{action}", post(service_action))
        .route("/audit", get(audit_log))
        .route("/updates", get(updates_view))
        .route("/updates/refresh", post(updates_refresh))
        .route("/updates/install", post(updates_install))
        .route("/reboot", post(reboot))
        .route("/reboot/cancel", post(reboot_cancel))
        .route("/http3", get(http3_show).put(http3_save))
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

#[derive(Deserialize)]
struct ServiceHistoryQuery {
    /// `day`, `week` ya da `month`.
    range: Option<String>,
}

#[derive(Serialize)]
struct ServiceHistory {
    unit: String,
    /// Bu servisin ölçüm geçmişi tutuluyor mu? (Yalnızca sağlık merkezinin izlediği servisler.)
    monitored: bool,
    range: String,
    points: Vec<crate::svchist::Point>,
    summary: Option<crate::svchist::Summary>,
}

/// Servisin bellek, işlemci, görev sayısı ve durum geçmişi; yeniden başlama özeti.
async fn service_history(State(state): State<AppState>, user: AuthUser, Path(name): Path<String>, Query(query): Query<ServiceHistoryQuery>) -> AppResult<Json<ServiceHistory>> {
    user.require_admin()?;
    services::validate_unit_name(&name)?;
    let range = query.range.unwrap_or_else(|| "day".to_string());
    let points = crate::svchist::history(&state.db, &name, &range)?.ok_or_else(|| AppError::bad("Geçersiz aralık"))?;
    let monitored = crate::svchist::is_monitored(&name);
    let summary = if monitored { Some(crate::svchist::summary(&state.db, &name)?) } else { None };
    Ok(Json(ServiceHistory { unit: name, monitored, range, points, summary }))
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

/// Bekleyen paketler, yeniden başlatma durumu, çekirdek ve otomatik güncelleme özeti.
async fn updates_view(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<updates::UpdatesView>> {
    user.require_admin()?;
    Ok(Json(updates::view(&state, None).await?))
}

/// Paket listesini ağdan yeniler ve yeniden sayar.
async fn updates_refresh(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<updates::UpdatesView>> {
    user.require_admin()?;
    let warning = updates::refresh(&state).await?;
    auth::audit(&state.db, &user, "system.updates.refresh", "", "");
    Ok(Json(updates::view(&state, warning).await?))
}

#[derive(Deserialize)]
struct InstallBody {
    /// `security` ya da `all`.
    scope: String,
}

async fn updates_install(State(state): State<AppState>, user: AuthUser, Json(body): Json<InstallBody>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let scope = updates::Scope::parse(&body.scope)?;
    let job_id = updates::start_install(&state, Some(user.id), scope)?;
    auth::audit(&state.db, &user, "system.updates.install", &body.scope, "");
    Ok(Json(json!({ "job_id": job_id })))
}

/// Sunucunun yeniden başlatılmasını bir dakika sonrasına zamanlar; `reboot/cancel` ile iptal edilir.
async fn reboot(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    if updates::job_running(&state) {
        return Err(AppError::conflict("Güncelleme görevi sürerken sunucu yeniden başlatılamaz"));
    }
    updates::schedule_reboot().await?;
    auth::audit(&state.db, &user, "system.reboot", "", "1 dakika sonra");
    Ok(Json(json!({ "ok": true })))
}

async fn reboot_cancel(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    updates::cancel_reboot().await?;
    auth::audit(&state.db, &user, "system.reboot.cancel", "", "");
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

// ---- HTTP/3 ----

/// HTTP/3 (QUIC) durumu: nginx destekliyor mu, açık mı.
async fn http3_show(user: AuthUser) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    use crate::hosting::nginx;
    Ok(Json(json!({ "supported": nginx::is_installed() && nginx::http3_supported().await, "enabled": nginx::http3_enabled() })))
}

#[derive(Deserialize)]
struct Http3Body {
    enabled: bool,
}

/// HTTP/3'ü sunucu genelinde açar ya da kapatır: işaret yazılır, tanımsız alan adlarının sunucusu ve
/// sertifikalı sitelerin yapılandırması yeniden üretilir. Yapılandırma geçersiz çıkarsa ayar geri alınır.
async fn http3_save(State(state): State<AppState>, user: AuthUser, Json(body): Json<Http3Body>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    use crate::hosting::{nginx, sites};
    if body.enabled && !nginx::http3_supported().await {
        return Err(AppError::bad("Kurulu nginx HTTP/3 desteğiyle derlenmemiş"));
    }
    let _guard = state.sys_lock.lock().await;
    let before = nginx::http3_enabled();
    if before == body.enabled {
        return Ok(Json(json!({ "supported": true, "enabled": before })));
    }
    nginx::set_http3(body.enabled)?;
    if let Err(err) = nginx::ensure_base().await {
        nginx::set_http3(before)?;
        let _ = nginx::ensure_base().await;
        return Err(err);
    }
    let mut failed = Vec::new();
    for site in sites::list_with_ssl(&state.db)? {
        if site.status != "active" || !site.ip.is_empty() {
            continue;
        }
        if let Err(err) = sites::provision(&site).await {
            tracing::warn!(domain = %site.domain, error = %err, "HTTP/3 ayarı siteye uygulanamadı");
            failed.push(site.domain.clone());
        }
    }
    auth::audit(&state.db, &user, "system.http3", if body.enabled { "on" } else { "off" }, "");
    Ok(Json(json!({ "supported": true, "enabled": body.enabled, "failed": failed })))
}
