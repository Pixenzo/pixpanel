//! Posta alanı başına "kendi adıyla bağlan": `mail.<alan>` için sertifika, Postfix'te SNI ve
//! Dovecot'ta `local_name` bloğu; isteğe bağlı `webmail.<alan>` adresi.
//!
//! - Sertifika tek bir Let's Encrypt (ACME) siparişiyle alınır: `mail.<alan>`, `autoconfig.<alan>`,
//!   `autodiscover.<alan>` ve (isteniyorsa) `webmail.<alan>` aynı sertifikanın SAN adlarıdır.
//!   Doğrulama web (HTTP-01) ya da alan adının DNS'i panelde yönetiliyorsa DNS (DNS-01) ile yapılır;
//!   yenileme aynı yöntemi kullanır.
//! - Dosyalar `/etc/pixpanel/mail/certs/<alan>/` altındadır (yalnızca root): `fullchain.pem`,
//!   `privkey.pem` ve Postfix için `combined.pem` (anahtar + zincir).
//! - Postfix: `tls_server_sni_maps` eşlemesi `postmap -F` ile üretilir (eşleme anahtar içerdiği için
//!   yalnızca root okur; smtpd anahtarları ayrıcalık düşürmeden önce yükler).
//! - Dovecot: ana yapılandırma şablonunun sonuna her alan için `local_name mail.<alan> { … }` bloğu
//!   eklenir.
//! - nginx: `webmail.<alan>` sunucu bloğu ve `autoconfig.`/`autodiscover.` adlarının HTTPS'i
//!   (`mail::autoconfig::sync_nginx`) bu sertifikayı kullanır.
//! - Sertifika yenilenince (ya da kapatılınca) haritalar ve bloklar yeniden üretilir, servisler yüklenir.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};

use super::write_dovecot;
use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::ssl::{self, CertInfo};
use crate::hosting::{acme, atomic_write};
use crate::state::AppState;
use crate::sys::{exec, services};

pub const CERT_DIR: &str = "/etc/pixpanel/mail/certs";
const SNI_MAP: &str = "/etc/pixpanel/mail/sni";
/// Bitimine bu kadar kalan sertifika yenilenir.
pub const RENEW_BEFORE_SECS: i64 = 30 * 86400;
const MAX_ERROR_LEN: usize = 400;

// ---- Adlar (saf işlevler) ----

pub fn mail_host(domain: &str) -> String {
    format!("mail.{domain}")
}

pub fn webmail_host(domain: &str) -> String {
    format!("webmail.{domain}")
}

/// Sertifikaya yazılan adlar; ilki ana ad (`mail.<alan>`).
pub fn certificate_names(domain: &str, webmail: bool) -> Vec<String> {
    let mut names = vec![mail_host(domain), format!("autoconfig.{domain}"), format!("autodiscover.{domain}")];
    if webmail {
        names.push(webmail_host(domain));
    }
    names
}

pub fn cert_dir(domain: &str) -> PathBuf {
    Path::new(CERT_DIR).join(domain)
}

pub fn fullchain_path(domain: &str) -> PathBuf {
    cert_dir(domain).join("fullchain.pem")
}

pub fn key_path(domain: &str) -> PathBuf {
    cert_dir(domain).join("privkey.pem")
}

fn combined_path(domain: &str) -> PathBuf {
    cert_dir(domain).join("combined.pem")
}

/// Sertifikadaki bir ad (joker olabilir) verilen adı kapsıyor mu?
fn covers(pattern: &str, name: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => name.split_once('.').is_some_and(|(label, rest)| !label.is_empty() && rest == suffix),
        None => pattern == name,
    }
}

/// Yeni sertifika gerekiyor mu: yok, adlardan biri kapsanmıyor ya da bitimi yaklaşmış.
pub fn needs_issue(info: Option<&CertInfo>, names: &[String], now_secs: i64) -> bool {
    match info {
        None => true,
        Some(info) => names.iter().any(|name| !info.names.iter().any(|pattern| covers(pattern, name))) || info.not_after - now_secs < RENEW_BEFORE_SECS,
    }
}

/// Postfix SNI eşlemesinin satırları: `ad  birleşik_dosya` (dosya anahtar + zincir içerir).
pub fn render_sni_map(entries: &[(String, PathBuf)]) -> String {
    let mut sorted: Vec<&(String, PathBuf)> = entries.iter().collect();
    sorted.sort();
    let mut out = String::from("# PixPanel tarafından üretildi. `postmap -F` ile derlenir.\n");
    for (host, combined) in sorted {
        out.push_str(&format!("{host} {}\n", combined.display()));
    }
    out
}

/// Dovecot 2.4 `local_name` blokları: `(ad, sertifika, anahtar)`.
pub fn render_dovecot_blocks(entries: &[(String, PathBuf, PathBuf)]) -> String {
    let mut sorted: Vec<&(String, PathBuf, PathBuf)> = entries.iter().collect();
    sorted.sort();
    let mut out = String::new();
    for (host, cert, key) in sorted {
        out.push_str(&format!(
            "local_name {host} {{\n  ssl_server_cert_file = {}\n  ssl_server_key_file = {}\n}}\n",
            cert.display(),
            key.display()
        ));
    }
    out
}

/// Postfix'in `-F` eşlemesindeki dosya: özel anahtar, ardından sertifika zinciri.
pub fn combine_pem(key: &str, chain: &str) -> String {
    format!("{}\n{}\n", key.trim(), chain.trim())
}

// ---- Kayıtlar ----

#[derive(Debug, Clone)]
pub struct OwnName {
    pub domain_id: i64,
    pub domain: String,
    pub owner_id: i64,
    pub webmail: bool,
    pub dns: bool,
    pub last_error: String,
    pub issued_at: Option<i64>,
}

const COLUMNS: &str = "c.domain_id, d.domain, d.user_id, c.webmail, c.dns, c.last_error, c.issued_at";

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OwnName> {
    Ok(OwnName { domain_id: row.get(0)?, domain: row.get(1)?, owner_id: row.get(2)?, webmail: row.get(3)?, dns: row.get(4)?, last_error: row.get(5)?, issued_at: row.get(6)? })
}

pub fn get(db: &Db, domain_id: i64) -> AppResult<Option<OwnName>> {
    db.call(|conn| {
        conn.query_row(&format!("SELECT {COLUMNS} FROM mail_domain_certs c JOIN mail_domains d ON d.id = c.domain_id WHERE c.domain_id = ?1"), params![domain_id], read_row).optional()
    })
}

pub fn list_all(db: &Db) -> AppResult<Vec<OwnName>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM mail_domain_certs c JOIN mail_domains d ON d.id = c.domain_id ORDER BY d.domain"))?;
        stmt.query_map([], read_row)?.collect()
    })
}

pub fn enable(db: &Db, domain_id: i64, webmail: bool, dns: bool) -> AppResult<()> {
    let created = now();
    db.call(|conn| {
        conn.execute(
            "INSERT INTO mail_domain_certs (domain_id, webmail, dns, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(domain_id) DO UPDATE SET webmail = excluded.webmail, dns = excluded.dns",
            params![domain_id, webmail, dns, created],
        )
    })?;
    Ok(())
}

pub fn disable(db: &Db, domain_id: i64) -> AppResult<()> {
    db.call(|conn| conn.execute("DELETE FROM mail_domain_certs WHERE domain_id = ?1", params![domain_id]))?;
    Ok(())
}

fn record_result(db: &Db, domain_id: i64, error: &str, issued: bool) {
    let error: String = error.chars().take(MAX_ERROR_LEN).collect();
    let _ = db.call(|conn| {
        if issued {
            conn.execute("UPDATE mail_domain_certs SET last_error = '', issued_at = ?2 WHERE domain_id = ?1", params![domain_id, now()])
        } else {
            conn.execute("UPDATE mail_domain_certs SET last_error = ?2 WHERE domain_id = ?1", params![domain_id, error])
        }
    });
}

/// Diskteki sertifikanın bilgisi.
pub fn certificate_info(domain: &str) -> Option<CertInfo> {
    ssl::parse_certificate(&std::fs::read(fullchain_path(domain)).ok()?).ok()
}

/// Etkin kayıtlar ve sertifika dosyaları yerinde olanlar: `(satır, sertifika, anahtar, birleşik)`.
fn ready_entries(db: &Db) -> Vec<(OwnName, PathBuf, PathBuf, PathBuf)> {
    list_all(db)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let (cert, key, combined) = (fullchain_path(&row.domain), key_path(&row.domain), combined_path(&row.domain));
            (cert.is_file() && key.is_file() && combined.is_file()).then_some((row, cert, key, combined))
        })
        .collect()
}

/// Alanın posta istemcileri için sunucu adı: özellik açık ve sertifika yerindeyse `mail.<alan>`,
/// değilse sunucunun genel posta adı.
pub fn client_host(db: &Db, domain: &str) -> String {
    let own = ready_entries(db).into_iter().any(|(row, ..)| row.domain == domain);
    if own { mail_host(domain) } else { super::hostname(db) }
}

/// `webmail.<alan>` adresi açık olan posta alanları.
pub fn webmail_domains(db: &Db) -> BTreeSet<String> {
    list_all(db).unwrap_or_default().into_iter().filter(|row| row.webmail).map(|row| row.domain).collect()
}

/// Verilen adı kapsayan, süresi dolmamış sertifika (nginx sunucu blokları için); yoksa `None`.
pub fn certificate_for(domain: &str, name: &str) -> Option<(PathBuf, PathBuf)> {
    let (cert, key) = (fullchain_path(domain), key_path(domain));
    if !cert.is_file() || !key.is_file() {
        return None;
    }
    let info = certificate_info(domain)?;
    (info.not_after > now() && info.names.iter().any(|pattern| covers(pattern, name))).then_some((cert, key))
}

/// Dovecot yapılandırmasına eklenecek `local_name` blokları.
pub fn dovecot_blocks(db: &Db) -> String {
    let entries: Vec<(String, PathBuf, PathBuf)> = ready_entries(db).into_iter().map(|(row, cert, key, _)| (mail_host(&row.domain), cert, key)).collect();
    render_dovecot_blocks(&entries)
}

/// İstenen adlardan biri başka bir hesabın sitesine ya da takma adına aitse o adı döner.
pub fn foreign_name(db: &Db, owner_id: i64, names: &[String]) -> Option<String> {
    names.iter().find_map(|name| {
        let taken: bool = db
            .call(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sites WHERE domain = ?1 AND user_id != ?2)
                     OR EXISTS(SELECT 1 FROM site_aliases a JOIN sites s ON s.id = a.site_id WHERE a.name = ?1 AND s.user_id != ?2)",
                    params![name, owner_id],
                    |row| row.get(0),
                )
            })
            .unwrap_or(false);
        taken.then(|| name.clone())
    })
}

/// Ad şu an bu sunucunun bir adresine çözülüyor mu?
pub async fn resolves_here(db: &Db, name: &str) -> bool {
    let own = crate::addresses::own_addresses(db).await;
    match tokio::time::timeout(std::time::Duration::from_secs(5), tokio::net::lookup_host((name, 80))).await {
        Ok(Ok(found)) => found.into_iter().any(|address| own.contains(&address.ip().to_canonical())),
        _ => false,
    }
}

// ---- Dosyalar ----

/// Sertifika dosyalarını yazar (`combined.pem` dahil); klasör ve anahtarlar yalnızca root'a açıktır.
pub fn install_files(domain: &str, chain: &str, private_key: &str) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = cert_dir(domain);
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    atomic_write(&key_path(domain), private_key.as_bytes(), 0o600)?;
    atomic_write(&combined_path(domain), combine_pem(private_key, chain).as_bytes(), 0o600)?;
    atomic_write(&fullchain_path(domain), chain.as_bytes(), 0o644)
}

pub fn remove_files(domain: &str) {
    let _ = std::fs::remove_dir_all(cert_dir(domain));
}

// ---- Postfix ----

/// `postmap -F` için kullanılabilir eşleme türü: `hash`, yoksa `lmdb`, yoksa `btree`.
async fn sni_map_type() -> AppResult<String> {
    let output = exec::run_ok("postconf", &["-m"]).await?;
    let types: Vec<&str> = output.lines().map(str::trim).collect();
    ["hash", "lmdb", "btree"].into_iter().find(|kind| types.contains(kind)).map(str::to_string).ok_or_else(|| AppError::internal("Postfix'te hash, lmdb ya da btree eşleme türü yok; SNI eşlemesi üretilemiyor"))
}

/// Eşleme dosyalarını (metin ve derlenmiş `.db`/`.lmdb`) siler.
fn remove_map_files() {
    for suffix in ["", ".db", ".lmdb"] {
        let _ = std::fs::remove_file(format!("{SNI_MAP}{suffix}"));
    }
}

/// `tls_server_sni_maps` eşlemesini üretir ve Postfix'e bağlar; `postfix check` başarısızsa
/// `main.cf` eski hâline döner. Postfix'i yeniden yüklemez.
async fn apply_postfix(entries: &[(String, PathBuf)]) -> AppResult<()> {
    let current = exec::run("postconf", &["-h", "tls_server_sni_maps"]).await.map(|out| out.stdout.trim().to_string()).unwrap_or_default();
    if entries.is_empty() {
        if current.contains(SNI_MAP) {
            let snapshot = super::PostfixSnapshot::take();
            let outcome = async {
                exec::run_ok("postconf", &["-X", "tls_server_sni_maps"]).await?;
                exec::run_ok("postfix", &["check"]).await
            }
            .await;
            if let Err(err) = outcome {
                snapshot.restore();
                return Err(err);
            }
        }
        remove_map_files();
        return Ok(());
    }
    let kind = sni_map_type().await?;
    atomic_write(Path::new(SNI_MAP), render_sni_map(entries).as_bytes(), 0o644)?;
    let spec = format!("{kind}:{SNI_MAP}");
    let snapshot = super::PostfixSnapshot::take();
    let outcome = async {
        exec::run_ok("postmap", &["-F", &spec]).await?;
        // Derlenmiş eşleme özel anahtarları içerir.
        for suffix in [".db", ".lmdb"] {
            let path = format!("{SNI_MAP}{suffix}");
            if Path::new(&path).exists() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        if current != spec {
            exec::run_ok("postconf", &["-e", &format!("tls_server_sni_maps={spec}")]).await?;
        }
        exec::run_ok("postfix", &["check"]).await
    }
    .await;
    if let Err(err) = outcome {
        snapshot.restore();
        return Err(err);
    }
    Ok(())
}

// ---- Hepsini uygula ----

/// Haritaları ve blokları yeniden üretir: Postfix SNI, Dovecot `local_name`, nginx. Her adım
/// bağımsız denenir; ilk hata sonunda döner. Servisleri yeniden yükler.
pub async fn apply_all(state: &AppState) -> AppResult<()> {
    let mut first_error: Option<AppError> = None;
    if super::is_installed() {
        let entries: Vec<(String, PathBuf)> = ready_entries(&state.db).into_iter().map(|(row, _, _, combined)| (mail_host(&row.domain), combined)).collect();
        if let Err(err) = apply_postfix(&entries).await {
            first_error.get_or_insert(err);
        }
        if let Err(err) = write_dovecot(&state.db, &state.cfg).await {
            first_error.get_or_insert(err);
        }
        if services::is_active("postfix.service").await {
            if let Err(err) = exec::run_ok("postfix", &["reload"]).await {
                first_error.get_or_insert(err);
            }
        }
        if services::is_active("dovecot.service").await {
            if let Err(err) = exec::run_ok("doveadm", &["reload"]).await {
                first_error.get_or_insert(err);
            }
        }
    }
    if let Err(err) = super::autoconfig::sync_nginx(&state.db).await {
        first_error.get_or_insert(err);
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Alan adı silindikten sonra: sertifika dosyaları kaldırılır, yapılandırmalar yenilenir.
pub async fn cleanup_domain(state: &AppState, domain: &str) {
    remove_files(domain);
    if let Err(err) = apply_all(state).await {
        tracing::warn!(error = %err, domain, "alan adı silindikten sonra posta sertifikası yapılandırması yenilenemedi");
    }
}

// ---- Sertifika alma ----

/// Sertifika alma/yenileme görevi. `user_id` yoksa kendiliğinden yenilemedir: başarısızlıkta alan
/// sahibine haber verilir.
pub fn start_issue_job(state: &AppState, user_id: Option<i64>, domain_id: i64) -> AppResult<i64> {
    let row = get(&state.db, domain_id)?.ok_or(AppError::NotFound)?;
    let title = format!("{} için posta sertifikası", row.domain);
    let state = state.clone();
    let jobs = state.jobs.clone();
    jobs.start("mail.certificate", &title, user_id, move |ctx| async move {
        let outcome: AppResult<()> = async {
            let row = get(&state.db, domain_id)?.ok_or_else(|| AppError::bad("Özellik bu sırada kapatılmış"))?;
            let names = certificate_names(&row.domain, row.webmail);
            let solver = if row.dns {
                if crate::dns::zone_for(&state.db, row.owner_id, &row.domain)?.is_none() {
                    return Err(AppError::bad("DNS ile doğrulama için alan adının DNS'i panelde yönetiliyor olmalı"));
                }
                Some(crate::dns::AcmeSolver::new(state.clone(), row.owner_id))
            } else {
                if !crate::hosting::nginx::is_installed() {
                    return Err(AppError::bad("Web üzerinden doğrulama için nginx kurulu olmalı; DNS ile doğrulamayı deneyin"));
                }
                None
            };
            let (chain, private_key) = acme::issue(&ctx, &state.db, &state.cfg, &names, solver).await?;
            let _guard = state.sys_lock.lock().await;
            let row = get(&state.db, domain_id)?.ok_or_else(|| AppError::bad("Özellik sertifika alınırken kapatılmış"))?;
            install_files(&row.domain, &chain, &private_key)?;
            record_result(&state.db, domain_id, "", true);
            ctx.log("Sertifika kuruldu; posta servisleri ve istemci adresleri yapılandırılıyor");
            apply_all(&state).await?;
            ctx.log(format!("{} artık kendi adıyla bağlanılabilir", mail_host(&row.domain)));
            Ok(())
        }
        .await;
        if let Err(err) = &outcome {
            record_result(&state.db, domain_id, &err.to_string(), false);
            if user_id.is_none() {
                if let Ok(Some(row)) = get(&state.db, domain_id) {
                    let body = format!("Alan adı: {}\nHata: {err}\n\nSertifikanın süresi dolmadan önce panel yeniden deneyecek.", row.domain);
                    crate::notify::send_event(&state, row.owner_id, "ssl_failed", &format!("{} posta sertifikası yenilenemedi", row.domain), &body).await;
                }
            }
        }
        outcome
    })
}

/// Süresi yaklaşan (ya da hiç alınamamış) sertifikaları yeniler; günde iki kez çağrılır.
pub async fn renew_due(state: &AppState) -> AppResult<()> {
    for row in list_all(&state.db)? {
        let names = certificate_names(&row.domain, row.webmail);
        if needs_issue(certificate_info(&row.domain).as_ref(), &names, now()) {
            tracing::info!(domain = %row.domain, "posta sertifikası yenileniyor");
            start_issue_job(state, None, row.domain_id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(names: &[&str], not_after: i64) -> CertInfo {
        CertInfo { subject: String::new(), issuer: String::new(), names: names.iter().map(|n| n.to_string()).collect(), not_before: 0, not_after, self_signed: false }
    }

    #[test]
    fn names_include_client_config_and_optional_webmail() {
        assert_eq!(certificate_names("ornek.test", false), vec!["mail.ornek.test", "autoconfig.ornek.test", "autodiscover.ornek.test"]);
        let with_webmail = certificate_names("ornek.test", true);
        assert_eq!(with_webmail.len(), 4);
        assert_eq!(with_webmail[3], "webmail.ornek.test");
    }

    #[test]
    fn sni_map_lists_host_and_combined_file() {
        let map = render_sni_map(&[("mail.b.test".into(), PathBuf::from("/c/b/combined.pem")), ("mail.a.test".into(), PathBuf::from("/c/a/combined.pem"))]);
        let lines: Vec<&str> = map.lines().filter(|line| !line.starts_with('#')).collect();
        assert_eq!(lines, vec!["mail.a.test /c/a/combined.pem", "mail.b.test /c/b/combined.pem"]);
    }

    #[test]
    fn dovecot_blocks_use_local_name_with_2_4_settings() {
        let blocks = render_dovecot_blocks(&[("mail.a.test".into(), PathBuf::from("/c/a/fullchain.pem"), PathBuf::from("/c/a/privkey.pem"))]);
        assert_eq!(
            blocks,
            "local_name mail.a.test {\n  ssl_server_cert_file = /c/a/fullchain.pem\n  ssl_server_key_file = /c/a/privkey.pem\n}\n"
        );
        assert_eq!(render_dovecot_blocks(&[]), "");
    }

    #[test]
    fn combined_file_has_key_before_chain() {
        let text = combine_pem("-----BEGIN PRIVATE KEY-----\nK\n-----END PRIVATE KEY-----\n", "-----BEGIN CERTIFICATE-----\nC\n-----END CERTIFICATE-----\n");
        assert!(text.find("PRIVATE KEY").unwrap() < text.find("CERTIFICATE").unwrap());
    }

    #[test]
    fn issue_needed_when_missing_uncovered_or_expiring() {
        let names = certificate_names("o.test", true);
        let day = 86400;
        let full = info(&["mail.o.test", "autoconfig.o.test", "autodiscover.o.test", "webmail.o.test"], 100 * day);
        assert!(needs_issue(None, &names, 0));
        assert!(!needs_issue(Some(&full), &names, 0));
        // 30 günden az kaldı.
        assert!(needs_issue(Some(&full), &names, 80 * day));
        // webmail adı sonradan açıldı: sertifika onu kapsamıyor.
        let without = info(&["mail.o.test", "autoconfig.o.test", "autodiscover.o.test"], 100 * day);
        assert!(needs_issue(Some(&without), &names, 0));
        // Joker sertifika adları kapsar.
        let wildcard = info(&["*.o.test"], 100 * day);
        assert!(!needs_issue(Some(&wildcard), &names, 0));
    }
}
