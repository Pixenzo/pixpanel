//! Panelin kendi sertifikası.
//!
//! Panel ilk açılışta kendinden imzalı bir sertifika üretir. Yönetici panele
//! bir alan adı verip Let's Encrypt sertifikası alabilir ya da kendi
//! sertifikasını yükleyebilir. Yeni sertifika panel yeniden başlatılmadan
//! devreye girer; posta servisleri ve FTP de aynı sertifikayı kullanır.

use std::path::PathBuf;

use crate::config::Config;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::ssl::{self, CertInfo};
use crate::hosting::{acme, atomic_write, domain, ftp};
use crate::state::AppState;
use crate::sys::{exec, services};

const HOSTNAME_KEY: &str = "panel.hostname";
const MODE_KEY: &str = "panel.cert_mode";

pub fn paths(cfg: &Config) -> (PathBuf, PathBuf) {
    (cfg.tls_dir().join("panel.crt"), cfg.tls_dir().join("panel.key"))
}

/// Panelin alan adı (ayarlanmamışsa boş).
pub fn hostname(db: &Db) -> String {
    db.setting(HOSTNAME_KEY).ok().flatten().unwrap_or_default()
}

/// `selfsigned`, `letsencrypt`, `letsencrypt_dns` ya da `custom`.
pub fn mode(db: &Db) -> String {
    db.setting(MODE_KEY).ok().flatten().unwrap_or_else(|| "selfsigned".into())
}

pub fn info(cfg: &Config) -> Option<CertInfo> {
    std::fs::read(paths(cfg).0).ok().and_then(|pem| ssl::parse_certificate(&pem).ok())
}

/// Sertifikayı kurar: dosyaları yazar, paneli yeni sertifikaya geçirir ve aynı
/// sertifikayı kullanan servisleri yeniden yükler. Panel yeni sertifikayı
/// yükleyemezse eski dosyalar geri konur.
pub async fn install(state: &AppState, hostname: &str, certificate: &str, private_key: &str, mode: &str) -> AppResult<()> {
    let (cert_path, key_path) = paths(&state.cfg);
    let previous = (std::fs::read(&cert_path).ok(), std::fs::read(&key_path).ok());
    atomic_write(&cert_path, certificate.as_bytes(), 0o644)?;
    atomic_write(&key_path, private_key.as_bytes(), 0o600)?;

    if let Some(tls) = &state.tls {
        if let Err(err) = tls.reload_from_pem_file(&cert_path, &key_path).await {
            if let (Some(cert), Some(key)) = previous {
                atomic_write(&cert_path, &cert, 0o644)?;
                atomic_write(&key_path, &key, 0o600)?;
                let _ = tls.reload_from_pem_file(&cert_path, &key_path).await;
            }
            return Err(AppError::bad(format!("Sertifika yüklenemedi: {err}")));
        }
    }
    state.db.set_setting(HOSTNAME_KEY, hostname)?;
    state.db.set_setting(MODE_KEY, mode)?;
    reload_dependents(state).await;
    Ok(())
}

/// Panelin sertifikasını kullanan servisleri (posta, FTP) yeni sertifikaya geçirir.
/// Buradaki hatalar sertifika kurulumunu başarısız saymaz; günlüğe yazılır.
async fn reload_dependents(state: &AppState) {
    if crate::mail::is_installed() {
        for (service, program, args) in [("postfix.service", "postfix", ["reload"]), ("dovecot.service", "doveadm", ["reload"])] {
            if services::is_active(service).await {
                if let Err(err) = exec::run_ok(program, &args).await {
                    tracing::warn!(error = %err, service, "servis yeni sertifikaya geçirilemedi");
                }
            }
        }
    }
    if ftp::is_installed() {
        if let Err(err) = ftp::install_certificate(&state.cfg).await {
            tracing::warn!(error = %err, "FTP sunucusu yeni sertifikaya geçirilemedi");
        }
    }
    // Uzaktan erişimi açık MariaDB yeni sertifikayı yeniden başlamadan yükler.
    crate::dbms::remote::refresh_mariadb_tls(&state.db, &state.cfg).await;
}

pub fn validate_hostname(value: &str) -> AppResult<String> {
    domain::normalize(value)
}

/// Let's Encrypt sertifikası alma görevini başlatır.
///
/// `dns` verilmezse doğrulama web üzerinden yapılır (nginx'in varsayılan
/// sunucusu doğrulama dosyasını her alan adı için sunar); verilirse alan adının
/// panelde yönetilen DNS bölgesine kayıt yazılır.
pub fn start_issue(state: &AppState, user_id: Option<i64>, zone_owner: i64, hostname: String, dns: bool) -> AppResult<i64> {
    let title = format!("Panel sertifikası: {hostname}");
    let state = state.clone();
    let jobs = state.jobs.clone();
    jobs.start("panel.certificate", &title, user_id, move |ctx| async move {
        let solver = dns.then(|| crate::dns::AcmeSolver::new(state.clone(), zone_owner));
        let (chain, private_key) = acme::issue(&ctx, &state.db, &state.cfg, std::slice::from_ref(&hostname), solver).await?;
        let _guard = state.sys_lock.lock().await;
        install(&state, &hostname, &chain, &private_key, if dns { "letsencrypt_dns" } else { "letsencrypt" }).await?;
        ctx.log("Sertifika kuruldu; panel, posta ve FTP artık bu sertifikayı kullanıyor");
        Ok(())
    })
}

/// Süresi yaklaşan Let's Encrypt panel sertifikasını yeniler.
pub async fn renew_if_due(state: &AppState, renew_before_secs: i64) -> AppResult<()> {
    let mode = mode(&state.db);
    let hostname = hostname(&state.db);
    if !mode.starts_with("letsencrypt") || hostname.is_empty() {
        return Ok(());
    }
    let expires_at = info(&state.cfg).map(|info| info.not_after).unwrap_or(0);
    if expires_at - crate::db::now() > renew_before_secs {
        return Ok(());
    }
    tracing::info!(%hostname, "panel sertifikası yenileniyor");
    // DNS doğrulamasında bölgenin sahibi gerekir; ilk yöneticinin bölgeleri kullanılır.
    let owner: i64 = state.db.call(|conn| conn.query_row("SELECT id FROM users WHERE role = 'admin' ORDER BY id LIMIT 1", [], |row| row.get(0)))?;
    start_issue(state, None, owner, hostname, mode == "letsencrypt_dns")?;
    Ok(())
}
