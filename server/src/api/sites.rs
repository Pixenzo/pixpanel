//! Site yönetimi uçları.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser, Role};
use crate::error::{AppError, AppResult};
use crate::hosting::php::{self, PhpSettings};
use crate::hosting::sites::{self, NewSite, Site};
use crate::hosting::{domain, nginx};
use crate::state::AppState;

const AREA: &str = "sites";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/options", get(options))
        .route("/{id}", get(detail).patch(update).delete(remove))
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<Site>>> {
    user.require(AREA)?;
    Ok(Json(sites::list(&state.db, &user)?))
}

async fn detail(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Site>> {
    user.require(AREA)?;
    Ok(Json(sites::get(&state.db, &user, id)?))
}

#[derive(Serialize)]
struct OwnerOption {
    id: i64,
    username: String,
}

#[derive(Serialize)]
struct Options {
    nginx_installed: bool,
    php_versions: Vec<String>,
    /// Sitenin atanabileceği hesaplar (yönetici ve bayiler için).
    owners: Vec<OwnerOption>,
    /// Sürüm başına genel PHP ayarları; site formunda devralınan değerleri göstermek için.
    php_defaults: std::collections::BTreeMap<String, php::PhpGlobal>,
}

async fn options(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Options>> {
    user.require(AREA)?;
    let owners = match user.role {
        Role::Admin | Role::Reseller => {
            let (all, me) = (user.is_admin(), user.id);
            state.db.call(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, username FROM users
                     WHERE role != 'member' AND (?1 OR id = ?2 OR parent_id = ?2) ORDER BY username",
                )?;
                let rows = stmt.query_map(params![all, me], |row| Ok(OwnerOption { id: row.get(0)?, username: row.get(1)? }))?;
                rows.collect()
            })?
        }
        _ => Vec::new(),
    };
    let php_versions = php::installed_versions();
    let php_defaults = php_versions.iter().map(|version| (version.clone(), php::load_global(version))).collect();
    Ok(Json(Options { nginx_installed: nginx::is_installed(), php_versions, owners, php_defaults }))
}

#[derive(Deserialize)]
struct CreateBody {
    domain: String,
    kind: String,
    php_version: Option<String>,
    proxy_target: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    /// Sitenin sahibi; verilmezse isteği yapan hesap.
    user_id: Option<i64>,
}

/// Sitenin hangi hesaba açılacağını belirler ve isteği yapanın buna yetkisini denetler.
fn resolve_owner(state: &AppState, user: &AuthUser, requested: Option<i64>) -> AppResult<i64> {
    let Some(target) = requested.filter(|id| *id != user.owner_id()) else {
        return Ok(user.owner_id());
    };
    let found = state.db.call(|conn| {
        conn.query_row("SELECT role, parent_id FROM users WHERE id = ?1", params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .optional()
    })?;
    let (role, parent_id) = found.ok_or_else(|| AppError::bad("Seçilen hesap bulunamadı"))?;
    let allowed = match user.role {
        Role::Admin => true,
        Role::Reseller => parent_id == Some(user.id),
        _ => false,
    };
    if !allowed || role == "member" {
        return Err(AppError::Forbidden);
    }
    Ok(target)
}

fn validate_kind(kind: &str) -> AppResult<()> {
    if sites::KINDS.contains(&kind) { Ok(()) } else { Err(AppError::bad("Geçersiz site türü")) }
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateBody>,
) -> AppResult<Json<Site>> {
    user.require(AREA)?;
    validate_kind(&body.kind)?;
    let owner_id = resolve_owner(&state, &user, body.user_id)?;
    let domain_name = domain::normalize(&body.domain)?;
    let aliases = sites::normalize_aliases(&domain_name, &body.aliases)?;

    let php_version = match body.kind.as_str() {
        "php" => {
            let version = body.php_version.ok_or_else(|| AppError::bad("PHP sürümü seçin"))?;
            php::validate_version(&version)?;
            Some(version)
        }
        _ => None,
    };
    let proxy_target = match body.kind.as_str() {
        "proxy" => {
            let target = body.proxy_target.ok_or_else(|| AppError::bad("Hedef adresi girin"))?;
            Some(sites::validate_proxy_target(&target)?)
        }
        _ => None,
    };

    // Sistem dosyalarına dokunan işlemler sırayla yürür.
    let _guard = state.sys_lock.lock().await;
    let id = sites::insert(
        &state.db,
        &NewSite { user_id: owner_id, domain: domain_name.clone(), aliases, kind: body.kind, php_version, proxy_target },
    )?;
    let site = sites::get(&state.db, &user, id)?;
    if let Err(err) = sites::provision(&site).await {
        // Yarım kalan kurulum geri alınır; kullanıcının dosyalarına dokunulmaz.
        if let Err(cleanup) = sites::deprovision(&site, false).await {
            tracing::warn!(error = %cleanup, domain = %site.domain, "yarım site temizlenemedi");
        }
        sites::delete_row(&state.db, id)?;
        return Err(err);
    }
    auth::audit(&state.db, &user, "site.create", &domain_name, &site.kind);
    Ok(Json(site))
}

#[derive(Deserialize)]
struct UpdateBody {
    kind: Option<String>,
    aliases: Option<Vec<String>>,
    public_dir: Option<String>,
    php_version: Option<String>,
    php_settings: Option<PhpSettings>,
    proxy_target: Option<String>,
    proxy_websocket: Option<bool>,
    force_https: Option<bool>,
    hsts: Option<bool>,
    nginx_custom: Option<String>,
    status: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<Site>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let previous = sites::get(&state.db, &user, id)?;
    let mut site = previous.clone();

    if let Some(kind) = body.kind {
        validate_kind(&kind)?;
        site.kind = kind;
    }
    if let Some(aliases) = &body.aliases {
        site.aliases = sites::normalize_aliases(&site.domain, aliases)?;
    }
    if let Some(public_dir) = &body.public_dir {
        site.public_dir = sites::validate_public_dir(public_dir)?;
    }
    if let Some(version) = body.php_version {
        php::validate_version(&version)?;
        site.php_version = Some(version);
    }
    if let Some(settings) = body.php_settings {
        settings.validate()?;
        site.php_settings = settings;
    }
    if let Some(target) = &body.proxy_target {
        site.proxy_target = Some(sites::validate_proxy_target(target)?);
    }
    if let Some(websocket) = body.proxy_websocket {
        site.proxy_websocket = websocket;
    }
    if let Some(force_https) = body.force_https {
        site.force_https = force_https;
    }
    if let Some(hsts) = body.hsts {
        site.hsts = hsts;
    }
    if let Some(custom) = body.nginx_custom {
        // Ham nginx yönergeleri sunucunun tamamını etkileyebilir.
        user.require_admin()?;
        if custom.len() > 16 * 1024 {
            return Err(AppError::bad("Özel yönergeler 16 KB'ı aşamaz"));
        }
        site.nginx_custom = custom;
    }
    if let Some(status) = body.status {
        if !matches!(status.as_str(), "active" | "suspended") {
            return Err(AppError::bad("Geçersiz durum"));
        }
        // Site sahibi, askıya alınmış sitesini kendi kendine açamaz.
        if !user.is_admin() && !(user.role == Role::Reseller && site.user_id != user.id) {
            return Err(AppError::Forbidden);
        }
        site.status = status;
    }
    if site.kind == "php" && site.php_version.is_none() {
        return Err(AppError::bad("PHP sürümü seçin"));
    }
    if site.kind == "proxy" && site.proxy_target.is_none() {
        return Err(AppError::bad("Hedef adresi girin"));
    }
    if (site.force_https || site.hsts) && !sites::ssl_ready(&site) {
        return Err(AppError::bad("HTTPS yönlendirmesi için önce siteye bir SSL sertifikası tanımlayın"));
    }

    sites::save(&state.db, &site)?;
    if let Err(err) = sites::provision(&site).await {
        // Yeni ayarlar uygulanamadıysa kayıt ve sistem eski hâline döndürülür.
        sites::save(&state.db, &previous)?;
        if let Err(restore) = sites::provision(&previous).await {
            tracing::error!(error = %restore, domain = %previous.domain, "site eski ayarlarına döndürülemedi");
        }
        return Err(err);
    }
    auth::audit(&state.db, &user, "site.update", &site.domain, "");
    Ok(Json(site))
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    delete_files: bool,
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(query): Query<RemoveQuery>,
) -> AppResult<Json<serde_json::Value>> {
    user.require(AREA)?;
    let _guard = state.sys_lock.lock().await;
    let site = sites::get(&state.db, &user, id)?;
    sites::deprovision(&site, query.delete_files).await?;
    sites::delete_row(&state.db, id)?;
    let detail = if query.delete_files { "dosyalarla birlikte" } else { "" };
    auth::audit(&state.db, &user, "site.delete", &site.domain, detail);
    Ok(Json(json!({ "ok": true })))
}
