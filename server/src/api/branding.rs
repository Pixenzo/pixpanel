//! Panelin markası: adı, logosu ve giriş sayfasında gösterilen metin.
//!
//! Okuma ucu oturum istemez; giriş sayfası da markayı gösterir. Bu yüzden burada yalnızca
//! herkesin görebileceği bilgiler tutulur.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const KEY: &str = "branding";
const MAX_NAME_CHARS: usize = 40;
const MAX_MESSAGE_CHARS: usize = 300;
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
}

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(show).put(save))
}

fn load(state: &AppState) -> Branding {
    state.db.setting(KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

async fn show(State(state): State<AppState>) -> Json<Branding> {
    Json(load(&state))
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

async fn save(State(state): State<AppState>, user: AuthUser, Json(body): Json<Branding>) -> AppResult<Json<Branding>> {
    user.require_admin()?;
    let branding = Branding {
        name: plain_text(&body.name, MAX_NAME_CHARS, "Panel adı")?.replace('\n', " "),
        logo: validate_logo(&body.logo)?,
        login_message: plain_text(&body.login_message, MAX_MESSAGE_CHARS, "Giriş sayfası metni")?,
    };
    state.db.set_setting(KEY, &serde_json::to_string(&branding).map_err(|err| AppError::internal(err.to_string()))?)?;
    auth::audit(&state.db, &user, "branding.update", &branding.name, "");
    Ok(Json(branding))
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
}
