//! Panelin kendi kendine yürüttüğü zamanlanmış işler.

use std::time::Duration;

use crate::api::ssl::start_issue_job;
use crate::db::now;
use crate::error::AppResult;
use crate::hosting::{sites, ssl};
use crate::state::AppState;

/// Bitimine bu kadar süre kalan sertifikalar yenilenir.
const RENEW_BEFORE_SECS: i64 = 30 * 86400;
const CHECK_INTERVAL: Duration = Duration::from_secs(12 * 3600);
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(120);

/// Süresi yaklaşan sertifikaları günde iki kez denetleyip yeniler.
pub fn spawn_certificate_renewal(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if let Err(err) = renew_due_certificates(&state).await {
                tracing::error!(error = %err, "sertifika yenileme denetimi başarısız");
            }
            if let Err(err) = crate::panelcert::renew_if_due(&state, RENEW_BEFORE_SECS).await {
                tracing::error!(error = %err, "panel sertifikası yenilenemedi");
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Veritabanı araçları için açılan geçici hesapları süreleri dolunca siler.
pub fn spawn_db_tool_cleanup(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(err) = crate::dbtools::cleanup(&state.db).await {
                tracing::warn!(error = %err, "geçici veritabanı hesapları temizlenemedi");
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}

/// Günde bir kez yeni sürüm yayınlanıp yayınlanmadığına bakar (yalnızca bildirir, kurmaz).
pub fn spawn_update_check(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(300)).await;
        loop {
            if let Err(err) = crate::update::check(&state).await {
                tracing::warn!(error = %err, "sürüm denetimi yapılamadı");
            }
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    });
}

/// Kendi kendine bakım denetimlerini düzenli çalıştırır (bkz. `health`).
pub fn spawn_health_monitor(state: AppState) {
    tokio::spawn(async move {
        // Açılışta servislerin ayağa kalkması beklenir; yoksa hepsi "çalışmıyor" görünür.
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            crate::health::run_checks(&state).await;
            tokio::time::sleep(crate::health::CHECK_INTERVAL).await;
        }
    });
}

/// WordPress sitelerini bekleyen güncellemeler için altı saatte bir tarar.
pub fn spawn_wordpress_scan(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            crate::health::scan_wordpress(&state).await;
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

/// Zamanı gelen yedekleme planlarını başlatır.
pub fn spawn_backup_scheduler(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(45)).await;
        loop {
            if let Err(err) = crate::backup::run::start_due_plans(&state).await {
                tracing::error!(error = %err, "yedekleme planları denetlenemedi");
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

async fn renew_due_certificates(state: &AppState) -> AppResult<()> {
    for site in sites::list_with_ssl(&state.db)? {
        let expires_at = ssl::read_info(&site.domain).map(|info| info.not_after).unwrap_or(0);
        if expires_at - now() > RENEW_BEFORE_SECS {
            continue;
        }
        let mut names = vec![site.domain.clone()];
        names.extend(site.aliases.iter().cloned());
        match site.ssl_mode.as_str() {
            mode @ ("letsencrypt" | "letsencrypt_dns" | "letsencrypt_wildcard") => {
                tracing::info!(domain = %site.domain, "sertifika yenileniyor");
                // Sertifika, ilk alındığı doğrulama yöntemiyle yenilenir.
                let mode: &'static str = match mode {
                    "letsencrypt_dns" => "letsencrypt_dns",
                    "letsencrypt_wildcard" => "letsencrypt_wildcard",
                    _ => "letsencrypt",
                };
                let names = crate::api::ssl::issue_names(&site, mode, true);
                start_issue_job(state, None, site.id, &site.domain, names, mode)?;
            }
            "selfsigned" => {
                let _guard = state.sys_lock.lock().await;
                let (certificate, private_key) = ssl::generate_self_signed(&names)?;
                if let Err(err) = sites::activate_ssl(&state.db, &site, "selfsigned", &certificate, &private_key).await {
                    tracing::error!(error = %err, domain = %site.domain, "kendinden imzalı sertifika yenilenemedi");
                }
            }
            // Yüklenen sertifikaları yalnızca sahibi yenileyebilir.
            _ => {}
        }
    }
    Ok(())
}
