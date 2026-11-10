//! fail2ban uçları. Yalnızca yöneticiler.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::fail2ban::{self, JailStatus, Settings};
use crate::state::AppState;
use crate::sys::services;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(show).put(save)).route("/unban", post(unban)).route("/ban", post(ban))
}

#[derive(Serialize)]
struct View {
    installed: bool,
    running: bool,
    settings: Settings,
    jails: Vec<JailStatus>,
    your_ip: String,
}

async fn view(state: &AppState, user: &AuthUser) -> View {
    let installed = fail2ban::is_installed();
    View {
        installed,
        running: installed && services::is_active(fail2ban::SERVICE).await,
        settings: fail2ban::load(&state.db),
        jails: fail2ban::status().await,
        your_ip: user.ip.clone(),
    }
}

fn ensure_installed() -> AppResult<()> {
    if fail2ban::is_installed() { Ok(()) } else { Err(AppError::bad("fail2ban kurulu değil; önce Bileşenler sayfasından kurun")) }
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(view(&state, &user).await))
}

#[derive(Deserialize)]
struct SaveBody {
    settings: Settings,
}

async fn save(State(state): State<AppState>, user: AuthUser, Json(body): Json<SaveBody>) -> AppResult<Json<View>> {
    user.require_admin()?;
    ensure_installed()?;
    let settings = body.settings.normalized()?;
    let _guard = state.sys_lock.lock().await;
    fail2ban::apply(&state.db, &settings).await?;
    auth::audit(&state.db, &user, "fail2ban.settings", "", "");
    Ok(Json(view(&state, &user).await))
}

#[derive(Deserialize)]
struct AddressBody {
    ip: String,
    /// Verilmezse engel bütün hapislerden kaldırılır.
    jail: Option<String>,
}

async fn unban(State(state): State<AppState>, user: AuthUser, Json(body): Json<AddressBody>) -> AppResult<Json<View>> {
    user.require_admin()?;
    ensure_installed()?;
    fail2ban::unban(&body.ip, body.jail.as_deref()).await?;
    auth::audit(&state.db, &user, "fail2ban.unban", body.ip.trim(), body.jail.as_deref().unwrap_or(""));
    Ok(Json(view(&state, &user).await))
}

async fn ban(State(state): State<AppState>, user: AuthUser, Json(body): Json<AddressBody>) -> AppResult<Json<View>> {
    user.require_admin()?;
    ensure_installed()?;
    let ip = fail2ban::parse_ip(&body.ip)?;
    // Yönetici kendi adresini elle engelleyip kendini dışarıda bırakmasın.
    if ip == user.ip {
        return Err(AppError::bad("Kendi adresinizi engelleyemezsiniz"));
    }
    let jail = body.jail.as_deref().ok_or_else(|| AppError::bad("Hapis seçin"))?;
    fail2ban::ban(&ip, jail).await?;
    auth::audit(&state.db, &user, "fail2ban.ban", &ip, jail);
    Ok(Json(view(&state, &user).await))
}
