//! Sunucu bileşenlerinin listelenmesi ve kurulması. Yalnızca yöneticiler.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::components::{self, ComponentStatus};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list)).route("/{id}/install", post(install)).route("/{id}/remove", post(remove))
}

#[derive(Serialize)]
struct ComponentView {
    #[serde(flatten)]
    status: ComponentStatus,
    /// Panelden kaldırılabilen türden mi?
    removable: bool,
    /// Kuruluysa ve kullanımdaysa kaldırılamama nedeni.
    in_use: Option<String>,
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<ComponentView>>> {
    user.require_admin()?;
    let mut list = Vec::new();
    for status in components::status_all().await {
        let removable = components::removable(&status.id);
        let in_use = if removable && status.installed { components::removal_blocker(&state.db, &components::find(&status.id)?).await? } else { None };
        list.push(ComponentView { status, removable, in_use });
    }
    Ok(Json(list))
}

/// Bileşeni kaldırır (`apt-get remove`; yapılandırma korunur). Kullanımdaki bileşen 409 döner.
async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let component = components::find(&id)?;
    if !components::removable(&id) {
        return Err(AppError::bad(format!("{} panelden kaldırılamaz; veri kaybına ya da kesintiye yol açabilir", component.name)));
    }
    if !components::status(&component).await.installed {
        return Err(AppError::conflict(format!("{} zaten kurulu değil", component.name)));
    }
    if let Some(reason) = components::removal_blocker(&state.db, &component).await? {
        return Err(AppError::conflict(reason));
    }
    let apt_lock = state.apt_lock.clone();
    let job_state = state.clone();
    let title = format!("{} kaldırma", component.name);
    let job_id = state.jobs.start("component.remove", &title, Some(user.id), move |ctx| async move {
        if apt_lock.try_lock().is_err() {
            ctx.log("Başka bir paket işleminin bitmesi bekleniyor");
        }
        let _guard = apt_lock.lock().await;
        // Beklerken kullanım durumu değişmiş olabilir (ör. siteye bu PHP sürümü atanmış).
        if let Some(reason) = components::removal_blocker(&job_state.db, &component).await? {
            return Err(AppError::conflict(reason));
        }
        components::remove(&ctx, &component).await?;
        if matches!(component.id.as_str(), "phpmyadmin" | "adminer" | "roundcube") {
            ctx.log("Araçların panel ortamı temizleniyor");
            let _sys = job_state.sys_lock.lock().await;
            crate::dbtools::after_removal(&job_state.cfg).await?;
        }
        if component.id.starts_with("php-") {
            // Kalan PHP havuzlarının port kısıtı yeniden uygulanır.
            crate::hosting::portguard::apply().await;
        }
        // Kaldırılan bileşenin hapsi ve güvenlik duvarı kümesi artık geçerli değil.
        if component.id != "fail2ban" && crate::fail2ban::is_installed() {
            if let Err(err) = crate::fail2ban::apply(&job_state.db, &crate::fail2ban::load(&job_state.db)).await {
                ctx.log(format!("Uyarı: fail2ban yapılandırması güncellenemedi: {err}"));
            }
        }
        if let Err(err) = crate::firewall::reapply(&job_state).await {
            ctx.log(format!("Uyarı: güvenlik duvarı kuralları güncellenemedi: {err}"));
        }
        Ok(())
    })?;
    auth::audit(&state.db, &user, "component.remove", &id, "");
    Ok(Json(json!({ "job_id": job_id })))
}

async fn install(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let component = components::find(&id)?;
    if components::status(&component).await.installed {
        return Err(AppError::conflict(format!("{} zaten kurulu", component.name)));
    }
    let apt_lock = state.apt_lock.clone();
    let job_state = state.clone();
    let title = format!("{} kurulumu", component.name);
    let job_id = state.jobs.start("component.install", &title, Some(user.id), move |ctx| async move {
        // Aynı anda yalnızca bir paket işlemi yürür; diğerleri sırasını bekler.
        if apt_lock.try_lock().is_err() {
            ctx.log("Başka bir kurulumun bitmesi bekleniyor");
        }
        let _guard = apt_lock.lock().await;
        if component.id == "mail" {
            // Postfix kurulurken soru sormasın ve internet sunucusu olarak yapılandırılsın.
            let answers = format!(
                "postfix postfix/main_mailer_type select Internet Site
postfix postfix/mailname string {}
",
                crate::mail::system_hostname()
            );
            let mut preseed = crate::sys::exec::base_command("debconf-set-selections");
            ctx.run_streaming(&mut preseed, "debconf-set-selections", Some(answers.into_bytes())).await?;
        }
        components::install(&ctx, &component).await?;
        if matches!(component.id.as_str(), "phpmyadmin" | "adminer" | "roundcube") {
            ctx.log("Araç panel üzerinden erişime hazırlanıyor");
            let _sys = job_state.sys_lock.lock().await;
            crate::dbtools::ensure_runtime(&job_state.cfg).await?;
        }
        if component.id.starts_with("php-") {
            // Yeni PHP havuz servisi de uygulama portlarını bağlayamamalı.
            crate::hosting::portguard::apply().await;
        }
        if component.id == "fail2ban" {
            ctx.log("fail2ban yapılandırılıyor");
            crate::fail2ban::apply(&job_state.db, &crate::fail2ban::load(&job_state.db)).await?;
        }
        if component.id == "dns" {
            ctx.log("DNS sunucusu yapılandırılıyor");
            crate::dns::local::setup(&crate::dns::cluster::transfer(&job_state.db)).await?;
        }
        if component.id == "ftp" {
            ctx.log("FTP sunucusu yapılandırılıyor");
            crate::hosting::ftp::setup(&job_state.cfg).await?;
        }
        if component.id == "mail" {
            ctx.log("Posta servisleri yapılandırılıyor");
            let _sys = job_state.sys_lock.lock().await;
            crate::mail::setup(&job_state.db, &job_state.cfg).await?;
            ctx.log("Posta sunucusu hazır");
        }
        // Yeni bileşen için bir hapis varsa (ör. FTP, posta) fail2ban onu izlemeye başlar.
        if component.id != "fail2ban" && crate::fail2ban::is_installed() {
            if let Err(err) = crate::fail2ban::apply(&job_state.db, &crate::fail2ban::load(&job_state.db)).await {
                ctx.log(format!("Uyarı: fail2ban yapılandırması güncellenemedi: {err}"));
            }
        }
        // Yeni bileşenin portları güvenlik duvarında kendiliğinden açılır.
        if let Err(err) = crate::firewall::reapply(&job_state).await {
            ctx.log(format!("Uyarı: güvenlik duvarı kuralları güncellenemedi: {err}"));
        }
        Ok(())
    })?;
    auth::audit(&state.db, &user, "component.install", &id, "");
    Ok(Json(json!({ "job_id": job_id })))
}
