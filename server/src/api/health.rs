//! Sağlık merkezi uçları: bulgular, tek tıkla düzeltme ve kendi kendine bakım
//! ayarları. Yalnızca yöneticiler.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{self, AuthUser};
use crate::error::AppResult;
use crate::health::{self, Action, Report, Settings};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(show))
        .route("/check", post(check))
        .route("/fix", post(fix))
        .route("/settings", axum::routing::put(save_settings))
        .route("/weekly-report", get(show_weekly).put(save_weekly))
        .route("/weekly-report/send", post(send_weekly))
        .route("/blacklist", get(show_blacklist))
        .route("/blacklist/check", post(check_blacklist))
}

#[derive(Serialize)]
struct View {
    report: Report,
    settings: Settings,
    /// Son yapılan düzeltmeler (yeniden eskiye).
    actions: Vec<Action>,
    /// Denetimler bu aralıkla yinelenir (saniye).
    interval_secs: u64,
}

fn view(state: &AppState) -> AppResult<View> {
    Ok(View {
        report: state.health.report(),
        settings: health::load_settings(state),
        actions: health::recent_actions(state, 30)?,
        interval_secs: health::CHECK_INTERVAL.as_secs(),
    })
}

async fn show(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    Ok(Json(view(&state)?))
}

/// Denetimleri beklemeden hemen yeniden çalıştırır.
async fn check(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<View>> {
    user.require_admin()?;
    health::run_checks(&state).await;
    Ok(Json(view(&state)?))
}

#[derive(Deserialize)]
struct FixBody {
    key: String,
}

async fn fix(State(state): State<AppState>, user: AuthUser, Json(body): Json<FixBody>) -> AppResult<Json<Value>> {
    user.require_admin()?;
    let message = health::fix_now(&state, &body.key, user.id).await?;
    auth::audit(&state.db, &user, "health.fix", &body.key, "");
    // Düzeltmenin sonucu bir sonraki turu beklemeden ekrana yansısın.
    let recheck = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        health::run_checks(&recheck).await;
    });
    Ok(Json(json!({ "message": message })))
}

async fn save_settings(State(state): State<AppState>, user: AuthUser, Json(settings): Json<Settings>) -> AppResult<Json<View>> {
    user.require_admin()?;
    health::save_settings(&state, &settings)?;
    // Paket kurulumu gerekebilir; isteği bekletmeden arka planda uygulanır.
    let apply = state.clone();
    let security_updates = settings.security_updates;
    tokio::spawn(async move { health::apply_security_updates(&apply, security_updates).await });
    auth::audit(&state.db, &user, "health.settings", "", "");
    Ok(Json(view(&state)?))
}

// ---- Haftalık özet ----

#[derive(Serialize)]
struct WeeklyView {
    settings: crate::weekly::Settings,
    /// Bir sonraki otomatik gönderim (Unix saniyesi); özet kapalıysa yok.
    next_run_at: Option<i64>,
    last: crate::weekly::Last,
    smtp_configured: bool,
}

async fn weekly_view(state: &AppState) -> WeeklyView {
    let settings = crate::weekly::load_settings(&state.db);
    let next_run_at = if settings.enabled {
        Some(crate::weekly::next_run(settings.weekday, settings.at_minute, crate::db::now(), crate::backup::local_offset().await))
    } else {
        None
    };
    WeeklyView {
        next_run_at,
        last: crate::weekly::load_last(&state.db),
        smtp_configured: crate::notify::load_smtp(&state.db).map(|smtp| smtp.configured()).unwrap_or(false),
        settings,
    }
}

async fn show_weekly(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<WeeklyView>> {
    user.require_admin()?;
    Ok(Json(weekly_view(&state).await))
}

async fn save_weekly(State(state): State<AppState>, user: AuthUser, Json(settings): Json<crate::weekly::Settings>) -> AppResult<Json<WeeklyView>> {
    user.require_admin()?;
    crate::weekly::save_settings(&state.db, &settings)?;
    auth::audit(&state.db, &user, "health.weekly_report.settings", "", if settings.enabled { "açık" } else { "kapalı" });
    Ok(Json(weekly_view(&state).await))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SendBody {
    /// Doğruysa hiçbir yere gönderilmez; yalnızca özetin metni döner.
    preview: bool,
}

/// Haftalık özeti şimdi üretir ve (önizleme değilse) bildirim kanallarına gönderir.
async fn send_weekly(State(state): State<AppState>, user: AuthUser, body: Option<Json<SendBody>>) -> AppResult<Json<crate::weekly::Delivery>> {
    user.require_admin()?;
    let preview = body.map(|Json(body)| body.preview).unwrap_or(false);
    // Önizleme isteği yapan yöneticinin dilindedir; gönderimde her alıcı kendi dilinde alır.
    let delivery = crate::weekly::build(&state, !preview, crate::i18n::normalize(&user.language)).await?;
    if !preview {
        auth::audit(&state.db, &user, "health.weekly_report.send", "", &format!("{} e-posta, {} Telegram", delivery.emailed.len(), delivery.telegram));
    }
    Ok(Json(delivery))
}

// ---- Kara liste ----

#[derive(Serialize)]
struct BlacklistView {
    mail_installed: bool,
    lists: Vec<&'static str>,
    #[serde(flatten)]
    outcome: crate::blacklist::Outcome,
}

fn blacklist_view(state: &AppState, outcome: Option<crate::blacklist::Outcome>) -> BlacklistView {
    BlacklistView {
        mail_installed: crate::mail::is_installed(),
        lists: crate::blacklist::LISTS.iter().map(|(name, _)| *name).collect(),
        outcome: outcome.unwrap_or_else(|| crate::blacklist::load(&state.db)),
    }
}

async fn show_blacklist(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<BlacklistView>> {
    user.require_admin()?;
    Ok(Json(blacklist_view(&state, None)))
}

/// Kara liste sorgusunu şimdi çalıştırır (posta bileşeni kurulu değilse atlanır).
async fn check_blacklist(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<BlacklistView>> {
    user.require_admin()?;
    if !crate::mail::is_installed() {
        return Ok(Json(blacklist_view(&state, None)));
    }
    let outcome = crate::blacklist::run(&state.db).await?;
    auth::audit(&state.db, &user, "health.blacklist.check", "", &format!("{} listelenmiş", outcome.listed().len()));
    health::run_checks(&state).await;
    Ok(Json(blacklist_view(&state, Some(outcome))))
}
