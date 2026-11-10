//! Oturum açma/kapama, parola değişimi, oturumlar ve API anahtarları.

use axum::extract::{ConnectInfo, Path, State};
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;

use crate::auth::{self, AuthUser, API_TOKEN_PREFIX};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/password", post(change_password))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", delete(revoke_session))
        .route("/tokens", get(list_tokens).post(create_token))
        .route("/tokens/{id}", delete(delete_token))
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

/// Var olmayan kullanıcıda da aynı süreyi harcamak için kullanılan sahte özet;
/// böylece yanıt süresinden kullanıcı adının varlığı anlaşılamaz.
static DUMMY_HASH: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    auth::hash_password(&auth::random_token(16)).unwrap_or_default()
});

async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> AppResult<Response> {
    let ip = addr.ip().to_canonical().to_string();
    state.throttle.check(&ip)?;

    let username = body.username.trim().to_string();
    let found = state.db.call(|conn| {
        conn.query_row(
            "SELECT id, password_hash, status FROM users WHERE username = ?1",
            params![username],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        )
        .optional()
    })?;

    let password = body.password;
    let hash = found.as_ref().map(|(_, hash, _)| hash.clone()).unwrap_or_else(|| DUMMY_HASH.clone());
    // Argon2 bilerek yavaştır; eşzamansız çalışanları tıkamamak için ayrı iş parçacığında koşar.
    let valid = tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
        .await
        .map_err(|err| AppError::internal(err.to_string()))?;

    let user_id = match found {
        Some((id, _, status)) if valid && status == "active" => id,
        _ => {
            state.throttle.record_failure(&ip);
            tracing::warn!(%ip, %username, "başarısız giriş denemesi");
            return Err(AppError::bad("Kullanıcı adı ya da parola hatalı"));
        }
    };
    state.throttle.clear(&ip);

    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let ttl = state.cfg.session_ttl_secs;
    let token = auth::create_session(&state.db, user_id, &ip, user_agent, ttl)?;
    state.db.call(|conn| {
        conn.execute(
            "INSERT INTO audit_log (user_id, username, action, ip, created_at)
             VALUES (?1, ?2, 'auth.login', ?3, ?4)",
            params![user_id, username, ip, now()],
        )
    })?;

    let cookie = auth::session_cookie(&token, ttl, state.cfg.tls);
    Ok(([(SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

async fn logout(State(state): State<AppState>, user: AuthUser) -> AppResult<Response> {
    if let Some(session_id) = user.session_id {
        state.db.call(|conn| conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id]))?;
    }
    let cookie = auth::session_cookie("", 0, state.cfg.tls);
    Ok(([(SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

async fn me(user: AuthUser) -> Json<AuthUser> {
    Json(user)
}

#[derive(Deserialize)]
struct PasswordBody {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<PasswordBody>,
) -> AppResult<Json<serde_json::Value>> {
    auth::validate_password(&body.new_password)?;
    let hash: String = state.db.call(|conn| {
        conn.query_row("SELECT password_hash FROM users WHERE id = ?1", params![user.id], |r| r.get(0))
    })?;
    if !auth::verify_password(&body.current_password, &hash) {
        return Err(AppError::bad("Mevcut parola hatalı"));
    }
    let new_hash = auth::hash_password(&body.new_password)?;
    // Parola değişince diğer bütün oturumlar kapatılır.
    let keep = user.session_id.unwrap_or(-1);
    state.db.call(|conn| {
        conn.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![new_hash, user.id])?;
        conn.execute("DELETE FROM sessions WHERE user_id = ?1 AND id != ?2", params![user.id, keep])
    })?;
    auth::audit(&state.db, &user, "auth.password_change", &user.username, "");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
struct SessionInfo {
    id: i64,
    ip: String,
    user_agent: String,
    created_at: i64,
    expires_at: i64,
    current: bool,
}

async fn list_sessions(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<SessionInfo>>> {
    let current = user.session_id;
    let sessions = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ip, user_agent, created_at, expires_at FROM sessions
             WHERE user_id = ?1 AND expires_at > ?2 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![user.id, now()], |row| {
            let id: i64 = row.get(0)?;
            Ok(SessionInfo {
                id,
                ip: row.get(1)?,
                user_agent: row.get(2)?,
                created_at: row.get(3)?,
                expires_at: row.get(4)?,
                current: Some(id) == current,
            })
        })?;
        rows.collect()
    })?;
    Ok(Json(sessions))
}

async fn revoke_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let deleted = state.db.call(|conn| {
        conn.execute("DELETE FROM sessions WHERE id = ?1 AND user_id = ?2", params![id, user.id])
    })?;
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
struct TokenInfo {
    id: i64,
    name: String,
    prefix: String,
    read_only: bool,
    allowed_ips: String,
    last_used_at: Option<i64>,
    expires_at: Option<i64>,
    created_at: i64,
}

async fn list_tokens(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<TokenInfo>>> {
    let tokens = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, prefix, read_only, allowed_ips, last_used_at, expires_at, created_at
             FROM api_tokens WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![user.id], |row| {
            Ok(TokenInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                prefix: row.get(2)?,
                read_only: row.get(3)?,
                allowed_ips: row.get(4)?,
                last_used_at: row.get(5)?,
                expires_at: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect()
    })?;
    Ok(Json(tokens))
}

#[derive(Deserialize)]
struct CreateTokenBody {
    name: String,
    #[serde(default)]
    read_only: bool,
    /// Virgülle ayrılmış IP listesi; boşsa her adresten kullanılabilir.
    #[serde(default)]
    allowed_ips: String,
    /// Kaç gün geçerli olacağı; verilmezse süresizdir.
    expires_in_days: Option<i64>,
}

async fn create_token(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateTokenBody>,
) -> AppResult<Json<serde_json::Value>> {
    // Anahtarlar yalnızca panel oturumundan üretilebilir; sızan bir anahtar yenisini üretememelidir.
    if user.via_token {
        return Err(AppError::Forbidden);
    }
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(AppError::bad("Anahtar adı 1-64 karakter olmalı"));
    }
    for entry in body.allowed_ips.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if entry.parse::<std::net::IpAddr>().is_err() {
            return Err(AppError::bad(format!("Geçersiz IP adresi: {entry}")));
        }
    }
    let token = format!("{API_TOKEN_PREFIX}{}", auth::random_token(24));
    let prefix: String = token.chars().take(12).collect();
    let created = now();
    let expires_at = body.expires_in_days.filter(|days| *days > 0).map(|days| created + days * 86400);
    let id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO api_tokens (user_id, name, token_hash, prefix, read_only, allowed_ips, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                user.id,
                name,
                auth::hash_token(&token),
                prefix,
                body.read_only,
                body.allowed_ips.trim(),
                expires_at,
                created
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    auth::audit(&state.db, &user, "token.create", name, "");
    // Ham anahtar yalnızca bu yanıtta gösterilir; sonradan geri alınamaz.
    Ok(Json(json!({ "id": id, "token": token })))
}

async fn delete_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let deleted = state.db.call(|conn| {
        conn.execute("DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2", params![id, user.id])
    })?;
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    auth::audit(&state.db, &user, "token.delete", &id.to_string(), "");
    Ok(Json(json!({ "ok": true })))
}
