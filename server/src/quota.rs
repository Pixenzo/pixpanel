//! Hizmet paketleri ve kotalar: bir hesabın kaç site, veritabanı, posta kutusu...
//! açabileceğini ve ne kadar disk kullanabileceğini belirler.
//!
//! Paket hesaba atanır; alt hesaplar bağlı oldukları hesabın paketine tabidir.
//! Yöneticilerin sınırı yoktur. Disk kotası "yumuşak"tır: panel kullanımı düzenli
//! ölçer, sınır aşılınca yeni kaynak açılmasını engeller ama var olan dosyalara
//! dokunmaz (dosya sistemi kotası konteynerlerde güvenilir çalışmadığı için).

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Site,
    Database,
    MailDomain,
    Mailbox,
    FtpAccount,
    DnsZone,
}

impl Resource {
    /// Sınır aşıldığında kullanıcıya gösterilen ad.
    fn label(self) -> &'static str {
        match self {
            Resource::Site => "site",
            Resource::Database => "veritabanı",
            Resource::MailDomain => "posta alan adı",
            Resource::Mailbox => "posta kutusu",
            Resource::FtpAccount => "FTP hesabı",
            Resource::DnsZone => "DNS bölgesi",
        }
    }

    /// Hesabın bu türden kaç kaynağı olduğunu sayan sorgu (`?1`: hesap kimliği).
    fn count_sql(self) -> &'static str {
        match self {
            Resource::Site => "SELECT COUNT(*) FROM sites WHERE user_id = ?1",
            Resource::Database => "SELECT COUNT(*) FROM databases WHERE user_id = ?1",
            Resource::MailDomain => "SELECT COUNT(*) FROM mail_domains WHERE user_id = ?1",
            Resource::Mailbox => "SELECT COUNT(*) FROM mailboxes m JOIN mail_domains d ON d.id = m.domain_id WHERE d.user_id = ?1",
            Resource::FtpAccount => "SELECT COUNT(*) FROM ftp_accounts f JOIN sites s ON s.id = f.site_id WHERE s.user_id = ?1",
            Resource::DnsZone => "SELECT COUNT(*) FROM dns_zones WHERE user_id = ?1",
        }
    }
}

/// Paketin sınırları. `None` sınırsız demektir.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub sites: Option<i64>,
    pub databases: Option<i64>,
    pub mail_domains: Option<i64>,
    pub mailboxes: Option<i64>,
    pub ftp_accounts: Option<i64>,
    pub dns_zones: Option<i64>,
    /// Disk sınırı (MB): site dosyaları ve posta kutuları.
    pub disk_mb: Option<i64>,
}

impl Limits {
    fn of(&self, resource: Resource) -> Option<i64> {
        match resource {
            Resource::Site => self.sites,
            Resource::Database => self.databases,
            Resource::MailDomain => self.mail_domains,
            Resource::Mailbox => self.mailboxes,
            Resource::FtpAccount => self.ftp_accounts,
            Resource::DnsZone => self.dns_zones,
        }
    }

    fn fields(&self) -> [(&'static str, Option<i64>); 7] {
        [
            ("site", self.sites),
            ("veritabanı", self.databases),
            ("posta alan adı", self.mail_domains),
            ("posta kutusu", self.mailboxes),
            ("FTP hesabı", self.ftp_accounts),
            ("DNS bölgesi", self.dns_zones),
            ("disk", self.disk_mb),
        ]
    }

    pub fn validate(&self) -> AppResult<()> {
        for (label, value) in self.fields() {
            if value.is_some_and(|value| !(0..=1_000_000_000).contains(&value)) {
                return Err(AppError::bad(format!("Geçersiz {label} sınırı")));
            }
        }
        Ok(())
    }

    /// Bu sınırlar `ceiling`i hiçbir alanda aşmıyor mu? Bayi, kendi paketinden geniş paket veremez.
    pub fn within(&self, ceiling: &Limits) -> Result<(), &'static str> {
        for ((label, mine), (_, theirs)) in self.fields().into_iter().zip(ceiling.fields()) {
            match (mine, theirs) {
                (_, None) => {}
                (Some(mine), Some(theirs)) if mine <= theirs => {}
                _ => return Err(label),
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub id: i64,
    /// Paketi tanımlayan hesap (yönetici ya da bayi).
    pub owner_id: i64,
    pub name: String,
    pub limits: Limits,
    /// Bu paketi kullanan hesap sayısı.
    pub accounts: i64,
    pub created_at: i64,
}

fn read_package(row: &rusqlite::Row) -> rusqlite::Result<Package> {
    let limits: String = row.get(3)?;
    Ok(Package {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        limits: serde_json::from_str(&limits).unwrap_or_default(),
        accounts: row.get(4)?,
        created_at: row.get(5)?,
    })
}

const PACKAGE_COLUMNS: &str = "p.id, p.owner_id, p.name, p.limits, (SELECT COUNT(*) FROM users u WHERE u.package_id = p.id), p.created_at";

/// `owner`: verilirse yalnızca o hesabın tanımladığı paketler.
pub fn list_packages(db: &Db, owner: Option<i64>) -> AppResult<Vec<Package>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {PACKAGE_COLUMNS} FROM packages p WHERE ?1 IS NULL OR p.owner_id = ?1 ORDER BY p.name"))?;
        let rows = stmt.query_map(params![owner], read_package)?;
        rows.collect()
    })
}

pub fn load_package(db: &Db, id: i64) -> AppResult<Package> {
    db.call(|conn| conn.query_row(&format!("SELECT {PACKAGE_COLUMNS} FROM packages p WHERE p.id = ?1"), params![id], read_package).optional())?
        .ok_or(AppError::NotFound)
}

pub fn validate_name(value: &str) -> AppResult<String> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err(AppError::bad("Paket adı 1-60 karakter olmalı"));
    }
    Ok(name.to_string())
}

/// Hesabın tabi olduğu sınırlar ve paketin adı. Yönetici ya da paketsiz hesap için `None`.
pub fn limits_for(db: &Db, account_id: i64) -> AppResult<Option<(String, Limits)>> {
    let row: Option<(String, Option<String>, Option<String>)> = db.call(|conn| {
        conn.query_row(
            "SELECT u.role, p.name, p.limits FROM users u LEFT JOIN packages p ON p.id = u.package_id WHERE u.id = ?1",
            params![account_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
    })?;
    Ok(match row {
        Some((role, Some(name), Some(limits))) if role != "admin" => Some((name, serde_json::from_str(&limits).unwrap_or_default())),
        _ => None,
    })
}

pub fn count(db: &Db, account_id: i64, resource: Resource) -> AppResult<i64> {
    db.call(|conn| conn.query_row(resource.count_sql(), params![account_id], |row| row.get(0)))
}

/// Son ölçülen disk kullanımı (bayt) ve ölçüm anı.
pub fn disk_usage(db: &Db, account_id: i64) -> AppResult<Option<(i64, i64)>> {
    db.call(|conn| conn.query_row("SELECT disk_bytes, measured_at FROM account_usage WHERE user_id = ?1", params![account_id], |row| Ok((row.get(0)?, row.get(1)?))).optional())
}

/// Hesap bu türden bir kaynak daha açabilir mi? Açamıyorsa nedenini söyleyen hata döner.
pub fn check(db: &Db, account_id: i64, resource: Resource) -> AppResult<()> {
    let Some((package, limits)) = limits_for(db, account_id)? else { return Ok(()) };
    if let Some(limit_mb) = limits.disk_mb {
        if let Some((bytes, _)) = disk_usage(db, account_id)? {
            if bytes >= limit_mb * 1024 * 1024 {
                return Err(AppError::bad(format!(
                    "Disk kotası doldu ({limit_mb} MB, “{package}” paketi). Yeni {} açmak için yer açın ya da paketi yükseltin.",
                    resource.label()
                )));
            }
        }
    }
    let Some(limit) = limits.of(resource) else { return Ok(()) };
    let used = count(db, account_id, resource)?;
    if used >= limit {
        return Err(AppError::bad(if limit == 0 {
            format!("“{package}” paketi {} içermiyor.", resource.label())
        } else {
            format!("“{package}” paketi en çok {limit} {} içeriyor; hepsi kullanılıyor.", resource.label())
        }));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub package: Option<String>,
    pub limits: Limits,
    pub sites: i64,
    pub databases: i64,
    pub mail_domains: i64,
    pub mailboxes: i64,
    pub ftp_accounts: i64,
    pub dns_zones: i64,
    /// İçinde bulunulan ayda sitelerin aktardığı veri (bayt).
    pub traffic_bytes: i64,
    /// Son ölçülen disk kullanımı (bayt); henüz ölçülmediyse `None`.
    pub disk_bytes: Option<i64>,
    pub disk_measured_at: Option<i64>,
}

pub fn usage(db: &Db, account_id: i64) -> AppResult<Usage> {
    let (package, limits) = match limits_for(db, account_id)? {
        Some((name, limits)) => (Some(name), limits),
        None => (None, Limits::default()),
    };
    let disk = disk_usage(db, account_id)?;
    Ok(Usage {
        package,
        limits,
        sites: count(db, account_id, Resource::Site)?,
        databases: count(db, account_id, Resource::Database)?,
        mail_domains: count(db, account_id, Resource::MailDomain)?,
        mailboxes: count(db, account_id, Resource::Mailbox)?,
        ftp_accounts: count(db, account_id, Resource::FtpAccount)?,
        dns_zones: count(db, account_id, Resource::DnsZone)?,
        traffic_bytes: crate::stats::account_month_bytes(db, account_id)?,
        disk_bytes: disk.map(|(bytes, _)| bytes),
        disk_measured_at: disk.map(|(_, at)| at),
    })
}

/// Bir klasörün diskte kapladığı yer (bayt); klasör yoksa 0.
async fn directory_size(path: &str) -> i64 {
    if !std::path::Path::new(path).is_dir() {
        return 0;
    }
    // `-x`: başka dosya sistemlerine geçmez; `--`: yol seçenek sanılmaz.
    let output = crate::sys::exec::run_with_timeout("du", &["-sxb", "--", path], std::time::Duration::from_secs(600)).await;
    output.ok().and_then(|out| out.stdout.split_whitespace().next().and_then(|size| size.parse().ok())).unwrap_or(0)
}

/// Bütün hesapların disk kullanımını ölçüp kaydeder: ev klasörü (siteler) ve posta kutuları.
/// Kotanın yüzde kaçı dolunca uyarı düzeyine geçilir.
const WARN_PERCENT: i64 = 90;

/// Kullanıma göre uyarı düzeyi: 0 (sorun yok), 1 (dolmak üzere), 2 (doldu).
pub fn warning_level(used_bytes: i64, limit_mb: i64) -> i64 {
    let limit = limit_mb.saturating_mul(1024 * 1024);
    if limit <= 0 {
        return 0;
    }
    if used_bytes >= limit {
        2
    } else if used_bytes.saturating_mul(100) >= limit.saturating_mul(WARN_PERCENT) {
        1
    } else {
        0
    }
}

/// Disk kotası dolmak üzere olan ya da dolan hesaplara haber verir. Bir düzey için yalnızca
/// bir kez gönderilir; kullanım düşünce düzey sıfırlanır ve sonraki aşımda yeniden haber verilir.
pub async fn notify_disk(state: &crate::state::AppState) -> AppResult<()> {
    let rows: Vec<(i64, i64, i64)> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT user_id, disk_bytes, warned_level FROM account_usage")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    })?;
    for (user_id, used, warned) in rows {
        let Some(limit_mb) = limits_for(&state.db, user_id)?.and_then(|(_, limits)| limits.disk_mb) else { continue };
        let level = warning_level(used, limit_mb);
        if level == warned {
            continue;
        }
        state.db.call(|conn| conn.execute("UPDATE account_usage SET warned_level = ?1 WHERE user_id = ?2", params![level, user_id]))?;
        if level > warned {
            let used_mb = used / 1024 / 1024;
            let (subject, body) = if level == 2 {
                (
                    "Disk kotanız doldu".to_string(),
                    format!("Hesabınız {used_mb} MB kullanıyor; paketinizin sınırı {limit_mb} MB. Yeni site, veritabanı ya da posta kutusu açamazsınız. Yer açmak için gereksiz dosyaları ve eski yedekleri silin ya da paketinizi yükseltin."),
                )
            } else {
                (
                    "Disk kotanız dolmak üzere".to_string(),
                    format!("Hesabınız {used_mb} MB kullanıyor; paketinizin sınırı {limit_mb} MB (yüzde {WARN_PERCENT} aşıldı). Sınır dolmadan gereksiz dosyaları silin ya da paketinizi yükseltin."),
                )
            };
            crate::notify::send_event(state, user_id, "quota_warning", &format!("PixPanel: {subject}"), &body).await;
        }
    }
    Ok(())
}

pub async fn measure_disk(db: &Db) -> AppResult<()> {
    let accounts: Vec<(i64, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, username FROM users WHERE role != 'member'")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })?;
    for (id, username) in accounts {
        // Kullanıcı adı oluşturulurken doğrulanır; yine de yola girmeden önce biçimi denetlenir.
        if crate::api::validate_account_name(&username).is_err() {
            continue;
        }
        let mut bytes = directory_size(&format!("/home/{username}")).await;
        let domains: Vec<String> = db.call(|conn| {
            let mut stmt = conn.prepare("SELECT domain FROM mail_domains WHERE user_id = ?1")?;
            let rows = stmt.query_map(params![id], |row| row.get(0))?;
            rows.collect()
        })?;
        for domain in domains {
            if crate::hosting::domain::normalize(&domain).is_ok() {
                bytes += directory_size(&format!("{}/{domain}", crate::mail::MAIL_ROOT)).await;
            }
        }
        db.call(|conn| {
            conn.execute(
                "INSERT INTO account_usage (user_id, disk_bytes, measured_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET disk_bytes = excluded.disk_bytes, measured_at = excluded.measured_at",
                params![id, bytes, now()],
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn disk_warning_levels() {
        let mb = 1024 * 1024;
        assert_eq!(super::warning_level(0, 1000), 0);
        assert_eq!(super::warning_level(899 * mb, 1000), 0);
        assert_eq!(super::warning_level(900 * mb, 1000), 1);
        assert_eq!(super::warning_level(999 * mb, 1000), 1);
        assert_eq!(super::warning_level(1000 * mb, 1000), 2);
        assert_eq!(super::warning_level(5000 * mb, 1000), 2);
        // Sınırı olmayan (0) pakette uyarı yoktur.
        assert_eq!(super::warning_level(5000 * mb, 0), 0);
    }

    use super::*;

    fn limits(sites: Option<i64>, disk_mb: Option<i64>) -> Limits {
        Limits { sites, disk_mb, ..Limits::default() }
    }

    #[test]
    fn reseller_cannot_hand_out_more_than_it_has() {
        let ceiling = limits(Some(10), Some(5000));
        assert!(limits(Some(10), Some(5000)).within(&ceiling).is_ok());
        assert!(limits(Some(3), Some(100)).within(&ceiling).is_ok());
        assert_eq!(limits(Some(11), Some(100)).within(&ceiling), Err("site"));
        assert_eq!(limits(Some(3), None).within(&ceiling), Err("disk"), "sınırsız, sınırlı tavanı aşar");
        // Tavanın sınırsız olduğu alanda her değer verilebilir.
        assert!(limits(None, None).within(&Limits::default()).is_ok());
        assert!(Limits { mailboxes: Some(500), ..limits(Some(1), Some(1)) }.within(&ceiling).is_ok());
    }

    #[test]
    fn limits_reject_negative_values() {
        assert!(limits(Some(0), Some(0)).validate().is_ok());
        assert!(limits(Some(-1), None).validate().is_err());
        assert!(limits(None, Some(-5)).validate().is_err());
    }

    #[test]
    fn missing_fields_mean_unlimited() {
        let parsed: Limits = serde_json::from_str(r#"{"sites":3}"#).unwrap();
        assert_eq!(parsed, limits(Some(3), None));
        assert_eq!(parsed.of(Resource::Site), Some(3));
        assert_eq!(parsed.of(Resource::Mailbox), None);
    }

    #[test]
    fn package_names_are_trimmed_and_bounded() {
        assert_eq!(validate_name("  Başlangıç  ").unwrap(), "Başlangıç");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"a".repeat(61)).is_err());
        assert!(validate_name("a\nb").is_err());
    }
}
