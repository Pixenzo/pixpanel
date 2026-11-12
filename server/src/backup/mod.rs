//! Yedekleme ve geri yükleme.
//!
//! Depolama için restic kullanılır: yedekler şifreli, artımlı ve yinelemesizdir.
//! Her hedefte (yerel klasör, SFTP, S3) tek bir restic deposu bulunur; her site,
//! veritabanı, posta alanı ve panelin kendisi ayrı bir anlık görüntü olarak
//! saklanır ve etiketlerle (`kind:`, `name:`, `plan:` …) ayırt edilir.
//!
//! İlk sürümde yedekleme yalnızca yöneticiye açıktır.

pub mod defaults;
pub mod restic;
pub mod run;

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::sites::{self, Site};
use crate::sys::exec;

/// Hedeflerin SSH anahtarlarının ve bilinen sunucu listesinin durduğu klasör.
pub const KEY_DIR: &str = "/etc/pixpanel/backup";

/// Aynı anda yalnızca bir yedekleme ya da geri yükleme yürür.
pub static RUN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn is_installed() -> bool {
    exec::binary_exists("restic")
}

// ---- Hedefler ----

pub const KINDS: &[&str] = &["local", "sftp", "s3", "ftp"];

/// Hedefin bağlantı ayarları; türe göre yalnızca ilgili alanlar kullanılır.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct TargetConfig {
    /// Yerel: mutlak klasör. SFTP: uzak sunucudaki klasör. S3: kova içindeki önek.
    pub path: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// S3 uç noktası, ör. `https://s3.eu-central-1.amazonaws.com`.
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    /// FTP: bağlantı TLS ile şifrelensin (FTPES). Kapalıysa parola ağda açık gider.
    pub tls: bool,
    /// FTP: sunucunun sertifikası doğrulanmasın (kendinden imzalı sertifikalı sunucular için).
    pub tls_skip_verify: bool,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub config: TargetConfig,
    /// S3 gizli anahtarı.
    pub secret: String,
    /// restic deposunun parolası.
    pub password: String,
    /// SFTP hedefinin uzak sunucuya eklenecek açık anahtarı.
    pub public_key: String,
    pub last_check_at: Option<i64>,
    pub last_check_ok: bool,
    pub last_check_error: String,
    pub created_at: i64,
}

impl Target {
    pub fn key_path(&self) -> PathBuf {
        Path::new(KEY_DIR).join(format!("target-{}.key", self.id))
    }
}

fn plain(value: &str, extra: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// Yol yalnızca güvenli karakterlerden oluşur ve üst klasöre çıkmaz.
fn validate_path(value: &str, what: &str) -> AppResult<String> {
    let path = value.trim().trim_end_matches('/').to_string();
    let valid = plain(&path, "._/-") && path.len() <= 255 && !path.split('/').any(|part| part == ".." || part == ".") && !path.starts_with('-');
    if valid { Ok(path) } else { Err(AppError::bad(format!("{what} geçersiz; yalnızca harf, rakam, nokta, tire, alt çizgi ve / içerebilir"))) }
}

/// Yedeklenen klasörlerin içine depo kurulmasına izin verilmez.
const FORBIDDEN_LOCAL: &[&str] = &["/home", "/etc", "/var/vmail", "/var/lib/pixpanel", "/proc", "/sys", "/dev", "/run", "/tmp", "/boot", "/usr", "/bin", "/sbin", "/lib"];

impl TargetConfig {
    /// Türe göre doğrular; kullanılmayan alanları boşaltır.
    pub fn normalized(&self, kind: &str) -> AppResult<Self> {
        let mut out = TargetConfig::default();
        match kind {
            "local" => {
                let path = validate_path(&self.path, "Klasör")?;
                if !path.starts_with('/') || path.len() < 2 {
                    return Err(AppError::bad("Klasör / ile başlayan tam bir yol olmalı, ör. /var/backups/pixpanel"));
                }
                if FORBIDDEN_LOCAL.iter().any(|dir| path == *dir || path.starts_with(&format!("{dir}/"))) {
                    return Err(AppError::bad("Bu klasör yedek hedefi olamaz; /var/backups/pixpanel ya da bağlı bir disk (/mnt/…) seçin"));
                }
                out.path = path;
            }
            "sftp" => {
                let host = self.host.trim().to_ascii_lowercase();
                if !plain(&host, ".-") || host.len() > 253 || host.starts_with('-') {
                    return Err(AppError::bad("Sunucu adresi geçersiz"));
                }
                let user = self.user.trim().to_string();
                if !plain(&user, "._-") || user.len() > 64 || user.starts_with('-') {
                    return Err(AppError::bad("Kullanıcı adı geçersiz"));
                }
                out.host = host;
                out.user = user;
                out.port = if self.port == 0 { 22 } else { self.port };
                out.path = validate_path(&self.path, "Uzak klasör")?;
            }
            "s3" => {
                let endpoint = self.endpoint.trim().trim_end_matches('/').to_string();
                let parsed = url::Url::parse(&endpoint).map_err(|_| AppError::bad("Uç nokta adresi geçersiz; ör. https://s3.eu-central-1.amazonaws.com"))?;
                let clean = matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some() && parsed.path() == "/" && parsed.query().is_none() && parsed.username().is_empty();
                if !clean || !plain(&endpoint, ".-:/") {
                    return Err(AppError::bad("Uç nokta yalnızca http(s)://sunucu[:port] biçiminde olabilir"));
                }
                let bucket = self.bucket.trim().to_string();
                if !(3..=63).contains(&bucket.len()) || !plain(&bucket, ".-") || bucket.starts_with('-') {
                    return Err(AppError::bad("Kova adı geçersiz"));
                }
                let region = self.region.trim().to_string();
                if !region.is_empty() && (!plain(&region, "-") || region.len() > 40) {
                    return Err(AppError::bad("Bölge geçersiz"));
                }
                let access_key = self.access_key.trim().to_string();
                if access_key.is_empty() || access_key.len() > 200 || access_key.chars().any(|c| c.is_control() || c.is_whitespace()) {
                    return Err(AppError::bad("Erişim anahtarı geçersiz"));
                }
                out.endpoint = endpoint;
                out.bucket = bucket;
                out.region = region;
                out.access_key = access_key;
                out.path = if self.path.trim().trim_matches('/').is_empty() { String::new() } else { validate_path(self.path.trim().trim_start_matches('/'), "Önek")? };
            }
            "ftp" => {
                let host = self.host.trim().to_ascii_lowercase();
                if !plain(&host, ".-") || host.len() > 253 || host.starts_with('-') {
                    return Err(AppError::bad("Sunucu adresi geçersiz"));
                }
                let user = self.user.trim().to_string();
                // FTP kullanıcı adlarında `@` yaygındır (ör. `yedek@ornek.com`).
                if !plain(&user, "._-@") || user.len() > 100 || user.starts_with('-') {
                    return Err(AppError::bad("Kullanıcı adı geçersiz"));
                }
                out.host = host;
                out.user = user;
                out.port = if self.port == 0 { 21 } else { self.port };
                out.path = validate_path(self.path.trim().trim_start_matches('/'), "Uzak klasör")?;
                out.tls = self.tls;
                out.tls_skip_verify = self.tls && self.tls_skip_verify;
            }
            _ => return Err(AppError::bad("Geçersiz hedef türü")),
        }
        Ok(out)
    }
}

pub fn validate_name(value: &str) -> AppResult<String> {
    let name = value.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err(AppError::bad("Ad 1–60 karakter olmalı"));
    }
    Ok(name)
}

pub fn validate_secret(value: &str) -> AppResult<String> {
    if value.is_empty() || value.len() > 300 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(AppError::bad("Gizli anahtar geçersiz"));
    }
    Ok(value.to_string())
}

const TARGET_COLUMNS: &str = "id, name, kind, config, secret, password, public_key, last_check_at, last_check_ok, last_check_error, created_at";

fn read_target(row: &Row<'_>) -> rusqlite::Result<Target> {
    let config: String = row.get(3)?;
    Ok(Target {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        config: serde_json::from_str(&config).unwrap_or_default(),
        secret: row.get(4)?,
        password: row.get(5)?,
        public_key: row.get(6)?,
        last_check_at: row.get(7)?,
        last_check_ok: row.get(8)?,
        last_check_error: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub fn list_targets(db: &Db) -> AppResult<Vec<Target>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {TARGET_COLUMNS} FROM backup_targets ORDER BY name"))?;
        let rows = stmt.query_map([], read_target)?;
        rows.collect()
    })
}

pub fn load_target(db: &Db, id: i64) -> AppResult<Target> {
    db.call(|conn| conn.query_row(&format!("SELECT {TARGET_COLUMNS} FROM backup_targets WHERE id = ?1"), [id], read_target).optional())?
        .ok_or(AppError::NotFound)
}

fn config_json(config: &TargetConfig) -> AppResult<String> {
    serde_json::to_string(config).map_err(|err| AppError::internal(err.to_string()))
}

pub fn insert_target(db: &Db, name: &str, kind: &str, config: &TargetConfig, secret: &str, password: &str) -> AppResult<i64> {
    let config = config_json(config)?;
    let outcome = db.call(|conn| {
        let taken: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM backup_targets WHERE name = ?1)", [name], |row| row.get(0))?;
        if taken {
            return Ok(None);
        }
        conn.execute(
            "INSERT INTO backup_targets (name, kind, config, secret, password, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, kind, config, secret, password, now()],
        )?;
        Ok(Some(conn.last_insert_rowid()))
    })?;
    outcome.ok_or_else(|| AppError::conflict("Bu adda bir hedef zaten var"))
}

pub fn update_target(db: &Db, target: &Target) -> AppResult<()> {
    let config = config_json(&target.config)?;
    let outcome = db.call(|conn| {
        let taken: bool =
            conn.query_row("SELECT EXISTS (SELECT 1 FROM backup_targets WHERE name = ?1 AND id <> ?2)", params![target.name, target.id], |row| row.get(0))?;
        if taken {
            return Ok(false);
        }
        conn.execute(
            "UPDATE backup_targets SET name = ?1, config = ?2, secret = ?3, public_key = ?4 WHERE id = ?5",
            params![target.name, config, target.secret, target.public_key, target.id],
        )?;
        Ok(true)
    })?;
    if outcome { Ok(()) } else { Err(AppError::conflict("Bu adda bir hedef zaten var")) }
}

pub fn save_check(db: &Db, id: i64, error: Option<&str>) -> AppResult<()> {
    db.call(|conn| {
        conn.execute(
            "UPDATE backup_targets SET last_check_at = ?1, last_check_ok = ?2, last_check_error = ?3 WHERE id = ?4",
            params![now(), error.is_none(), error.unwrap_or(""), id],
        )
    })?;
    Ok(())
}

/// Hedefi siler. Kullanan plan varsa reddedilir; depodaki yedeklere dokunulmaz.
pub fn delete_target(db: &Db, target: &Target) -> AppResult<()> {
    let used: i64 = db.call(|conn| conn.query_row("SELECT COUNT(*) FROM backup_plans WHERE target_id = ?1", [target.id], |row| row.get(0)))?;
    if used > 0 {
        return Err(AppError::conflict("Bu hedefi kullanan planlar var; önce onları silin ya da başka hedefe taşıyın"));
    }
    db.call(|conn| conn.execute("DELETE FROM backup_targets WHERE id = ?1", [target.id]))?;
    let key = target.key_path();
    let _ = std::fs::remove_file(key.with_extension("key.pub"));
    let _ = std::fs::remove_file(key);
    Ok(())
}

/// SFTP hedefinin anahtar çiftini (yoksa) üretir ve açık anahtarı döner.
pub async fn ensure_key(target: &Target) -> AppResult<String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(KEY_DIR)?;
    std::fs::set_permissions(KEY_DIR, std::fs::Permissions::from_mode(0o700))?;
    let key = target.key_path();
    let key_str = key.to_string_lossy().into_owned();
    if !key.exists() {
        exec::run_ok("ssh-keygen", &["-q", "-t", "ed25519", "-N", "", "-C", "pixpanel-backup", "-f", &key_str]).await?;
    }
    let public = exec::run_ok("ssh-keygen", &["-y", "-f", &key_str]).await?;
    Ok(format!("{} pixpanel-backup", public.trim()))
}

// ---- Kapsam: neyin yedekleneceği ----

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Scope {
    pub all_sites: bool,
    pub site_ids: Vec<i64>,
    pub all_databases: bool,
    pub database_ids: Vec<i64>,
    pub all_mail: bool,
    pub mail_domain_ids: Vec<i64>,
    /// Panelin kendi veritabanı ve `/etc/pixpanel`.
    pub panel: bool,
}

impl Scope {
    pub fn is_empty(&self) -> bool {
        !(self.all_sites || self.all_databases || self.all_mail || self.panel) && self.site_ids.is_empty() && self.database_ids.is_empty() && self.mail_domain_ids.is_empty()
    }
}

/// Seçilebilir bir veritabanı.
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseSource {
    pub id: i64,
    pub engine: String,
    pub name: String,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailSource {
    pub id: i64,
    pub domain: String,
    pub owner: String,
}

pub fn all_sites(db: &Db) -> AppResult<Vec<Site>> {
    let ids: Vec<i64> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM sites ORDER BY domain")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    })?;
    let mut list = Vec::new();
    for id in ids {
        if let Some(site) = sites::load(db, id)? {
            list.push(site);
        }
    }
    Ok(list)
}

pub fn all_databases(db: &Db) -> AppResult<Vec<DatabaseSource>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT d.id, d.engine, d.name, u.username FROM databases d JOIN users u ON u.id = d.user_id ORDER BY d.engine, d.name")?;
        let rows = stmt.query_map([], |row| Ok(DatabaseSource { id: row.get(0)?, engine: row.get(1)?, name: row.get(2)?, owner: row.get(3)? }))?;
        rows.collect()
    })
}

pub fn all_mail(db: &Db) -> AppResult<Vec<MailSource>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT m.id, m.domain, u.username FROM mail_domains m JOIN users u ON u.id = m.user_id ORDER BY m.domain")?;
        let rows = stmt.query_map([], |row| Ok(MailSource { id: row.get(0)?, domain: row.get(1)?, owner: row.get(2)? }))?;
        rows.collect()
    })
}

/// Yedeklenecek tek bir birim.
#[derive(Debug, Clone)]
pub enum Item {
    Site(Box<Site>),
    Database(DatabaseSource),
    Mail(MailSource),
    Panel,
}

impl Item {
    pub fn label(&self) -> String {
        match self {
            Item::Site(site) => format!("site {}", site.domain),
            Item::Database(database) => format!("veritabanı {} ({})", database.name, database.engine),
            Item::Mail(mail) => format!("posta {}", mail.domain),
            Item::Panel => "panel".to_string(),
        }
    }
}

/// Kapsamı, o an var olan birimlere çevirir. Silinmiş kayıtlara ait kimlikler atlanır.
pub fn resolve(db: &Db, scope: &Scope) -> AppResult<Vec<Item>> {
    let mut items = Vec::new();
    for site in all_sites(db)? {
        if scope.all_sites || scope.site_ids.contains(&site.id) {
            items.push(Item::Site(Box::new(site)));
        }
    }
    for database in all_databases(db)? {
        if scope.all_databases || scope.database_ids.contains(&database.id) {
            items.push(Item::Database(database));
        }
    }
    for mail in all_mail(db)? {
        if scope.all_mail || scope.mail_domain_ids.contains(&mail.id) {
            items.push(Item::Mail(mail));
        }
    }
    if scope.panel {
        items.push(Item::Panel);
    }
    Ok(items)
}

// ---- Planlar ----

pub const FREQUENCIES: &[&str] = &["manual", "daily", "weekly", "monthly"];

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub target_id: i64,
    pub scope: Scope,
    pub frequency: String,
    /// Gece yarısından itibaren dakika (sunucunun yerel saatiyle).
    pub at_minute: i64,
    /// 1 = Pazartesi … 7 = Pazar.
    pub weekday: i64,
    pub month_day: i64,
    pub keep_daily: i64,
    pub keep_weekly: i64,
    pub keep_monthly: i64,
    pub enabled: bool,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_status: String,
    pub last_job_id: Option<i64>,
    pub created_at: i64,
}

impl Plan {
    pub fn validate(&self) -> AppResult<()> {
        if !FREQUENCIES.contains(&self.frequency.as_str()) {
            return Err(AppError::bad("Geçersiz sıklık"));
        }
        if !(0..1440).contains(&self.at_minute) || !(1..=7).contains(&self.weekday) || !(1..=28).contains(&self.month_day) {
            return Err(AppError::bad("Geçersiz zaman; ayın günü 1–28 arasında olmalı"));
        }
        for keep in [self.keep_daily, self.keep_weekly, self.keep_monthly] {
            if !(0..=366).contains(&keep) {
                return Err(AppError::bad("Saklanacak kopya sayısı 0–366 arasında olmalı"));
            }
        }
        if self.scope.is_empty() {
            return Err(AppError::bad("Yedeklenecek en az bir şey seçin"));
        }
        Ok(())
    }

    /// Planın `now` anından sonraki ilk çalışma zamanı. `offset` sunucunun UTC'ye
    /// göre saniye cinsinden farkıdır.
    pub fn next_run(&self, now: i64, offset: i64) -> Option<i64> {
        if !self.enabled || self.frequency == "manual" {
            return None;
        }
        let local = now + offset;
        let day_start = local - local.rem_euclid(86400);
        (0..=62).find_map(|ahead| {
            let start = day_start + ahead * 86400;
            let candidate = start + self.at_minute * 60;
            let days = start.div_euclid(86400);
            let matches = match self.frequency.as_str() {
                "daily" => true,
                // 1 Ocak 1970 perşembeydi.
                "weekly" => (days + 3).rem_euclid(7) + 1 == self.weekday,
                _ => civil_day(days) == self.month_day,
            };
            (matches && candidate > local).then_some(candidate - offset)
        })
    }
}

/// Unix gün sayısından ayın gününü bulur (Howard Hinnant'ın takvim algoritması).
fn civil_day(days: i64) -> i64 {
    let z = days + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    doy - (153 * mp + 2) / 5 + 1
}

/// Sunucunun yerel saatinin UTC'ye göre farkı (saniye).
pub async fn local_offset() -> i64 {
    let Ok(output) = exec::run("date", &["+%z"]).await else { return 0 };
    let text = output.stdout.trim();
    if text.len() != 5 {
        return 0;
    }
    let sign = if text.starts_with('-') { -1 } else { 1 };
    let hours: i64 = text[1..3].parse().unwrap_or(0);
    let minutes: i64 = text[3..5].parse().unwrap_or(0);
    sign * (hours * 3600 + minutes * 60)
}

const PLAN_COLUMNS: &str = "id, user_id, name, target_id, scope, frequency, at_minute, weekday, month_day, keep_daily, keep_weekly, keep_monthly,
     enabled, next_run_at, last_run_at, last_status, last_job_id, created_at";

fn read_plan(row: &Row<'_>) -> rusqlite::Result<Plan> {
    let scope: String = row.get(4)?;
    Ok(Plan {
        id: row.get(0)?,
        user_id: row.get(1)?,
        name: row.get(2)?,
        target_id: row.get(3)?,
        scope: serde_json::from_str(&scope).unwrap_or_default(),
        frequency: row.get(5)?,
        at_minute: row.get(6)?,
        weekday: row.get(7)?,
        month_day: row.get(8)?,
        keep_daily: row.get(9)?,
        keep_weekly: row.get(10)?,
        keep_monthly: row.get(11)?,
        enabled: row.get(12)?,
        next_run_at: row.get(13)?,
        last_run_at: row.get(14)?,
        last_status: row.get(15)?,
        last_job_id: row.get(16)?,
        created_at: row.get(17)?,
    })
}

pub fn list_plans(db: &Db) -> AppResult<Vec<Plan>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {PLAN_COLUMNS} FROM backup_plans ORDER BY name"))?;
        let rows = stmt.query_map([], read_plan)?;
        rows.collect()
    })
}

pub fn load_plan(db: &Db, id: i64) -> AppResult<Plan> {
    db.call(|conn| conn.query_row(&format!("SELECT {PLAN_COLUMNS} FROM backup_plans WHERE id = ?1"), [id], read_plan).optional())?.ok_or(AppError::NotFound)
}

/// Planı ekler (kimliği 0 ise) ya da günceller; kimliğini döner.
pub fn save_plan(db: &Db, plan: &Plan) -> AppResult<i64> {
    let scope = serde_json::to_string(&plan.scope).map_err(|err| AppError::internal(err.to_string()))?;
    db.call(|conn| {
        if plan.id == 0 {
            conn.execute(
                "INSERT INTO backup_plans (user_id, name, target_id, scope, frequency, at_minute, weekday, month_day,
                        keep_daily, keep_weekly, keep_monthly, enabled, next_run_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    plan.user_id, plan.name, plan.target_id, scope, plan.frequency, plan.at_minute, plan.weekday, plan.month_day,
                    plan.keep_daily, plan.keep_weekly, plan.keep_monthly, plan.enabled, plan.next_run_at, now()
                ],
            )?;
            Ok(conn.last_insert_rowid())
        } else {
            conn.execute(
                "UPDATE backup_plans SET name = ?1, target_id = ?2, scope = ?3, frequency = ?4, at_minute = ?5, weekday = ?6, month_day = ?7,
                        keep_daily = ?8, keep_weekly = ?9, keep_monthly = ?10, enabled = ?11, next_run_at = ?12
                 WHERE id = ?13",
                params![
                    plan.name, plan.target_id, scope, plan.frequency, plan.at_minute, plan.weekday, plan.month_day,
                    plan.keep_daily, plan.keep_weekly, plan.keep_monthly, plan.enabled, plan.next_run_at, plan.id
                ],
            )?;
            Ok(plan.id)
        }
    })
}

pub fn delete_plan(db: &Db, id: i64) -> AppResult<()> {
    db.call(|conn| conn.execute("DELETE FROM backup_plans WHERE id = ?1", [id]))?;
    Ok(())
}

pub fn set_plan_next(db: &Db, id: i64, next_run_at: Option<i64>) -> AppResult<()> {
    db.call(|conn| conn.execute("UPDATE backup_plans SET next_run_at = ?1 WHERE id = ?2", params![next_run_at, id]))?;
    Ok(())
}

pub fn set_plan_started(db: &Db, id: i64, job_id: i64) -> AppResult<()> {
    db.call(|conn| conn.execute("UPDATE backup_plans SET last_run_at = ?1, last_status = 'running', last_job_id = ?2 WHERE id = ?3", params![now(), job_id, id]))?;
    Ok(())
}

pub fn set_plan_status(db: &Db, id: i64, status: &str) -> AppResult<()> {
    db.call(|conn| conn.execute("UPDATE backup_plans SET last_status = ?1 WHERE id = ?2", params![status, id]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(frequency: &str) -> Plan {
        Plan {
            id: 1,
            user_id: 1,
            name: "p".into(),
            target_id: 1,
            scope: Scope { panel: true, ..Scope::default() },
            frequency: frequency.into(),
            at_minute: 3 * 60,
            weekday: 1,
            month_day: 15,
            keep_daily: 7,
            keep_weekly: 4,
            keep_monthly: 6,
            enabled: true,
            next_run_at: None,
            last_run_at: None,
            last_status: String::new(),
            last_job_id: None,
            created_at: 0,
        }
    }

    // 10 Kasım 2026 Salı 12:00 UTC
    const NOW: i64 = 1_794_312_000;

    #[test]
    fn daily_plan_runs_next_morning() {
        // UTC+3: yerel saat 15:00, sıradaki 03:00 ertesi gün → 11 Kasım 00:00 UTC.
        assert_eq!(plan("daily").next_run(NOW, 3 * 3600), Some(NOW + 12 * 3600));
        // Saat henüz gelmediyse aynı gün çalışır.
        let mut late = plan("daily");
        late.at_minute = 23 * 60;
        assert_eq!(late.next_run(NOW, 0), Some(NOW + 11 * 3600));
    }

    #[test]
    fn weekly_and_monthly_plans_pick_the_right_day() {
        // Sıradaki pazartesi 16 Kasım 03:00 UTC.
        assert_eq!(plan("weekly").next_run(NOW, 0), Some(NOW - 12 * 3600 + 6 * 86400 + 3 * 3600));
        // Ayın 15'i 03:00 UTC.
        assert_eq!(plan("monthly").next_run(NOW, 0), Some(NOW - 12 * 3600 + 5 * 86400 + 3 * 3600));
        assert_eq!(civil_day(NOW / 86400), 10);
    }

    #[test]
    fn manual_and_disabled_plans_never_run() {
        assert_eq!(plan("manual").next_run(NOW, 0), None);
        let mut disabled = plan("daily");
        disabled.enabled = false;
        assert_eq!(disabled.next_run(NOW, 0), None);
    }

    #[test]
    fn target_config_is_validated() {
        let local = |path: &str| TargetConfig { path: path.into(), ..TargetConfig::default() }.normalized("local");
        assert!(local("/var/backups/pixpanel").is_ok());
        assert!(local("/home/ali/yedek").is_err());
        assert!(local("/var/backups/../etc").is_err());
        assert!(local("yedek").is_err());
        assert!(local("/mnt/disk; rm -rf /").is_err());

        let sftp = |host: &str, user: &str| TargetConfig { host: host.into(), user: user.into(), path: "yedek".into(), ..TargetConfig::default() }.normalized("sftp");
        assert_eq!(sftp("Yedek.Example.com", "ali").unwrap().port, 22);
        assert!(sftp("-oProxyCommand=x", "ali").is_err());
        assert!(sftp("example.com", "ali -o x").is_err());

        let s3 = |endpoint: &str| {
            TargetConfig { endpoint: endpoint.into(), bucket: "yedekler".into(), access_key: "AKIA".into(), ..TargetConfig::default() }.normalized("s3")
        };
        assert!(s3("https://s3.example.com").is_ok());
        assert!(s3("https://user:pw@s3.example.com").is_err());
        assert!(s3("file:///etc").is_err());
    }
}
