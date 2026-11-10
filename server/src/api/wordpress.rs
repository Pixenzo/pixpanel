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
use crate::wordpress::{self, InstallRequest, Settings, Status};

const AREA: &str = "sites";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/wordpress", get(show))
        .route("/{id}/wordpress/install", post(install))
        .route("/{id}/wordpress/update", post(update))
        .route("/{id}/wordpress/login", post(login))
        .route("/{id}/wordpress/settings", put(save_settings))
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
    wordpress::check_installable(&site).await?;

    auth::audit(&state.db, &user, "wordpress.install", &site.domain, "");
    let job_state = state.clone();
    let title = format!("WordPress kurulumu: {}", site.domain);
    let job_id = state.jobs.start("wordpress.install", &title, Some(user.id), move |ctx| async move {
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
