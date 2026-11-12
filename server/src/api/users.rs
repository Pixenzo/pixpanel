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
        .route("/{id}/usage", get(usage))
        .route("/{id}/2fa/reset", axum::routing::post(reset_two_factor))
        .route("/{id}/impersonate", axum::routing::post(impersonate))
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
    package_id: Option<i64>,
    package_name: Option<String>,
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
                    u.status, u.language, u.created_at, u.last_login_at, u.totp_enabled, u.package_id, k.name
             FROM users u LEFT JOIN users p ON p.id = u.parent_id LEFT JOIN packages k ON k.id = u.package_id
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
                package_id: row.get(12)?,
                package_name: row.get(13)?,
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
    // Hesap adı aynı zamanda Linux kullanıcı adıdır. Bir servisin kullanıcısıyla aynı adı taşıyan
    // hesap, o servis sonradan kurulursa onun dosyalarına sahip olurdu.
    const RESERVED: &[&str] = &[
        "root", "daemon", "bin", "sys", "sync", "games", "man", "mail", "news", "uucp", "proxy", "backup", "list", "irc", "nobody", "sshd", "www-data",
        "systemd-network", "systemd-resolve", "systemd-timesync", "messagebus", "syslog", "admin-panel", "mysql", "mariadb", "postgres", "redis", "pdns",
        "nginx", "postfix", "dovecot", "dovenull", "vmail", "opendkim", "rspamd", "_rspamd", "clamav", "ftp", "ftpuser", "pure-ftpd", "docker", "pixpanel",
        "ubuntu", "debian", "sudo", "adm", "staff", "users", "wheel", "ssl-cert", "tss", "polkitd", "lxd", "uuidd", "tcpdump", "landscape",
    ];
    if RESERVED.contains(&username) || username.starts_with("systemd-") || username.starts_with("pixpanel-") {
        return Err(AppError::bad("Bu kullanıcı adı sistem tarafından kullanılıyor; başka bir ad seçin"));
    }
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
    /// Hesaba atanacak hizmet paketi.
    package_id: Option<i64>,
}

/// Paketin bu hesaba atanabildiğini denetler: paketi yalnızca tanımlayan (ya da yönetici)
/// atayabilir; yöneticilere ve alt hesaplara paket atanmaz (alt hesap üst hesabına tabidir).
fn assignable_package(state: &AppState, user: &AuthUser, package_id: i64, target_role: &str) -> AppResult<i64> {
    if matches!(target_role, "admin" | "member") {
        return Err(AppError::bad("Yöneticilere ve alt hesaplara paket atanmaz"));
    }
    let package = crate::quota::load_package(&state.db, package_id).map_err(|_| AppError::bad("Paket bulunamadı"))?;
    if !user.is_admin() && package.owner_id != user.id {
        return Err(AppError::bad("Paket bulunamadı"));
    }
    Ok(package.id)
}

/// Yönetilen bir hesabın paketi ve kullanımı.
async fn usage(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<crate::quota::Usage>> {
    load_manageable(&state, &user, id)?;
    let mut usage = crate::quota::usage(&state.db, id)?;
    usage.runtime = crate::hosting::resource_limits::runtime(&state.db, id).await;
    Ok(Json(usage))
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
    // Bu adla bir Linux hesabı duruyorsa (silinmiş bir hesaptan kalan dosyalarıyla birlikte) yeni
    // hesap onun ev klasörünü devralırdı.
    if crate::hosting::sysuser::lookup(&username).is_some() {
        return Err(AppError::conflict(format!("“{username}” adı sunucuda kullanılıyor (daha önce silinmiş bir hesabın dosyaları duruyor olabilir); başka bir ad seçin")));
    }
    auth::validate_password(&body.password)?;
    validate_permissions(&body.permissions)?;
    let package_id = match body.package_id {
        Some(package_id) => Some(assignable_package(&state, &user, package_id, role.as_str())?),
        None => None,
    };

    let hash = auth::hash_password(&body.password)?;
    let permissions = serde_json::to_string(&body.permissions).unwrap_or_else(|_| "[]".into());
    let id = state.db.call(|conn| {
        let tx = conn.transaction()?;
        let id = crate::db::next_user_id(&tx)?;
        tx.execute(
            "INSERT INTO users (id, username, email, password_hash, role, parent_id, permissions, created_at, package_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, username, body.email.trim(), hash, role.as_str(), parent_id, permissions, now(), package_id],
        )?;
        tx.commit()?;
        Ok(id)
    })?;
    // Paketin işlemci/bellek sınırı kaydedilir; Linux kullanıcısı ilk site açılınca oluşturulur ve sınır o zaman uygulanır.
    if package_id.is_some() {
        crate::hosting::resource_limits::sync(&state, Some(&[id])).await;
    }
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

/// Yönettiği bir hesabın panelini onun gözünden görmek için o hesap adına oturum açar.
/// İstek yapanın kendi oturumu kapanır; geri dönüş `POST /auth/impersonate/stop` iledir.
async fn impersonate(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> AppResult<axum::response::Response> {
    use axum::response::IntoResponse;
    // Yalnızca gerçek bir panel oturumundan: anahtarla ya da zaten başka hesap adınayken yapılamaz.
    let Some(session_id) = user.session_id.filter(|_| !user.via_token) else { return Err(AppError::Forbidden) };
    if user.impersonator.is_some() {
        return Err(AppError::bad("Önce kendi hesabınıza dönün"));
    }
    if id == user.id {
        return Err(AppError::bad("Zaten kendi hesabınızdasınız"));
    }
    let target = load_manageable(&state, &user, id)?;
    // Yönetici başka bir yöneticinin yerine geçemez: yetki devri değil, destek aracıdır.
    if target.role == "admin" {
        return Err(AppError::bad("Yönetici hesabı adına oturum açılamaz"));
    }
    let active: bool = state.db.call(|conn| conn.query_row("SELECT status = 'active' FROM users WHERE id = ?1", params![id], |row| row.get(0)))?;
    if !active {
        return Err(AppError::bad("Askıya alınmış hesap adına oturum açılamaz"));
    }
    let user_agent = headers.get(axum::http::header::USER_AGENT).and_then(|value| value.to_str().ok()).unwrap_or("");
    let ttl = state.cfg.session_ttl_secs.min(2 * 3600);
    let token = auth::create_impersonation_session(&state.db, id, user.id, &user.ip, user_agent, ttl)?;
    state.db.call(|conn| conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id]))?;
    auth::audit(&state.db, &user, "user.impersonate", &target.username, "");
    let cookie = auth::session_cookie(&token, ttl, state.cfg.tls);
    Ok(([(axum::http::header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
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
    /// Atanacak paket; 0 paketi kaldırır.
    package_id: Option<i64>,
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateBody>,
) -> AppResult<Json<serde_json::Value>> {
    // Hesap kendi kaydında yalnızca arayüz dilini değiştirebilir; parola ve e-posta için ayrı
    // uçlar vardır, durum, yetki ve paket ise hesabı yönetenin işidir.
    let own_language_only = id == user.id
        && body.language.is_some()
        && body.email.is_none()
        && body.password.is_none()
        && body.status.is_none()
        && body.permissions.is_none()
        && body.package_id.is_none();
    let target = if own_language_only {
        Target { username: user.username.clone(), role: user.role.as_str().to_string(), parent_id: user.parent_id }
    } else {
        load_manageable(&state, &user, id)?
    };

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
    // Paketi yalnızca hesabı yöneten (yönetici ya da bayisi) değiştirebilir; hesap kendi paketini değiştiremez.
    let package_change: Option<Option<i64>> = match body.package_id {
        None => None,
        Some(_) if id == user.id => return Err(AppError::Forbidden),
        Some(0) => Some(None),
        Some(package_id) => Some(Some(assignable_package(&state, &user, package_id, &target.role)?)),
    };
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
        if let Some(package_id) = package_change {
            tx.execute("UPDATE users SET package_id = ?1 WHERE id = ?2", params![package_id, id])?;
        }
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
    // Askıya alma yalnız panel oturumunu değil sunucudaki erişimi de kapatır: SSH/SFTP girişi ve
    // zamanlı görevler durur; hesap yeniden etkinleşince geri gelir.
    if let Some(status) = &body.status {
        let active = status == "active";
        if let Err(err) = crate::hosting::sysuser::set_locked(&target.username, !active).await {
            tracing::warn!(account = %target.username, error = %err, "Linux hesabının kilidi değiştirilemedi");
        }
        if let Err(err) = crate::api::cron::set_account_enabled(&state, &target.username, active) {
            tracing::warn!(account = %target.username, error = %err, "zamanlı görevler güncellenemedi");
        }
    }
    // Paket atandı ya da kaldırıldı: hesabın işlemci/bellek sınırları sisteme uygulanır.
    if package_change.is_some() {
        crate::hosting::resource_limits::sync(&state, Some(&[id])).await;
    }
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
    state.db.call(|conn| {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        // Görev ve denetim kayıtları kalır ama artık hiçbir hesaba bağlı değildir (yalnız yönetici görür).
        tx.execute("UPDATE jobs SET user_id = NULL WHERE user_id = ?1", params![id])?;
        tx.execute("UPDATE audit_log SET user_id = NULL WHERE user_id = ?1", params![id])?;
        tx.commit()
    })?;
    // Hesabın SSH anahtar dosyası geride kalmasın (aynı adla yeni hesap açılırsa eski anahtarlar geçerli olurdu).
    crate::hosting::sshaccess::remove_keys(&target.username);
    // Zamanlı görevler ve Linux hesabı da kalkar; aynı adla açılacak yeni hesap eskisinin
    // görevlerini, anahtarlarını ya da dosyalarını devralmaz.
    if let Err(err) = crate::api::cron::remove_account(&state, &target.username) {
        tracing::warn!(account = %target.username, error = %err, "hesabın zamanlı görevleri kaldırılamadı");
    }
    // Kaynak sınırı kayıtları ve dilimin drop-in dosyası, Linux kullanıcısı (ve uid'i) silinmeden önce kaldırılır.
    crate::hosting::resource_limits::forget(&target.username).await;
    match crate::hosting::sysuser::remove(&target.username).await {
        Ok(true) => {}
        Ok(false) => tracing::info!(account = %target.username, "Linux hesabı silinmedi (saklanmış site dosyaları var); kilitlendi"),
        Err(err) => tracing::warn!(account = %target.username, error = %err, "Linux hesabı kaldırılamadı"),
    }
    // Hesabın Redis örneği de onunla birlikte kapanır (süreç, yapılandırma ve veri).
    crate::hosting::redis_account::remove_for_username(&state.db, id, &target.username).await;
    auth::audit(&state.db, &user, "user.delete", &target.username, "");
    Ok(Json(json!({ "ok": true })))
}
