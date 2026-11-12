//! Panelin markası: adı, logosu, giriş sayfasında gösterilen metin ve destek bağlantısı.
//!
//! İki katman vardır: yöneticinin tanımladığı sunucu geneli marka ve bayi bazında marka
//! (`reseller_brands`). Bayinin müşterileri ve onların alt hesapları panelde bayinin markasını görür.
//!
//! Okuma ucu oturum istemez; giriş sayfası da markayı gösterir. Bu yüzden burada yalnızca
//! herkesin görebileceği bilgiler tutulur.

use axum::extract::{FromRequestParts, Path, Request, State};
use axum::http::header::HOST;
use axum::http::request::Parts;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser, Role};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::domain;
use crate::state::AppState;

const KEY: &str = "branding";
const MAX_NAME_CHARS: usize = 40;
const MAX_MESSAGE_CHARS: usize = 300;
const MAX_SUPPORT_CHARS: usize = 200;
/// Logonun (veri adresi olarak) en büyük boyutu. Her sayfada gönderildiği için küçük tutulur.
const MAX_LOGO_BYTES: usize = 200 * 1024;
/// Logo `<img>` içinde gösterilir; orada betik çalışmaz. Yine de yalnızca bilinen görsel türleri kabul edilir.
const LOGO_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/svg+xml"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Branding {
    /// Boşsa panelin kendi adı gösterilir.
    pub name: String,
    /// `data:image/...;base64,...` biçiminde görsel; boşsa panelin kendi işareti.
    pub logo: String,
    /// Giriş sayfasında formun altında gösterilen kısa metin (ör. destek adresi).
    pub login_message: String,
    /// İsteğe bağlı destek bağlantısı: `https://` ya da `mailto:`. Boşsa arayüzde gösterilmez.
    pub support_url: String,
}

impl Branding {
    fn is_empty(&self) -> bool {
        self.name.is_empty() && self.logo.is_empty() && self.login_message.is_empty() && self.support_url.is_empty()
    }
}

/// Bayinin kendi markası ve panel alan adı (bayi ya da yönetici düzenlerken görülen hâli).
#[derive(Debug, Serialize)]
struct ResellerBrand {
    #[serde(flatten)]
    branding: Branding,
    /// Bayinin müşterilerine verdiği panel adresi (ör. `panel.bayi.com`); yoksa boş.
    panel_domain: String,
    /// Bayi için kayıtlı bir marka var mı?
    defined: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ResellerBody {
    name: String,
    logo: String,
    login_message: String,
    support_url: String,
    panel_domain: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(show).put(save))
        .route("/resellers/{id}", get(reseller_show).put(reseller_save).delete(reseller_remove))
}

fn load(state: &AppState) -> Branding {
    state.db.setting(KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

/// İsteğin hangi alan adına geldiği (port ve büyük/küçük harf farkı atılır); IP adresi ve geçersiz adlar `None`.
fn host_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    // `[::1]:8443` gibi IPv6 yazımları zaten alan adı değildir.
    if raw.starts_with('[') {
        return None;
    }
    domain::normalize(raw.split(':').next()?).ok()
}

fn request_host(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(HOST).and_then(|value| value.to_str().ok()).or_else(|| parts.uri.authority().map(|authority| authority.as_str()))?;
    host_name(raw)
}

/// Hesabın bağlı olduğu bayi: hesabın kendisi bayiyse kendisi, müşteriyse ya da müşterinin alt hesabıysa üstündeki bayi.
fn reseller_for(conn: &Connection, user_id: i64) -> rusqlite::Result<Option<i64>> {
    let mut current = Some(user_id);
    // Hiyerarşi en çok bayi → müşteri → alt hesap kadar derindir; döngü sınırlıdır.
    for _ in 0..4 {
        let Some(id) = current else { break };
        let row: Option<(String, Option<i64>)> =
            conn.query_row("SELECT role, parent_id FROM users WHERE id = ?1", params![id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        let Some((role, parent)) = row else { break };
        if role == "reseller" {
            return Ok(Some(id));
        }
        current = parent;
    }
    Ok(None)
}

fn reseller_by_domain(conn: &Connection, host: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT reseller_id FROM reseller_brands WHERE panel_domain = ?1", params![host], |row| row.get(0)).optional()
}

/// Bayinin markası; kayıt yoksa ya da hiçbir alanı doldurulmamışsa `None` (sunucu geneli marka geçerli olur).
fn reseller_branding(conn: &Connection, reseller_id: i64) -> rusqlite::Result<Option<Branding>> {
    let found = conn
        .query_row(
            "SELECT name, logo, login_message, support_url FROM reseller_brands WHERE reseller_id = ?1",
            params![reseller_id],
            |row| Ok(Branding { name: row.get(0)?, logo: row.get(1)?, login_message: row.get(2)?, support_url: row.get(3)? }),
        )
        .optional()?;
    Ok(found.filter(|branding| !branding.is_empty()))
}

/// Görünen marka. Oturum açıksa hesabın bağlı olduğu bayinin markası (varsa); oturum yoksa
/// isteğin geldiği alan adına bağlı bayinin markası; aksi hâlde sunucu geneli marka.
///
/// `Host` başlığı yalnızca GÖRÜNÜMÜ seçer: hiçbir yetki kararı ona dayanmaz (CLAUDE.md'deki
/// "başlıklara güvenilmez" kuralı yetki içindir). Başlığı sahteleyen biri en fazla kendi
/// tarayıcısında başka bir bayinin herkese açık markasını görür.
async fn show(State(state): State<AppState>, request: Request) -> Json<Branding> {
    let (mut parts, _) = request.into_parts();
    let user = AuthUser::from_request_parts(&mut parts, &state).await.ok();
    let host = request_host(&parts);
    let found = state
        .db
        .call(|conn| {
            let reseller = match (&user, &host) {
                (Some(user), _) => reseller_for(conn, user.id)?,
                (None, Some(host)) => reseller_by_domain(conn, host)?,
                (None, None) => None,
            };
            match reseller {
                Some(id) => reseller_branding(conn, id),
                None => Ok(None),
            }
        })
        .ok()
        .flatten();
    Json(found.unwrap_or_else(|| load(&state)))
}

fn plain_text(value: &str, limit: usize, what: &str) -> AppResult<String> {
    let text = value.trim();
    if text.chars().count() > limit {
        return Err(AppError::bad(format!("{what} en çok {limit} karakter olabilir")));
    }
    if text.chars().any(|ch| ch.is_control() && ch != '\n') {
        return Err(AppError::bad(format!("{what} denetim karakteri içeremez")));
    }
    Ok(text.to_string())
}

/// Logoyu doğrular: bilinen bir görsel türünde, gerçekten base64 ve boyut sınırının altında olmalı.
/// Sunucu geneli marka ve bayi markası aynı işlevden geçer.
pub fn validate_logo(logo: &str) -> AppResult<String> {
    let logo = logo.trim();
    if logo.is_empty() {
        return Ok(String::new());
    }
    if logo.len() > MAX_LOGO_BYTES {
        return Err(AppError::bad("Logo çok büyük; 150 KB'ın altında bir görsel seçin"));
    }
    let invalid = || AppError::bad("Logo PNG, JPEG, WebP ya da SVG biçiminde bir görsel olmalı");
    let (header, data) = logo.strip_prefix("data:").and_then(|rest| rest.split_once(";base64,")).ok_or_else(invalid)?;
    if !LOGO_TYPES.contains(&header) {
        return Err(invalid());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| invalid())?;
    if bytes.is_empty() {
        return Err(invalid());
    }
    Ok(logo.to_string())
}

/// Destek bağlantısını doğrular: boş, `https://` adresi ya da `mailto:` adresi olabilir.
/// Başka şemalar (`javascript:`, `data:` …), boşluk/satır sonu ve kullanıcı bilgisi içeren adresler reddedilir.
fn validate_support_url(value: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let invalid = || AppError::bad("Destek bağlantısı https:// ya da mailto: ile başlayan geçerli bir adres olmalı");
    if value.chars().count() > MAX_SUPPORT_CHARS {
        return Err(AppError::bad(format!("Destek bağlantısı en çok {MAX_SUPPORT_CHARS} karakter olabilir")));
    }
    // Ayrıştırıcı sekme ve satır sonlarını sessizce atar; bu yüzden önce ham metne bakılır.
    if value.chars().any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '<' | '>' | '"' | '\'' | '`' | '\\')) {
        return Err(invalid());
    }
    let parsed = url::Url::parse(value).map_err(|_| invalid())?;
    match parsed.scheme() {
        "https" => {
            if parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(invalid());
            }
        }
        "mailto" => {
            let path = parsed.path();
            if !path.contains('@') || path.starts_with('@') || path.ends_with('@') {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    Ok(parsed.to_string())
}

/// Marka alanlarını doğrular ve temizler.
fn clean(name: &str, logo: &str, login_message: &str, support_url: &str) -> AppResult<Branding> {
    Ok(Branding {
        name: plain_text(name, MAX_NAME_CHARS, "Panel adı")?.replace('\n', " "),
        logo: validate_logo(logo)?,
        login_message: plain_text(login_message, MAX_MESSAGE_CHARS, "Giriş sayfası metni")?,
        support_url: validate_support_url(support_url)?,
    })
}

async fn save(State(state): State<AppState>, user: AuthUser, Json(body): Json<Branding>) -> AppResult<Json<Branding>> {
    user.require_admin()?;
    let branding = clean(&body.name, &body.logo, &body.login_message, &body.support_url)?;
    state.db.set_setting(KEY, &serde_json::to_string(&branding).map_err(|err| AppError::internal(err.to_string()))?)?;
    auth::audit(&state.db, &user, "branding.update", &branding.name, "");
    Ok(Json(branding))
}

/// Markası düzenlenecek bayiyi ve isteği yapanın bunu yapma yetkisini denetler; bayinin kullanıcı adını döner.
/// Yönetici her bayiyi, bayi yalnızca kendini düzenler; müşteri ve alt hesap hiçbirini.
fn manageable_reseller(state: &AppState, user: &AuthUser, id: i64) -> AppResult<String> {
    match user.role {
        Role::Admin => {}
        // Başkasının hesabının varlığı da sızdırılmaz.
        Role::Reseller if id == user.id => {}
        Role::Reseller => return Err(AppError::NotFound),
        Role::User | Role::Member => return Err(AppError::Forbidden),
    }
    let username: Option<String> =
        state.db.call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1 AND role = 'reseller'", params![id], |row| row.get(0)).optional())?;
    username.ok_or(AppError::NotFound)
}

fn load_reseller_brand(state: &AppState, id: i64) -> AppResult<ResellerBrand> {
    let found = state.db.call(|conn| {
        conn.query_row(
            "SELECT name, logo, login_message, support_url, COALESCE(panel_domain, '') FROM reseller_brands WHERE reseller_id = ?1",
            params![id],
            |row| {
                Ok(ResellerBrand {
                    branding: Branding { name: row.get(0)?, logo: row.get(1)?, login_message: row.get(2)?, support_url: row.get(3)? },
                    panel_domain: row.get(4)?,
                    defined: true,
                })
            },
        )
        .optional()
    })?;
    Ok(found.unwrap_or(ResellerBrand { branding: Branding::default(), panel_domain: String::new(), defined: false }))
}

async fn reseller_show(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<ResellerBrand>> {
    manageable_reseller(&state, &user, id)?;
    Ok(Json(load_reseller_brand(&state, id)?))
}

/// Panelin kendi adları: bunlar bir bayiye bağlanırsa sunucunun kendi giriş sayfası bayinin markasıyla görünürdü.
fn panel_own_names(state: &AppState) -> Vec<String> {
    [crate::panelcert::hostname(&state.db), crate::mail::hostname(&state.db)]
        .iter()
        .filter_map(|name| domain::normalize(name).ok())
        .collect()
}

async fn reseller_save(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>, Json(body): Json<ResellerBody>) -> AppResult<Json<ResellerBrand>> {
    let username = manageable_reseller(&state, &user, id)?;
    let branding = clean(&body.name, &body.logo, &body.login_message, &body.support_url)?;
    let panel_domain = match body.panel_domain.trim() {
        "" => None,
        raw => Some(domain::normalize(raw)?),
    };
    // Hiçbir şey tanımlı değilse bu, markayı kaldırmaktır.
    if branding.is_empty() && panel_domain.is_none() {
        let removed = state.db.call(|conn| conn.execute("DELETE FROM reseller_brands WHERE reseller_id = ?1", params![id]))?;
        if removed > 0 {
            auth::audit(&state.db, &user, "branding.reseller.delete", &username, "");
        }
        return Ok(Json(load_reseller_brand(&state, id)?));
    }
    if let Some(name) = &panel_domain {
        if panel_own_names(&state).contains(name) {
            return Err(AppError::bad("Bu ad panelin kendi adı; bayi panel alan adı olarak kullanılamaz"));
        }
    }
    // Denetim ve yazma aynı bağlantı kilidi altında yapılır: iki bayi aynı anda aynı adı alamaz.
    let taken = state.db.call(|conn| {
        if let Some(name) = &panel_domain {
            let used: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM reseller_brands WHERE panel_domain = ?1 AND reseller_id <> ?2)",
                params![name, id],
                |row| row.get(0),
            )?;
            if used {
                return Ok(true);
            }
        }
        conn.execute(
            "INSERT INTO reseller_brands (reseller_id, name, logo, login_message, support_url, panel_domain, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(reseller_id) DO UPDATE SET name = excluded.name, logo = excluded.logo, login_message = excluded.login_message,
                 support_url = excluded.support_url, panel_domain = excluded.panel_domain, updated_at = excluded.updated_at",
            params![id, branding.name, branding.logo, branding.login_message, branding.support_url, panel_domain, now()],
        )?;
        Ok(false)
    })?;
    if taken {
        return Err(AppError::conflict("Bu panel alan adı başka bir bayiye bağlı"));
    }
    auth::audit(&state.db, &user, "branding.reseller.update", &username, panel_domain.as_deref().unwrap_or(""));
    Ok(Json(load_reseller_brand(&state, id)?))
}

async fn reseller_remove(State(state): State<AppState>, user: AuthUser, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    let username = manageable_reseller(&state, &user, id)?;
    let removed = state.db.call(|conn| conn.execute("DELETE FROM reseller_brands WHERE reseller_id = ?1", params![id]))?;
    if removed > 0 {
        auth::audit(&state.db, &user, "branding.reseller.delete", &username, "");
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_rules() {
        assert_eq!(validate_logo("").unwrap(), "");
        assert!(validate_logo("data:image/png;base64,iVBORw0KGgo=").is_ok());
        assert!(validate_logo("data:image/svg+xml;base64,PHN2Zy8+").is_ok());
        for bad in [
            "https://ornek.com/logo.png",
            "data:text/html;base64,PHNjcmlwdD4=",
            "data:image/png;base64,***",
            "data:image/png;base64,",
            "data:image/png,ham-veri",
            "javascript:alert(1)",
        ] {
            assert!(validate_logo(bad).is_err(), "{bad}");
        }
        let huge = format!("data:image/png;base64,{}", "A".repeat(MAX_LOGO_BYTES));
        assert!(validate_logo(&huge).is_err());
    }

    #[test]
    fn text_rules() {
        assert_eq!(plain_text("  Örnek Hosting  ", 40, "Ad").unwrap(), "Örnek Hosting");
        assert!(plain_text(&"a".repeat(41), 40, "Ad").is_err());
        assert!(plain_text("a\u{0007}b", 40, "Ad").is_err());
        assert!(plain_text("satır\nikinci", 300, "Metin").is_ok());
    }

    #[test]
    fn support_url_rules() {
        assert_eq!(validate_support_url("").unwrap(), "");
        assert_eq!(validate_support_url(" https://destek.ornek.com/yardim ").unwrap(), "https://destek.ornek.com/yardim");
        assert_eq!(validate_support_url("mailto:destek@ornek.com").unwrap(), "mailto:destek@ornek.com");
        for bad in [
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "http://ornek.com",
            "ftp://ornek.com",
            "https://",
            "https://ornek.com/\nx",
            "https://ornek.com/a b",
            "https://kullanici:parola@ornek.com",
            "https://ornek.com/\"onclick=1",
            "mailto:destek",
            "mailto:",
            "ornek.com",
        ] {
            assert!(validate_support_url(bad).is_err(), "{bad:?}");
        }
        assert!(validate_support_url(&format!("https://ornek.com/{}", "a".repeat(MAX_SUPPORT_CHARS))).is_err());
    }

    #[test]
    fn host_rules() {
        assert_eq!(host_name("Panel.Bayi.Test:8443").as_deref(), Some("panel.bayi.test"));
        assert_eq!(host_name("panel.bayi.test").as_deref(), Some("panel.bayi.test"));
        assert_eq!(host_name("192.168.1.25:8443"), None);
        assert_eq!(host_name("[::1]:8443"), None);
        assert_eq!(host_name("localhost"), None);
        assert_eq!(host_name(""), None);
    }
}
