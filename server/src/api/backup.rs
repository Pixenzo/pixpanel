//! Yedekleme uçları (yönetici). Hesapların kendi yedekleri `backup_mine` içindedir.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::backup::restic::{self, Entry, Snapshot};
use crate::backup::run::{self, RestoreOptions};
use crate::backup::{self, DatabaseSource, MailSource, Plan, Scope, Target, TargetConfig};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/run", post(run_manual))
        .route("/restore", post(restore))
        .route("/targets", post(create_target))
        .route("/targets/{id}", put(update_target).delete(delete_target))
        .route("/targets/{id}/check", post(check_target))
        .route("/targets/{id}/password", get(show_password))
        .route("/targets/{id}/snapshots", get(list_snapshots))
        .route("/targets/{id}/snapshots/{snapshot}", delete(delete_snapshot))
        .route("/targets/{id}/snapshots/{snapshot}/files", get(list_files))
        .route("/plans", post(create_plan))
        .route("/plans/{id}", put(update_plan).delete(delete_plan))
        .route("/plans/{id}/run", post(run_plan))
}

fn ensure_installed() -> AppResult<()> {
    if backup::is_installed() { Ok(()) } else { Err(AppError::bad("restic kurulu değil; önce Bileşenler sayfasından kurun")) }
}

/// FTP hedefleri rclone ister; kurulu değilse kurar (başka bir kurulum sürüyorsa bitmesini bekler).
async fn ensure_rclone(state: &AppState) -> AppResult<()> {
    if crate::sys::exec::binary_exists("rclone") {
        return Ok(());
    }
    let _guard = state.apt_lock.lock().await;
    let install = crate::sys::exec::run_with_timeout(
        "apt-get",
        &["install", "-y", "--no-install-recommends", "-o", "DPkg::Lock::Timeout=120", "--", "rclone"],
        std::time::Duration::from_secs(300),
    )
    .await?;
    if install.success() && crate::sys::exec::binary_exists("rclone") {
        Ok(())
    } else {
        Err(AppError::bad("FTP hedefi için gereken rclone kurulamadı; sunucunun paket deposuna erişebildiğini denetleyin"))
    }
}

// ---- Genel görünüm ----

#[derive(Serialize)]
struct TargetView {
    id: i64,
    name: String,
    kind: String,
    config: TargetConfig,
    has_secret_key: bool,
    public_key: String,
    last_check_at: Option<i64>,
    last_check_ok: bool,
    last_check_error: String,
    created_at: i64,
    plan_count: usize,
}

fn target_view(target: Target, plans: &[Plan]) -> TargetView {
    TargetView {
        plan_count: plans.iter().filter(|plan| plan.target_id == target.id).count(),
        id: target.id,
        name: target.name,
        kind: target.kind,
        config: target.config,
        has_secret_key: !target.secret.is_empty(),
        public_key: target.public_key,
        last_check_at: target.last_check_at,
        last_check_ok: target.last_check_ok,
        last_check_error: target.last_check_error,
        created_at: target.created_at,
    }
}

#[derive(Serialize)]
struct SiteSource {
    id: i64,
    domain: String,
    owner: String,
}

#[derive(Serialize)]
struct Sources {
    sites: Vec<SiteSource>,
    databases: Vec<DatabaseSource>,
    mail_domains: Vec<MailSource>,
}

#[derive(Serialize)]
struct Overview {
    installed: bool,
    /// Kurulu restic bu panelin gerektirdiği sürümü karşılıyor mu?
    supported: bool,
    version: String,
    targets: Vec<TargetView>,
    plans: Vec<Plan>,
    sources: Sources,
}

async fn build_overview(state: &AppState) -> AppResult<Overview> {
    let version = restic::version().await;
    let plans = backup::list_plans(&state.db)?;
    let targets = backup::list_targets(&state.db)?.into_iter().map(|target| target_view(target, &plans)).collect();
    Ok(Overview {
        installed: backup::is_installed(),
        supported: restic::require_supported().await.is_ok(),
        version: version.map(|(major, minor)| format!("{major}.{minor}")).unwrap_or_default(),
        targets,
        plans,
        sources: Sources {
            sites: backup::all_sites(&state.db)?.into_iter().map(|site| SiteSource { id: site.id, domain: site.domain, owner: site.owner }).collect(),
            databases: backup::all_databases(&state.db)?,
            mail_domains: backup::all_mail(&state.db)?,
        },
    })
}

async fn overview(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    Ok(Json(build_overview(&state).await?))
}

// ---- Hedefler ----

#[derive(Deserialize)]
struct TargetBody {
    name: String,
    #[serde(default)]
    kind: String,
    config: TargetConfig,
    /// S3 gizli anahtarı; güncellemede boş bırakılırsa eskisi korunur.
    #[serde(default)]
    secret_key: String,
    /// Var olan bir depoyu bağlamak için o deponun parolası; boşsa yenisi üretilir.
    #[serde(default)]
    password: String,
}

async fn create_target(State(state): State<AppState>, user: AuthUser, Json(body): Json<TargetBody>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    ensure_installed()?;
    if !backup::KINDS.contains(&body.kind.as_str()) {
        return Err(AppError::bad("Geçersiz hedef türü"));
    }
    let name = backup::validate_name(&body.name)?;
    let config = body.config.normalized(&body.kind)?;
    let secret = match body.kind.as_str() {
        "s3" => backup::validate_secret(&body.secret_key)?,
        "ftp" => {
            ensure_rclone(&state).await?;
            restic::obscure_ftp_password(&body.secret_key).await?
        }
        _ => String::new(),
    };
    let password = if body.password.is_empty() {
        auth::random_password(40)
    } else if body.password.len() > 200 || body.password.chars().any(char::is_control) {
        return Err(AppError::bad("Depo parolası geçersiz"));
    } else {
        body.password.clone()
    };
    let id = backup::insert_target(&state.db, &name, &body.kind, &config, &secret, &password)?;
    if body.kind == "sftp" {
        let mut target = backup::load_target(&state.db, id)?;
        target.public_key = backup::ensure_key(&target).await?;
        backup::update_target(&state.db, &target)?;
    }
    auth::audit(&state.db, &user, "backup.target.create", &name, &body.kind);
    Ok(Json(build_overview(&state).await?))
}

async fn update_target(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<TargetBody>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    let mut target = backup::load_target(&state.db, id)?;
    target.name = backup::validate_name(&body.name)?;
    target.config = body.config.normalized(&target.kind)?;
    if target.kind == "s3" && !body.secret_key.is_empty() {
        target.secret = backup::validate_secret(&body.secret_key)?;
    }
    if target.kind == "ftp" && !body.secret_key.is_empty() {
        ensure_rclone(&state).await?;
        target.secret = restic::obscure_ftp_password(&body.secret_key).await?;
    }
    backup::update_target(&state.db, &target)?;
    auth::audit(&state.db, &user, "backup.target.update", &target.name, "");
    Ok(Json(build_overview(&state).await?))
}

async fn delete_target(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    let target = backup::load_target(&state.db, id)?;
    backup::delete_target(&state.db, &target)?;
    auth::audit(&state.db, &user, "backup.target.delete", &target.name, "");
    Ok(Json(build_overview(&state).await?))
}

/// Hedefe bağlanmayı dener; depo yoksa oluşturur. Sonuç hedefe kaydedilir.
async fn check_target(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    ensure_installed()?;
    restic::require_supported().await?;
    let target = backup::load_target(&state.db, id)?;
    let outcome = restic::check(&target).await;
    backup::save_check(&state.db, id, outcome.as_ref().err().map(String::as_str))?;
    Ok(Json(build_overview(&state).await?))
}

/// Depo parolası: sunucu tümüyle kaybedilirse yedekleri açmanın tek yolu.
async fn show_password(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let target = backup::load_target(&state.db, id)?;
    auth::audit(&state.db, &user, "backup.target.password", &target.name, "");
    Ok(Json(json!({ "password": target.password })))
}

// ---- Yedekler ----

async fn list_snapshots(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let target = backup::load_target(&state.db, id)?;
    let snapshots: Vec<Snapshot> = restic::snapshots(&target).await?;
    Ok(Json(json!({ "snapshots": snapshots })))
}

#[derive(Deserialize)]
struct FilesQuery {
    #[serde(default)]
    path: String,
}

/// Yedeğin gezilebilen kökü: site klasörü ya da posta alanının klasörü.
pub(super) fn browse_root(snapshot: &Snapshot) -> AppResult<String> {
    let prefix = match snapshot.kind.as_str() {
        "site" => "/home/",
        "mail" => "/var/vmail/",
        _ => return Err(AppError::bad("Bu yedeğin içinde gezilemez")),
    };
    snapshot.paths.iter().find(|path| path.starts_with(prefix)).cloned().ok_or_else(|| AppError::bad("Yedekte gezilecek klasör yok"))
}

async fn list_files(State(state): State<AppState>, user: AuthUser, Path((id, snapshot)): Path<(i64, String)>, Query(query): Query<FilesQuery>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let target = backup::load_target(&state.db, id)?;
    let snapshot = restic::find_snapshot(&target, &snapshot).await?;
    let root = browse_root(&snapshot)?;
    let path = if query.path.is_empty() { root.clone() } else { query.path.trim_end_matches('/').to_string() };
    let inside = std::path::Path::new(&path).starts_with(&root)
        && !path.split('/').any(|part| part == "..")
        && !path.chars().any(char::is_control)
        && path.len() < 4096;
    if !inside {
        return Err(AppError::bad("Bu yol yedeğin kapsamı dışında"));
    }
    let entries: Vec<Entry> = restic::list_dir(&target, &snapshot.id, &path).await?;
    Ok(Json(json!({ "root": root, "path": path, "entries": entries })))
}

async fn delete_snapshot(State(state): State<AppState>, user: AuthUser, Path((id, snapshot)): Path<(i64, String)>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    let target = backup::load_target(&state.db, id)?;
    let snapshot = restic::find_snapshot(&target, &snapshot).await?;
    auth::audit(&state.db, &user, "backup.snapshot.delete", &format!("{} {}", snapshot.kind, snapshot.name), &snapshot.short_id);
    let job_id = run::start_forget(&state, user.id, target, snapshot)?;
    Ok(Json(json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct ManualBody {
    target_id: i64,
    scope: Scope,
}

async fn run_manual(State(state): State<AppState>, user: AuthUser, Json(body): Json<ManualBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    restic::require_supported().await?;
    if body.scope.is_empty() {
        return Err(AppError::bad("Yedeklenecek en az bir şey seçin"));
    }
    let target = backup::load_target(&state.db, body.target_id)?;
    auth::audit(&state.db, &user, "backup.run", &target.name, "elle");
    let job_id = run::start_backup(&state, Some(user.id), target, body.scope, None)?;
    Ok(Json(json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct RestoreBody {
    target_id: i64,
    snapshot: String,
    #[serde(flatten)]
    options: RestoreOptions,
}

async fn restore(State(state): State<AppState>, user: AuthUser, Json(body): Json<RestoreBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    restic::require_supported().await?;
    let target = backup::load_target(&state.db, body.target_id)?;
    let snapshot = restic::find_snapshot(&target, &body.snapshot).await?;
    auth::audit(&state.db, &user, "backup.restore", &format!("{} {}", snapshot.kind, snapshot.name), &snapshot.short_id);
    let job_id = run::start_restore(&state, user.id, target, snapshot, body.options)?;
    Ok(Json(json!({ "job_id": job_id })))
}

// ---- Planlar ----

#[derive(Deserialize)]
struct PlanBody {
    name: String,
    target_id: i64,
    scope: Scope,
    frequency: String,
    /// `SS:DD` biçiminde, sunucunun yerel saatiyle.
    #[serde(default)]
    time: String,
    #[serde(default)]
    weekday: Option<i64>,
    #[serde(default)]
    month_day: Option<i64>,
    keep_daily: i64,
    keep_weekly: i64,
    keep_monthly: i64,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

fn parse_time(value: &str) -> AppResult<i64> {
    let invalid = || AppError::bad("Saat SS:DD biçiminde olmalı, ör. 03:30");
    let (hours, minutes) = value.trim().split_once(':').ok_or_else(invalid)?;
    let (hours, minutes): (i64, i64) = (hours.parse().map_err(|_| invalid())?, minutes.parse().map_err(|_| invalid())?);
    if (0..24).contains(&hours) && (0..60).contains(&minutes) { Ok(hours * 60 + minutes) } else { Err(invalid()) }
}

/// Gövdeyi plana işler, doğrular ve bir sonraki çalışma zamanını hesaplar.
async fn fill_plan(state: &AppState, mut plan: Plan, body: PlanBody) -> AppResult<Plan> {
    plan.name = backup::validate_name(&body.name)?;
    plan.target_id = backup::load_target(&state.db, body.target_id)?.id;
    plan.scope = body.scope;
    plan.frequency = body.frequency;
    plan.at_minute = if plan.frequency == "manual" && body.time.trim().is_empty() { plan.at_minute } else { parse_time(&body.time)? };
    plan.weekday = body.weekday.unwrap_or(plan.weekday);
    plan.month_day = body.month_day.unwrap_or(plan.month_day);
    plan.keep_daily = body.keep_daily;
    plan.keep_weekly = body.keep_weekly;
    plan.keep_monthly = body.keep_monthly;
    plan.enabled = body.enabled;
    plan.validate()?;
    plan.next_run_at = plan.next_run(now(), backup::local_offset().await);
    Ok(plan)
}

async fn create_plan(State(state): State<AppState>, user: AuthUser, Json(body): Json<PlanBody>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    let blank = Plan {
        id: 0,
        user_id: user.id,
        name: String::new(),
        target_id: 0,
        scope: Scope::default(),
        frequency: "daily".to_string(),
        at_minute: 180,
        weekday: 1,
        month_day: 1,
        keep_daily: 7,
        keep_weekly: 4,
        keep_monthly: 6,
        enabled: true,
        next_run_at: None,
        last_run_at: None,
        last_status: String::new(),
        last_job_id: None,
        created_at: now(),
    };
    let plan = fill_plan(&state, blank, body).await?;
    backup::save_plan(&state.db, &plan)?;
    auth::audit(&state.db, &user, "backup.plan.create", &plan.name, &plan.frequency);
    Ok(Json(build_overview(&state).await?))
}

async fn update_plan(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<PlanBody>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    let plan = fill_plan(&state, backup::load_plan(&state.db, id)?, body).await?;
    backup::save_plan(&state.db, &plan)?;
    auth::audit(&state.db, &user, "backup.plan.update", &plan.name, &plan.frequency);
    Ok(Json(build_overview(&state).await?))
}

async fn delete_plan(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Overview>> {
    user.require_admin()?;
    let plan = backup::load_plan(&state.db, id)?;
    backup::delete_plan(&state.db, id)?;
    auth::audit(&state.db, &user, "backup.plan.delete", &plan.name, "");
    Ok(Json(build_overview(&state).await?))
}

async fn run_plan(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    ensure_installed()?;
    restic::require_supported().await?;
    let plan = backup::load_plan(&state.db, id)?;
    auth::audit(&state.db, &user, "backup.run", &plan.name, "plan");
    let job_id = run::start_plan(&state, Some(user.id), plan)?;
    Ok(Json(json!({ "job_id": job_id })))
}
