//! Genel PHP yönetimi: sürüm başına ayarlar ve eklentiler. Yalnızca yöneticiler.
//!
//! Buradaki ayarlar o sürümü kullanan bütün sitelerin varsayılanıdır; siteler
//! tek tek değerleri kendi ayarlarında ezebilir.

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::components;
use crate::error::{AppError, AppResult};
use crate::hosting::php::{self, Extension, PhpGlobal};
use crate::hosting::{sites, sysuser};
use crate::state::AppState;
use crate::sys::services;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{version}/settings", put(save_settings))
        .route("/{version}/extensions", get(list_extensions))
        .route("/{version}/extensions/{name}", post(toggle_extension))
        .route("/{version}/extensions/{name}/install", post(install_extension))
}

#[derive(Serialize)]
struct VersionInfo {
    version: String,
    running: bool,
    site_count: usize,
    settings: PhpGlobal,
}

fn require_installed(version: &str) -> AppResult<()> {
    php::validate_version(version)?;
    if php::installed_versions().iter().any(|installed| installed == version) {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<VersionInfo>>> {
    user.require_admin()?;
    let mut versions = Vec::new();
    for version in php::installed_versions() {
        versions.push(VersionInfo {
            running: services::is_active(&format!("php{version}-fpm.service")).await,
            site_count: sites::list_by_php(&state.db, &version)?.len(),
            settings: php::load_global(&version),
            version,
        });
    }
    Ok(Json(versions))
}

async fn save_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(version): Path<String>,
    Json(settings): Json<PhpGlobal>,
) -> AppResult<Json<PhpGlobal>> {
    user.require_admin()?;
    require_installed(&version)?;
    let _guard = state.sys_lock.lock().await;
    php::save_global(&version, &settings).await?;

    // Devralınan değerler havuz dosyalarına da yazıldığından, bu sürümdeki
    // sitelerin havuzları yeni genel ayarlarla yeniden üretilir.
    let mut failed = Vec::new();
    for site in sites::list_by_php(&state.db, &version)? {
        let Some(account) = sysuser::lookup(&site.owner) else { continue };
        if let Err(err) = php::apply_pool(site.id, &site.domain, &version, &account, &site.root(), &site.php_settings).await {
            tracing::error!(error = %err, domain = %site.domain, "site havuzu genel ayarlarla yenilenemedi");
            failed.push(site.domain);
        }
    }
    php::reload(&version).await?;
    auth::audit(&state.db, &user, "php.settings", &version, "");
    if failed.is_empty() {
        Ok(Json(settings))
    } else {
        Err(AppError::internal(format!("Ayarlar kaydedildi ama şu sitelere uygulanamadı: {}", failed.join(", "))))
    }
}

#[derive(Serialize)]
struct Extensions {
    installed: Vec<Extension>,
    /// Kurulu olmayan ama panelden kurulabilen eklentiler.
    installable: Vec<&'static str>,
}

async fn list_extensions(user: AuthUser, Path(version): Path<String>) -> AppResult<Json<Extensions>> {
    user.require_admin()?;
    require_installed(&version)?;
    let installed = php::extensions(&version)?;
    // Paket adı ile modül adı her zaman aynı değildir (ör. `mysql` paketi
    // `mysqli`, `mysqlnd`, `pdo_mysql` modüllerini getirir); kaba eşleşme yeterli.
    let installable = php::INSTALLABLE_EXTENSIONS
        .iter()
        .copied()
        .filter(|name| !installed.iter().any(|extension| extension.name.contains(name)))
        .collect();
    Ok(Json(Extensions { installed, installable }))
}

#[derive(Deserialize)]
struct ToggleBody {
    enabled: bool,
}

async fn toggle_extension(
    State(state): State<AppState>,
    user: AuthUser,
    Path((version, name)): Path<(String, String)>,
    Json(body): Json<ToggleBody>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    require_installed(&version)?;
    let _guard = state.sys_lock.lock().await;
    php::set_extension(&version, &name, body.enabled).await?;
    auth::audit(&state.db, &user, "php.extension", &format!("{version}/{name}"), if body.enabled { "açıldı" } else { "kapatıldı" });
    Ok(Json(json!({ "ok": true })))
}

async fn install_extension(
    State(state): State<AppState>,
    user: AuthUser,
    Path((version, name)): Path<(String, String)>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    require_installed(&version)?;
    if !php::INSTALLABLE_EXTENSIONS.contains(&name.as_str()) {
        return Err(AppError::bad("Bu eklenti panelden kurulamıyor"));
    }
    let package = format!("php{version}-{name}");
    let apt_lock = state.apt_lock.clone();
    let title = format!("PHP {version} eklentisi: {name}");
    let reload_version = version.clone();
    let job_id = state.jobs.start("php.extension", &title, Some(user.id), move |ctx| async move {
        let _guard = apt_lock.lock().await;
        ctx.exec("apt-get", &["update", "-o", "DPkg::Lock::Timeout=180"]).await?;
        components::apt_install(&ctx, &[package]).await?;
        php::reload(&reload_version).await?;
        ctx.log("Eklenti kuruldu");
        Ok(())
    })?;
    auth::audit(&state.db, &user, "php.extension.install", &format!("{version}/{name}"), "");
    Ok(Json(json!({ "job_id": job_id })))
}
