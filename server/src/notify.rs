//! Bildirimler: e-posta (SMTP) ve Telegram.
//!
//! SMTP sunucusunu yönetici bir kez tanımlar; her hesap kendi alıcı adresini,
//! Telegram botunu ve hangi olaylarda haber almak istediğini kendisi seçer.

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Bildirim gönderilebilen olaylar ve arayüzde gösterilen adları.
pub const EVENTS: &[(&str, &str)] = &[
    ("deploy_failed", "Yayınlama başarısız olduğunda"),
    ("deploy_success", "Yayınlama başarıyla bittiğinde"),
    ("ssl_failed", "SSL sertifikası yenilenemediğinde"),
    ("login_new_ip", "Hesabınıza yeni bir adresten giriş yapıldığında"),
    ("backup_failed", "Yedekleme başarısız olduğunda"),
    ("health_alert", "Sunucuda dikkat gerektiren bir sorun çıktığında ya da panel bir sorunu kendiliğinden düzelttiğinde"),
];

const SMTP_KEY: &str = "smtp";
const SEND_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SmtpSettings {
    pub host: String,
    pub port: u16,
    /// `starttls`, `tls` ya da `none`.
    pub security: String,
    pub username: String,
    pub password: String,
    /// Gönderen adresi (ör. `PixPanel <panel@ornek.com>`).
    pub from: String,
}

impl SmtpSettings {
    pub fn configured(&self) -> bool {
        !self.host.is_empty() && !self.from.is_empty()
    }

    pub fn validate(&self) -> AppResult<()> {
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && self.host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
        if !host_ok {
            return Err(AppError::bad("Geçersiz SMTP sunucu adresi"));
        }
        if self.port == 0 {
            return Err(AppError::bad("Geçersiz SMTP portu"));
        }
        if !matches!(self.security.as_str(), "starttls" | "tls" | "none") {
            return Err(AppError::bad("Geçersiz bağlantı güvenliği"));
        }
        self.from.parse::<Mailbox>().map_err(|_| AppError::bad("Geçersiz gönderen adresi (örnek: panel@ornek.com)"))?;
        Ok(())
    }
}

pub fn load_smtp(db: &Db) -> AppResult<SmtpSettings> {
    Ok(db.setting(SMTP_KEY)?.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default())
}

pub fn save_smtp(db: &Db, settings: &SmtpSettings) -> AppResult<()> {
    let encoded = serde_json::to_string(settings).map_err(|err| AppError::internal(err.to_string()))?;
    db.set_setting(SMTP_KEY, &encoded)
}

/// Bir hesabın bildirim tercihleri.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Channels {
    pub email_enabled: bool,
    pub email_to: String,
    pub telegram_enabled: bool,
    pub telegram_token: String,
    pub telegram_chat_id: String,
    pub events: Vec<String>,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            email_enabled: false,
            email_to: String::new(),
            telegram_enabled: false,
            telegram_token: String::new(),
            telegram_chat_id: String::new(),
            events: vec!["deploy_failed".into(), "ssl_failed".into()],
        }
    }
}

impl Channels {
    pub fn validate(&self) -> AppResult<()> {
        if self.email_enabled || !self.email_to.is_empty() {
            self.email_to.parse::<Mailbox>().map_err(|_| AppError::bad("Geçersiz e-posta adresi"))?;
        }
        if self.telegram_enabled || !self.telegram_token.is_empty() {
            validate_telegram_token(&self.telegram_token)?;
        }
        if self.telegram_enabled || !self.telegram_chat_id.is_empty() {
            let id = self.telegram_chat_id.as_str();
            let numeric = id.strip_prefix('-').unwrap_or(id);
            let chat_ok = (!numeric.is_empty() && numeric.len() <= 20 && numeric.bytes().all(|b| b.is_ascii_digit()))
                || (id.starts_with('@') && id.len() <= 64 && id[1..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
            if !chat_ok {
                return Err(AppError::bad("Geçersiz Telegram sohbet kimliği (örnek: 123456789 ya da @kanal)"));
            }
        }
        if self.events.iter().any(|event| !EVENTS.iter().any(|(id, _)| id == event)) {
            return Err(AppError::bad("Bilinmeyen bildirim olayı"));
        }
        Ok(())
    }
}

/// Bot anahtarı adresin içine yazıldığından biçimi sıkı denetlenir.
fn validate_telegram_token(token: &str) -> AppResult<()> {
    let valid = token.split_once(':').is_some_and(|(id, secret)| {
        !id.is_empty()
            && id.len() <= 12
            && id.bytes().all(|b| b.is_ascii_digit())
            && (30..=64).contains(&secret.len())
            && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz Telegram bot anahtarı")) }
}

pub fn load_channels(db: &Db, user_id: i64) -> AppResult<Channels> {
    let raw: Option<String> = db.call(|conn| {
        conn.query_row("SELECT config FROM user_notify WHERE user_id = ?1", params![user_id], |row| row.get(0)).optional()
    })?;
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default())
}

pub fn save_channels(db: &Db, user_id: i64, channels: &Channels) -> AppResult<()> {
    let encoded = serde_json::to_string(channels).map_err(|err| AppError::internal(err.to_string()))?;
    db.call(|conn| {
        conn.execute(
            "INSERT INTO user_notify (user_id, config) VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET config = excluded.config",
            params![user_id, encoded],
        )
    })?;
    Ok(())
}

pub async fn send_email(smtp: &SmtpSettings, to: &str, subject: &str, body: &str) -> AppResult<()> {
    if !smtp.configured() {
        return Err(AppError::bad("E-posta gönderimi için yöneticinin SMTP sunucusunu tanımlaması gerekiyor"));
    }
    let failed = |err: &dyn std::fmt::Display| AppError::bad(format!("E-posta gönderilemedi: {err}"));
    let builder = match smtp.security.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host).map_err(|err| failed(&err))?,
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host).map_err(|err| failed(&err))?,
        _ => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host),
    };
    let mut builder = builder.port(smtp.port).timeout(Some(SEND_TIMEOUT));
    if !smtp.username.is_empty() {
        builder = builder.credentials(Credentials::new(smtp.username.clone(), smtp.password.clone()));
    }
    let message = Message::builder()
        .from(smtp.from.parse().map_err(|_| AppError::bad("Gönderen adresi geçersiz"))?)
        .to(to.parse().map_err(|_| AppError::bad("Alıcı adresi geçersiz"))?)
        .subject(subject)
        .body(body.to_string())
        .map_err(|err| failed(&err))?;
    builder.build().send(message).await.map_err(|err| failed(&err))?;
    Ok(())
}

pub async fn send_telegram(api_base: &str, token: &str, chat_id: &str, text: &str) -> AppResult<()> {
    validate_telegram_token(token)?;
    let failed = |detail: String| AppError::bad(format!("Telegram iletisi gönderilemedi: {detail}"));
    let client = reqwest::Client::builder().timeout(SEND_TIMEOUT).build().map_err(|err| failed(err.to_string()))?;
    // Telegram tek iletide en fazla 4096 karakter kabul eder.
    let text: String = text.chars().take(3900).collect();
    let response = client
        .post(format!("{}/bot{token}/sendMessage", api_base.trim_end_matches('/')))
        .json(&json!({ "chat_id": chat_id, "text": text, "disable_web_page_preview": true }))
        .send()
        .await
        // Hata iletisindeki adres bot anahtarını içerir; günlüğe ve kullanıcıya sızmamalı.
        .map_err(|err| failed(err.without_url().to_string()))?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let description = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| body.get("description").and_then(|d| d.as_str()).map(str::to_string))
        .unwrap_or_default();
    Err(failed(format!("{status} {description}")))
}

/// Olayı, hesabın açık kanallarına gönderir. Gönderim hataları yalnızca günlüğe
/// yazılır; bildirim gidememesi asıl işi başarısız saymaz.
pub async fn send_event(state: &AppState, user_id: i64, event: &str, subject: &str, body: &str) {
    let channels = match load_channels(&state.db, user_id) {
        Ok(channels) => channels,
        Err(err) => {
            tracing::warn!(error = %err, "bildirim tercihleri okunamadı");
            return;
        }
    };
    if !channels.events.iter().any(|enabled| enabled == event) {
        return;
    }
    if channels.email_enabled {
        let smtp = load_smtp(&state.db).unwrap_or_default();
        if let Err(err) = send_email(&smtp, &channels.email_to, subject, body).await {
            tracing::warn!(error = %err, user_id, event, "e-posta bildirimi gönderilemedi");
        }
    }
    if channels.telegram_enabled {
        let text = format!("{subject}\n\n{body}");
        if let Err(err) = send_telegram(&state.cfg.telegram_api, &channels.telegram_token, &channels.telegram_chat_id, &text).await {
            tracing::warn!(error = %err, user_id, event, "Telegram bildirimi gönderilemedi");
        }
    }
}
