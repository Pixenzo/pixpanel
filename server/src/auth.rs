//! Kimlik doğrulama: parola özetleme, oturumlar, API anahtarları ve
//! isteklerden kullanıcıyı çıkaran `AuthUser`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{ConnectInfo, FromRequestParts, MatchedPath};
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
    /// Oturum başka bir hesap (yönetici ya da bayi) tarafından bu hesap adına açıldıysa onun adı.
    #[serde(rename = "impersonated_by")]
    pub impersonator: Option<String>,
    #[serde(skip)]
    pub impersonator_id: Option<i64>,
    /// İstek alan kapsamlı bir API anahtarıyla yapıldıysa anahtarın erişebildiği alanlar
    /// (`None`: kapsam yok, hesabın bütün yetkileri). Kapsam yetkiyi hiçbir zaman genişletmez;
    /// hesabın kendi yetkisiyle kesişim alınır.
    #[serde(rename = "token_scope", skip_serializing_if = "Option::is_none")]
    pub token_scope: Option<Vec<String>>,
    /// Kapsamlı anahtarın isteği, yol tablosunda (`scope_gate`) hangi alana eşlendi?
    /// Alan uçlarının `require_admin` çağırması kapsamlı anahtarı düşürmesin diye tutulur.
    #[serde(skip)]
    pub scope_area: Option<&'static str>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    /// Yalnızca yönetici. Kapsamlı bir anahtar, yol tablosunda bir alana eşlenmemiş
    /// (alan dışı) yönetim uçlarına hiçbir zaman erişemez.
    pub fn require_admin(&self) -> AppResult<()> {
        if self.token_scope.is_some() && self.scope_area.is_none() {
            return Err(AppError::Forbidden);
        }
        if self.is_admin() { Ok(()) } else { Err(AppError::Forbidden) }
    }

    /// Anahtarın kapsamı bu alana izin veriyor mu? Kapsamsız anahtar ve oturum için hep doğru.
    pub fn scope_allows(&self, area: &str) -> bool {
        self.token_scope.as_ref().is_none_or(|scope| scope.iter().any(|a| a == area))
    }

    /// Kaynakların sahibi olan hesap: alt hesaplar üst hesabın kaynaklarını yönetir.
    pub fn owner_id(&self) -> i64 {
        match self.role {
            Role::Member => self.parent_id.unwrap_or(self.id),
            _ => self.id,
        }
    }

    /// Alt hesaplar yalnızca kendilerine verilen alanlarda işlem yapabilir.
    /// API anahtarının kapsamı varsa onunla da kesişim alınır.
    pub fn can(&self, area: &str) -> bool {
        self.scope_allows(area) && (self.role != Role::Member || self.permissions.iter().any(|p| p == area))
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

/// Başka hesap adına oturum açar: `impersonator_id`, oturumu asıl açan hesaptır.
/// Hedef hesabın "son giriş" bilgisi değişmez; giriş yapan o değildir.
pub fn create_impersonation_session(db: &Db, user_id: i64, impersonator_id: i64, ip: &str, user_agent: &str, ttl: i64) -> AppResult<String> {
    let token = random_token(32);
    let token_hash = hash_token(&token);
    let created = now();
    db.call(|conn| {
        conn.execute(
            "INSERT INTO sessions (user_id, token_hash, ip, user_agent, created_at, expires_at, impersonator_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![user_id, token_hash, ip, user_agent, created, created + ttl, impersonator_id],
        )
    })?;
    Ok(token)
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

/// Alan kapsamlı bir API anahtarının bir uca girebilme kararı.
#[derive(Debug, PartialEq, Eq)]
pub enum ScopeGate {
    /// Alan gerektirmeyen zararsız uç (kimlik okuma, kendi görevleri).
    Open,
    /// Uç bu alana aittir; anahtarın kapsamı alanı içermiyorsa reddedilir.
    Area(&'static str),
    /// Kapsamlı anahtara kapalı (yönetim, hesap ve oturum uçları dahil).
    Deny,
}

/// Eşleşen yol şablonunu (`/api/v1/sites/{id}/ssl` gibi) bir alana eşler. Tek merkezdir ve
/// "varsayılan ret" ilkesiyle çalışır: tabloda olmayan her uç kapsamlı anahtara kapalıdır;
/// yeni eklenen bir uç, buraya yazılmadıkça kapsamlı anahtardan erişilemez. Uçların kendi
/// `require(alan)` denetimi ayrıca geçerlidir (kesişim).
pub fn scope_gate(template: &str) -> ScopeGate {
    let Some(rest) = template.strip_prefix("/api/v1/") else { return ScopeGate::Deny };
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        ["auth", "me"] => ScopeGate::Open,
        // Görevler (ör. site ekleyince dönen job_id): içerik alanına göre `api::jobs` içinde süzülür.
        ["jobs", ..] => ScopeGate::Open,
        ["sites", _, "ssl", ..] => ScopeGate::Area("ssl"),
        ["sites", _, "ftp", ..] => ScopeGate::Area("ftp"),
        ["sites", ..] => ScopeGate::Area("sites"),
        ["databases", ..] => ScopeGate::Area("databases"),
        // Redis: yalnızca hesabın kendi örneği; sunucu geneli yönetim uçları (özet, ayar, hesaplar) kapalı.
        ["redis", "mine", ..] | ["redis", "keys"] | ["redis", "key"] | ["redis", "command"] | ["redis", "flush"] => ScopeGate::Area("databases"),
        ["mail", ..] => ScopeGate::Area("mail"),
        ["dns", ..] => ScopeGate::Area("dns"),
        ["files", ..] => ScopeGate::Area("files"),
        ["cron", ..] => ScopeGate::Area("cron"),
        ["logs", ..] => ScopeGate::Area("logs"),
        ["docker", ..] => ScopeGate::Area("docker"),
        ["backup", "mine", ..] => ScopeGate::Area("backups"),
        _ => ScopeGate::Deny,
    }
}

/// Anahtar oluştururken istenen alan listesini doğrular ve saklanacak JSON'a çevirir.
/// Boş liste "kapsam yok" demektir (`None`). Alt hesap, kendi yetkisinde olmayan alanı kapsama alamaz.
pub fn normalize_scope(requested: &[String], creator: &AuthUser) -> AppResult<Option<String>> {
    let mut areas: Vec<&str> = Vec::new();
    for area in requested {
        let area = area.trim();
        let Some(&known) = PERMISSION_AREAS.iter().find(|known| **known == area) else {
            return Err(AppError::bad(format!("Geçersiz erişim alanı: {area}")));
        };
        if creator.role == Role::Member && !creator.permissions.iter().any(|p| p == known) {
            return Err(AppError::bad(format!("Bu alana yetkiniz yok: {known}")));
        }
        if !areas.contains(&known) {
            areas.push(known);
        }
    }
    if areas.is_empty() {
        return Ok(None);
    }
    // Saklama düzeni sabit olsun diye alanlar tanım sırasına dizilir.
    areas.sort_by_key(|area| PERMISSION_AREAS.iter().position(|known| known == area));
    serde_json::to_string(&areas).map(Some).map_err(|err| AppError::internal(err.to_string()))
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let ip = client_ip(parts);
        let current = now();
        let safe_method = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);

        let mut impersonation: Option<(i64, String)> = None;
        // Kapsamlı anahtarın bu istek için geçtiği kapı: `Some(None)` alan gerektirmeyen açık uç.
        let mut gate: Option<Option<&'static str>> = None;
        let mut token_scope: Option<Vec<String>> = None;
        let (row, via_token, session_id) = if let Some(token) = bearer_token(parts) {
            let token_hash = hash_token(token);
            let found = state.db.call(|conn| {
                conn.query_row(
                    &format!(
                        "SELECT {USER_COLUMNS}, t.id, t.read_only, t.allowed_ips, t.expires_at, t.scope
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
                            row.get::<_, Option<String>>(12)?,
                        ))
                    },
                )
                .optional()
            })?;
            let (user, token_id, read_only, allowed_ips, expires_at, scope) =
                found.ok_or(AppError::Unauthorized)?;
            if expires_at.is_some_and(|at| at < current) || !ip_allowed(&allowed_ips, &ip) {
                return Err(AppError::Unauthorized);
            }
            if read_only && !safe_method {
                return Err(AppError::Forbidden);
            }
            // Alan kapsamlı anahtar: yol tablosunda yer almayan ya da kapsamı dışında kalan uçlar reddedilir.
            // Çözümlenemeyen kapsam sütunu "hiçbir alan" sayılır (güvenli yönde).
            if let Some(raw) = scope {
                let areas: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
                let matched = parts.extensions.get::<MatchedPath>().map(|path| path.as_str().to_string());
                gate = match matched.as_deref().map(scope_gate) {
                    Some(ScopeGate::Open) => Some(None),
                    Some(ScopeGate::Area(area)) if areas.iter().any(|a| a == area) => Some(Some(area)),
                    _ => return Err(AppError::Forbidden),
                };
                token_scope = Some(areas);
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
                        "SELECT {USER_COLUMNS}, s.id, s.expires_at, s.impersonator_id, i.username, i.status
                         FROM sessions s JOIN users u ON u.id = s.user_id
                         LEFT JOIN users i ON i.id = s.impersonator_id
                         WHERE s.token_hash = ?1"
                    ),
                    params![token_hash],
                    |row| {
                        Ok((
                            read_user(row)?,
                            row.get::<_, i64>(8)?,
                            row.get::<_, i64>(9)?,
                            row.get::<_, Option<i64>>(10)?,
                            row.get::<_, Option<String>>(11)?,
                            row.get::<_, Option<String>>(12)?,
                        ))
                    },
                )
                .optional()
            })?;
            let (user, session_id, expires_at, impersonator_id, impersonator_name, impersonator_status) = found.ok_or(AppError::Unauthorized)?;
            if let Some(id) = impersonator_id {
                // Oturumu açan hesap askıya alındıysa onun adına açtığı oturumlar da geçersizdir.
                if impersonator_status.as_deref() != Some("active") {
                    return Err(AppError::Unauthorized);
                }
                impersonation = Some((id, impersonator_name.unwrap_or_default()));
            }
            if expires_at < current {
                return Err(AppError::Unauthorized);
            }
            // Oturum kullanıldıkça uzar; her istekte yazmamak için yarı ömürde yenilenir.
            // Başka hesap adına açılan oturum uzamaz: açıldığı andaki süresi dolunca biter.
            let ttl = state.cfg.session_ttl_secs;
            if impersonation.is_none() && expires_at - current < ttl / 2 {
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
            impersonator: impersonation.as_ref().map(|(_, name)| name.clone()),
            impersonator_id: impersonation.map(|(id, _)| id),
            token_scope,
            scope_area: gate.flatten(),
        })
    }
}

/// İstekteki oturum çerezinin geçerli bir panel oturumuna ait olduğunu doğrular.
///
/// Panelin vekillediği araçlar (phpMyAdmin, Adminer) için kullanılır: bu
/// araçların formları panelin CSRF başlığını gönderemez; çapraz site isteklerine
/// karşı koruma, oturum çerezinin `SameSite=Strict` olmasıyla sağlanır.
pub fn session_is_valid(parts: &Parts, state: &AppState) -> bool {
    let Some(token) = cookie_value(parts, SESSION_COOKIE) else { return false };
    let token_hash = hash_token(token);
    state
        .db
        .call(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions s JOIN users u ON u.id = s.user_id
                               WHERE s.token_hash = ?1 AND s.expires_at >= ?2 AND u.status = 'active')",
                params![token_hash, now()],
                |row| row.get::<_, bool>(0),
            )
        })
        .unwrap_or(false)
}

/// Denetim günlüğüne kayıt düşer. Günlük yazılamaması asıl işlemi bozmamalıdır.
pub fn audit(db: &Db, user: &AuthUser, action: &str, target: &str, detail: &str) {
    // Başka hesap adına yapılan işlemde kimin yaptığı da kayda geçer.
    let username = match &user.impersonator {
        Some(by) => format!("{by} → {}", user.username),
        None => user.username.clone(),
    };
    let result = db.call(|conn| {
        conn.execute(
            "INSERT INTO audit_log (user_id, username, action, target, detail, ip, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![user.id, username, action, target, detail, user.ip, now()],
        )
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, action, "denetim kaydı yazılamadı");
    }
    // Denetlenen her işlem bir şeyi değiştirmiştir; açık arayüzler anında haberdar edilir.
    crate::events::changed(action, user.owner_id());
    // Hesabın tanımladığı web kancalarına da iletilir.
    crate::webhooks::emit_audit(action, user.owner_id(), &username, target, detail);
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    fn user(role: Role, permissions: &[&str]) -> AuthUser {
        AuthUser {
            id: 1,
            username: "u".into(),
            email: String::new(),
            role,
            parent_id: None,
            permissions: permissions.iter().map(|p| p.to_string()).collect(),
            language: "tr".into(),
            ip: String::new(),
            via_token: true,
            session_id: None,
            impersonator: None,
            impersonator_id: None,
            token_scope: None,
            scope_area: None,
        }
    }

    #[test]
    fn gate_maps_areas_and_denies_the_rest() {
        assert_eq!(scope_gate("/api/v1/auth/me"), ScopeGate::Open);
        assert_eq!(scope_gate("/api/v1/jobs/{id}/stream"), ScopeGate::Open);
        assert_eq!(scope_gate("/api/v1/sites"), ScopeGate::Area("sites"));
        assert_eq!(scope_gate("/api/v1/sites/{id}"), ScopeGate::Area("sites"));
        assert_eq!(scope_gate("/api/v1/sites/{id}/ssl/letsencrypt"), ScopeGate::Area("ssl"));
        assert_eq!(scope_gate("/api/v1/sites/{id}/ftp/{account}"), ScopeGate::Area("ftp"));
        assert_eq!(scope_gate("/api/v1/redis/mine/enable"), ScopeGate::Area("databases"));
        assert_eq!(scope_gate("/api/v1/backup/mine/run"), ScopeGate::Area("backups"));
        for denied in [
            "/api/v1/users",
            "/api/v1/users/{id}/access",
            "/api/v1/auth/tokens",
            "/api/v1/auth/password",
            "/api/v1/system/services/{name}/{action}",
            "/api/v1/firewall",
            "/api/v1/components/{id}/install",
            "/api/v1/terminal/ws",
            "/api/v1/backup/targets",
            "/api/v1/redis",
            "/api/v1/redis/accounts",
            "/api/v1/webhooks",
            "/api/v1/events",
            "/other/sites",
        ] {
            assert_eq!(scope_gate(denied), ScopeGate::Deny, "{denied}");
        }
    }

    #[test]
    fn scope_intersects_with_member_permissions() {
        let mut member = user(Role::Member, &["sites"]);
        member.token_scope = Some(vec!["sites".into(), "dns".into()]);
        assert!(member.can("sites"));
        assert!(!member.can("dns"), "kapsam alt hesabın yetkisini genişletmemeli");
        let mut owner = user(Role::User, &[]);
        owner.token_scope = Some(vec!["sites".into()]);
        assert!(owner.can("sites"));
        assert!(!owner.can("mail"));
    }

    #[test]
    fn scoped_token_cannot_pass_require_admin_outside_areas() {
        let mut admin = user(Role::Admin, &[]);
        assert!(admin.require_admin().is_ok());
        admin.token_scope = Some(vec!["sites".into()]);
        assert!(admin.require_admin().is_err());
        admin.scope_area = Some("docker");
        assert!(admin.require_admin().is_ok());
    }

    #[test]
    fn normalize_scope_validates_names_and_member_permissions() {
        let owner = user(Role::User, &[]);
        assert_eq!(normalize_scope(&[], &owner).unwrap(), None);
        let wanted = vec!["dns".to_string(), "sites".to_string(), "dns".to_string()];
        assert_eq!(normalize_scope(&wanted, &owner).unwrap().as_deref(), Some(r#"["sites","dns"]"#));
        assert!(normalize_scope(&["users".to_string()], &owner).is_err());
        assert!(normalize_scope(&["".to_string()], &owner).is_err());
        let member = user(Role::Member, &["sites"]);
        assert!(normalize_scope(&["sites".to_string()], &member).is_ok());
        assert!(normalize_scope(&["dns".to_string()], &member).is_err());
    }
}
