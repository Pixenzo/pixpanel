//! Kimlik doğrulama: parola özetleme, oturumlar, API anahtarları ve
//! isteklerden kullanıcıyı çıkaran `AuthUser`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use axum::http::Method;
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "pixpanel_session";
pub const CSRF_HEADER: &str = "x-pixpanel-csrf";
pub const API_TOKEN_PREFIX: &str = "pix_";

/// Alt hesaplara (member) verilebilecek yetki alanları.
pub const PERMISSION_AREAS: &[&str] = &[
    "sites", "files", "databases", "mail", "dns", "ftp", "cron", "backups", "ssl", "logs",
    "docker",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Reseller,
    User,
    Member,
}

impl Role {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Self::Admin),
            "reseller" => Some(Self::Reseller),
            "user" => Some(Self::User),
            "member" => Some(Self::Member),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Reseller => "reseller",
            Self::User => "user",
            Self::Member => "member",
        }
    }
}

/// İsteği yapan, kimliği doğrulanmış kullanıcı.
#[derive(Debug, Clone, Serialize)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub role: Role,
    pub parent_id: Option<i64>,
    pub permissions: Vec<String>,
    pub language: String,
    #[serde(skip)]
    pub ip: String,
    /// İstek API anahtarıyla mı yapıldı?
    pub via_token: bool,
    #[serde(skip)]
    pub session_id: Option<i64>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    pub fn require_admin(&self) -> AppResult<()> {
        if self.is_admin() { Ok(()) } else { Err(AppError::Forbidden) }
    }

    /// Kaynakların sahibi olan hesap: alt hesaplar üst hesabın kaynaklarını yönetir.
    pub fn owner_id(&self) -> i64 {
        match self.role {
            Role::Member => self.parent_id.unwrap_or(self.id),
            _ => self.id,
        }
    }

    /// Alt hesaplar yalnızca kendilerine verilen alanlarda işlem yapabilir.
    pub fn can(&self, area: &str) -> bool {
        self.role != Role::Member || self.permissions.iter().any(|p| p == area)
    }

    pub fn require(&self, area: &str) -> AppResult<()> {
        if self.can(area) { Ok(()) } else { Err(AppError::Forbidden) }
    }
}

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::internal(format!("parola özetlenemedi: {err}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

pub fn validate_password(password: &str) -> AppResult<()> {
    if password.chars().count() < 10 {
        return Err(AppError::bad("Parola en az 10 karakter olmalı"));
    }
    Ok(())
}

/// Kriptografik olarak güvenli rastgele onaltılık dizge üretir.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Okunaklı rastgele parola üretir (karışabilen karakterler hariç).
pub fn random_password(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut buf = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect()
}

/// Anahtarlar veritabanında yalnızca özet olarak saklanır.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Yeni oturum açar ve çereze yazılacak ham anahtarı döner.
pub fn create_session(db: &Db, user_id: i64, ip: &str, user_agent: &str, ttl: i64) -> AppResult<String> {
    let token = random_token(32);
    let token_hash = hash_token(&token);
    let created = now();
    db.call(|conn| {
        conn.execute("DELETE FROM sessions WHERE expires_at < ?1", params![created])?;
        conn.execute(
            "INSERT INTO sessions (user_id, token_hash, ip, user_agent, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![user_id, token_hash, ip, user_agent, created, created + ttl],
        )?;
        conn.execute("UPDATE users SET last_login_at = ?1 WHERE id = ?2", params![created, user_id])
    })?;
    Ok(token)
}

pub fn session_cookie(token: &str, max_age: i64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}")
}

/// Başarısız giriş denemelerini IP başına sınırlar.
#[derive(Default)]
pub struct LoginThrottle {
    attempts: Mutex<HashMap<String, (u32, i64)>>,
}

impl LoginThrottle {
    const WINDOW_SECS: i64 = 300;
    const MAX_FAILURES: u32 = 8;

    pub fn check(&self, ip: &str) -> AppResult<()> {
        let mut map = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        let current = now();
        map.retain(|_, (_, since)| current - *since < Self::WINDOW_SECS);
        match map.get(ip) {
            Some((count, _)) if *count >= Self::MAX_FAILURES => Err(AppError::TooManyRequests),
            _ => Ok(()),
        }
    }

    pub fn record_failure(&self, ip: &str) {
        let mut map = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        let entry = map.entry(ip.to_string()).or_insert((0, now()));
        entry.0 += 1;
    }

    pub fn clear(&self, ip: &str) {
        self.attempts.lock().unwrap_or_else(|p| p.into_inner()).remove(ip);
    }
}

pub fn client_ip(parts: &Parts) -> String {
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_canonical().to_string())
        .unwrap_or_default()
}

fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// Virgülle ayrılmış izinli IP listesi boşsa her adres kabul edilir.
fn ip_allowed(allowed: &str, ip: &str) -> bool {
    let entries: Vec<&str> = allowed.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if entries.is_empty() {
        return true;
    }
    let Ok(ip) = ip.parse::<IpAddr>() else { return false };
    entries.iter().any(|entry| entry.parse::<IpAddr>().map(|allowed| allowed == ip).unwrap_or(false))
}

struct UserRow {
    id: i64,
    username: String,
    email: String,
    role: String,
    parent_id: Option<i64>,
    permissions: String,
    language: String,
    status: String,
}

const USER_COLUMNS: &str =
    "u.id, u.username, u.email, u.role, u.parent_id, u.permissions, u.language, u.status";

fn read_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        id: row.get(0)?,
        username: row.get(1)?,
        email: row.get(2)?,
        role: row.get(3)?,
        parent_id: row.get(4)?,
        permissions: row.get(5)?,
        language: row.get(6)?,
        status: row.get(7)?,
    })
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let ip = client_ip(parts);
        let current = now();
        let safe_method = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);

        let (row, via_token, session_id) = if let Some(token) = bearer_token(parts) {
            let token_hash = hash_token(token);
            let found = state.db.call(|conn| {
                conn.query_row(
                    &format!(
                        "SELECT {USER_COLUMNS}, t.id, t.read_only, t.allowed_ips, t.expires_at
                         FROM api_tokens t JOIN users u ON u.id = t.user_id
                         WHERE t.token_hash = ?1"
                    ),
                    params![token_hash],
                    |row| {
                        Ok((
                            read_user(row)?,
                            row.get::<_, i64>(8)?,
                            row.get::<_, bool>(9)?,
                            row.get::<_, String>(10)?,
                            row.get::<_, Option<i64>>(11)?,
                        ))
                    },
                )
                .optional()
            })?;
            let (user, token_id, read_only, allowed_ips, expires_at) =
                found.ok_or(AppError::Unauthorized)?;
            if expires_at.is_some_and(|at| at < current) || !ip_allowed(&allowed_ips, &ip) {
                return Err(AppError::Unauthorized);
            }
            if read_only && !safe_method {
                return Err(AppError::Forbidden);
            }
            state.db.call(|conn| {
                conn.execute(
                    "UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2",
                    params![current, token_id],
                )
            })?;
            (user, true, None)
        } else {
            let token = cookie_value(parts, SESSION_COOKIE).ok_or(AppError::Unauthorized)?;
            // Çerezle yapılan değişiklik isteklerinde özel başlık şartı, başka
            // sitelerin kullanıcı adına istek göndermesini (CSRF) engeller.
            if !safe_method && !parts.headers.contains_key(CSRF_HEADER) {
                return Err(AppError::Forbidden);
            }
            let token_hash = hash_token(token);
            let found = state.db.call(|conn| {
                conn.query_row(
                    &format!(
                        "SELECT {USER_COLUMNS}, s.id, s.expires_at
                         FROM sessions s JOIN users u ON u.id = s.user_id
                         WHERE s.token_hash = ?1"
                    ),
                    params![token_hash],
                    |row| Ok((read_user(row)?, row.get::<_, i64>(8)?, row.get::<_, i64>(9)?)),
                )
                .optional()
            })?;
            let (user, session_id, expires_at) = found.ok_or(AppError::Unauthorized)?;
            if expires_at < current {
                return Err(AppError::Unauthorized);
            }
            // Oturum kullanıldıkça uzar; her istekte yazmamak için yarı ömürde yenilenir.
            let ttl = state.cfg.session_ttl_secs;
            if expires_at - current < ttl / 2 {
                state.db.call(|conn| {
                    conn.execute(
                        "UPDATE sessions SET expires_at = ?1 WHERE id = ?2",
                        params![current + ttl, session_id],
                    )
                })?;
            }
            (user, false, Some(session_id))
        };

        if row.status != "active" {
            return Err(AppError::Unauthorized);
        }
        let role = Role::parse(&row.role).ok_or(AppError::Unauthorized)?;
        Ok(AuthUser {
            id: row.id,
            username: row.username,
            email: row.email,
            role,
            parent_id: row.parent_id,
            permissions: serde_json::from_str(&row.permissions).unwrap_or_default(),
            language: row.language,
            ip,
            via_token,
            session_id,
        })
    }
}

/// Denetim günlüğüne kayıt düşer. Günlük yazılamaması asıl işlemi bozmamalıdır.
pub fn audit(db: &Db, user: &AuthUser, action: &str, target: &str, detail: &str) {
    let result = db.call(|conn| {
        conn.execute(
            "INSERT INTO audit_log (user_id, username, action, target, detail, ip, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![user.id, user.username, action, target, detail, user.ip, now()],
        )
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, action, "denetim kaydı yazılamadı");
    }
}
