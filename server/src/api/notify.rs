//! Bildirim tercihleri (her hesap için) ve SMTP sunucusu (yönetici için).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::notify::{self, Channels, SmtpSettings};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(show).put(save))
        .route("/test", post(test))
        .route("/smtp", get(show_smtp).put(save_smtp))
        .route("/smtp/test", post(test_smtp))
}

#[derive(Serialize)]
struct EventOption {
    id: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
struct View {
    email_enabled: bool,
    email_to: String,
    telegram_enabled: bool,
    /// Anahtarın kendisi geri gönderilmez; yalnızca tanımlı olup olmadığı.
    telegram_token_set: bool,
    telegram_chat_id: String,
    events: Vec<String>,
    available_events: Vec<EventOption>,
    smtp_configured: bool,
}

fn view(state: &AppState, channels: Channels) -> AppResult<View> {
    Ok(View {
        email_enabled: channels.email_enabled,
        email_to: channels.email_to,
        telegram_enabled: channels.telegram_enabled,
        telegram_token_set: !channels.telegram_token.is_empty(),
        telegram_chat_id: channels.telegram_chat_id,
        events: channels.events,
        available_events: notify::EVENTS.iter().map(|(id, label)| EventOption { id, label }).collect(),
        smtp_configured: notify::load_smtp(&state.db)?.configured(),
    })
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    let mut channels = notify::load_channels(&state.db, user.id)?;
    if channels.email_to.is_empty() {
        channels.email_to = user.email.clone();
    }
    Ok(Json(view(&state, channels)?))
}

#[derive(Deserialize)]
struct SaveBody {
    email_enabled: bool,
    email_to: String,
    telegram_enabled: bool,
    /// Verilmezse kayıtlı anahtar korunur.
    telegram_token: Option<String>,
    telegram_chat_id: String,
    events: Vec<String>,
}

async fn save(State(state): State<AppState>, user: AuthUser, Json(body): Json<SaveBody>) -> AppResult<Json<View>> {
    let current = notify::load_channels(&state.db, user.id)?;
    let channels = Channels {
        email_enabled: body.email_enabled,
        email_to: body.email_to.trim().to_string(),
        telegram_enabled: body.telegram_enabled,
        telegram_token: body.telegram_token.map(|token| token.trim().to_string()).unwrap_or(current.telegram_token),
        telegram_chat_id: body.telegram_chat_id.trim().to_string(),
        events: body.events,
    };
    channels.validate()?;
    notify::save_channels(&state.db, user.id, &channels)?;
    auth::audit(&state.db, &user, "notify.update", "", "");
    Ok(Json(view(&state, channels)?))
}

#[derive(Deserialize)]
struct TestBody {
    channel: String,
}

/// Kayıtlı ayarlarla bir deneme iletisi gönderir; hata olursa nedenini döner.
async fn test(State(state): State<AppState>, user: AuthUser, Json(body): Json<TestBody>) -> AppResult<Json<serde_json::Value>> {
    let channels = notify::load_channels(&state.db, user.id)?;
    let subject = "PixPanel deneme bildirimi";
    let text = "Bu ileti bildirim ayarlarınızı sınamak için gönderildi. Ulaştıysa her şey yolunda.";
    // Deneme iletisi de hesabın dilinde gider.
    let language = crate::i18n::normalize(&user.language);
    match body.channel.as_str() {
        "email" => {
            let to = if channels.email_to.is_empty() { user.email.clone() } else { channels.email_to };
            if to.is_empty() {
                return Err(AppError::bad("Önce bir e-posta adresi kaydedin"));
            }
            notify::send_email(&notify::load_smtp(&state.db)?, &to, &crate::i18n::translate(language, subject), &crate::i18n::translate(language, text)).await?;
        }
        "telegram" => {
            if channels.telegram_token.is_empty() || channels.telegram_chat_id.is_empty() {
                return Err(AppError::bad("Önce bot anahtarını ve sohbet kimliğini kaydedin"));
            }
            let message = format!("{}\n\n{}", crate::i18n::translate(language, subject), crate::i18n::translate(language, text));
            notify::send_telegram(&state.cfg.telegram_api, &channels.telegram_token, &channels.telegram_chat_id, &message).await?;
        }
        _ => return Err(AppError::bad("Geçersiz bildirim kanalı")),
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
struct SmtpView {
    host: String,
    port: u16,
    security: String,
    username: String,
    password_set: bool,
    from: String,
}

fn smtp_view(settings: SmtpSettings) -> SmtpView {
    SmtpView {
        host: settings.host,
        port: if settings.port == 0 { 587 } else { settings.port },
        security: if settings.security.is_empty() { "starttls".into() } else { settings.security },
        username: settings.username,
        password_set: !settings.password.is_empty(),
        from: settings.from,
    }
}

async fn show_smtp(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<SmtpView>> {
    user.require_admin()?;
    Ok(Json(smtp_view(notify::load_smtp(&state.db)?)))
}

#[derive(Deserialize)]
struct SmtpBody {
    host: String,
    port: u16,
    security: String,
    username: String,
    /// Verilmezse kayıtlı parola korunur.
    password: Option<String>,
    from: String,
}

async fn save_smtp(State(state): State<AppState>, user: AuthUser, Json(body): Json<SmtpBody>) -> AppResult<Json<SmtpView>> {
    user.require_admin()?;
    let current = notify::load_smtp(&state.db)?;
    let settings = SmtpSettings {
        host: body.host.trim().to_string(),
        port: body.port,
        security: body.security,
        username: body.username.trim().to_string(),
        password: body.password.unwrap_or(current.password),
        from: body.from.trim().to_string(),
    };
    settings.validate()?;
    notify::save_smtp(&state.db, &settings)?;
    auth::audit(&state.db, &user, "smtp.update", &settings.host, "");
    Ok(Json(smtp_view(settings)))
}

#[derive(Deserialize)]
struct SmtpTestBody {
    to: String,
}

async fn test_smtp(State(state): State<AppState>, user: AuthUser, Json(body): Json<SmtpTestBody>) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let smtp = notify::load_smtp(&state.db)?;
    let language = crate::i18n::normalize(&user.language);
    notify::send_email(&smtp, body.to.trim(), &crate::i18n::translate(language, "PixPanel SMTP denemesi"), &crate::i18n::translate(language, "SMTP ayarlarınız çalışıyor.")).await?;
    Ok(Json(json!({ "ok": true })))
}
