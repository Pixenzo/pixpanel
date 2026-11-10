//! Site başına FTP hesapları. `/sites/{id}/ftp` altında yayınlanır.

use axum::extract::{Path, State};
use axum::routing::{get, patch};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthUser};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::sites::{self, Site};
use crate::hosting::{ftp, sysuser};
use crate::state::AppState;
use crate::sys::services;

const AREA: &str = "ftp";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/ftp", get(list).post(create))
        .route("/{id}/ftp/{account}", patch(update).delete(remove))
}

#[derive(Serialize)]
struct Account {
    id: i64,
    name: String,
    /// Site klasörü içindeki alt klasör; boşsa sitenin kökü.
    subdir: String,
    created_at: i64,
}

#[derive(Serialize)]
struct View {
    installed: bool,
    running: bool,
    accounts: Vec<Account>,
    passive_ports: (u16, u16),
}

fn accounts_of(state: &AppState, site_id: i64) -> AppResult<Vec<Account>> {
    state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, name, subdir, created_at FROM ftp_accounts WHERE site_id = ?1 ORDER BY name")?;
        stmt.query_map(params![site_id], |row| Ok(Account { id: row.get(0)?, name: row.get(1)?, subdir: row.get(2)?, created_at: row.get(3)? }))?.collect()
    })
}

async fn view(state: &AppState, site: &Site) -> AppResult<View> {
    let installed = ftp::is_installed();
    Ok(View {
        installed,
        running: installed && services::is_active(ftp::SERVICE).await,
        accounts: accounts_of(state, site.id)?,
        passive_ports: ftp::PASSIVE_PORTS,
    })
}

/// FTP hesabı açılabilecek siteyi okur: vekil sitelerin yüklenecek dosyası yoktur.
fn ftp_site(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Site> {
    let site = sites::get(&state.db, user, id)?;
    if site.kind == "proxy" { Err(AppError::bad("Ters vekil sitelerinde FTP hesabı açılamaz")) } else { Ok(site) }
}

fn ensure_installed() -> AppResult<()> {
    if ftp::is_installed() { Ok(()) } else { Err(AppError::bad("FTP sunucusu kurulu değil; yöneticinin Bileşenler sayfasından kurması gerekiyor")) }
}

async fn list(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<View>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    Ok(Json(view(&state, &site).await?))
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    password: String,
    #[serde(default)]
    subdir: String,
}

async fn create(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<CreateBody>) -> AppResult<Json<View>> {
    user.require(AREA)?;
    ensure_installed()?;
    let site = ftp_site(&state, &user, id)?;
    let name = ftp::validate_name(&body.name)?;
    ftp::validate_password(&body.password)?;
    let (subdir, directory) = ftp::resolve_directory(&site.root(), &body.subdir)?;
    let owner = sysuser::lookup(&site.owner).ok_or_else(|| AppError::bad("Hesabın Linux kullanıcısı bulunamadı"))?;

    let _guard = state.sys_lock.lock().await;
    let inserted = state.db.call(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO ftp_accounts (site_id, name, subdir, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![site.id, name, subdir, now()],
        )?;
        Ok(conn.changes() > 0)
    })?;
    if !inserted {
        return Err(AppError::conflict("Bu adda bir FTP hesabı zaten var"));
    }
    if let Err(err) = ftp::create_account(&name, &body.password, &owner, &directory).await {
        state.db.call(|conn| conn.execute("DELETE FROM ftp_accounts WHERE name = ?1", params![name]))?;
        return Err(err);
    }
    auth::audit(&state.db, &user, "ftp.create", &name, &site.domain);
    Ok(Json(view(&state, &site).await?))
}

/// Hesabın bu siteye ait olduğunu doğrulayıp adını ve klasörünü döner.
fn load_account(state: &AppState, site_id: i64, account_id: i64) -> AppResult<(String, String)> {
    state
        .db
        .call(|conn| {
            conn.query_row("SELECT name, subdir FROM ftp_accounts WHERE id = ?1 AND site_id = ?2", params![account_id, site_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
        })?
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct UpdateBody {
    password: Option<String>,
    subdir: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, account_id)): Path<(i64, i64)>,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<View>> {
    user.require(AREA)?;
    ensure_installed()?;
    let site = ftp_site(&state, &user, id)?;
    let (name, _) = load_account(&state, site.id, account_id)?;
    let _guard = state.sys_lock.lock().await;
    if let Some(password) = &body.password {
        ftp::validate_password(password)?;
        ftp::set_password(&name, password).await?;
    }
    if let Some(subdir) = &body.subdir {
        let (subdir, directory) = ftp::resolve_directory(&site.root(), subdir)?;
        ftp::set_directory(&name, &directory).await?;
        state.db.call(|conn| conn.execute("UPDATE ftp_accounts SET subdir = ?1 WHERE id = ?2", params![subdir, account_id]))?;
    }
    auth::audit(&state.db, &user, "ftp.update", &name, &site.domain);
    Ok(Json(view(&state, &site).await?))
}

async fn remove(State(state): State<AppState>, user: AuthUser, Path((id, account_id)): Path<(i64, i64)>) -> AppResult<Json<View>> {
    user.require(AREA)?;
    let site = sites::get(&state.db, &user, id)?;
    let (name, _) = load_account(&state, site.id, account_id)?;
    let _guard = state.sys_lock.lock().await;
    if ftp::is_installed() {
        ftp::remove_account(&name).await?;
    }
    state.db.call(|conn| conn.execute("DELETE FROM ftp_accounts WHERE id = ?1", params![account_id]))?;
    auth::audit(&state.db, &user, "ftp.delete", &name, &site.domain);
    Ok(Json(view(&state, &site).await?))
}

/// Site silinirken o siteye bağlı FTP hesaplarını da sunucudan kaldırır.
pub async fn remove_for_site(state: &AppState, site_id: i64) {
    let Ok(accounts) = accounts_of(state, site_id) else { return };
    if !ftp::is_installed() {
        return;
    }
    for account in accounts {
        if let Err(err) = ftp::remove_account(&account.name).await {
            tracing::warn!(error = %err, account = %account.name, "FTP hesabı kaldırılamadı");
        }
    }
}
