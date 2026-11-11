//! Hesapların SFTP ve kabuk (SSH) erişimi. `/users/{id}/access` altında yayınlanır.
//!
//! Erişim düzeyini hesabı yöneten (yönetici ya da bayisi) belirler; kabuk erişimini
//! yalnızca yönetici verebilir. Hesap kendi açık anahtarlarını ve SFTP parolasını
//! kendisi yönetir.

use axum::extract::{Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthUser, Role};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::sshaccess;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/access", get(show).put(set_mode))
        .route("/{id}/access/password", put(set_password))
        .route("/{id}/access/keys", post(add_key))
        .route("/{id}/access/keys/{key}", delete(remove_key))
}

struct Account {
    id: i64,
    username: String,
    mode: String,
    password_set: bool,
    /// İsteği yapan bu hesabı yönetiyor mu (yoksa hesabın kendisi mi)?
    managed: bool,
}

/// Hesabı okur. Erişebilenler: hesabın kendisi ve onu yöneten (yönetici ya da üst hesabı).
/// Alt hesapların Linux kullanıcısı olmadığından erişimleri de olmaz.
fn load(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Account> {
    let row: Option<(String, String, Option<i64>, String, bool)> = state.db.call(|conn| {
        conn.query_row("SELECT username, role, parent_id, ssh_mode, ssh_password_set FROM users WHERE id = ?1", params![id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .optional()
    })?;
    let (username, role, parent_id, mode, password_set) = row.ok_or(AppError::NotFound)?;
    let own = id == user.id && user.role != Role::Member;
    let managed = user.is_admin() || parent_id == Some(user.id);
    if !(own || managed) || role == "member" {
        // Başkasının hesabının varlığı da sızdırılmaz.
        return Err(AppError::NotFound);
    }
    Ok(Account { id, username, mode, password_set, managed: managed && id != user.id || user.is_admin() })
}

#[derive(Serialize)]
struct KeyView {
    id: i64,
    name: String,
    fingerprint: String,
    created_at: i64,
}

#[derive(Serialize)]
struct View {
    /// Sunucuda sshd kurulu mu?
    available: bool,
    /// `none`, `sftp` ya da `shell`.
    mode: String,
    username: String,
    port: u16,
    password_set: bool,
    keys: Vec<KeyView>,
    /// İsteği yapan erişim düzeyini değiştirebilir mi?
    can_change_mode: bool,
    /// İsteği yapan kabuk erişimi verebilir mi?
    can_grant_shell: bool,
}

fn keys(state: &AppState, id: i64) -> AppResult<Vec<KeyView>> {
    state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, name, fingerprint, created_at FROM ssh_keys WHERE user_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![id], |row| Ok(KeyView { id: row.get(0)?, name: row.get(1)?, fingerprint: row.get(2)?, created_at: row.get(3)? }))?;
        rows.collect()
    })
}

async fn view(state: &AppState, user: &AuthUser, account: &Account) -> AppResult<View> {
    Ok(View {
        available: sshaccess::is_available(),
        mode: account.mode.clone(),
        username: account.username.clone(),
        port: sshaccess::port().await,
        password_set: account.password_set,
        keys: keys(state, account.id)?,
        can_change_mode: account.managed,
        can_grant_shell: user.is_admin(),
    })
}

async fn show(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<View>> {
    let account = load(&state, &user, id)?;
    Ok(Json(view(&state, &user, &account).await?))
}

#[derive(Deserialize)]
struct ModeBody {
    mode: String,
}

async fn set_mode(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<ModeBody>) -> AppResult<Json<View>> {
    let mut account = load(&state, &user, id)?;
    if !account.managed {
        return Err(AppError::Forbidden);
    }
    if !sshaccess::MODES.contains(&body.mode.as_str()) {
        return Err(AppError::bad("Geçersiz erişim düzeyi"));
    }
    // Kabuk, sunucudaki herkesin okuyabildiği dosyalara erişim demektir; yalnızca yönetici verebilir.
    if body.mode == "shell" && !user.is_admin() {
        return Err(AppError::Forbidden);
    }
    let _guard = state.sys_lock.lock().await;
    sshaccess::apply_mode(&account.username, &body.mode).await?;
    // Kabuk erişiminde parolayla giriş yoktur; erişim kapanınca da parola kilitlenir.
    let keep_password = body.mode == "sftp" && account.password_set;
    state.db.call(|conn| conn.execute("UPDATE users SET ssh_mode = ?1, ssh_password_set = ?2 WHERE id = ?3", params![body.mode, keep_password, id]))?;
    auth::audit(&state.db, &user, "user.access", &account.username, &body.mode);
    account.mode = body.mode;
    account.password_set = keep_password;
    Ok(Json(view(&state, &user, &account).await?))
}

#[derive(Deserialize)]
struct PasswordBody {
    password: String,
}

async fn set_password(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<PasswordBody>) -> AppResult<Json<View>> {
    if user.via_token {
        return Err(AppError::Forbidden);
    }
    let mut account = load(&state, &user, id)?;
    if account.mode != "sftp" {
        return Err(AppError::bad(if account.mode == "shell" { "Kabuk erişiminde yalnızca açık anahtarla girilir" } else { "Bu hesap için SFTP erişimi açık değil" }));
    }
    sshaccess::validate_password(&body.password)?;
    let _guard = state.sys_lock.lock().await;
    sshaccess::set_password(&account.username, &body.password).await?;
    state.db.call(|conn| conn.execute("UPDATE users SET ssh_password_set = 1 WHERE id = ?1", params![id]))?;
    auth::audit(&state.db, &user, "user.access.password", &account.username, "");
    account.password_set = true;
    Ok(Json(view(&state, &user, &account).await?))
}

#[derive(Deserialize)]
struct KeyBody {
    #[serde(default)]
    name: String,
    public_key: String,
}

/// Veritabanındaki anahtarları diske yazar (erişim açıksa sshd hemen görür).
async fn sync_keys(state: &AppState, account: &Account) -> AppResult<()> {
    let lines: Vec<String> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT key_line FROM ssh_keys WHERE user_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![account.id], |row| row.get(0))?;
        rows.collect()
    })?;
    sshaccess::write_keys(&account.username, &lines).await
}

async fn add_key(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<KeyBody>) -> AppResult<Json<View>> {
    if user.via_token {
        return Err(AppError::Forbidden);
    }
    let account = load(&state, &user, id)?;
    let key = sshaccess::parse_key(&body.public_key)?;
    let name: String = body.name.trim().chars().filter(|c| !c.is_control()).take(60).collect();
    let name = if name.is_empty() { if key.comment.is_empty() { "anahtar".to_string() } else { key.comment.clone() } } else { name };

    let _guard = state.sys_lock.lock().await;
    let inserted = state.db.call(|conn| {
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM ssh_keys WHERE user_id = ?1", params![id], |row| row.get(0))?;
        if count as usize >= sshaccess::MAX_KEYS {
            return Ok(Err("Bir hesaba en çok 20 anahtar eklenebilir"));
        }
        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM ssh_keys WHERE user_id = ?1 AND fingerprint = ?2)", params![id, key.fingerprint], |row| row.get(0))?;
        if exists {
            return Ok(Err("Bu anahtar zaten ekli"));
        }
        conn.execute(
            "INSERT INTO ssh_keys (user_id, name, key_line, fingerprint, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, key.line, key.fingerprint, now()],
        )?;
        Ok(Ok(()))
    })?;
    inserted.map_err(AppError::conflict)?;
    sync_keys(&state, &account).await?;
    auth::audit(&state.db, &user, "user.access.key.add", &account.username, &key.fingerprint);
    Ok(Json(view(&state, &user, &account).await?))
}

async fn remove_key(State(state): State<AppState>, user: AuthUser, Path((id, key_id)): Path<(i64, i64)>) -> AppResult<Json<View>> {
    let account = load(&state, &user, id)?;
    let _guard = state.sys_lock.lock().await;
    let removed = state.db.call(|conn| conn.execute("DELETE FROM ssh_keys WHERE id = ?1 AND user_id = ?2", params![key_id, id]))?;
    if removed == 0 {
        return Err(AppError::NotFound);
    }
    sync_keys(&state, &account).await?;
    auth::audit(&state.db, &user, "user.access.key.remove", &account.username, "");
    Ok(Json(view(&state, &user, &account).await?))
}
