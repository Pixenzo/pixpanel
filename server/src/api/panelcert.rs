//! Panelin kendi sertifikasının yönetimi. Yalnızca yöneticiler.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::hosting::ssl::{self, CertInfo};
use crate::hosting::nginx;
use crate::panelcert;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(show))
        .route("/letsencrypt", post(letsencrypt))
        .route("/custom", post(custom))
        .route("/selfsigned", post(self_signed))
}

#[derive(Serialize)]
struct View {
    hostname: String,
    /// `selfsigned`, `letsencrypt`, `letsencrypt_dns` ya da `custom`.
    mode: String,
    certificate: Option<CertInfo>,
    /// Web üzerinden doğrulama için nginx gerekir.
    nginx_installed: bool,
    /// Panelin alan adının DNS'i panelde yönetiliyor mu? (DNS ile doğrulama için.)
    dns_managed: bool,
}

fn view(state: &AppState, user: &AuthUser) -> AppResult<View> {
    let hostname = panelcert::hostname(&state.db);
    Ok(View {
        dns_managed: !hostname.is_empty() && crate::dns::zone_for(&state.db, user.owner_id(), &hostname)?.is_some(),
        mode: panelcert::mode(&state.db),
        certificate: panelcert::info(&state.cfg),
        nginx_installed: nginx::is_installed(),
        hostname,
    })
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(view(&state, &user)?))
}

#[derive(Deserialize)]
struct LetsEncryptBody {
    hostname: String,
    /// Doğrulama DNS kaydıyla yapılsın mı?
    #[serde(default)]
    dns: bool,
}

async fn letsencrypt(State(state): State<AppState>, user: AuthUser, Json(body): Json<LetsEncryptBody>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let hostname = panelcert::validate_hostname(&body.hostname)?;
    if body.dns {
        if crate::dns::zone_for(&state.db, user.owner_id(), &hostname)?.is_none() {
            return Err(AppError::bad("DNS ile doğrulama için alan adının DNS'i panelde yönetiliyor olmalı"));
        }
    } else if !nginx::is_installed() {
        return Err(AppError::bad("Web üzerinden doğrulama için nginx kurulu olmalı; DNS ile doğrulamayı deneyin"));
    }
    let job_id = panelcert::start_issue(&state, Some(user.id), user.owner_id(), hostname.clone(), body.dns)?;
    auth::audit(&state.db, &user, "panel.certificate.letsencrypt", &hostname, "");
    Ok(Json(json!({ "job_id": job_id })))
}

#[derive(Deserialize)]
struct CustomBody {
    hostname: String,
    certificate: String,
    private_key: String,
}

async fn custom(State(state): State<AppState>, user: AuthUser, Json(body): Json<CustomBody>) -> AppResult<Json<View>> {
    user.require_admin()?;
    let hostname = panelcert::validate_hostname(&body.hostname)?;
    // Sertifikanın alan adını kapsadığı ve anahtarla eşleştiği denetlenir.
    ssl::validate_custom(&hostname, &body.certificate, &body.private_key)?;
    let _guard = state.sys_lock.lock().await;
    panelcert::install(&state, &hostname, &body.certificate, &body.private_key, "custom").await?;
    auth::audit(&state.db, &user, "panel.certificate.custom", &hostname, "");
    Ok(Json(view(&state, &user)?))
}

#[derive(Deserialize)]
struct SelfSignedBody {
    #[serde(default)]
    hostname: String,
}

async fn self_signed(State(state): State<AppState>, user: AuthUser, Json(body): Json<SelfSignedBody>) -> AppResult<Json<View>> {
    user.require_admin()?;
    let hostname = if body.hostname.trim().is_empty() { crate::mail::system_hostname() } else { panelcert::validate_hostname(&body.hostname)? };
    let (certificate, private_key) = ssl::generate_self_signed(std::slice::from_ref(&hostname))?;
    let _guard = state.sys_lock.lock().await;
    panelcert::install(&state, &hostname, &certificate, &private_key, "selfsigned").await?;
    auth::audit(&state.db, &user, "panel.certificate.selfsigned", &hostname, "");
    Ok(Json(view(&state, &user)?))
}
