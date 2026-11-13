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
        .route("/impersonate/stop", post(stop_impersonation))
        .route("/me", get(me))
        .route("/password", post(change_password))
        .route("/2fa", get(two_factor_status))
        .route("/2fa/setup", post(two_factor_setup))
        .route("/2fa/enable", post(two_factor_enable))
        .route("/2fa/disable", post(two_factor_disable))
        .route("/2fa/recovery-codes", post(two_factor_recovery_codes))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", delete(revoke_session))
        .route("/tokens", get(list_tokens).post(create_token))
        .route("/tokens/{id}", delete(delete_token))
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    /// İki adımlı doğrulama açıksa: uygulamadaki 6 haneli kod ya da bir kurtarma kodu.
    #[serde(default)]
    code: String,
    /// Tarayıcının dili (`tr` ya da `en`); hesabın dili henüz belirlenmemişse ilk girişte bu kaydedilir.
    #[serde(default)]
    language: String,
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
    // Parola doğru; iki adımlı doğrulama açıksa ikinci adım da istenir.
    let (totp_enabled, secret, last_step, recovery): (bool, Option<String>, i64, String) = state.db.call(|conn| {
        conn.query_row("SELECT totp_enabled, totp_secret, totp_last_step, recovery_codes FROM users WHERE id = ?1", params![user_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
    })?;
    if totp_enabled {
        let code = body.code.trim();
        if code.is_empty() {
            // Parola doğru olduğu için bu bir başarısız deneme sayılmaz.
            return Err(AppError::TwoFactorRequired);
        }
        let mut codes: Vec<String> = serde_json::from_str(&recovery).unwrap_or_default();
        let recovery_hash = auth::hash_token(&code.to_ascii_lowercase());
        if let Some(step) = secret.as_deref().and_then(|secret| crate::totp::verify(secret, code, now(), last_step)) {
            // Aynı kod ikinci kez kullanılamasın diye son adım saklanır.
            state.db.call(|conn| conn.execute("UPDATE users SET totp_last_step = ?1 WHERE id = ?2", params![step, user_id]))?;
        } else if let Some(position) = codes.iter().position(|saved| *saved == recovery_hash) {
            // Kurtarma kodu tek kullanımlıktır.
            codes.remove(position);
            let remaining = serde_json::to_string(&codes).map_err(|err| AppError::internal(err.to_string()))?;
            state.db.call(|conn| conn.execute("UPDATE users SET recovery_codes = ?1 WHERE id = ?2", params![remaining, user_id]))?;
        } else {
            state.throttle.record_failure(&ip);
            tracing::warn!(%ip, %username, "başarısız giriş denemesi");
            return Err(AppError::bad("Doğrulama kodu hatalı ya da süresi dolmuş"));
        }
    }
    state.throttle.clear(&ip);

    // Yeni açılan hesabın dili boştur; ilk girişte tarayıcının diline göre belirlenir ve
    // sonrasında yalnızca kullanıcı Hesabım sayfasından değiştirir.
    let detected = if matches!(body.language.as_str(), "tr" | "en") { body.language.as_str() } else { crate::i18n::request_language(&headers) };
    state.db.call(|conn| conn.execute("UPDATE users SET language = ?1 WHERE id = ?2 AND language = ''", params![detected, user_id]))?;

    // Daha önce giriş yapılmamış bir adresse hesap sahibine haber verilir.
    let (known_ip, first_login): (bool, bool) = state.db.call(|conn| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM audit_log WHERE user_id = ?1 AND action = 'auth.login' AND ip = ?2),
                    NOT EXISTS(SELECT 1 FROM audit_log WHERE user_id = ?1 AND action = 'auth.login')",
            params![user_id, ip],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    })?;
    if !known_ip && !first_login {
        let (notify_state, notify_ip, notify_name) = (state.clone(), ip.clone(), username.clone());
        tokio::spawn(async move {
            let body = format!("Hesap: {notify_name}\nAdres: {notify_ip}\n\nBu giriş size ait değilse parolanızı değiştirin ve açık oturumları kapatın.");
            crate::notify::send_event(&notify_state, user_id, "login_new_ip", "PixPanel hesabınıza yeni bir adresten giriş yapıldı", &body).await;
        });
    }

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

/// Başka hesap adına açılmış oturumu kapatır ve oturumu açan hesaba geri döner.
async fn stop_impersonation(State(state): State<AppState>, user: AuthUser, headers: HeaderMap) -> AppResult<Response> {
    let (Some(session_id), Some(impersonator_id)) = (user.session_id, user.impersonator_id) else {
        return Err(AppError::bad("Bu oturum başka hesap adına açılmamış"));
    };
    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let ttl = state.cfg.session_ttl_secs;
    state.db.call(|conn| conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id]))?;
    let token = auth::create_session(&state.db, impersonator_id, &user.ip, user_agent, ttl)?;
    auth::audit(&state.db, &user, "user.impersonate.stop", &user.username, "");
    let cookie = auth::session_cookie(&token, ttl, state.cfg.tls);
    Ok(([(SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

async fn me(user: AuthUser) -> Json<AuthUser> {
    Json(user)
}

// ---- İki adımlı doğrulama ----

/// İki adımlı doğrulama ayarları yalnızca tarayıcı oturumundan değiştirilebilir;
/// ele geçirilmiş bir API anahtarı hesabın korumasını kapatamaz.
fn require_session(user: &AuthUser) -> AppResult<()> {
    if user.via_token { Err(AppError::Forbidden) } else { Ok(()) }
}

fn check_password(state: &AppState, user: &AuthUser, password: &str) -> AppResult<()> {
    let hash: String = state.db.call(|conn| conn.query_row("SELECT password_hash FROM users WHERE id = ?1", params![user.id], |row| row.get(0)))?;
    if auth::verify_password(password, &hash) { Ok(()) } else { Err(AppError::bad("Parola hatalı")) }
}

async fn two_factor_status(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<serde_json::Value>> {
    let (enabled, recovery): (bool, String) = state.db.call(|conn| {
        conn.query_row("SELECT totp_enabled, recovery_codes FROM users WHERE id = ?1", params![user.id], |row| Ok((row.get(0)?, row.get(1)?)))
    })?;
    let codes: Vec<String> = serde_json::from_str(&recovery).unwrap_or_default();
    Ok(Json(json!({ "enabled": enabled, "recovery_codes_left": codes.len() })))
}

#[derive(Deserialize)]
struct PasswordOnly {
    password: String,
}

/// Yeni bir gizli anahtar üretir. Kullanıcı uygulamasından bir kod girip
/// `enable` ile doğrulayana kadar iki adımlı doğrulama açılmaz.
async fn two_factor_setup(State(state): State<AppState>, user: AuthUser, Json(body): Json<PasswordOnly>) -> AppResult<Json<serde_json::Value>> {
    require_session(&user)?;
    check_password(&state, &user, &body.password)?;
    let enabled: bool = state.db.call(|conn| conn.query_row("SELECT totp_enabled FROM users WHERE id = ?1", params![user.id], |row| row.get(0)))?;
    if enabled {
        return Err(AppError::conflict("İki adımlı doğrulama zaten açık"));
    }
    let secret = crate::totp::generate_secret();
    state.db.call(|conn| conn.execute("UPDATE users SET totp_secret = ?1, totp_last_step = 0 WHERE id = ?2", params![secret, user.id]))?;
    let uri = crate::totp::provisioning_uri(&secret, &user.username, "PixPanel");
    let qr = qrcode::QrCode::new(uri.as_bytes())
        .map(|code| code.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build())
        .map_err(|err| AppError::internal(err.to_string()))?;
    Ok(Json(json!({ "secret": secret, "uri": uri, "qr_svg": qr })))
}

fn new_recovery_codes() -> (Vec<String>, String) {
    let codes: Vec<String> = (0..10).map(|_| crate::totp::generate_recovery_code()).collect();
    let hashes: Vec<String> = codes.iter().map(|code| auth::hash_token(code)).collect();
    (codes, serde_json::to_string(&hashes).unwrap_or_else(|_| "[]".into()))
}

#[derive(Deserialize)]
struct CodeBody {
    code: String,
}

async fn two_factor_enable(State(state): State<AppState>, user: AuthUser, Json(body): Json<CodeBody>) -> AppResult<Json<serde_json::Value>> {
    require_session(&user)?;
    let (enabled, secret): (bool, Option<String>) = state.db.call(|conn| {
        conn.query_row("SELECT totp_enabled, totp_secret FROM users WHERE id = ?1", params![user.id], |row| Ok((row.get(0)?, row.get(1)?)))
    })?;
    if enabled {
        return Err(AppError::conflict("İki adımlı doğrulama zaten açık"));
    }
    let secret = secret.ok_or_else(|| AppError::bad("Önce kurulumu başlatın"))?;
    let step = crate::totp::verify(&secret, &body.code, now(), 0).ok_or_else(|| AppError::bad("Kod hatalı. Uygulamadaki güncel kodu girin; telefonunuzun saatinin doğru olduğundan emin olun."))?;
    let (codes, hashes) = new_recovery_codes();
    // Açılınca diğer oturumlar kapatılır; onlar ikinci adımı geçmemiştir.
    let keep = user.session_id.unwrap_or(-1);
    state.db.call(|conn| {
        conn.execute("UPDATE users SET totp_enabled = 1, totp_last_step = ?1, recovery_codes = ?2 WHERE id = ?3", params![step, hashes, user.id])?;
        conn.execute("DELETE FROM sessions WHERE user_id = ?1 AND id != ?2", params![user.id, keep])
    })?;
    auth::audit(&state.db, &user, "auth.2fa.enable", &user.username, "");
    Ok(Json(json!({ "recovery_codes": codes })))
}

async fn two_factor_disable(State(state): State<AppState>, user: AuthUser, Json(body): Json<PasswordOnly>) -> AppResult<Json<serde_json::Value>> {
    require_session(&user)?;
    check_password(&state, &user, &body.password)?;
    state.db.call(|conn| conn.execute("UPDATE users SET totp_enabled = 0, totp_secret = NULL, recovery_codes = '[]' WHERE id = ?1", params![user.id]))?;
    auth::audit(&state.db, &user, "auth.2fa.disable", &user.username, "");
    Ok(Json(json!({ "ok": true })))
}

async fn two_factor_recovery_codes(State(state): State<AppState>, user: AuthUser, Json(body): Json<PasswordOnly>) -> AppResult<Json<serde_json::Value>> {
    require_session(&user)?;
    check_password(&state, &user, &body.password)?;
    let enabled: bool = state.db.call(|conn| conn.query_row("SELECT totp_enabled FROM users WHERE id = ?1", params![user.id], |row| row.get(0)))?;
    if !enabled {
        return Err(AppError::bad("İki adımlı doğrulama açık değil"));
    }
    let (codes, hashes) = new_recovery_codes();
    state.db.call(|conn| conn.execute("UPDATE users SET recovery_codes = ?1 WHERE id = ?2", params![hashes, user.id]))?;
    auth::audit(&state.db, &user, "auth.2fa.recovery_codes", &user.username, "");
    Ok(Json(json!({ "recovery_codes": codes })))
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
    /// Anahtarın erişebildiği alanlar; `null`: kapsam yok (hesabın bütün yetkileri).
    scope: Option<Vec<String>>,
}

async fn list_tokens(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<TokenInfo>>> {
    let tokens = state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, prefix, read_only, allowed_ips, last_used_at, expires_at, created_at, scope
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
                // Çözümlenemeyen kapsam "hiçbir alan" gösterilir; kimlik doğrulamadaki davranışla aynı.
                scope: row.get::<_, Option<String>>(8)?.map(|raw| serde_json::from_str(&raw).unwrap_or_default()),
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
    /// Anahtarın erişebileceği alanlar (alt hesap yetki alanlarıyla aynı adlar); boş ya da verilmezse
    /// hesabın bütün yetkileri. Kapsamlı anahtar yalnızca bu alanların uçlarını çağırabilir.
    #[serde(default)]
    scope: Vec<String>,
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
    // Başka hesap adına oturumdayken o hesaba kalıcı erişim (anahtar) üretilemez.
    if user.impersonator.is_some() {
        return Err(AppError::bad("Başka hesap adına oturumdayken API anahtarı üretilemez"));
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
    let scope = auth::normalize_scope(&body.scope, &user)?;
    let token = format!("{API_TOKEN_PREFIX}{}", auth::random_token(24));
    let prefix: String = token.chars().take(12).collect();
    let created = now();
    let expires_at = body.expires_in_days.filter(|days| *days > 0).map(|days| created + days * 86400);
    let id = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO api_tokens (user_id, name, token_hash, prefix, read_only, allowed_ips, expires_at, created_at, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                user.id,
                name,
                auth::hash_token(&token),
                prefix,
                body.read_only,
                body.allowed_ips.trim(),
                expires_at,
                created,
                scope
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })?;
    auth::audit(&state.db, &user, "token.create", name, scope.as_deref().unwrap_or(""));
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
