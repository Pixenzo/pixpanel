//! Posta sunucusu: Postfix (SMTP), Dovecot (IMAP, teslimat, Sieve) ve Rspamd
//! (istenmeyen posta süzgeci, DKIM imzası).
//!
//! Hesaplar panelin veritabanında tutulur. Her değişiklikte Postfix ve
//! Dovecot'un okuduğu düz eşleme dosyaları (`/etc/pixpanel/mail/`) baştan
//! üretilir; servisler panelin veritabanına doğrudan erişmez.

pub mod autoconfig;
pub mod owner;
pub mod policy;
pub mod quarantine;
pub mod sieve;

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthUser, Role};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::sysuser::{self, SysUser};
use crate::hosting::{atomic_write, domain as domain_rules, nginx};
use crate::sys::{exec, runas, services};
use sieve::{AutoReply, Filter};

pub const MAIL_DIR: &str = "/etc/pixpanel/mail";
pub const MAIL_ROOT: &str = "/var/vmail";
const VMAIL_USER: &str = "vmail";
const VMAIL_UID: u32 = 5000;
const DKIM_DIR: &str = "/var/lib/rspamd/dkim";
pub const DKIM_SELECTOR: &str = "pix";
/// Bir iletinin hangi alan adı adına imzalanabileceğine karar veren Rspamd koşulu.
const DKIM_SIGN_CONDITION: &str = include_str!("dkim-sign-condition.lua");
/// Yalnızca `sendmail` ile gelen postayı işleyen Postfix temizlik servisi.
const LOCAL_CLEANUP_SERVICE: &str = "pixcleanup";
/// Postfix'in `sendmail` yolunda eklediği, gönderenin Linux kullanıcı numarasını taşıyan iç başlık.
const UID_HEADER: &str = "X-PixPanel-Uid";
const HOSTNAME_KEY: &str = "mail.hostname";
const SPAM_KEY: &str = "mail.spam";

const DOVECOT_TEMPLATE: &str = include_str!("templates/dovecot.conf");
const BEFORE_SIEVE: &str = include_str!("templates/before.sieve");

pub const SERVICES: &[(&str, &str)] =
    &[("postfix.service", "Postfix (SMTP)"), ("dovecot.service", "Dovecot (IMAP)"), ("rspamd.service", "Rspamd (istenmeyen posta süzgeci)")];

pub fn is_installed() -> bool {
    exec::binary_exists("postfix") && exec::binary_exists("dovecot") && exec::binary_exists("rspamd")
}

fn mail_file(name: &str) -> PathBuf {
    Path::new(MAIL_DIR).join(name)
}

// ---- Adres doğrulama ----

/// Kutu ve takma adların yerel bölümü (`@` öncesi).
pub fn validate_local(value: &str) -> AppResult<String> {
    let local = value.trim().to_ascii_lowercase();
    let mut chars = local.bytes();
    let valid = local.len() <= 64
        && chars.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && chars.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        && !local.ends_with('.')
        && !local.contains("..");
    if valid { Ok(local) } else { Err(AppError::bad("Geçersiz adres (harf, rakam, nokta, tire ve alt çizgi kullanılabilir)")) }
}

/// Tam e-posta adresi (yönlendirme ve takma ad hedefleri için; dış adresler olabilir).
pub fn validate_address(value: &str) -> AppResult<String> {
    let address = value.trim();
    let invalid = || AppError::bad(format!("Geçersiz e-posta adresi: {address}"));
    let (local, host) = address.rsplit_once('@').ok_or_else(invalid)?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b));
    if !local_ok {
        return Err(invalid());
    }
    let host = domain_rules::normalize(host).map_err(|_| invalid())?;
    Ok(format!("{}@{host}", local.to_ascii_lowercase()))
}

fn validate_addresses(values: &[String], max: usize) -> AppResult<Vec<String>> {
    let mut list = Vec::new();
    for value in values {
        let address = validate_address(value)?;
        if !list.contains(&address) {
            list.push(address);
        }
    }
    if list.len() > max {
        return Err(AppError::bad(format!("En fazla {max} adres girilebilir")));
    }
    Ok(list)
}

// ---- Kayıtlar ----

#[derive(Debug, Clone, Serialize)]
pub struct MailDomain {
    pub id: i64,
    pub user_id: i64,
    pub owner: String,
    pub domain: String,
    /// `@alan` adresine gelen ve hiçbir kutuya uymayan iletilerin gideceği adres.
    pub catch_all: String,
    pub dkim_record: String,
    pub mailbox_count: i64,
    pub alias_count: i64,
    pub created_at: i64,
    /// Alan adının saatlik alıcı sınırı (yalnızca yönetici/bayi ayarlar): `None` varsayılan, 0 sınırsız.
    pub send_limit: Option<i64>,
    /// Geçerli sınır (varsayılan uygulanmış hâli; 0 sınırsız).
    pub effective_limit: i64,
    /// Son bir saatte alan adından gönderilen alıcı sayısı.
    pub sent_last_hour: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mailbox {
    pub id: i64,
    pub domain_id: i64,
    pub local: String,
    pub address: String,
    pub quota_mb: i64,
    pub enabled: bool,
    pub forward_to: Vec<String>,
    pub forward_keep: bool,
    pub auto_reply: AutoReply,
    pub filters: Vec<Filter>,
    /// Kullanılan alan (bayt); Dovecot'tan okunur.
    pub used_bytes: Option<i64>,
    pub created_at: i64,
    /// Kutunun saatlik alıcı sınırı (yalnızca yönetici/bayi ayarlar): `None` varsayılan, 0 sınırsız.
    pub send_limit: Option<i64>,
    /// Geçerli sınır (varsayılan uygulanmış hâli; 0 sınırsız).
    pub effective_limit: i64,
    /// Son bir saatte kutunun gönderdiği alıcı sayısı.
    pub sent_last_hour: u32,
    #[serde(skip)]
    pub password_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailAlias {
    pub id: i64,
    pub domain_id: i64,
    pub local: String,
    pub address: String,
    pub destinations: Vec<String>,
    pub created_at: i64,
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

const DOMAIN_COLUMNS: &str = "d.id, d.user_id, u.username, d.domain, d.catch_all, d.dkim_record, d.created_at,
     (SELECT COUNT(*) FROM mailboxes m WHERE m.domain_id = d.id),
     (SELECT COUNT(*) FROM mail_aliases a WHERE a.domain_id = d.id), d.send_limit";

fn read_domain(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailDomain> {
    Ok(MailDomain {
        id: row.get(0)?,
        user_id: row.get(1)?,
        owner: row.get(2)?,
        domain: row.get(3)?,
        catch_all: row.get(4)?,
        dkim_record: row.get(5)?,
        created_at: row.get(6)?,
        mailbox_count: row.get(7)?,
        alias_count: row.get(8)?,
        send_limit: row.get(9)?,
        effective_limit: 0,
        sent_last_hour: 0,
    })
}

/// Kullanıcının görebildiği posta alan adları (siteler ile aynı kural).
pub fn list_domains(db: &Db, user: &AuthUser) -> AppResult<Vec<MailDomain>> {
    let owner = user.owner_id();
    let (all, include_children) = (user.is_admin(), user.role == Role::Reseller);
    let mut domains: Vec<MailDomain> = db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {DOMAIN_COLUMNS} FROM mail_domains d JOIN users u ON u.id = d.user_id
             WHERE ?1 OR d.user_id = ?2 OR (?3 AND u.parent_id = ?2) ORDER BY d.domain"
        ))?;
        stmt.query_map(params![all, owner, include_children], read_domain)?.collect()
    })?;
    policy::decorate_domains(db, &mut domains);
    Ok(domains)
}

pub fn get_domain(db: &Db, user: &AuthUser, id: i64) -> AppResult<MailDomain> {
    let found = db.call(|conn| {
        conn.query_row(
            &format!("SELECT {DOMAIN_COLUMNS}, u.parent_id FROM mail_domains d JOIN users u ON u.id = d.user_id WHERE d.id = ?1"),
            params![id],
            |row| Ok((read_domain(row)?, row.get::<_, Option<i64>>(10)?)),
        )
        .optional()
    })?;
    let (mut domain, owner_parent) = found.ok_or(AppError::NotFound)?;
    let allowed = user.is_admin() || domain.user_id == user.owner_id() || (user.role == Role::Reseller && owner_parent == Some(user.id));
    if !allowed {
        return Err(AppError::NotFound);
    }
    policy::decorate_domains(db, std::slice::from_mut(&mut domain));
    Ok(domain)
}

const MAILBOX_COLUMNS: &str = "m.id, m.domain_id, m.local, d.domain, m.quota_mb, m.enabled, m.forward_to, m.forward_keep,
     m.auto_reply, m.filters, m.created_at, m.password_hash, m.send_limit";

fn read_mailbox(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mailbox> {
    let (local, domain): (String, String) = (row.get(2)?, row.get(3)?);
    let (forward_to, auto_reply, filters): (String, String, String) = (row.get(6)?, row.get(8)?, row.get(9)?);
    Ok(Mailbox {
        id: row.get(0)?,
        domain_id: row.get(1)?,
        address: format!("{local}@{domain}"),
        local,
        quota_mb: row.get(4)?,
        enabled: row.get(5)?,
        forward_to: split_list(&forward_to),
        forward_keep: row.get(7)?,
        auto_reply: serde_json::from_str(&auto_reply).unwrap_or_default(),
        filters: serde_json::from_str(&filters).unwrap_or_default(),
        used_bytes: None,
        created_at: row.get(10)?,
        password_hash: row.get(11)?,
        send_limit: row.get(12)?,
        effective_limit: 0,
        sent_last_hour: 0,
    })
}

pub fn list_mailboxes(db: &Db, domain_id: Option<i64>) -> AppResult<Vec<Mailbox>> {
    let mut mailboxes: Vec<Mailbox> = db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {MAILBOX_COLUMNS} FROM mailboxes m JOIN mail_domains d ON d.id = m.domain_id
             WHERE ?1 IS NULL OR m.domain_id = ?1 ORDER BY d.domain, m.local"
        ))?;
        stmt.query_map(params![domain_id], read_mailbox)?.collect()
    })?;
    policy::decorate_mailboxes(db, &mut mailboxes);
    Ok(mailboxes)
}

pub fn load_mailbox(db: &Db, id: i64) -> AppResult<Mailbox> {
    let mut mailbox = db
        .call(|conn| {
            conn.query_row(
                &format!("SELECT {MAILBOX_COLUMNS} FROM mailboxes m JOIN mail_domains d ON d.id = m.domain_id WHERE m.id = ?1"),
                params![id],
                read_mailbox,
            )
            .optional()
        })?
        .ok_or(AppError::NotFound)?;
    policy::decorate_mailboxes(db, std::slice::from_mut(&mut mailbox));
    Ok(mailbox)
}

/// Adresine göre etkin kutuyu ve alan adının sahibi hesabın numarasını bulur (kutu sahibinin
/// oturumsuz işlemleri için). Kapalı ya da var olmayan kutu için `None`.
pub fn find_enabled_mailbox(db: &Db, local: &str, domain: &str) -> AppResult<Option<(Mailbox, i64)>> {
    let found = db.call(|conn| {
        conn.query_row(
            &format!(
                "SELECT {MAILBOX_COLUMNS}, d.user_id FROM mailboxes m JOIN mail_domains d ON d.id = m.domain_id
                 WHERE m.local = ?1 AND d.domain = ?2 AND m.enabled = 1"
            ),
            params![local, domain],
            |row| Ok((read_mailbox(row)?, row.get::<_, i64>(13)?)),
        )
        .optional()
    })?;
    Ok(found)
}

pub fn save_mailbox(db: &Db, mailbox: &Mailbox) -> AppResult<()> {
    let (auto_reply, filters) = (to_json(&mailbox.auto_reply)?, to_json(&mailbox.filters)?);
    db.call(|conn| {
        conn.execute(
            "UPDATE mailboxes SET password_hash = ?1, quota_mb = ?2, enabled = ?3, forward_to = ?4, forward_keep = ?5,
                    auto_reply = ?6, filters = ?7, send_limit = ?8 WHERE id = ?9",
            params![
                mailbox.password_hash, mailbox.quota_mb, mailbox.enabled, mailbox.forward_to.join(","),
                mailbox.forward_keep, auto_reply, filters, mailbox.send_limit, mailbox.id
            ],
        )
    })?;
    Ok(())
}

fn to_json<T: Serialize>(value: &T) -> AppResult<String> {
    serde_json::to_string(value).map_err(|err| AppError::internal(err.to_string()))
}

fn read_alias(row: &rusqlite::Row<'_>) -> rusqlite::Result<MailAlias> {
    let (local, domain, destinations): (String, String, String) = (row.get(2)?, row.get(3)?, row.get(4)?);
    Ok(MailAlias {
        id: row.get(0)?,
        domain_id: row.get(1)?,
        address: format!("{local}@{domain}"),
        local,
        destinations: split_list(&destinations),
        created_at: row.get(5)?,
    })
}

const ALIAS_COLUMNS: &str = "a.id, a.domain_id, a.local, d.domain, a.destinations, a.created_at";

pub fn list_aliases(db: &Db, domain_id: Option<i64>) -> AppResult<Vec<MailAlias>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {ALIAS_COLUMNS} FROM mail_aliases a JOIN mail_domains d ON d.id = a.domain_id
             WHERE ?1 IS NULL OR a.domain_id = ?1 ORDER BY d.domain, a.local"
        ))?;
        stmt.query_map(params![domain_id], read_alias)?.collect()
    })
}

pub fn load_alias(db: &Db, id: i64) -> AppResult<MailAlias> {
    db.call(|conn| {
        conn.query_row(
            &format!("SELECT {ALIAS_COLUMNS} FROM mail_aliases a JOIN mail_domains d ON d.id = a.domain_id WHERE a.id = ?1"),
            params![id],
            read_alias,
        )
        .optional()
    })?
    .ok_or(AppError::NotFound)
}

/// Takma ad hedeflerini doğrular (en fazla 50 adres).
pub fn validate_destinations(values: &[String]) -> AppResult<Vec<String>> {
    let list = validate_addresses(values, 50)?;
    if list.is_empty() {
        return Err(AppError::bad("En az bir hedef adres girin"));
    }
    Ok(list)
}

pub fn validate_forwards(values: &[String]) -> AppResult<Vec<String>> {
    validate_addresses(values, sieve::MAX_FORWARDS)
}

pub fn validate_quota(quota_mb: i64) -> AppResult<()> {
    if (10..=1_000_000).contains(&quota_mb) { Ok(()) } else { Err(AppError::bad("Kota 10 MB ile 1.000.000 MB arasında olmalı")) }
}

// ---- Sistem kullanıcısı ve kutu klasörleri ----

fn vmail() -> AppResult<SysUser> {
    sysuser::lookup(VMAIL_USER).ok_or_else(|| AppError::bad("Posta sunucusu kurulu değil; önce Bileşenler sayfasından kurun"))
}

fn mailbox_home(domain: &str, local: &str) -> PathBuf {
    Path::new(MAIL_ROOT).join(domain).join(local)
}

/// `vmail` kullanıcısı adına komut çalıştırır; başarısızsa hata çıktısını döner.
async fn as_vmail(cwd: &Path, program: &str, args: &[&str], stdin: Option<&[u8]>) -> AppResult<Result<String, String>> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = runas::command(&vmail()?, cwd, program, args);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("{program} başlatılamadı: {err}")))?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(data).await?;
        pipe.shutdown().await?;
    }
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| AppError::internal(format!("{program} zaman aşımına uğradı")))??;
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned());
    Ok(if output.status.success() { Ok(stdout) } else { Err(if stderr.trim().is_empty() { stdout } else { stderr }) })
}

/// Panelin ürettiği Sieve betiğinin klasörü (kutu klasörünün içinde). Dovecot'ta `sieve_script panel`
/// (`type = before`) olarak tanımlıdır: betik kutu sahibinin kendi betiğinden ÖNCE çalışır. Klasör
/// ManageSieve'in gördüğü kişisel betik klasörünün (`sieve/`) dışındadır; kutu sahibi bu betiği
/// ne görebilir, ne değiştirebilir, ne de silebilir.
const PANEL_SIEVE_DIR: &str = "sieve-before";
/// Kutu sahibinin ManageSieve ile yönettiği kişisel betik klasörü (Dovecot `sieve_script personal`).
const PERSONAL_SIEVE_DIR: &str = "sieve";
const PANEL_SIEVE_FILE: &str = "pixpanel.sieve";

/// Kutunun klasörlerini (panel betiği ve kişisel betikler) oluşturur.
async fn ensure_sieve_dirs(root: &Path, home_str: &str) -> AppResult<()> {
    as_vmail(root, "mkdir", &["-p", "--", &format!("{home_str}/{PERSONAL_SIEVE_DIR}"), &format!("{home_str}/{PANEL_SIEVE_DIR}")], None)
        .await?
        .map_err(|detail| AppError::internal(format!("Kutu klasörü oluşturulamadı: {}", detail.trim())))?;
    Ok(())
}

/// Eski düzenin (panel betiği kutu sahibinin betik klasöründe ve `.dovecot.sieve` ile etkin) artıklarını
/// siler; yalnızca geçişte (`migrate_sieve_layout`) çağrılır. Kutu sahibinin kendi etkin betiğine
/// dokunulmaz: `.dovecot.sieve` yalnızca eski panel betiğine işaret ediyorsa kaldırılır.
async fn remove_legacy_sieve(root: &Path, home_str: &str) {
    let active = format!("{home_str}/.dovecot.sieve");
    if let Ok(Ok(target)) = as_vmail(root, "readlink", &["--", &active], None).await {
        if target.trim() == "sieve/pixpanel.sieve" {
            let _ = as_vmail(root, "rm", &["-f", "--", &active, &format!("{home_str}/.dovecot.svbin")], None).await;
        }
    }
    let legacy: Vec<String> = ["pixpanel.sieve", "pixpanel.svbin", "pixpanel.new.sieve", "pixpanel.new.svbin"]
        .iter()
        .map(|name| format!("{home_str}/{PERSONAL_SIEVE_DIR}/{name}"))
        .collect();
    let mut args = vec!["-f", "--"];
    args.extend(legacy.iter().map(String::as_str));
    let _ = as_vmail(root, "rm", &args, None).await;
}

/// Kutunun panel Sieve betiğini kurallarına göre yazar ya da (kural yoksa) kaldırır.
/// Betik derlenemezse etkinleştirilmez ve hata döner. Kutu sahibinin kendi (ManageSieve) betiğine
/// dokunulmaz; panel betiği ondan önce çalışır.
pub async fn apply_sieve(mailbox: &Mailbox) -> AppResult<()> {
    let (local, domain) = mailbox.address.split_once('@').ok_or_else(|| AppError::internal("adres bozuk"))?;
    let home = mailbox_home(domain, local);
    let root = Path::new(MAIL_ROOT);
    let home_str = home.to_string_lossy().into_owned();
    let run = |result: Result<String, String>, what: &str| result.map_err(|detail| AppError::internal(format!("{what}: {}", detail.trim())));

    ensure_sieve_dirs(root, &home_str).await?;
    let target = format!("{home_str}/{PANEL_SIEVE_DIR}/{PANEL_SIEVE_FILE}");
    let target_binary = format!("{home_str}/{PANEL_SIEVE_DIR}/pixpanel.svbin");
    if !sieve::is_needed(&mailbox.filters, &mailbox.auto_reply, &mailbox.forward_to) {
        run(as_vmail(root, "rm", &["-f", "--", &target, &target_binary], None).await?, "Betik kaldırılamadı")?;
        return Ok(());
    }

    let script = sieve::render(&mailbox.address, &mailbox.filters, &mailbox.auto_reply, &mailbox.forward_to, mailbox.forward_keep);
    // Önce Dovecot'un taramadığı bir yere yazılıp derlenir; hatalı betik etkin betiğin yerini almaz.
    let candidate = format!("{home_str}/pixpanel-new.sieve");
    let candidate_binary = format!("{home_str}/pixpanel-new.svbin");
    run(as_vmail(root, "tee", &["--", &candidate], Some(script.as_bytes())).await?, "Betik yazılamadı")?;
    if let Err(detail) = as_vmail(root, "sievec", &["--", &candidate], None).await? {
        let _ = as_vmail(root, "rm", &["-f", "--", &candidate, &candidate_binary], None).await;
        return Err(AppError::bad(format!("Kurallar uygulanamadı: {}", detail.trim())));
    }
    run(as_vmail(root, "mv", &["-f", "--", &candidate, &target], None).await?, "Betik etkinleştirilemedi")?;
    // Eski derlenmiş sürüm silinir; Dovecot yeni betiği ilk teslimde yeniden derler.
    let _ = as_vmail(root, "rm", &["-f", "--", &candidate_binary, &target_binary], None).await;
    Ok(())
}

/// Var olan her kutuyu yeni Sieve düzenine geçirir: panel betiği `sieve-before/` altına yazılır,
/// eski düzenin artıkları silinir, her kutuya ManageSieve için `sieve/` klasörü açılır.
/// Dovecot'un yeni yapılandırması (`write_dovecot`) yazıldıktan sonra çağrılır.
pub async fn migrate_sieve_layout(db: &Db) -> AppResult<()> {
    let mut failures = 0;
    for mailbox in list_mailboxes(db, None)? {
        match apply_sieve(&mailbox).await {
            // Eski düzenin artıkları, yeni betik yerine yazıldıktan sonra silinir (kural kaybı olmasın).
            // Yalnızca geçişte yapılır: sonradan kutu sahibinin açtığı "pixpanel" adlı kendi betiğine dokunulmaz.
            Ok(()) => {
                if let Some((local, domain)) = mailbox.address.split_once('@') {
                    remove_legacy_sieve(Path::new(MAIL_ROOT), &mailbox_home(domain, local).to_string_lossy()).await;
                }
            }
            Err(err) => {
                failures += 1;
                tracing::warn!(error = %err, mailbox = %mailbox.address, "kutunun Sieve düzeni yeni biçime geçirilemedi");
            }
        }
    }
    if failures > 0 {
        return Err(AppError::internal(format!("{failures} kutunun Sieve düzeni geçirilemedi")));
    }
    Ok(())
}

/// Kutunun (ya da `local` verilmezse alan adının) bütün postalarını siler.
pub async fn remove_mail_data(domain: &str, local: Option<&str>) -> AppResult<()> {
    let target = match local {
        Some(local) => mailbox_home(domain, local),
        None => Path::new(MAIL_ROOT).join(domain),
    };
    // Adlar doğrulanmış olsa da kökün dışına çıkılmadığı ayrıca denetlenir.
    if !target.starts_with(MAIL_ROOT) || target == Path::new(MAIL_ROOT) {
        return Err(AppError::internal("geçersiz posta klasörü"));
    }
    let target = target.to_string_lossy().into_owned();
    as_vmail(Path::new(MAIL_ROOT), "rm", &["-rf", "--", &target], None)
        .await?
        .map_err(|detail| AppError::internal(format!("Postalar silinemedi: {}", detail.trim())))?;
    Ok(())
}

/// Kutuların kullandığı alan (adres → bayt).
pub async fn usage() -> std::collections::HashMap<String, i64> {
    let mut map = std::collections::HashMap::new();
    let Ok(output) = exec::run_with_timeout("doveadm", &["-f", "tab", "quota", "get", "-A"], Duration::from_secs(30)).await else {
        return map;
    };
    // Sütunlar: Username, Quota name, Type, Value, Limit, %
    for line in output.stdout.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() >= 5 && fields[2] == "STORAGE" {
            if let Ok(kilobytes) = fields[3].parse::<i64>() {
                map.insert(fields[0].to_string(), kilobytes * 1024);
            }
        }
    }
    map
}

// ---- Eşleme dosyaları ----

/// Postfix ve Dovecot'un okuduğu dosyaları veritabanından baştan üretir.
pub async fn sync(db: &Db) -> AppResult<()> {
    if !is_installed() {
        return Ok(());
    }
    let account = vmail()?;
    let domains: Vec<(i64, String, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, domain, catch_all FROM mail_domains ORDER BY domain")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect()
    })?;
    let mailboxes = list_mailboxes(db, None)?;
    let aliases = list_aliases(db, None)?;

    let mut domain_map = String::new();
    let mut mailbox_map = String::new();
    let mut alias_map = String::new();
    let mut sender_map = String::new();
    let mut users = String::new();

    for (id, domain, catch_all) in &domains {
        domain_map.push_str(&format!("{domain} OK\n"));
        let boxes: Vec<&Mailbox> = mailboxes.iter().filter(|m| m.domain_id == *id && m.enabled).collect();
        for mailbox in &boxes {
            mailbox_map.push_str(&format!("{} {domain}/{}/\n", mailbox.address, mailbox.local));
            sender_map.push_str(&format!("{0} {0}\n", mailbox.address));
            users.push_str(&format!(
                "{}:{{ARGON2ID}}{}:{uid}:{gid}::{}::userdb_quota_storage_size={}M\n",
                mailbox.address,
                mailbox.password_hash,
                mailbox_home(domain, &mailbox.local).display(),
                mailbox.quota_mb,
                uid = account.uid,
                gid = account.gid,
            ));
            // "Hepsini yakala" tanımlıysa kutuların kendi adresi ondan önce eşleşmelidir.
            if !catch_all.is_empty() {
                alias_map.push_str(&format!("{0} {0}\n", mailbox.address));
            }
        }
        for alias in aliases.iter().filter(|a| a.domain_id == *id) {
            alias_map.push_str(&format!("{} {}\n", alias.address, alias.destinations.join(", ")));
            // Takma ad adresinden, hedefi olan yerel kutular gönderim yapabilir.
            let owners: Vec<&str> = alias.destinations.iter().filter(|d| boxes.iter().any(|m| &m.address == *d)).map(String::as_str).collect();
            if !owners.is_empty() {
                sender_map.push_str(&format!("{} {}\n", alias.address, owners.join(", ")));
            }
        }
        if !catch_all.is_empty() {
            alias_map.push_str(&format!("@{domain} {catch_all}\n"));
        }
    }

    std::fs::create_dir_all(MAIL_DIR)?;
    atomic_write(&mail_file("domains"), domain_map.as_bytes(), 0o644)?;
    atomic_write(&mail_file("mailboxes"), mailbox_map.as_bytes(), 0o644)?;
    atomic_write(&mail_file("aliases"), alias_map.as_bytes(), 0o644)?;
    atomic_write(&mail_file("senders"), sender_map.as_bytes(), 0o644)?;
    // Parola özetlerini yalnızca Dovecot okuyabilir.
    let users_path = mail_file("dovecot-users");
    atomic_write(&users_path, users.as_bytes(), 0o640)?;
    std::os::unix::fs::chown(&users_path, Some(0), Some(sysuser::group_id("dovecot")?))?;

    write_dkim_owners(db)?;

    // texthash eşlemeleri süreç başlarken okunur; yeniden yükleme yeni içeriği devreye alır.
    if services::is_active("postfix.service").await {
        exec::run_ok("postfix", &["reload"]).await?;
    }
    // Dovecot kullanıcı dosyasındaki değişikliği birkaç saniye gecikmeyle fark eder;
    // kapatılan kutunun ya da değişen parolanın hemen geçerli olması için yeniden yüklenir.
    if services::is_active("dovecot.service").await {
        exec::run_ok("doveadm", &["reload"]).await?;
    }
    // Posta alan adları değiştiyse istemci otomatik ayarının nginx tarafı yenilenir (hata postayı bozmaz).
    if let Err(err) = autoconfig::sync_nginx(db).await {
        tracing::warn!(error = %err, "posta istemcisi otomatik ayarı nginx yapılandırması yenilenemedi");
    }
    Ok(())
}

/// Kullanıcının açık IMAP oturumlarını sonlandırır (kutu kapatıldığında ya da silindiğinde).
pub async fn kick(address: &str) {
    let _ = exec::run("doveadm", &["kick", "--", address]).await;
}

// ---- DKIM ve DNS ----

fn dkim_key_path(domain: &str) -> PathBuf {
    Path::new(DKIM_DIR).join(format!("{domain}.{DKIM_SELECTOR}.key"))
}

/// Alan adının DKIM anahtarını (yoksa) üretir ve DNS'e eklenecek kaydın değerini döner.
pub async fn ensure_dkim(domain: &str) -> AppResult<String> {
    let key = dkim_key_path(domain);
    let key_str = key.to_string_lossy().into_owned();
    std::fs::create_dir_all(DKIM_DIR)?;
    let record_path = key.with_extension("txt");
    if !key.exists() {
        let output = exec::run_ok("rspamadm", &["dkim_keygen", "-s", DKIM_SELECTOR, "-d", domain, "-b", "2048", "-k", &key_str]).await?;
        std::fs::write(&record_path, &output)?;
        let rspamd = sysuser::lookup("_rspamd").ok_or_else(|| AppError::internal("_rspamd kullanıcısı bulunamadı"))?;
        for path in [&key, &record_path] {
            std::os::unix::fs::chown(path, Some(rspamd.uid), Some(rspamd.gid))?;
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o640))?;
    }
    // Çıktı bir BIND kaydıdır: tırnak içindeki parçalar birleştirilince kaydın değeri elde edilir.
    let raw = std::fs::read_to_string(&record_path)?;
    let value: String = raw.split('"').skip(1).step_by(2).collect();
    if value.is_empty() {
        return Err(AppError::internal("DKIM kaydı okunamadı"));
    }
    Ok(value)
}

pub fn remove_dkim(domain: &str) {
    let key = dkim_key_path(domain);
    let _ = std::fs::remove_file(key.with_extension("txt"));
    let _ = std::fs::remove_file(key);
}

#[derive(Debug, Serialize)]
pub struct DnsRecord {
    pub kind: &'static str,
    pub name: String,
    pub value: String,
    pub purpose: &'static str,
    /// `ok`, `missing`, `mismatch` ya da (denetlenmediyse) `unknown`.
    pub status: &'static str,
    /// DNS'te bulunan değer (uyuşmuyorsa göstermek için).
    pub found: String,
}

/// Postanın düzgün çalışması için alan adına eklenmesi gereken kayıtlar.
pub fn dns_records(domain: &MailDomain, hostname: &str) -> Vec<DnsRecord> {
    let record = |kind, name: String, value: String, purpose| DnsRecord { kind, name, value, purpose, status: "unknown", found: String::new() };
    let mut records = vec![
        record("MX", domain.domain.clone(), format!("10 {hostname}."), "Gelen postanın bu sunucuya teslim edilmesi"),
        record("TXT", domain.domain.clone(), "v=spf1 mx ~all".to_string(), "SPF: bu alan adı adına yalnızca posta sunucunuz gönderebilir"),
        record("TXT", format!("{DKIM_SELECTOR}._domainkey.{}", domain.domain), domain.dkim_record.clone(), "DKIM: giden postanın imzasını doğrular"),
        record(
            "TXT",
            format!("_dmarc.{}", domain.domain),
            format!("v=DMARC1; p=quarantine; rua=mailto:postmaster@{}", domain.domain),
            "DMARC: SPF/DKIM doğrulanamazsa alıcının ne yapacağı",
        ),
    ];
    // İstemci otomatik ayarı (autoconfig, autodiscover, RFC 6186).
    for client in autoconfig::client_dns_records(&domain.domain, hostname) {
        let value = match client.kind {
            "SRV" => format!("{} {}.", client.priority.unwrap_or(0), client.content),
            _ => format!("{}.", client.content),
        };
        records.push(record(client.kind, client.name, value, client.purpose));
    }
    records
}

async fn dig(kind: &str, name: &str) -> Vec<String> {
    let Ok(output) = exec::run_with_timeout("dig", &["+short", "+time=3", "+tries=1", kind, "--", name], Duration::from_secs(10)).await else {
        return Vec::new();
    };
    // Uzun TXT kayıtları tırnaklı parçalara bölünür; parçalar birleştirilir.
    output.stdout.lines().map(|line| line.replace("\" \"", "").trim_matches('"').to_string()).filter(|line| !line.is_empty()).collect()
}

/// Kayıtları genel DNS'te arar ve her birinin durumunu işaretler.
pub async fn check_dns(records: &mut [DnsRecord]) {
    for record in records.iter_mut() {
        // CNAME'in doğrusu "ad çözülüyor mu"dur (A kaydı da olabilir); bu yüzden A sorulur.
        let answers = dig(if record.kind == "CNAME" { "A" } else { record.kind }, &record.name).await;
        let normalize = |value: &str| value.to_ascii_lowercase().replace(' ', "").trim_end_matches('.').to_string();
        let expected = normalize(&record.value);
        let relevant: Vec<&String> = match record.name.as_str() {
            name if name.starts_with("_dmarc.") => answers.iter().filter(|a| a.to_ascii_lowercase().starts_with("v=dmarc1")).collect(),
            name if name.contains("._domainkey.") => answers.iter().filter(|a| a.contains("p=")).collect(),
            _ if record.kind == "TXT" => answers.iter().filter(|a| a.to_ascii_lowercase().starts_with("v=spf1")).collect(),
            _ => answers.iter().collect(),
        };
        record.found = relevant.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(" | ");
        record.status = if relevant.is_empty() {
            "missing"
        } else if record.name.starts_with("_dmarc.") {
            // DMARC ilkesi alan adı sahibinin tercihidir; kaydın bulunması yeterlidir.
            "ok"
        } else if record.kind == "MX" {
            // Öncelik değeri farklı olabilir; hedef sunucu aynıysa kayıt doğrudur.
            let target = expected.trim_start_matches(|c: char| c.is_ascii_digit());
            if relevant.iter().any(|a| normalize(a).trim_start_matches(|c: char| c.is_ascii_digit()) == target) { "ok" } else { "mismatch" }
        } else if record.name.contains("._domainkey.") {
            if relevant.iter().any(|a| normalize(a) == expected) { "ok" } else { "mismatch" }
        } else if record.kind == "SRV" {
            // Öncelik ve ağırlık farklı olabilir; port ve hedef aynıysa kayıt doğrudur.
            let tail = |value: &str| value.split_whitespace().rev().take(2).map(|part| part.trim_end_matches('.').to_ascii_lowercase()).collect::<Vec<_>>();
            let target = tail(&record.value);
            if relevant.iter().any(|a| tail(a) == target) { "ok" } else { "mismatch" }
        } else if record.kind == "CNAME" {
            // Ad bir adrese çözülüyorsa (CNAME ya da A) otomatik ayar adresi çalışır.
            "ok"
        } else {
            // SPF: sunucuyu kapsayan herhangi bir kayıt kabul edilir (mx, a ya da ip4 ile).
            "ok"
        };
    }
}

// ---- Genel ayarlar ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpamSettings {
    /// Bu puanın üstündeki iletiler reddedilir.
    pub reject_score: f64,
    /// Bu puanın üstündekiler istenmeyen posta olarak işaretlenip Junk klasörüne alınır.
    pub junk_score: f64,
    pub greylisting: bool,
    /// Bu puanın üstündeki (ama işaretlenmeyen) iletiler gri listeye alınır.
    pub greylist_score: f64,
    /// Her zaman kabul edilen gönderenler: adres ya da `@alan`.
    pub whitelist: Vec<String>,
    /// Her zaman reddedilen gönderenler.
    pub blacklist: Vec<String>,
    /// Reddetme eşiğini aşan iletilere ne yapılacağı: `reject` (SMTP'de reddet, varsayılan) ya da
    /// `quarantine` (reddetmek yerine karantinaya al; panelden bırakılır ya da silinir).
    pub reject_action: String,
}

pub const REJECT_ACTIONS: [&str; 2] = ["reject", "quarantine"];

impl Default for SpamSettings {
    fn default() -> Self {
        Self {
            reject_score: 15.0,
            junk_score: 6.0,
            greylisting: true,
            greylist_score: 4.0,
            whitelist: Vec::new(),
            blacklist: Vec::new(),
            reject_action: "reject".into(),
        }
    }
}

fn normalize_sender_entry(value: &str) -> AppResult<String> {
    let entry = value.trim();
    match entry.strip_prefix('@') {
        Some(domain) => Ok(format!("@{}", domain_rules::normalize(domain)?)),
        None => validate_address(entry),
    }
}

impl SpamSettings {
    pub fn normalized(mut self) -> AppResult<Self> {
        if !REJECT_ACTIONS.contains(&self.reject_action.as_str()) {
            return Err(AppError::bad("Reddetme eşiğini aşan iletiler için geçersiz işlem (reject ya da quarantine olmalı)"));
        }
        let in_range = |value: f64| (0.5..=100.0).contains(&value);
        if !in_range(self.reject_score) || !in_range(self.junk_score) || !in_range(self.greylist_score) {
            return Err(AppError::bad("Puan eşikleri 0,5 ile 100 arasında olmalı"));
        }
        if self.junk_score >= self.reject_score {
            return Err(AppError::bad("Reddetme eşiği, istenmeyen posta eşiğinden büyük olmalı"));
        }
        if self.greylist_score > self.junk_score {
            return Err(AppError::bad("Gri liste eşiği, istenmeyen posta eşiğini aşamaz"));
        }
        for list in [&mut self.whitelist, &mut self.blacklist] {
            if list.len() > 500 {
                return Err(AppError::bad("Listeler en fazla 500 kayıt içerebilir"));
            }
            *list = list.iter().filter(|entry| !entry.trim().is_empty()).map(|entry| normalize_sender_entry(entry)).collect::<AppResult<_>>()?;
            list.sort();
            list.dedup();
        }
        Ok(self)
    }
}

pub fn load_spam(db: &Db) -> AppResult<SpamSettings> {
    Ok(db.setting(SPAM_KEY)?.and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default())
}

pub fn system_hostname() -> String {
    std::process::Command::new("hostname")
        .arg("-f")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

/// Posta sunucusunun kendini tanıttığı ad (MX kayıtlarının gösterdiği ad).
pub fn hostname(db: &Db) -> String {
    db.setting(HOSTNAME_KEY).ok().flatten().filter(|name| !name.is_empty()).unwrap_or_else(system_hostname)
}

/// Posta servislerinin kullanacağı sertifika: sunucu adıyla açılmış ve sertifikası
/// olan bir site varsa onunki, yoksa panelin kendi sertifikası.
fn tls_paths(cfg: &crate::config::Config, hostname: &str) -> (PathBuf, PathBuf) {
    let (cert, key) = nginx::certificate_paths(hostname);
    if cert.exists() && key.exists() {
        return (cert, key);
    }
    (cfg.tls_dir().join("panel.crt"), cfg.tls_dir().join("panel.key"))
}

// ---- DKIM imza yetkisi ----

fn dkim_owners_file() -> PathBuf {
    Path::new(MAIL_DIR).join("rspamd").join("dkim_owners.map")
}

/// Hangi Linux kullanıcısının hangi alan adı adına imzalı posta gönderebileceğini yazar
/// (`kullanıcı numarası:alan adı` satırları). Rspamd koşulu bu dosyayı okur; sunucunun içinden
/// `sendmail` ile gönderilen posta ancak alan adının sahibi hesaptan geliyorsa imzalanır.
/// Hesabın Linux kullanıcısı ilk sitesiyle oluştuğu için dosya düzenli aralıkla da yenilenir.
pub fn write_dkim_owners(db: &Db) -> AppResult<()> {
    if !is_installed() {
        return Ok(());
    }
    let rows: Vec<(String, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT u.username, d.domain FROM mail_domains d JOIN users u ON u.id = d.user_id ORDER BY d.domain")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
    })?;
    let mut content = String::new();
    for (username, domain) in rows {
        // Kök kullanıcı koşulda ayrıca güvenilir sayılır; buraya yazılmaz.
        if let Some(user) = sysuser::lookup(&username).filter(|user| user.uid != 0) {
            content.push_str(&format!("{}:{}\n", user.uid, domain.to_lowercase()));
        }
    }
    let path = dkim_owners_file();
    if std::fs::read_to_string(&path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(&path, content.as_bytes(), 0o644)
}

/// Postfix'i, `sendmail` ile gelen postanın başına gönderenin kullanıcı numarasını yazacak
/// biçimde ayarlar.
///
/// Postfix yerel gönderimde iletiye "(Postfix, from userid N)" içeren bir alındı başlığı ekler,
/// ama posta süzgeci (Rspamd) bu başlığı görmez. Bu yüzden yalnızca `sendmail` yolunu işleyen
/// ayrı bir temizlik servisi tanımlanır; o servis alındı başlığından numarayı alıp iç başlığa
/// yazar. SMTP ile gelen postayı işleyen asıl servis ise aynı adlı başlığı siler; böylece başlık
/// dışarıdan ya da yerel SMTP bağlantısıyla taklit edilemez.
async fn configure_local_sender_mark() -> AppResult<()> {
    let strip = Path::new("/etc/postfix/pixpanel-header-checks");
    let mark = Path::new("/etc/postfix/pixpanel-local-header-checks");
    let strip_rule = format!("/^{UID_HEADER}:/ IGNORE\n");
    atomic_write(strip, format!("# PixPanel tarafından üretildi.\n{strip_rule}").as_bytes(), 0o644)?;
    atomic_write(
        mark,
        format!(
            "# PixPanel tarafından üretildi.\n{strip_rule}\
             /^Received: by .* \\(Postfix, from userid ([0-9]+)\\)/ PREPEND {UID_HEADER}: $1\n"
        )
        .as_bytes(),
        0o644,
    )?;
    postconf(&["-e".into(), format!("header_checks=regexp:{}", strip.display())]).await?;
    postconf(&["-M".into(), format!("{LOCAL_CLEANUP_SERVICE}/unix={LOCAL_CLEANUP_SERVICE} unix n - y - 0 cleanup")]).await?;
    postconf(&["-P".into(), format!("{LOCAL_CLEANUP_SERVICE}/unix/header_checks=regexp:{}", mark.display())]).await?;
    postconf(&["-P".into(), format!("pickup/unix/cleanup_service_name={LOCAL_CLEANUP_SERVICE}")]).await?;
    Ok(())
}

/// Bu korumadan önce kurulmuş posta sunucusunu panel açılırken günceller (bir kez).
pub async fn upgrade_dkim_policy(db: &Db) -> AppResult<()> {
    if !is_installed() || Path::new("/etc/postfix/pixpanel-local-header-checks").exists() || !Path::new("/etc/rspamd/local.d/dkim_signing.conf").exists() {
        return Ok(());
    }
    write_dkim_owners(db)?;
    configure_local_sender_mark().await?;
    exec::run_ok("postfix", &["check"]).await?;
    if services::is_active("postfix.service").await {
        exec::run_ok("postfix", &["reload"]).await?;
    }
    write_rspamd(&load_spam(db)?).await?;
    tracing::info!("posta sunucusu DKIM imza yetkisi denetimine geçirildi");
    Ok(())
}

// ---- Temel yapılandırma ----

async fn postconf(args: &[String]) -> AppResult<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    exec::run_ok("postconf", &args).await?;
    Ok(())
}

fn rspamd_number(value: f64) -> String {
    format!("{value:.2}")
}

async fn write_rspamd(spam: &SpamSettings) -> AppResult<()> {
    let local = Path::new("/etc/rspamd/local.d");
    let maps = Path::new(MAIL_DIR).join("rspamd");
    std::fs::create_dir_all(&maps)?;
    std::fs::create_dir_all(local)?;

    let split = |list: &[String]| -> (String, String) {
        let (mut addresses, mut domains) = (String::new(), String::new());
        for entry in list {
            match entry.strip_prefix('@') {
                Some(domain) => domains.push_str(&format!("{domain}\n")),
                None => addresses.push_str(&format!("{entry}\n")),
            }
        }
        (addresses, domains)
    };
    let (white_addresses, white_domains) = split(&spam.whitelist);
    let (black_addresses, black_domains) = split(&spam.blacklist);
    for (name, content) in [
        ("whitelist_from.map", white_addresses),
        ("whitelist_domains.map", white_domains),
        ("blacklist_from.map", black_addresses),
        ("blacklist_domains.map", black_domains),
    ] {
        atomic_write(&maps.join(name), content.as_bytes(), 0o644)?;
    }

    let maps = maps.display();
    let files: [(&str, String); 6] = [
        (
            "actions.conf",
            format!(
                "# PixPanel tarafından üretildi.\nreject = {};\nadd_header = {};\ngreylist = {};\n",
                rspamd_number(spam.reject_score),
                rspamd_number(spam.junk_score),
                rspamd_number(spam.greylist_score)
            ),
        ),
        ("greylist.conf", format!("# PixPanel tarafından üretildi.\nenabled = {};\n", spam.greylisting)),
        (
            "milter_headers.conf",
            "# PixPanel tarafından üretildi.\nuse = [\"x-spamd-bar\", \"x-spam-level\", \"x-spam-status\", \"authentication-results\"];\n".to_string(),
        ),
        ("redis.conf", "# PixPanel tarafından üretildi.\nservers = \"127.0.0.1\";\n".to_string()),
        (
            "dkim_signing.conf",
            format!(
                "# PixPanel tarafından üretildi.\npath = \"{DKIM_DIR}/$domain.$selector.key\";\nselector = \"{DKIM_SELECTOR}\";\n\
                 allow_username_mismatch = false;\nuse_domain = \"header\";\n\
                 # İmza yalnızca gönderenin alan adı üzerindeki hakkı kanıtlanınca atılır.\n\
                 sign_condition = <<EOD\n{condition}\nEOD;\n",
                condition = DKIM_SIGN_CONDITION
                    .replace("{{map}}", &dkim_owners_file().to_string_lossy())
                    .replace("{{dkim_dir}}", DKIM_DIR)
                    .replace("{{selector}}", DKIM_SELECTOR)
            ),
        ),
        (
            "multimap.conf",
            format!(
                "# PixPanel tarafından üretildi.\n\
                 PIX_WHITELIST_FROM {{ type = \"from\"; map = \"{maps}/whitelist_from.map\"; score = -30.0; description = \"Her zaman kabul edilen gönderen\"; }}\n\
                 PIX_WHITELIST_DOMAIN {{ type = \"from\"; filter = \"email:domain\"; map = \"{maps}/whitelist_domains.map\"; score = -30.0; description = \"Her zaman kabul edilen alan adı\"; }}\n\
                 PIX_BLACKLIST_FROM {{ type = \"from\"; map = \"{maps}/blacklist_from.map\"; score = 30.0; description = \"Engellenen gönderen\"; }}\n\
                 PIX_BLACKLIST_DOMAIN {{ type = \"from\"; filter = \"email:domain\"; map = \"{maps}/blacklist_domains.map\"; score = 30.0; description = \"Engellenen alan adı\"; }}\n"
            ),
        ),
    ];
    let previous: Vec<Option<Vec<u8>>> = files.iter().map(|(name, _)| std::fs::read(local.join(name)).ok()).collect();
    for (name, content) in &files {
        atomic_write(&local.join(name), content.as_bytes(), 0o644)?;
    }
    // Karantina kipi: Rspamd milter'ı reddedilecek iletiler için Postfix'e "karantina" (bekletme kuyruğu)
    // komutu gönderir. Dosyanın panele ait olmayan satırlarına dokunulmaz.
    let proxy_path = local.join("worker-proxy.inc");
    let proxy_previous = std::fs::read_to_string(&proxy_path).ok();
    match quarantine::proxy_config(proxy_previous.as_deref(), spam.reject_action == "quarantine") {
        Some(content) => {
            if proxy_previous.as_deref() != Some(content.as_str()) {
                atomic_write(&proxy_path, content.as_bytes(), 0o644)?;
            }
        }
        None => {
            if proxy_previous.is_some() {
                std::fs::remove_file(&proxy_path)?;
            }
        }
    }
    let test = exec::run("rspamadm", &["configtest"]).await?;
    if !test.success() {
        for ((name, _), content) in files.iter().zip(previous) {
            match content {
                Some(content) => atomic_write(&local.join(name), &content, 0o644)?,
                None => {
                    let _ = std::fs::remove_file(local.join(name));
                }
            }
        }
        match proxy_previous {
            Some(content) => atomic_write(&proxy_path, content.as_bytes(), 0o644)?,
            None => {
                let _ = std::fs::remove_file(&proxy_path);
            }
        }
        return Err(AppError::bad(format!("Rspamd ayarları geçersiz: {}", test.stdout.trim())));
    }
    exec::run_ok("systemctl", &["reload-or-restart", "rspamd.service"]).await?;
    Ok(())
}

pub async fn save_spam(db: &Db, spam: &SpamSettings) -> AppResult<()> {
    write_rspamd(spam).await?;
    let encoded = serde_json::to_string(spam).map_err(|err| AppError::internal(err.to_string()))?;
    db.set_setting(SPAM_KEY, &encoded)
}

pub fn set_hostname(db: &Db, value: &str) -> AppResult<String> {
    let name = domain_rules::normalize(value)?;
    db.set_setting(HOSTNAME_KEY, &name)?;
    Ok(name)
}

/// Dovecot yapılandırmasını (ve genel "istenmeyen posta" Sieve betiğini) yazar, `doveconf -n` ile
/// sınar; geçersizse eski dosyayı geri koyar. Servisi yeniden başlatmaz ya da yüklemez.
async fn write_dovecot(db: &Db, cfg: &crate::config::Config) -> AppResult<()> {
    let account = vmail()?;
    let sieve_dir = Path::new(MAIL_DIR).join("sieve");
    std::fs::create_dir_all(&sieve_dir)?;
    let before = sieve_dir.join("before.sieve");
    atomic_write(&before, BEFORE_SIEVE.as_bytes(), 0o644)?;
    exec::run_ok("sievec", &["--", &before.to_string_lossy()]).await?;

    let name = hostname(db);
    let (cert, key) = tls_paths(cfg, &name);
    let dovecot_conf = Path::new("/etc/dovecot/dovecot.conf");
    let original = Path::new("/etc/dovecot/dovecot.conf.pixpanel-orig");
    let previous = std::fs::read(dovecot_conf).ok();
    if !original.exists() {
        if let Some(content) = &previous {
            std::fs::write(original, content)?;
        }
    }
    let rendered = DOVECOT_TEMPLATE
        .replace("{{users_file}}", &mail_file("dovecot-users").to_string_lossy())
        .replace("{{mail_root}}", MAIL_ROOT)
        .replace("{{vmail_uid}}", &account.uid.to_string())
        .replace("{{tls_cert}}", &cert.to_string_lossy())
        .replace("{{tls_key}}", &key.to_string_lossy())
        .replace("{{sieve_before}}", &before.to_string_lossy())
        .replace("{{panel_sieve_dir}}", PANEL_SIEVE_DIR);
    if previous.as_deref() == Some(rendered.as_bytes()) {
        return Ok(());
    }
    atomic_write(dovecot_conf, rendered.as_bytes(), 0o644)?;
    let test = exec::run("doveconf", &["-n"]).await?;
    if !test.success() {
        if let Some(content) = previous {
            atomic_write(dovecot_conf, &content, 0o644)?;
        }
        return Err(AppError::internal(format!("Dovecot yapılandırması geçersiz: {}", test.stderr.trim())));
    }
    Ok(())
}

/// Sieve düzeninin (panel betiği `sieve-before/` altında, ManageSieve yalnız 127.0.0.1'de) geçiş
/// işareti: dosya varsa var olan kurulum yeni düzene geçirilmiştir.
fn sieve_layout_marker() -> PathBuf {
    mail_file("sieve-layout")
}

/// Kutu sahibinin kendi işleri (parola, ManageSieve) için gereken Dovecot yapılandırmasına ve Sieve
/// düzenine var olan kurulumu panel açılırken bir kez geçirir. Sırası: yeni Dovecot yapılandırması
/// yazılıp yüklenir, sonra her kutunun panel betiği yeni yere taşınır, en sonda işaret dosyası yazılır
/// (bir adım başarısız olursa işaret yazılmaz, sonraki açılışta yeniden denenir).
pub async fn upgrade_owner_features(db: &Db, cfg: &crate::config::Config) -> AppResult<()> {
    if !is_installed() || sieve_layout_marker().exists() || sysuser::lookup(VMAIL_USER).is_none() || !Path::new("/etc/dovecot/dovecot.conf.pixpanel-orig").exists() {
        return Ok(());
    }
    write_dovecot(db, cfg).await?;
    if services::is_active("dovecot.service").await {
        // managesieve-login dinleme adresini değiştirdiği için servis yeniden yüklenir.
        exec::run_ok("systemctl", &["reload-or-restart", "dovecot.service"]).await?;
    }
    migrate_sieve_layout(db).await?;
    atomic_write(&sieve_layout_marker(), b"2\n", 0o644)?;
    tracing::info!("posta sunucusu kutu sahibi özelliklerine (ManageSieve, panel betiği ayrımı) geçirildi");
    Ok(())
}

/// Postfix, Dovecot ve Rspamd'nin temel yapılandırmasını yazar ve servisleri
/// yeniden başlatır. Tekrar tekrar çağrılabilir.
pub async fn setup(db: &Db, cfg: &crate::config::Config) -> AppResult<()> {
    if sysuser::lookup(VMAIL_USER).is_none() {
        exec::run_ok(
            "useradd",
            &["--system", "--uid", &VMAIL_UID.to_string(), "--user-group", "--home-dir", MAIL_ROOT, "--shell", "/usr/sbin/nologin", VMAIL_USER],
        )
        .await?;
    }
    let account = vmail()?;
    std::fs::create_dir_all(MAIL_ROOT)?;
    std::os::unix::fs::chown(MAIL_ROOT, Some(account.uid), Some(account.gid))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(MAIL_ROOT, std::fs::Permissions::from_mode(0o750))?;
    }

    let name = hostname(db);
    let (cert, key) = tls_paths(cfg, &name);
    let (cert, key) = (cert.to_string_lossy().into_owned(), key.to_string_lossy().into_owned());
    // Eşleme dosyaları servisler başlamadan önce var olmalı.
    sync(db).await?;

    // ---- Dovecot ----
    write_dovecot(db, cfg).await?;

    // ---- Postfix ----
    let map = |file: &str| format!("texthash:{}", mail_file(file).display());
    let mut main = vec!["-e".to_string()];
    main.extend(
        [
            ("myhostname", name.clone()),
            ("inet_interfaces", "all".into()),
            ("virtual_mailbox_domains", map("domains")),
            ("virtual_mailbox_maps", map("mailboxes")),
            ("virtual_alias_maps", map("aliases")),
            ("virtual_transport", "lmtp:unix:private/dovecot-lmtp".into()),
            ("smtpd_sasl_type", "dovecot".into()),
            ("smtpd_sasl_path", "private/auth".into()),
            ("smtpd_sender_login_maps", map("senders")),
            ("smtpd_tls_cert_file", cert.clone()),
            ("smtpd_tls_key_file", key.clone()),
            ("smtpd_tls_security_level", "may".into()),
            ("smtp_tls_security_level", "may".into()),
            ("smtpd_relay_restrictions", "permit_mynetworks,permit_sasl_authenticated,reject_unauth_destination".into()),
            ("smtpd_milters", "inet:127.0.0.1:11332".into()),
            ("non_smtpd_milters", "inet:127.0.0.1:11332".into()),
            ("milter_default_action", "accept".into()),
            ("milter_protocol", "6".into()),
            ("message_size_limit", "52428800".into()),
            ("smtpd_helo_required", "yes".into()),
            ("disable_vrfy_command", "yes".into()),
        ]
        .into_iter()
        .map(|(key, value)| format!("{key}={value}")),
    );
    postconf(&main).await?;
    for (service, extra) in [("submission", "smtpd_tls_security_level=encrypt"), ("submissions", "smtpd_tls_wrappermode=yes")] {
        postconf(&["-M".into(), format!("{service}/inet={service} inet n - y - - smtpd")]).await?;
        let mut options = vec!["-P".to_string()];
        options.extend(
            [
                format!("syslog_name=postfix/{service}"),
                extra.to_string(),
                "smtpd_sasl_auth_enable=yes".into(),
                "smtpd_tls_auth_only=yes".into(),
                "smtpd_client_restrictions=permit_sasl_authenticated,reject".into(),
                // Kimliği doğrulanan kullanıcı yalnızca kendi adresinden gönderebilir.
                "smtpd_sender_restrictions=reject_sender_login_mismatch".into(),
            ]
            .into_iter()
            .map(|option| format!("{service}/inet/{option}")),
        );
        postconf(&options).await?;
    }
    configure_local_sender_mark().await?;
    policy::configure_postfix().await?;
    exec::run_ok("postfix", &["check"]).await?;

    // ---- Rspamd ----
    write_rspamd(&load_spam(db)?).await?;

    for (service, _) in SERVICES {
        exec::run_ok("systemctl", &["enable", "--quiet", "--", service]).await?;
        exec::run_ok("systemctl", &["restart", "--", service]).await?;
    }
    // Var olan kutuların panel betikleri yeni düzene taşınır (yeni kurulumda kutu yoktur).
    match migrate_sieve_layout(db).await {
        Ok(()) => atomic_write(&sieve_layout_marker(), b"2\n", 0o644)?,
        Err(err) => tracing::warn!(error = %err, "Sieve düzeni geçişi tamamlanamadı; panel açılışında yeniden denenecek"),
    }
    Ok(())
}
