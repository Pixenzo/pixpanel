//! WordPress araçları. `/sites/{id}/wordpress` altında yayınlanır.

use axum::extract::{Path, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::hosting::sites;
use crate::state::AppState;
use crate::wordpress::{self, staging, InstallRequest, Settings, Status};

const AREA: &str = "sites";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/wordpress", get(show))
        .route("/{id}/wordpress/install", post(install))
        .route("/{id}/wordpress/update", post(update))
        .route("/{id}/wordpress/login", post(login))
        .route("/{id}/wordpress/settings", put(save_settings))
        .route("/{id}/wordpress/staging", get(staging_show).post(staging_create))
        .route("/{id}/wordpress/staging/sync", post(staging_sync))
        .route("/{id}/wordpress/{kind}/{slug}", post(extension))
}

async fn show(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Status>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    Ok(Json(wordpress::status(&site).await?))
}

async fn install(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<InstallRequest>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let request = body.validated()?;
    // Kurulum bir veritabanı açar; paketin veritabanı sınırı burada da geçerlidir.
    crate::quota::check(&state.db, site.user_id, crate::quota::Resource::Database)?;
    wordpress::check_installable(&site).await?;
    let lock = wordpress::SiteLock::acquire(&site)?;

    auth::audit(&state.db, &user, "wordpress.install", &site.domain, "");
    let job_state = state.clone();
    let title = format!("WordPress kurulumu: {}", site.domain);
    let job_id = state.jobs.start("wordpress.install", &title, Some(user.id), move |ctx| async move {
        let _lock = lock;
        wordpress::install(&ctx, &job_state, &site, &request).await
    })?;
    Ok(Json(json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct UpdateBody {
    /// `all`, `core`, `plugins` ya da `themes`.
    what: String,
}

async fn update(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<UpdateBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    if !matches!(body.what.as_str(), "all" | "core" | "plugins" | "themes") {
        return Err(AppError::bad("Geçersiz güncelleme kapsamı"));
    }
    auth::audit(&state.db, &user, "wordpress.update", &site.domain, &body.what);
    let job_id = wordpress::start_update_job(&state, Some(user.id), site, body.what)?;
    Ok(Json(json!({ "job_id": job_id })))
}

async fn login(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let url = wordpress::login_url(&site).await?;
    auth::audit(&state.db, &user, "wordpress.login", &site.domain, "");
    Ok(Json(json!({ "url": url })))
}

async fn save_settings(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<Settings>) -> AppResult<Json<Status>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    wordpress::apply_settings(&site, &body).await?;
    auth::audit(&state.db, &user, "wordpress.settings", &site.domain, "");
    Ok(Json(wordpress::status(&site).await?))
}

#[derive(Deserialize)]
struct ExtensionBody {
    /// `activate`, `deactivate`, `update`, `delete`, `auto-on` ya da `auto-off`.
    action: String,
}

async fn extension(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, kind, slug)): Path<(i64, String, String)>,
    Json(body): Json<ExtensionBody>,
) -> AppResult<Json<Status>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let kind = match kind.as_str() {
        "plugins" => "plugin",
        "themes" => "theme",
        _ => return Err(AppError::NotFound),
    };
    wordpress::extension_action(&site, kind, &slug, &body.action).await?;
    auth::audit(&state.db, &user, "wordpress.extension", &site.domain, &format!("{kind} {slug} {}", body.action));
    Ok(Json(wordpress::status(&site).await?))
}

// ---- Hazırlık ortamı ----

/// Sitenin hazırlık ortamı bağları: site bir hazırlık ortamıysa asıl sitesi, değilse kendi
/// hazırlık ortamları.
async fn staging_show(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let live = match site.staging_of {
        Some(live_id) => sites::get(&state.db, &user, live_id).ok().map(|live| json!({ "id": live.id, "domain": live.domain })),
        None => None,
    };
    let copies: Vec<Value> = sites::stagings_of(&state.db, site.id)?.into_iter().map(|(id, domain)| json!({ "id": id, "domain": domain })).collect();
    Ok(Json(json!({ "live": live, "copies": copies, "suggested": staging::suggested_domain(&site.domain) })))
}

#[derive(Deserialize)]
struct StagingBody {
    domain: String,
}

/// Sitenin hazırlık ortamını açar: site yeni alan adına veritabanıyla birlikte kopyalanır.
async fn staging_create(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<StagingBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    if site.staging_of.is_some() {
        return Err(AppError::bad("Bu site zaten bir hazırlık ortamı; hazırlık ortamı asıl siteden açılır"));
    }
    let (job_id, site_id) = super::sites::start_clone(&state, &user, site, &body.domain, true).await?;
    Ok(Json(json!({ "job_id": job_id, "site_id": site_id })))
}

#[derive(Deserialize)]
struct StagingSyncBody {
    /// `push`: hazırlık ortamı canlıya aktarılır; `refresh`: hazırlık ortamı canlıdan yenilenir.
    direction: String,
    #[serde(default)]
    files: bool,
    #[serde(default)]
    database: bool,
}

/// Hazırlık ortamı ile asıl site arasında aktarım (arka plan görevi). `id` hazırlık ortamıdır.
async fn staging_sync(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<StagingSyncBody>) -> AppResult<Json<Value>> {
    user.require(AREA)?;
    let copy = sites::get(&state.db, &user, id)?;
    let live_id = copy.staging_of.ok_or_else(|| AppError::bad("Bu site bir hazırlık ortamı değil"))?;
    let live = sites::get(&state.db, &user, live_id)?;
    let direction = match body.direction.as_str() {
        "push" => staging::Direction::Push,
        "refresh" => staging::Direction::Refresh,
        _ => return Err(AppError::bad("Geçersiz aktarım yönü")),
    };
    if !body.files && !body.database {
        return Err(AppError::bad("Aktarılacak en az bir şey seçin: dosyalar ya da veritabanı"));
    }
    if copy.status != "active" || live.status != "active" || copy.user_id != live.user_id {
        return Err(AppError::bad("Aktarım için iki site de etkin ve aynı hesapta olmalı"));
    }
    let locks = (wordpress::SiteLock::acquire(&copy)?, wordpress::SiteLock::acquire(&live)?);
    let options = staging::SyncOptions { files: body.files, database: body.database };
    let what = match (body.files, body.database) {
        (true, true) => "files+database",
        (true, false) => "files",
        _ => "database",
    };
    let (action, title, from, to) = match direction {
        staging::Direction::Push => ("wordpress.staging.push", format!("Canlıya aktarma: {} → {}", copy.domain, live.domain), copy, live),
        staging::Direction::Refresh => ("wordpress.staging.refresh", format!("Hazırlık ortamını yenileme: {} → {}", live.domain, copy.domain), live, copy),
    };
    auth::audit(&state.db, &user, action, &to.domain, &format!("{} {what}", from.domain));
    let job_state = state.clone();
    let job_id = state.jobs.start("wordpress.staging", &title, Some(user.id), move |ctx| async move {
        let _locks = locks;
        staging::sync(&ctx, &job_state, &from, &to, direction, options).await
    })?;
    Ok(Json(json!({ "job_id": job_id })))
}
