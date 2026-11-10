//! Panel kullanıcıları: yöneticiler, bayiler, müşteriler ve alt hesaplar.
//!
//! Hiyerarşi: yönetici herkesi yönetir; bayi kendi müşterilerini; müşteri ise
//! kendi alt hesaplarını (ör. web yöneticisi) açar ve yetkilerini belirler.

use axum::extract::{Path, State};
use axum::routing::{get, patch};
use axum::{Json, Router};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser, Role, PERMISSION_AREAS};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", patch(update).delete(remove))
        .route("/{id}/2fa/reset", axum::routing::post(reset_two_factor))
}

#[derive(Serialize)]
struct UserInfo {
    id: i64,
    username: String,
    email: String,
    role: String,
    parent_id: Option<i64>,
    parent_username: Option<String>,
    permissions: Vec<String>,
    status: String,
    language: String,
    created_at: i64,
    last_login_at: Option<i64>,
    /// İki adımlı doğrulama açık mı?
    two_factor: bool,
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<UserInfo>>> {
    if user.role == Role::Member {
        return Err(AppError::Forbidden);
    }
    // Yönetici herkesi, diğerleri yalnızca kendi açtığı hesapları görür.
    let scope = if user.is_admin() { None } else { Some(user.id) };
    let users = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT u.id, u.username, u.email, u.role, u.parent_id, p.username, u.permissions,
                    u.status, u.language, u.created_at, u.last_login_at, u.totp_enabled
             FROM users u LEFT JOIN users p ON p.id = u.parent_id
             WHERE ?1 IS NULL OR u.parent_id = ?1
             ORDER BY u.username",
        )?;
        let rows = stmt.query_map(params![scope], |row| {
            let permissions: String = row.get(6)?;
            Ok(UserInfo {
                id: row.get(0)?,
                username: row.get(1)?,
                email: row.get(2)?,
                role: row.get(3)?,
                parent_id: row.get(4)?,
                parent_username: row.get(5)?,
                permissions: serde_json::from_str(&permissions).unwrap_or_default(),
                status: row.get(7)?,
                language: row.get(8)?,
                created_at: row.get(9)?,
                last_login_at: row.get(10)?,
                two_factor: row.get(11)?,
            })
        })?;
        rows.collect()
    })?;
    Ok(Json(users))
}

/// Kullanıcı adı ileride Linux kullanıcısına da karşılık geldiğinden kısıtlıdır.
pub fn validate_username(username: &str) -> AppResult<()> {
    let mut chars = username.chars();
    let valid = (3..=32).contains(&username.len())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(AppError::bad(
            "Kullanıcı adı 3-32 karakter olmalı, küçük harfle başlamalı; yalnızca küçük harf, rakam, - ve _ içerebilir",
        ))
    }
}

fn validate_permissions(permissions: &[String]) -> AppResult<()> {
    match permissions.iter().find(|p| !PERMISSION_AREAS.contains(&p.as_str())) {
        Some(unknown) => Err(AppError::bad(format!("Bilinmeyen yetki alanı: {unknown}"))),
        None => Ok(()),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    username: String,
    #[serde(default)]
    email: String,
    password: String,
    role: String,
    #[serde(default)]
    permissions: Vec<String>,
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateBody>,
) -> AppResult<Json<serde_json::Value>> {
    let role = Role::parse(&body.role).ok_or_else(|| AppError::bad("Geçersiz rol"))?;
    // Kim hangi rolü açabilir ve yeni hesabın üst hesabı kim olur?
    let parent_id = match (user.role, role) {
        (Role::Admin, Role::Admin | Role::Reseller | Role::User) => None,
        (Role::Reseller, Role::User) => Some(user.id),
        (Role::User, Role::Member) => Some(user.id),
        _ => return Err(AppError::Forbidden),
    };
    let username = body.username.trim().to_lowercase();
    validate_username(&username)?;
    auth::validate_password(&body.password)?;
    validate_permissions(&body.permissions)?;

    let hash = auth::hash_password(&body.password)?;
    let permissions = serde_json::to_string(&body.permissions).unwrap_or_else(|_| "[]".into());
    let id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, role, parent_id, permissions, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![username, body.email.trim(), hash, role.as_str(), parent_id, permissions, now()],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    auth::audit(&state.db, &user, "user.create", &username, role.as_str());
    Ok(Json(json!({ "id": id })))
}

struct Target {
    username: String,
    role: String,
    parent_id: Option<i64>,
}

/// Hedef hesabı okur ve isteği yapanın onu yönetme yetkisini denetler.
fn load_manageable(state: &AppState, user: &AuthUser, id: i64) -> AppResult<Target> {
    let target = state.db.call(|conn| {
        conn.query_row("SELECT username, role, parent_id FROM users WHERE id = ?1", params![id], |row| {
            Ok(Target { username: row.get(0)?, role: row.get(1)?, parent_id: row.get(2)? })
        })
    })?;
    if user.is_admin() || target.parent_id == Some(user.id) {
        Ok(target)
    } else {
        // Başkasının hesabının varlığı da sızdırılmaz.
        Err(AppError::NotFound)
    }
}

/// Telefonunu kaybeden bir hesabın iki adımlı doğrulamasını kapatır ve oturumlarını sonlandırır.
async fn reset_two_factor(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    if user.via_token {
        return Err(AppError::Forbidden);
    }
    let target = load_manageable(&state, &user, id)?;
    state.db.call(|conn| {
        conn.execute("UPDATE users SET totp_enabled = 0, totp_secret = NULL, recovery_codes = '[]' WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![id])
    })?;
    auth::audit(&state.db, &user, "user.2fa.reset", &target.username, "");
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct UpdateBody {
    email: Option<String>,
    password: Option<String>,
    status: Option<String>,
    permissions: Option<Vec<String>>,
    language: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<serde_json::Value>> {
    let target = load_manageable(&state, &user, id)?;

    if let Some(status) = &body.status {
        if !matches!(status.as_str(), "active" | "suspended") {
            return Err(AppError::bad("Geçersiz durum"));
        }
        if id == user.id {
            return Err(AppError::bad("Kendi hesabınızı askıya alamazsınız"));
        }
    }
    if let Some(language) = &body.language {
        if !matches!(language.as_str(), "tr" | "en") {
            return Err(AppError::bad("Desteklenmeyen dil"));
        }
    }
    if let Some(permissions) = &body.permissions {
        validate_permissions(permissions)?;
    }
    let password_hash = match &body.password {
        Some(password) => {
            auth::validate_password(password)?;
            Some(auth::hash_password(password)?)
        }
        None => None,
    };
    let permissions = body.permissions.as_ref().map(|p| serde_json::to_string(p).unwrap_or_else(|_| "[]".into()));
    let suspended = body.status.as_deref() == Some("suspended");

    state.db.call(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE users SET
                email = COALESCE(?1, email),
                password_hash = COALESCE(?2, password_hash),
                status = COALESCE(?3, status),
                permissions = COALESCE(?4, permissions),
                language = COALESCE(?5, language)
             WHERE id = ?6",
            params![body.email.as_deref().map(str::trim), password_hash, body.status, permissions, body.language, id],
        )?;
        // Parolası değişen ya da askıya alınan hesabın açık oturumları ve
        // (askıda ise) alt hesaplarının oturumları kapatılır.
        if password_hash.is_some() || suspended {
            tx.execute("DELETE FROM sessions WHERE user_id = ?1", params![id])?;
        }
        if suspended {
            tx.execute(
                "DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE parent_id = ?1)",
                params![id],
            )?;
        }
        tx.commit()
    })?;
    auth::audit(&state.db, &user, "user.update", &target.username, "");
    Ok(Json(json!({ "ok": true })))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    if id == user.id {
        return Err(AppError::bad("Kendi hesabınızı silemezsiniz"));
    }
    let target = load_manageable(&state, &user, id)?;
    if target.role == "admin" {
        let admins: i64 = state
            .db
            .call(|conn| conn.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin'", [], |r| r.get(0)))?;
        if admins <= 1 {
            return Err(AppError::bad("Son yönetici hesabı silinemez"));
        }
    }
    state.db.call(|conn| conn.execute("DELETE FROM users WHERE id = ?1", params![id]))?;
    auth::audit(&state.db, &user, "user.delete", &target.username, "");
    Ok(Json(json!({ "ok": true })))
}
