//! fail2ban: parola deneme saldırılarını kendiliğinden engelleme.
//!
//! Panel fail2ban'in yapılandırmasını üretir (hapisler, süzgeçler) ve durumunu
//! `fail2ban-client` ile okur. Engeller fail2ban'in kendi nftables tablosuna
//! yazılır ve bütün portlar için geçerlidir; panelin güvenlik duvarından bağımsızdır.
//!
//! Süzgeçlerde dikkat edilen nokta: günlük satırlarının bir kısmını saldırgan
//! belirler (ör. denediği kullanıcı adı). Düzenli ifadeler, adresin satırdaki
//! *gerçek* konumunu yakalayacak biçimde yazılmıştır; kullanıcı adına gömülen
//! sahte bir adres başkasının engellenmesine yol açamaz.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, ftp};
use crate::sys::{exec, services};

const SETTING_KEY: &str = "fail2ban";
const JAIL_FILE: &str = "/etc/fail2ban/jail.d/pixpanel.local";
const FILTER_DIR: &str = "/etc/fail2ban/filter.d";
pub const SERVICE: &str = "fail2ban.service";

/// Panelin tanıdığı hapisler.
struct JailSpec {
    key: &'static str,
    label: &'static str,
    description: &'static str,
    /// Hapsin yapılandırma satırları (süzgeç ve günlük kaynağı).
    body: &'static str,
    available: fn() -> bool,
}

fn always() -> bool {
    true
}

fn has_roundcube() -> bool {
    crate::dbtools::installed().roundcube
}

const JAILS: &[JailSpec] = &[
    JailSpec {
        key: "sshd",
        label: "SSH",
        description: "Sunucuya SSH ile yapılan parola denemeleri.",
        body: "filter = sshd\nbackend = systemd\n",
        available: always,
    },
    JailSpec {
        key: "pixpanel",
        label: "PixPanel girişi",
        description: "Panelin giriş ekranındaki hatalı parola ve doğrulama kodu denemeleri.",
        body: "filter = pixpanel\nbackend = systemd\njournalmatch = _SYSTEMD_UNIT=pixpanel.service\n",
        available: always,
    },
    JailSpec {
        key: "pixpanel-dovecot",
        label: "Posta kutuları (IMAP)",
        description: "Posta kutularına yapılan hatalı giriş denemeleri.",
        body: "filter = pixpanel-dovecot\nbackend = systemd\njournalmatch = _SYSTEMD_UNIT=dovecot.service\n",
        available: crate::mail::is_installed,
    },
    JailSpec {
        key: "pixpanel-postfix",
        label: "Posta gönderimi (SMTP)",
        description: "Posta gönderimi için yapılan hatalı kimlik doğrulama denemeleri.",
        body: "filter = postfix[mode=auth]\nbackend = systemd\njournalmatch = _SYSTEMD_UNIT=postfix.service + _SYSTEMD_UNIT=postfix@-.service\n",
        available: crate::mail::is_installed,
    },
    JailSpec {
        key: "pixpanel-ftp",
        label: "FTP",
        description: "FTP hesaplarına yapılan hatalı giriş denemeleri.",
        body: "filter = pixpanel-ftp\nbackend = systemd\njournalmatch = _SYSTEMD_UNIT=pure-ftpd.service\n",
        available: ftp::is_installed,
    },
    JailSpec {
        key: "pixpanel-webmail",
        label: "Web postası",
        description: "Web postasının giriş ekranındaki hatalı denemeler.",
        body: "filter = pixpanel-webmail\nbackend = systemd\njournalmatch = SYSLOG_IDENTIFIER=roundcube\n",
        available: has_roundcube,
    },
];

/// Panelin yazdığı süzgeçler: (dosya adı, içerik).
const FILTERS: &[(&str, &str)] = &[
    (
        "pixpanel.conf",
        // İletide `=` bulunmadığından `[^=]*` ilk `ip=` alanından öteye geçemez;
        // kullanıcı adına gömülen sahte bir `ip=` yakalanmaz.
        "# PixPanel tarafından üretildi.\n[Definition]\nfailregex = pixpanel::api::auth: [^=]* ip=<HOST> username=\nignoreregex =\n",
    ),
    (
        "pixpanel-dovecot.conf",
        // Açgözlü `.*` satırdaki son `rip=…, lip=` çiftini yakalar; gerçek adres her zaman sondadır.
        "# PixPanel tarafından üretildi.\n[Definition]\nfailregex = -login: .*\\(auth failed, \\d+ attempts[^)]*\\).*rip=<HOST>, lip=\nignoreregex =\n",
    ),
    (
        "pixpanel-ftp.conf",
        "# PixPanel tarafından üretildi.\n[Definition]\nfailregex = pure-ftpd\\[\\d+\\]: \\([^@)]*@<HOST>\\) \\[WARNING\\] Authentication failed for user\nignoreregex =\n",
    ),
    (
        "pixpanel-webmail.conf",
        "# PixPanel tarafından üretildi.\n[Definition]\nfailregex = IMAP Error: Login failed for .* against \\S+ from <HOST>\\. AUTHENTICATE\nignoreregex =\n",
    ),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JailSettings {
    pub key: String,
    pub enabled: bool,
    /// Bu kadar hatalı denemeden sonra engellenir.
    pub max_retry: u32,
    /// Denemelerin sayıldığı süre (saniye).
    pub find_time: u32,
    /// Engelin süresi (saniye).
    pub ban_time: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub jails: Vec<JailSettings>,
    /// Hiçbir zaman engellenmeyen adresler ve ağlar.
    pub whitelist: Vec<String>,
    /// Tekrar engellenen adreslerin süresi her seferinde uzasın mı?
    pub escalate: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { jails: Vec::new(), whitelist: Vec::new(), escalate: true }
    }
}

pub fn is_installed() -> bool {
    exec::binary_exists("fail2ban-client")
}

/// Adres ya da ağ (`203.0.113.5`, `203.0.113.0/24`, IPv6 dahil).
fn parse_address(value: &str) -> AppResult<String> {
    let entry = value.trim();
    let invalid = || AppError::bad(format!("Geçersiz adres ya da ağ: {entry}"));
    let (address, prefix) = match entry.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix.parse::<u8>().map_err(|_| invalid())?)),
        None => (entry, None),
    };
    let ip: std::net::IpAddr = address.parse().map_err(|_| invalid())?;
    if prefix.is_some_and(|bits| bits > if ip.is_ipv4() { 32 } else { 128 }) {
        return Err(invalid());
    }
    Ok(match prefix {
        Some(bits) => format!("{ip}/{bits}"),
        None => ip.to_string(),
    })
}

/// Tek bir adres (engelleme ve engel kaldırma için).
pub fn parse_ip(value: &str) -> AppResult<String> {
    value.trim().parse::<std::net::IpAddr>().map(|ip| ip.to_string()).map_err(|_| AppError::bad("Geçersiz adres"))
}

impl Settings {
    /// Ayarları doğrular ve bilinen hapislerle eşitler.
    pub fn normalized(mut self) -> AppResult<Self> {
        self.jails = JAILS
            .iter()
            .map(|spec| {
                self.jails.iter().find(|jail| jail.key == spec.key).cloned().unwrap_or(JailSettings {
                    key: spec.key.to_string(),
                    enabled: true,
                    max_retry: 5,
                    find_time: 600,
                    ban_time: 3600,
                })
            })
            .collect();
        for jail in &self.jails {
            if !(1..=100).contains(&jail.max_retry) {
                return Err(AppError::bad("Deneme sayısı 1-100 arasında olmalı"));
            }
            if !(10..=86_400).contains(&jail.find_time) {
                return Err(AppError::bad("Sayım süresi 10 saniye ile 1 gün arasında olmalı"));
            }
            if !(10..=31_536_000).contains(&jail.ban_time) {
                return Err(AppError::bad("Engel süresi 10 saniye ile 1 yıl arasında olmalı"));
            }
        }
        if self.whitelist.len() > 500 {
            return Err(AppError::bad("Beyaz liste 500 kaydı aşamaz"));
        }
        let mut whitelist = Vec::new();
        for entry in self.whitelist.iter().filter(|entry| !entry.trim().is_empty()) {
            let address = parse_address(entry)?;
            if !whitelist.contains(&address) {
                whitelist.push(address);
            }
        }
        self.whitelist = whitelist;
        Ok(self)
    }
}

pub fn load(db: &Db) -> Settings {
    let saved: Settings = db.setting(SETTING_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    saved.clone().normalized().unwrap_or(saved)
}

fn render(settings: &Settings) -> String {
    let mut ignore = vec!["127.0.0.1/8".to_string(), "::1".to_string()];
    ignore.extend(settings.whitelist.iter().cloned());
    let mut out = format!(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         [DEFAULT]\n\
         ignoreip = {}\n\
         banaction = nftables[type=allports]\n\
         banaction_allports = nftables[type=allports]\n\
         bantime.increment = {}\n\
         bantime.maxtime = 30d\n\n",
        ignore.join(" "),
        settings.escalate
    );
    for spec in JAILS {
        let Some(jail) = settings.jails.iter().find(|jail| jail.key == spec.key) else { continue };
        out.push_str(&format!(
            "[{}]\nenabled = {}\n{}maxretry = {}\nfindtime = {}\nbantime = {}\n\n",
            spec.key,
            jail.enabled && (spec.available)(),
            spec.body,
            jail.max_retry,
            jail.find_time,
            jail.ban_time
        ));
    }
    out
}

/// Yapılandırmayı yazar, sınar ve fail2ban'i yeniden yükler. Sınama başarısız olursa eski dosya geri konur.
pub async fn apply(db: &Db, settings: &Settings) -> AppResult<()> {
    for (name, content) in FILTERS {
        atomic_write(&Path::new(FILTER_DIR).join(name), content.as_bytes(), 0o644)?;
    }
    let path = Path::new(JAIL_FILE);
    let previous = std::fs::read(path).ok();
    atomic_write(path, render(settings).as_bytes(), 0o644)?;
    let test = exec::run_with_timeout("fail2ban-client", &["-t"], Duration::from_secs(60)).await?;
    if !test.success() {
        match previous {
            Some(content) => atomic_write(path, &content, 0o644)?,
            None => std::fs::remove_file(path)?,
        }
        let detail = if test.stderr.trim().is_empty() { test.stdout.trim().to_string() } else { test.stderr.trim().to_string() };
        return Err(AppError::internal(format!("fail2ban yapılandırması geçersiz: {detail}")));
    }
    exec::run_ok("systemctl", &["enable", "--quiet", "--", SERVICE]).await?;
    if services::is_active(SERVICE).await {
        exec::run_with_timeout("fail2ban-client", &["reload"], Duration::from_secs(60)).await?;
    } else {
        exec::run_ok("systemctl", &["start", "--", SERVICE]).await?;
    }
    db.set_setting(SETTING_KEY, &serde_json::to_string(settings).map_err(|err| AppError::internal(err.to_string()))?)
}

#[derive(Debug, Serialize)]
pub struct JailStatus {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Hapsin izlediği bileşen kurulu mu?
    pub available: bool,
    /// fail2ban'de şu an çalışıyor mu?
    pub active: bool,
    pub failed_now: u32,
    pub failed_total: u32,
    pub banned_total: u32,
    pub banned: Vec<String>,
}

/// `fail2ban-client status <hapis>` çıktısından bir alanı okur.
fn field(output: &str, name: &str) -> String {
    output.lines().find_map(|line| line.split_once(name).map(|(_, value)| value.trim_start_matches(':').trim().to_string())).unwrap_or_default()
}

pub async fn status() -> Vec<JailStatus> {
    let running = is_installed() && services::is_active(SERVICE).await;
    let mut list = Vec::new();
    for spec in JAILS {
        let mut jail = JailStatus {
            key: spec.key,
            label: spec.label,
            description: spec.description,
            available: (spec.available)(),
            active: false,
            failed_now: 0,
            failed_total: 0,
            banned_total: 0,
            banned: Vec::new(),
        };
        if running {
            if let Ok(output) = exec::run("fail2ban-client", &["status", spec.key]).await {
                if output.success() {
                    jail.active = true;
                    jail.failed_now = field(&output.stdout, "Currently failed").parse().unwrap_or(0);
                    jail.failed_total = field(&output.stdout, "Total failed").parse().unwrap_or(0);
                    jail.banned_total = field(&output.stdout, "Total banned").parse().unwrap_or(0);
                    jail.banned = field(&output.stdout, "Banned IP list").split_whitespace().map(str::to_string).collect();
                }
            }
        }
        list.push(jail);
    }
    list
}

fn jail_key(key: &str) -> AppResult<&'static str> {
    JAILS.iter().find(|spec| spec.key == key).map(|spec| spec.key).ok_or_else(|| AppError::bad("Bilinmeyen hapis"))
}

/// Adresin engelini kaldırır; `jail` verilmezse bütün hapislerden.
pub async fn unban(ip: &str, jail: Option<&str>) -> AppResult<()> {
    let ip = parse_ip(ip)?;
    match jail {
        Some(jail) => exec::run_ok("fail2ban-client", &["set", jail_key(jail)?, "unbanip", &ip]).await?,
        None => exec::run_ok("fail2ban-client", &["unban", &ip]).await?,
    };
    Ok(())
}

pub async fn ban(ip: &str, jail: &str) -> AppResult<()> {
    let ip = parse_ip(ip)?;
    exec::run_ok("fail2ban-client", &["set", jail_key(jail)?, "banip", &ip]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_values() {
        let with = |whitelist: &str| Settings { whitelist: vec![whitelist.into()], ..Settings::default() }.normalized();
        assert!(with("203.0.113.0/24").is_ok());
        for bad in ["1.2.3.4 5.6.7.8", "1.2.3.4\nbantime = 1", "ornek.com", "1.2.3.4/99"] {
            assert!(with(bad).is_err(), "{bad}");
        }
        assert!(parse_ip("1.2.3.4; reboot").is_err());
        assert!(jail_key("sshd; x").is_err());
        let mut settings = Settings::default().normalized().unwrap();
        settings.jails[0].max_retry = 0;
        assert!(settings.normalized().is_err());
    }

    #[test]
    fn rendered_config_lists_every_jail() {
        let text = render(&Settings::default().normalized().unwrap());
        assert!(text.contains("[sshd]") && text.contains("[pixpanel]") && text.contains("ignoreip = 127.0.0.1/8 ::1"));
        assert!(text.contains("banaction = nftables[type=allports]"));
    }
}
