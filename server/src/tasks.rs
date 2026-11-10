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
            tokio::time::sleep(CHECK_INTERVAL).await;
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
            "letsencrypt" => {
                tracing::info!(domain = %site.domain, "sertifika yenileniyor");
                start_issue_job(state, None, site.id, &site.domain, names)?;
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
