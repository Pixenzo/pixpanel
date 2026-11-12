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

/// Bölge listesini ikincil ad sunucularına düzenli olarak yeniden iletir; kaçırılmış bir
/// eşitleme (ikincil o an kapalıydı) böylece kendiliğinden kapanır.
pub fn spawn_dns_cluster_sync(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            if crate::dns::cluster::secondaries(&state.db).iter().any(|item| !item.panel_url.is_empty()) {
                let _ = crate::dns::cluster::sync(&state).await;
            }
            crate::dns::refresh_signed_zones(&state).await;
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
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

/// WordPress sitelerini bekleyen güncellemeler için altı saatte bir tarar; ayar açıksa
/// gece aralığında güncellemeleri kurar.
pub fn spawn_wordpress_scan(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        let mut last_update_day = 0;
        let mut tick: u64 = 0;
        loop {
            if tick % 12 == 0 {
                crate::health::scan_wordpress(&state).await;
            }
            crate::health::update_wordpress_nightly(&state, &mut last_update_day).await;
            tick += 1;
            // Yarım saatte bir uyanmak, üç saatlik gece aralığını kaçırmamaya yeter.
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        }
    });
}

/// Giden posta sınırı (Postfix policy servisi) ve istemci otomatik ayarının nginx tarafı.
/// Not: `main.rs`e dokunmamak için `spawn_dkim_owner_sync` içinden başlatılır.
fn spawn_mail_depth(state: AppState) {
    crate::mail::autoconfig::init(&state.cfg);
    let owner_state = state.clone();
    let policy_state = state.clone();
    tokio::spawn(async move { crate::mail::policy::run(policy_state).await });
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(45)).await;
        loop {
            if crate::mail::is_installed() {
                // Yeni alınan sertifikalar ve eklenen/silinen siteler de nginx bloklarına yansısın diye
                // düzenli aralıkla yenilenir; içerik değişmediyse hiçbir şey yazılmaz.
                let _guard = state.sys_lock.lock().await;
                if let Err(err) = crate::mail::autoconfig::sync_nginx(&state.db).await {
                    tracing::warn!(error = %err, "posta istemcisi otomatik ayarı nginx yapılandırması yenilenemedi");
                }
                drop(_guard);
                // Karantinada 14 günden uzun kalan iletiler silinir (Postfix bekletme kuyruğu).
                match crate::mail::quarantine::purge_expired(now()).await {
                    Ok(0) => {}
                    Ok(count) => tracing::info!(count, "süresi dolan karantina iletileri silindi"),
                    Err(err) => tracing::warn!(error = %err, "karantina temizliği yapılamadı"),
                }
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
    // Kutu sahibi özellikleri (parola değiştirme, ManageSieve, panel betiği ayrımı): var olan kurulumlar
    // panel açılırken bir kez yeni düzene geçirilir; web postasının yapılandırması da yenilenir.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let _guard = owner_state.sys_lock.lock().await;
        if crate::dbtools::installed().roundcube {
            if let Err(err) = crate::dbtools::ensure_runtime(&owner_state.cfg).await {
                tracing::warn!(error = %err, "web postası yapılandırması yenilenemedi");
            }
        }
        if let Err(err) = crate::mail::upgrade_owner_features(&owner_state.db, &owner_state.cfg).await {
            tracing::warn!(error = %err, "posta sunucusu kutu sahibi özelliklerine geçirilemedi");
        }
    });
}

/// DKIM imza yetkisi eşlemesini güncel tutar: hesabın Linux kullanıcısı ilk sitesiyle oluşur,
/// o ana kadar eşlemede yer almaz. Dosya yalnızca içeriği değiştiyse yeniden yazılır.
pub fn spawn_dkim_owner_sync(state: AppState) {
    spawn_mail_depth(state.clone());
    tokio::spawn(async move {
        {
            let _guard = state.sys_lock.lock().await;
            if let Err(err) = crate::mail::upgrade_dkim_policy(&state.db).await {
                tracing::warn!(error = %err, "posta sunucusu DKIM imza yetkisi denetimine geçirilemedi");
            }
        }
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        loop {
            ticker.tick().await;
            if let Err(err) = crate::mail::write_dkim_owners(&state.db) {
                tracing::warn!(error = %err, "DKIM imza yetkisi eşlemesi yazılamadı");
            }
        }
    });
}

/// Hesapların disk kullanımını saatte bir ölçer (paketlerin disk kotası için).
pub fn spawn_disk_measurement(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            if let Err(err) = crate::quota::measure_disk(&state.db).await {
                tracing::warn!(error = %err, "disk kullanımı ölçülemedi");
            }
            if let Err(err) = crate::quota::measure_sites(&state.db).await {
                tracing::warn!(error = %err, "site boyutları ölçülemedi");
            }
            if let Err(err) = crate::quota::notify_disk(&state).await {
                tracing::warn!(error = %err, "disk kotası uyarıları gönderilemedi");
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

/// Site erişim günlüklerindeki yeni satırları beş dakikada bir istatistiklere işler.
pub fn spawn_stats_collector(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let collector = state.clone();
            // Günlük okumak ve veritabanına yazmak eşzamanlı işlerdir; istekleri bekletmesin.
            match tokio::task::spawn_blocking(move || crate::stats::collect(&collector)).await {
                Ok(Err(err)) => tracing::warn!(error = %err, "site istatistikleri toplanamadı"),
                Err(err) => tracing::warn!(error = %err, "istatistik görevi sonlandı"),
                Ok(Ok(())) => {}
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
}

/// Sunucu yükünün dakikalık ortalamasını kalıcı geçmişe yazar (uzun dönem grafikler için).
pub fn spawn_metrics_archive(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            // Örnekler iki saniyede bir alınır; son 30 örnek son dakikadır.
            let samples = state.metrics.history(30);
            if let Err(err) = crate::sys::metrics::archive_minute(&state.db, &samples) {
                tracing::warn!(error = %err, "sunucu yükü geçmişe yazılamadı");
            }
        }
    });
}

/// İşletim geçmişi paketi: servis ölçümleri, haftalık özet, kara liste denetimi ve veritabanı
/// çöp kutusunun temizliği.
pub fn spawn_ops_history(state: AppState) {
    crate::svchist::spawn(state.clone());
    crate::weekly::spawn(state.clone());
    crate::blacklist::spawn(state.clone());
    crate::dbtrash::spawn_cleanup(state);
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
