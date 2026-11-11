//! Sunucu bileşenlerinin listelenmesi ve kurulması. Yalnızca yöneticiler.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::components::{self, ComponentStatus};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list)).route("/{id}/install", post(install))
}

async fn list(user: AuthUser) -> AppResult<Json<Vec<ComponentStatus>>> {
    user.require_admin()?;
    Ok(Json(components::status_all().await))
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
