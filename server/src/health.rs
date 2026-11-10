//! Kendi kendine bakım: yöneticinin düzenli olarak elle yaptığı denetimleri panel
//! sürekli yapar, güvenli olanları kendisi düzeltir, kalanını tek tıkla düzeltilecek
//! biçimde sunar.
//!
//! Her denetim bir dizi bulgu döner. Bulgunun anahtarı (`kural:hedef`) kalıcıdır:
//! aynı sorun sürdükçe aynı anahtarla gelir; böylece ne zamandır sürdüğü, kaç kez
//! düzeltilmeye çalışıldığı izlenir ve sorun giderilince kayıt kendiliğinden silinir.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::hosting::ssl;
use crate::state::AppState;
use crate::sys::exec;

const SETTINGS_KEY: &str = "health";
/// Hızlı denetimlerin aralığı.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Paket güncellemeleri gibi ağır denetimlerin aralığı.
const SLOW_INTERVAL_SECS: i64 = 6 * 3600;
/// Alan adının bu sunucuyu gösterip göstermediği bu süre boyunca yeniden sorulmaz.
const DNS_CACHE_SECS: i64 = 10 * 60;
/// Bir bulgu kendiliğinden en çok bu kadar kez düzeltilmeye çalışılır; sonrası yöneticiye kalır.
const MAX_AUTO_ATTEMPTS: i64 = 3;
/// Site bu kadar ardışık denetimde yanıt vermezse bulgu sayılır (tek seferlik takılmalar elenir).
const SITE_FAILURES_BEFORE_ALERT: u32 = 2;
const SITE_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
/// Bitimine bu kadar kalan sertifika "yenilenemiyor" sayılır (yenileme 30 gün kala başlar).
const CERT_WARN_SECS: i64 = 10 * 86400;
/// Güvenlik desteği bitmiş PHP sürümleri.
const PHP_END_OF_LIFE: &[&str] = &["7.4", "8.0", "8.1"];
const DISK_WARN_PERCENT: f64 = 85.0;
const DISK_CRITICAL_PERCENT: f64 = 95.0;
const MAIL_QUEUE_WARN: usize = 50;
/// apt'nin günlük işleri bu dosyaya göre güvenlik güncellemelerini kendiliğinden kurar.
const AUTO_UPGRADES_FILE: &str = "/etc/apt/apt.conf.d/20auto-upgrades";

/// Çalışır görünen servisin gerçekten bağlantı kabul ettiği yerel port. systemd, süreci
/// kendisi izlemediği (SysV kökenli) ya da takılıp kalmış servislerin öldüğünü fark etmez.
const SERVICE_PORTS: &[(&str, u16)] = &[
    ("nginx.service", 80),
    ("postfix.service", 25),
    ("dovecot.service", 993),
    ("pdns.service", 53),
    ("pure-ftpd.service", 21),
];

/// Çalışması beklenen servisler: kurulu ve açılışta başlayacak biçimde ayarlıysa izlenir.
const WATCHED_SERVICES: &[(&str, &str)] = &[
    ("nginx.service", "Web sunucusu (nginx)"),
    ("mariadb.service", "MariaDB"),
    ("postgresql.service", "PostgreSQL"),
    ("redis-server.service", "Redis"),
    ("postfix.service", "Postfix (SMTP)"),
    ("dovecot.service", "Dovecot (IMAP)"),
    ("rspamd.service", "Rspamd"),
    ("pdns.service", "DNS sunucusu"),
    ("pure-ftpd.service", "FTP sunucusu"),
    ("fail2ban.service", "fail2ban"),
    ("docker.service", "Docker"),
    ("ssh.service", "SSH"),
    ("cron.service", "Zamanlı görevler (cron)"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Çöken servisler kendiliğinden yeniden başlatılsın.
    pub restart_services: bool,
    /// Alan adı bu sunucuyu gösteren sertifikasız sitelere kendiliğinden sertifika alınsın.
    pub issue_certificates: bool,
    /// Siteler düzenli olarak yoklanıp yanıt vermeyenler bildirilsin.
    pub check_sites: bool,
    /// Yeni kritik bulgular ve kendiliğinden yapılan işlemler yöneticilere bildirilsin.
    pub notify: bool,
    /// İşletim sisteminin güvenlik güncellemeleri her gün kendiliğinden kurulsun.
    pub security_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { restart_services: true, issue_certificates: true, check_sites: true, notify: true, security_updates: true }
    }
}

pub fn load_settings(state: &AppState) -> Settings {
    state.db.setting(SETTINGS_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

pub fn save_settings(state: &AppState, settings: &Settings) -> AppResult<()> {
    let raw = serde_json::to_string(settings).map_err(|err| AppError::internal(err.to_string()))?;
    state.db.set_setting(SETTINGS_KEY, &raw)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// `kural:hedef` biçiminde kalıcı anahtar.
    pub key: String,
    pub rule: &'static str,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// Sorunun giderilebileceği panel sayfası.
    pub link: Option<String>,
    /// Tek tıkla düzeltilebiliyorsa düğmenin adı.
    pub fix: Option<&'static str>,
    /// Panel bunu kendiliğinden düzeltecek (ayar açık ve deneme hakkı var).
    pub auto: bool,
    /// Sorunun ilk görüldüğü an.
    pub since: i64,
    /// Bulgunun ilgili olduğu şeyin adı (alan adı gibi); benzer bulgular birleştirilirken kullanılır.
    #[serde(skip)]
    subject: String,
}

impl Finding {
    fn new(rule: &'static str, target: impl std::fmt::Display, severity: Severity, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            key: format!("{rule}:{target}"),
            rule,
            severity,
            title: title.into(),
            detail: detail.into(),
            link: None,
            fix: None,
            auto: false,
            since: 0,
            subject: String::new(),
        }
    }

    fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    fn link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }

    fn fix(mut self, label: &'static str) -> Self {
        self.fix = Some(label);
        self
    }

    fn auto(mut self, enabled: bool) -> Self {
        self.auto = enabled;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub checked_at: i64,
    /// Bu turda denetlenen şey sayısı (servis, site, disk...).
    pub checks: usize,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub id: i64,
    pub created_at: i64,
    pub rule: String,
    pub target: String,
    pub summary: String,
    pub automatic: bool,
    pub success: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
struct PendingUpdates {
    checked_at: i64,
    total: usize,
    security: usize,
}

/// Denetimlerin bellekteki durumu.
#[derive(Clone, Default)]
pub struct Monitor {
    report: Arc<RwLock<Report>>,
    site_failures: Arc<Mutex<HashMap<i64, u32>>>,
    port_failures: Arc<Mutex<HashMap<String, u32>>>,
    dns_cache: Arc<Mutex<HashMap<String, (bool, i64)>>>,
    updates: Arc<RwLock<PendingUpdates>>,
    /// Aynı anda tek denetim turu yürür.
    running: Arc<tokio::sync::Mutex<()>>,
}

impl Monitor {
    pub fn report(&self) -> Report {
        self.report.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Paket listesi bir sonraki turda yeniden okunsun (güncelleme kurulduktan sonra).
    fn forget_updates(&self) {
        *self.updates.write().unwrap_or_else(|p| p.into_inner()) = PendingUpdates::default();
    }
}

// ---- Denetimler ----

struct Batch {
    findings: Vec<Finding>,
    checks: usize,
}

impl Batch {
    fn new() -> Self {
        Self { findings: Vec::new(), checks: 0 }
    }

    fn push(&mut self, finding: Finding) {
        self.findings.push(finding);
    }
}

/// `systemctl show` çıktısını birim başına alan→değer eşlemesine çevirir.
fn parse_unit_blocks(output: &str) -> Vec<HashMap<&str, &str>> {
    output
        .split("\n\n")
        .map(|block| block.lines().filter_map(|line| line.split_once('=')).collect::<HashMap<_, _>>())
        .filter(|block| !block.is_empty())
        .collect()
}

fn watched_units() -> Vec<(String, String)> {
    let mut units: Vec<(String, String)> = WATCHED_SERVICES.iter().map(|(unit, label)| (unit.to_string(), label.to_string())).collect();
    for version in crate::hosting::php::installed_versions() {
        units.push((format!("php{version}-fpm.service"), format!("PHP {version}")));
    }
    units
}

async fn check_services(state: &AppState, batch: &mut Batch, settings: &Settings) {
    let units = watched_units();
    let mut args = vec!["show", "--no-pager", "-p", "Id,LoadState,ActiveState,UnitFileState,Result", "--"];
    args.extend(units.iter().map(|(unit, _)| unit.as_str()));
    let Ok(output) = exec::run("systemctl", &args).await else { return };
    let mut active: Vec<(String, String)> = Vec::new();
    for block in parse_unit_blocks(&output.stdout) {
        let Some(id) = block.get("Id").copied() else { continue };
        let Some((unit, label)) = units.iter().find(|(unit, _)| unit == id) else { continue };
        // Kurulu olmayan ya da açılışta başlamayacak biçimde bırakılmış servis izlenmez.
        // `generated`: SysV betiğinden üretilmiş, açılışta başlayan birim (pure-ftpd gibi).
        let starts_at_boot = matches!(block.get("UnitFileState").copied(), Some("enabled" | "generated"));
        if block.get("LoadState").copied() != Some("loaded") || !starts_at_boot {
            continue;
        }
        batch.checks += 1;
        match block.get("ActiveState").copied().unwrap_or("") {
            "failed" => batch.push(
                Finding::new("service.failed", unit, Severity::Critical, format!("{label} çöktü"), format!("{unit} hata vererek durdu."))
                    .link("/services")
                    .fix("Yeniden başlat")
                    .auto(settings.restart_services),
            ),
            "inactive" => batch.push(
                Finding::new(
                    "service.stopped",
                    unit,
                    Severity::Warning,
                    format!("{label} çalışmıyor"),
                    format!("{unit} durdurulmuş. Bilerek durdurmadıysanız başlatın."),
                )
                .link("/services")
                .fix("Başlat"),
            ),
            "active" => active.push((unit.clone(), label.clone())),
            _ => {}
        }
    }

    // Çalışır görünen servis portunu dinlemiyorsa yanıt vermiyordur.
    let mut silent: Vec<(String, String, u16)> = Vec::new();
    for (unit, label) in &active {
        let Some((_, port)) = SERVICE_PORTS.iter().find(|(name, _)| name == unit) else { continue };
        let connect = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(("127.0.0.1", *port))).await;
        if !matches!(connect, Ok(Ok(_))) {
            silent.push((unit.clone(), label.clone(), *port));
        }
    }
    let monitor = &state.health;
    let mut failures = monitor.port_failures.lock().unwrap_or_else(|p| p.into_inner());
    failures.retain(|unit, _| silent.iter().any(|(name, _, _)| name == unit));
    for (unit, label, port) in silent {
        let count = failures.entry(unit.clone()).or_insert(0);
        *count += 1;
        // Yeniden başlatılan servisin portu açması birkaç saniye sürebilir; tek yoklama yetmez.
        if *count < SITE_FAILURES_BEFORE_ALERT {
            continue;
        }
        batch.push(
            Finding::new(
                "service.unresponsive",
                &unit,
                Severity::Critical,
                format!("{label} yanıt vermiyor"),
                format!("{unit} çalışıyor görünüyor ama {port}. portta bağlantı kabul etmiyor."),
            )
            .link("/services")
            .fix("Yeniden başlat")
            .auto(settings.restart_services),
        );
    }
}

async fn check_disks(batch: &mut Batch) {
    for disk in crate::sys::metrics::disks().await {
        batch.checks += 1;
        let percent = disk.used as f64 / disk.total as f64 * 100.0;
        if percent < DISK_WARN_PERCENT {
            continue;
        }
        let severity = if percent >= DISK_CRITICAL_PERCENT { Severity::Critical } else { Severity::Warning };
        let free_gb = disk.available as f64 / 1_073_741_824.0;
        batch.push(
            Finding::new(
                "disk.full",
                &disk.mount,
                severity,
                format!("Disk dolmak üzere: {}", disk.mount),
                format!("%{percent:.0} dolu, {free_gb:.1} GB boş yer kaldı. Disk dolarsa siteler ve veritabanları yazamaz."),
            )
            .link("/server"),
        );
    }
    // Çok sayıda küçük dosya, yer varken de diski kullanılmaz kılabilir.
    if let Ok(output) = exec::run("df", &["--output=target,ipcent", "-x", "tmpfs", "-x", "devtmpfs", "-x", "overlay", "-x", "squashfs"]).await {
        for line in output.stdout.lines().skip(1) {
            let Some((mount, percent)) = line.trim().rsplit_once(char::is_whitespace) else { continue };
            let Ok(percent) = percent.trim_end_matches('%').parse::<f64>() else { continue };
            if percent >= 90.0 {
                batch.push(
                    Finding::new(
                        "disk.inodes",
                        mount.trim(),
                        Severity::Warning,
                        format!("Dosya sayısı sınırına yaklaşıldı: {}", mount.trim()),
                        format!("Dosya düğümlerinin %{percent:.0} kadarı kullanılıyor; sınır dolunca yeni dosya oluşturulamaz."),
                    )
                    .link("/server"),
                );
            }
        }
    }
}

/// Alan adı bu sunucunun adresine çözülüyor mu? Sonuç kısa süre saklanır.
async fn points_here(monitor: &Monitor, domain: &str, server_ip: &str) -> bool {
    let current = now();
    if let Some((result, at)) = monitor.dns_cache.lock().unwrap_or_else(|p| p.into_inner()).get(domain).copied() {
        if current - at < DNS_CACHE_SECS {
            return result;
        }
    }
    let lookup = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((domain, 80))).await;
    let result = match lookup {
        Ok(Ok(addresses)) => addresses.into_iter().any(|address| address.ip().to_string() == server_ip),
        _ => false,
    };
    monitor.dns_cache.lock().unwrap_or_else(|p| p.into_inner()).insert(domain.to_string(), (result, current));
    result
}

fn renewable(mode: &str) -> bool {
    matches!(mode, "letsencrypt" | "letsencrypt_dns" | "letsencrypt_wildcard" | "selfsigned")
}

async fn check_certificates(state: &AppState, batch: &mut Batch, settings: &Settings, sites: &[Site]) {
    let server_ip = crate::dns::server_ip(&state.db);
    for site in sites {
        batch.checks += 1;
        let link = format!("/sites/{}/ssl", site.id);
        if site.ssl_mode == "none" {
            if points_here(&state.health, &site.domain, &server_ip).await {
                batch.push(
                    Finding::new(
                        "ssl.missing",
                        site.id,
                        Severity::Warning,
                        format!("{} sertifikasız yayında", site.domain),
                        "Site şifresiz (HTTP) sunuluyor; tarayıcılar ziyaretçileri uyarır. Alan adı bu sunucuyu gösterdiği için ücretsiz Let's Encrypt sertifikası alınabilir.",
                    )
                    .link(link)
                    .fix("Sertifika al")
                    .auto(settings.issue_certificates),
                );
            } else {
                batch.push(
                    Finding::new(
                        "ssl.unreachable",
                        site.id,
                        Severity::Info,
                        format!("{} sertifikasız", site.domain),
                        format!("Alan adı henüz bu sunucuyu ({server_ip}) göstermiyor; DNS kaydı yönlendirildiğinde sertifika alınabilir."),
                    )
                    .subject(&site.domain)
                    .link(link),
                );
            }
            continue;
        }
        let Some(info) = ssl::read_info(&site.domain) else { continue };
        let left = info.not_after - now();
        if left > CERT_WARN_SECS {
            continue;
        }
        let (rule, severity, title) = if left <= 0 {
            ("ssl.expired", Severity::Critical, format!("{} sertifikasının süresi doldu", site.domain))
        } else {
            ("ssl.expiring", Severity::Warning, format!("{} sertifikası {} gün içinde bitiyor", site.domain, left / 86400 + 1))
        };
        let mut finding = Finding::new(
            rule,
            site.id,
            severity,
            title,
            if renewable(&site.ssl_mode) {
                "Sertifika kendiliğinden yenilenemedi; yenilemeyi yeniden deneyin."
            } else {
                "Bu sertifikayı siz yüklediniz; yenisini yüklemeniz gerekir."
            },
        )
        .link(link);
        if renewable(&site.ssl_mode) {
            finding = finding.fix("Yenile");
        }
        batch.push(finding);
    }
}

fn check_php_versions(batch: &mut Batch, sites: &[Site]) {
    for site in sites {
        let Some(version) = site.php_version.as_deref().filter(|_| site.kind == "php") else { continue };
        if PHP_END_OF_LIFE.contains(&version) {
            batch.push(
                Finding::new(
                    "php.eol",
                    site.id,
                    Severity::Info,
                    format!("{} desteği bitmiş PHP {version} kullanıyor", site.domain),
                    "Bu sürüm artık güvenlik güncellemesi almıyor. Uygulamanız destekliyorsa daha yeni bir sürüme geçin.",
                )
                .subject(&site.domain)
                .link(format!("/sites/{}/php", site.id)),
            );
        }
    }
}

/// Siteleri sunucunun içinden yoklar: nginx'e site adıyla istek atılır, 5xx ya da
/// yanıtsızlık üst üste görülürse bulgu olur.
async fn check_site_responses(state: &AppState, batch: &mut Batch, sites: &[Site]) {
    if !crate::sys::services::is_active("nginx.service").await {
        return;
    }
    let Ok(client) = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(SITE_CHECK_TIMEOUT).build() else {
        return;
    };
    let targets: Vec<(i64, String)> = sites.iter().map(|site| (site.id, site.domain.clone())).collect();
    let results: Vec<(i64, Option<String>)> = stream::iter(targets.into_iter().map(|(id, domain)| {
        let client = client.clone();
        async move {
            let outcome = client.get("http://127.0.0.1/").header("Host", domain).send().await;
            let problem = match outcome {
                Ok(response) if response.status().is_server_error() => Some(format!("HTTP {}", response.status().as_u16())),
                Ok(_) => None,
                Err(err) if err.is_timeout() => Some("yanıt süresi aşıldı".to_string()),
                Err(_) => Some("bağlantı kurulamadı".to_string()),
            };
            (id, problem)
        }
    }))
    .buffer_unordered(8)
    .collect()
    .await;

    let mut failures = state.health.site_failures.lock().unwrap_or_else(|p| p.into_inner());
    failures.retain(|id, _| sites.iter().any(|site| site.id == *id));
    for (id, problem) in results {
        batch.checks += 1;
        let Some(problem) = problem else {
            failures.remove(&id);
            continue;
        };
        let count = failures.entry(id).or_insert(0);
        *count += 1;
        if *count < SITE_FAILURES_BEFORE_ALERT {
            continue;
        }
        let Some(site) = sites.iter().find(|site| site.id == id) else { continue };
        let hint = match site.kind.as_str() {
            "node" => "Node.js uygulaması çalışmıyor ya da yanıt vermiyor olabilir.",
            "proxy" => "Yönlendirilen uygulama çalışmıyor ya da yanıt vermiyor olabilir.",
            "php" => "PHP uygulaması hata veriyor olabilir; sitenin hata günlüğüne bakın.",
            _ => "Web sunucusu bu siteyi sunamıyor.",
        };
        batch.push(
            Finding::new("site.down", id, Severity::Critical, format!("{} yanıt vermiyor ({problem})", site.domain), hint)
                .link(format!("/sites/{id}/logs")),
        );
    }
}

fn check_backups(state: &AppState, batch: &mut Batch, has_content: bool) -> AppResult<()> {
    batch.checks += 1;
    let targets = if crate::backup::is_installed() { crate::backup::list_targets(&state.db)? } else { Vec::new() };
    if targets.is_empty() {
        if has_content {
            batch.push(
                Finding::new(
                    "backup.none",
                    "server",
                    Severity::Warning,
                    "Yedekleme kurulmamış",
                    "Sitelerinizin ve veritabanlarınızın yedeği alınmıyor. Bir yedek yeri ekleyip günlük plan oluşturun.",
                )
                .link("/backups"),
            );
        }
        return Ok(());
    }
    let plans = crate::backup::list_plans(&state.db)?;
    if has_content && !plans.iter().any(|plan| plan.enabled && plan.frequency != "manual") {
        batch.push(
            Finding::new(
                "backup.unscheduled",
                "server",
                Severity::Warning,
                "Düzenli yedek planı yok",
                "Yedek yeri tanımlı ama kendiliğinden çalışan bir plan yok; yedekler yalnızca elle alınırsa unutulur. Tek tıkla her gece 03:00'te bütün siteleri, veritabanlarını, postaları ve panel ayarlarını yedekleyen bir plan oluşturabilirsiniz.",
            )
            .link("/backups")
            .fix("Günlük plan oluştur"),
        );
    }
    for plan in plans.iter().filter(|plan| plan.enabled && plan.last_status == "failed") {
        batch.push(
            Finding::new(
                "backup.failed",
                plan.id,
                Severity::Warning,
                format!("“{}” yedek planı başarısız oldu", plan.name),
                "Son çalışmada yedek alınamadı. Ayrıntı görev günlüğünde; yeniden çalıştırmayı deneyin.",
            )
            .link("/backups")
            .fix("Yeniden çalıştır"),
        );
    }
    Ok(())
}

async fn check_security(state: &AppState, batch: &mut Batch) {
    batch.checks += 3;
    if crate::firewall::is_available() && !crate::firewall::load(&state.db, &state.cfg).enabled {
        batch.push(
            Finding::new(
                "security.firewall",
                "server",
                Severity::Warning,
                "Güvenlik duvarı kapalı",
                "Sunucunun bütün portları internete açık. Güvenlik duvarını açıp yalnızca gereken kapıları bırakın.",
            )
            .link("/firewall"),
        );
    }
    if !crate::fail2ban::is_installed() {
        batch.push(
            Finding::new(
                "security.fail2ban",
                "server",
                Severity::Info,
                "Saldırı engelleme kurulu değil",
                "Parola deneme saldırıları engellenmiyor. Bileşenler sayfasından fail2ban'ı kurun.",
            )
            .link("/components"),
        );
    }
    let admins = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT username FROM users WHERE role = 'admin' AND totp_enabled = 0 AND status = 'active'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    });
    if let Ok(admins) = admins {
        for username in admins {
            batch.push(
                Finding::new(
                    "security.two-factor",
                    &username,
                    Severity::Info,
                    format!("{username} hesabında iki adımlı doğrulama kapalı"),
                    "Yönetici parolası ele geçirilirse sunucunun tamamı ele geçer. İki adımlı doğrulamayı açın.",
                )
                .link("/account"),
            );
        }
    }
    // Parolayla root girişi açıksa parola deneme saldırılarının hedefi doğrudan root olur.
    if let Ok(output) = exec::run("sshd", &["-T"]).await {
        let value = |key: &str| output.stdout.lines().find_map(|line| line.strip_prefix(key)).map(|rest| rest.trim().to_string());
        let root = value("permitrootlogin ").unwrap_or_default();
        let password = value("passwordauthentication ").unwrap_or_default();
        if output.success() && root == "yes" && password == "yes" {
            batch.push(Finding::new(
                "security.ssh-root-password",
                "server",
                Severity::Warning,
                "SSH'de root parolayla giriş yapabiliyor",
                "Önce anahtarınızla giriş yapabildiğinizi doğrulayın, sonra /etc/ssh/sshd_config içinde “PermitRootLogin prohibit-password” yapın.",
            ));
        }
    }
    if matches!(crate::panelcert::mode(&state.db).as_str(), "" | "selfsigned") {
        batch.push(
            Finding::new(
                "panel.certificate",
                "server",
                Severity::Info,
                "Panel kendinden imzalı sertifikayla çalışıyor",
                "Tarayıcılar panele girerken uyarı verir. Panele bir alan adı verip gerçek sertifika alın.",
            )
            .link("/settings"),
        );
    }
}

async fn check_mail_queue(batch: &mut Batch) {
    if !crate::mail::is_installed() {
        return;
    }
    batch.checks += 1;
    let Ok(output) = exec::run("postqueue", &["-j"]).await else { return };
    let waiting = output.stdout.lines().filter(|line| !line.trim().is_empty()).count();
    if waiting >= MAIL_QUEUE_WARN {
        batch.push(
            Finding::new(
                "mail.queue",
                "server",
                Severity::Warning,
                format!("Posta kuyruğunda {waiting} ileti bekliyor"),
                "İletiler teslim edilemiyor olabilir (alıcı sunucu reddediyor, 25. port kapalı ya da sunucu kara listede).",
            )
            .link("/mail"),
        );
    }
}

/// Bekleyen paket güncellemelerini sayar. Ağır olduğundan seyrek çalışır ve sonucu saklanır.
async fn refresh_updates(monitor: &Monitor) {
    let stale = now() - monitor.updates.read().unwrap_or_else(|p| p.into_inner()).checked_at >= SLOW_INTERVAL_SECS;
    if !stale {
        return;
    }
    // Liste yenilenemese de (ağ yok) eldeki listeyle sayım yapılır.
    let _ = exec::run_with_timeout("apt-get", &["update", "-qq", "-o", "DPkg::Lock::Timeout=30"], Duration::from_secs(180)).await;
    let Ok(output) = exec::run_with_timeout("apt-get", &["-s", "-o", "Debug::NoLocking=true", "upgrade"], Duration::from_secs(120)).await
    else {
        return;
    };
    let (total, security) = count_upgrades(&output.stdout);
    *monitor.updates.write().unwrap_or_else(|p| p.into_inner()) = PendingUpdates { checked_at: now(), total, security };
}

/// `apt-get -s upgrade` çıktısından (toplam, güvenlik) güncelleme sayısını çıkarır.
fn count_upgrades(simulation: &str) -> (usize, usize) {
    let installs: Vec<&str> = simulation.lines().filter(|line| line.starts_with("Inst ")).collect();
    let security = installs.iter().filter(|line| line.to_ascii_lowercase().contains("security")).count();
    (installs.len(), security)
}

fn check_updates(state: &AppState, batch: &mut Batch) {
    batch.checks += 2;
    let pending = state.health.updates.read().unwrap_or_else(|p| p.into_inner()).clone();
    if pending.total > 0 {
        let severity = if pending.security > 0 { Severity::Warning } else { Severity::Info };
        let title = if pending.security > 0 {
            format!("{} güncelleme bekliyor ({} güvenlik)", pending.total, pending.security)
        } else {
            format!("{} paket güncellemesi bekliyor", pending.total)
        };
        batch.push(
            Finding::new("updates.pending", "server", severity, title, "İşletim sistemi paketlerinin yeni sürümleri var. Güvenlik güncellemelerini geciktirmeyin.")
                .fix("Güncellemeleri kur"),
        );
    }
    if std::path::Path::new("/var/run/reboot-required").exists() {
        batch.push(Finding::new(
            "system.reboot",
            "server",
            Severity::Info,
            "Sunucunun yeniden başlatılması gerekiyor",
            "Kurulan bir güncelleme (çoğunlukla çekirdek) ancak yeniden başlatınca devreye girer. Uygun bir saatte sunucuyu yeniden başlatın.",
        ));
    }
    if crate::update::view(state).available {
        batch.push(
            Finding::new("panel.update", "server", Severity::Info, "PixPanel'in yeni sürümü var", "Ayarlar sayfasından tek tıkla güncelleyebilirsiniz.")
                .link("/settings"),
        );
    }
}

/// Aynı kuraldan çok sayıda öneri varsa tek bulguda toplar; onlarca sitesi olan
/// sunucuda liste aynı cümlenin tekrarıyla dolmasın diye.
fn collapse(findings: &mut Vec<Finding>, rule: &'static str, title: impl Fn(usize) -> String, detail: &str) {
    const KEEP_SEPARATE: usize = 3;
    const NAMED: usize = 5;
    let subjects: Vec<String> = findings.iter().filter(|finding| finding.rule == rule).map(|finding| finding.subject.clone()).collect();
    if subjects.len() <= KEEP_SEPARATE {
        return;
    }
    findings.retain(|finding| finding.rule != rule);
    let mut names = subjects.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", ");
    if subjects.len() > NAMED {
        names.push_str(&format!(" ve {} site daha", subjects.len() - NAMED));
    }
    findings.push(Finding::new(rule, "many", Severity::Info, title(subjects.len()), format!("{detail} ({names})")).link("/sites"));
}

// ---- Tur ----

/// Bütün denetimleri çalıştırır, sonucu saklar, kendiliğinden düzeltilebilenleri düzeltir.
pub async fn run_checks(state: &AppState) -> Report {
    let monitor = state.health.clone();
    let _turn = monitor.running.lock().await;
    let settings = load_settings(state);
    let mut batch = Batch::new();

    let sites: Vec<Site> = crate::backup::all_sites(&state.db).unwrap_or_default().into_iter().filter(|site| site.status == "active").collect();
    check_services(state, &mut batch, &settings).await;
    check_disks(&mut batch).await;
    check_certificates(state, &mut batch, &settings, &sites).await;
    check_php_versions(&mut batch, &sites);
    if settings.check_sites {
        check_site_responses(state, &mut batch, &sites).await;
    }
    let has_content = !sites.is_empty() || crate::backup::all_databases(&state.db).map(|list| !list.is_empty()).unwrap_or(false);
    if let Err(err) = check_backups(state, &mut batch, has_content) {
        tracing::warn!(error = %err, "yedek denetimi yapılamadı");
    }
    check_security(state, &mut batch).await;
    check_mail_queue(&mut batch).await;
    apply_security_updates(state, settings.security_updates).await;
    refresh_updates(&monitor).await;
    check_updates(state, &mut batch);

    collapse(
        &mut batch.findings,
        "ssl.unreachable",
        |count| format!("{count} site sertifikasız"),
        "Alan adları henüz bu sunucuyu göstermiyor; DNS kayıtları yönlendirildiğinde sertifikaları alınabilir.",
    );
    collapse(
        &mut batch.findings,
        "php.eol",
        |count| format!("{count} site desteği bitmiş PHP sürümü kullanıyor"),
        "Bu sürümler artık güvenlik güncellemesi almıyor; uygulamalar destekliyorsa daha yeni bir sürüme geçin.",
    );

    // En önemli bulgu en üstte; aynı önemdekiler en eskiden yeniye.
    let (fresh, records) = remember(state, &mut batch.findings).unwrap_or_default();
    batch.findings.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.since.cmp(&b.since)));

    let previous: HashSet<String> = monitor.report().findings.into_iter().map(|finding| finding.key).collect();
    let current: HashSet<String> = batch.findings.iter().map(|finding| finding.key.clone()).collect();
    let report = Report { checked_at: now(), checks: batch.checks, findings: batch.findings };
    *monitor.report.write().unwrap_or_else(|p| p.into_inner()) = report.clone();
    if previous != current {
        crate::events::publish(crate::events::Event::Changed { area: "health".into() }, None);
    }

    if settings.notify {
        for finding in report.findings.iter().filter(|finding| finding.severity == Severity::Critical && fresh.contains(&finding.key) && !finding.auto) {
            notify_admins(state, &finding.title, &finding.detail).await;
        }
    }
    auto_fix(state, &report, &records, &settings).await;
    report
}

/// Bulguların ilk görülme anını kaydeder, giderilmiş olanları siler. Bu turda ilk kez
/// görülen anahtarları ve her bulgunun (deneme sayısı, son deneme) bilgisini döner.
fn remember(state: &AppState, findings: &mut [Finding]) -> AppResult<(HashSet<String>, HashMap<String, (i64, i64)>)> {
    let current = now();
    state.db.call(|conn| {
        let tx = conn.transaction()?;
        let mut known: HashMap<String, (i64, i64, i64)> = HashMap::new();
        {
            let mut stmt = tx.prepare("SELECT key, first_seen, fix_attempts, COALESCE(last_fix_at, 0) FROM health_findings")?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?, row.get(3)?))))?;
            for row in rows {
                let (key, value) = row?;
                known.insert(key, value);
            }
        }
        let mut fresh = HashSet::new();
        let mut records = HashMap::new();
        for finding in findings.iter_mut() {
            match known.remove(&finding.key) {
                Some((first_seen, attempts, last_fix_at)) => {
                    finding.since = first_seen;
                    records.insert(finding.key.clone(), (attempts, last_fix_at));
                }
                None => {
                    finding.since = current;
                    fresh.insert(finding.key.clone());
                    records.insert(finding.key.clone(), (0, 0));
                    tx.execute("INSERT INTO health_findings (key, rule, first_seen) VALUES (?1, ?2, ?3)", params![finding.key, finding.rule, current])?;
                }
            }
        }
        // Artık görülmeyen bulgular giderilmiştir.
        for key in known.keys() {
            tx.execute("DELETE FROM health_findings WHERE key = ?1", params![key])?;
        }
        tx.commit()?;
        Ok((fresh, records))
    })
}

async fn notify_admins(state: &AppState, subject: &str, body: &str) {
    let admins = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE role = 'admin' AND status = 'active'")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    });
    for admin in admins.unwrap_or_default() {
        crate::notify::send_event(state, admin, "health_alert", &format!("PixPanel: {subject}"), body).await;
    }
}

/// Deneme aralığı: servisler için kısa, sertifika gibi dış sınırı olan işler için uzun.
fn retry_gap_secs(rule: &str) -> i64 {
    if rule.starts_with("ssl.") { 3600 } else { 5 * 60 }
}

async fn auto_fix(state: &AppState, report: &Report, records: &HashMap<String, (i64, i64)>, settings: &Settings) {
    for finding in report.findings.iter().filter(|finding| finding.auto) {
        let (attempts, last_fix_at) = records.get(&finding.key).copied().unwrap_or((0, 0));
        if attempts >= MAX_AUTO_ATTEMPTS {
            // Deneme hakkı bitti: bir kez haber verilir, gerisi yöneticiye kalır.
            if attempts == MAX_AUTO_ATTEMPTS {
                let _ = bump_attempts(state, &finding.key);
                if settings.notify {
                    let body = format!("{}\n\nPanel {MAX_AUTO_ATTEMPTS} kez düzeltmeyi denedi ama sorun sürüyor; elle bakmanız gerekiyor.", finding.detail);
                    notify_admins(state, &finding.title, &body).await;
                }
            }
            continue;
        }
        if now() - last_fix_at < retry_gap_secs(finding.rule) {
            continue;
        }
        let _ = bump_attempts(state, &finding.key);
        let outcome = apply_fix(state, &finding.key, None).await;
        let (success, detail) = match &outcome {
            Ok(message) => (true, message.clone()),
            Err(err) => (false, err.to_string()),
        };
        record_action(state, finding, true, success, &detail);
        if settings.notify {
            let subject = if success { format!("{} (panel düzeltti)", finding.title) } else { format!("{} (düzeltilemedi)", finding.title) };
            notify_admins(state, &subject, &detail).await;
        }
    }
}

fn bump_attempts(state: &AppState, key: &str) -> AppResult<()> {
    state.db.call(|conn| {
        conn.execute("UPDATE health_findings SET fix_attempts = fix_attempts + 1, last_fix_at = ?1 WHERE key = ?2", params![now(), key])
    })?;
    Ok(())
}

fn record_action(state: &AppState, finding: &Finding, automatic: bool, success: bool, detail: &str) {
    let target = finding.key.split_once(':').map(|(_, target)| target).unwrap_or("");
    let saved = state.db.call(|conn| {
        conn.execute(
            "INSERT INTO health_actions (created_at, rule, target, summary, automatic, success, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![now(), finding.rule, target, finding.title, automatic, success, detail],
        )?;
        // Günlük sınırsız büyümesin.
        conn.execute("DELETE FROM health_actions WHERE id NOT IN (SELECT id FROM health_actions ORDER BY id DESC LIMIT 500)", [])
    });
    if let Err(err) = saved {
        tracing::warn!(error = %err, "bakım işlemi kaydedilemedi");
    }
    crate::events::publish(crate::events::Event::Changed { area: "health".into() }, None);
}

pub fn recent_actions(state: &AppState, limit: i64) -> AppResult<Vec<Action>> {
    state.db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, created_at, rule, target, summary, automatic, success, detail FROM health_actions ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(Action {
                id: row.get(0)?,
                created_at: row.get(1)?,
                rule: row.get(2)?,
                target: row.get(3)?,
                summary: row.get(4)?,
                automatic: row.get(5)?,
                success: row.get(6)?,
                detail: row.get(7)?,
            })
        })?;
        rows.collect()
    })
}

// ---- Düzeltmeler ----

/// Yöneticinin "Düzelt" düğmesiyle istediği düzeltme.
pub async fn fix_now(state: &AppState, key: &str, user_id: i64) -> AppResult<String> {
    let finding = state.health.report().findings.into_iter().find(|finding| finding.key == key).ok_or(AppError::NotFound)?;
    if finding.fix.is_none() {
        return Err(AppError::bad("Bu bulgu tek tıkla düzeltilemiyor"));
    }
    let outcome = apply_fix(state, key, Some(user_id)).await;
    let (success, detail) = match &outcome {
        Ok(message) => (true, message.clone()),
        Err(err) => (false, err.to_string()),
    };
    record_action(state, &finding, false, success, &detail);
    outcome
}

/// Anahtarın gösterdiği düzeltmeyi uygular. Hedef her zaman anahtardan yeniden
/// doğrulanarak okunur; bulgunun başlığındaki metne güvenilmez.
async fn apply_fix(state: &AppState, key: &str, user_id: Option<i64>) -> AppResult<String> {
    let (rule, target) = key.split_once(':').ok_or_else(|| AppError::bad("Geçersiz bulgu anahtarı"))?;
    match rule {
        "service.failed" | "service.stopped" => {
            let (unit, label) = watched_units().into_iter().find(|(unit, _)| unit == target).ok_or_else(|| AppError::bad("Bu servis izlenmiyor"))?;
            // Çökmüş birimin hata sayacı temizlenmezse systemd yeniden başlatmayı reddedebilir.
            let _ = exec::run("systemctl", &["reset-failed", "--", &unit]).await;
            crate::sys::services::action(&unit, "restart").await?;
            Ok(format!("{label} yeniden başlatıldı."))
        }
        "ssl.missing" | "ssl.expiring" | "ssl.expired" => {
            let id: i64 = target.parse().map_err(|_| AppError::bad("Geçersiz site"))?;
            let site = crate::hosting::sites::load(&state.db, id)?.ok_or(AppError::NotFound)?;
            if site.ssl_mode == "selfsigned" {
                let _guard = state.sys_lock.lock().await;
                let mut names = vec![site.domain.clone()];
                names.extend(site.aliases.iter().cloned());
                let (certificate, private_key) = ssl::generate_self_signed(&names)?;
                crate::hosting::sites::activate_ssl(&state.db, &site, "selfsigned", &certificate, &private_key).await?;
                return Ok(format!("{} için kendinden imzalı sertifika yenilendi.", site.domain));
            }
            // Var olan sertifika ilk alındığı yöntemle yenilenir; sertifikasız siteye HTTP doğrulamasıyla alınır.
            let mode: &'static str = match site.ssl_mode.as_str() {
                "letsencrypt_dns" => "letsencrypt_dns",
                "letsencrypt_wildcard" => "letsencrypt_wildcard",
                "none" | "letsencrypt" => "letsencrypt",
                _ => return Err(AppError::bad("Bu sertifikayı siz yüklediniz; yenisini SSL sayfasından yükleyin")),
            };
            // Takma adlar yalnızca var olan sertifika yenilenirken eklenir: sertifikasız sitede
            // bir takma adın DNS'i eksikse bütün istek reddedilirdi.
            let names = crate::api::ssl::issue_names(&site, mode, site.ssl_mode != "none");
            crate::api::ssl::start_issue_job(state, user_id, site.id, &site.domain, names, mode)?;
            Ok(format!("{} için sertifika isteği başlatıldı.", site.domain))
        }
        "backup.failed" => {
            let id: i64 = target.parse().map_err(|_| AppError::bad("Geçersiz plan"))?;
            let plan = crate::backup::load_plan(&state.db, id)?;
            let name = plan.name.clone();
            crate::backup::run::start_plan(state, user_id, plan)?;
            Ok(format!("“{name}” yedek planı yeniden başlatıldı."))
        }
        "backup.unscheduled" => {
            let name = create_daily_backup_plan(state, user_id).await?;
            Ok(format!("“{name}” planı oluşturuldu; ilk yedek bu gece 03:00'te alınacak."))
        }
        "service.unresponsive" => {
            let (unit, label) = watched_units().into_iter().find(|(unit, _)| unit == target).ok_or_else(|| AppError::bad("Bu servis izlenmiyor"))?;
            crate::sys::services::action(&unit, "restart").await?;
            Ok(format!("{label} yeniden başlatıldı."))
        }
        "updates.pending" => {
            start_upgrade_job(state, user_id)?;
            Ok("Güncellemelerin kurulumu başlatıldı.".into())
        }
        _ => Err(AppError::bad("Bu bulgu tek tıkla düzeltilemiyor")),
    }
}

/// İlk yedek yerine, her şeyi kapsayan günlük bir plan ekler ve adını döner.
async fn create_daily_backup_plan(state: &AppState, user_id: Option<i64>) -> AppResult<String> {
    use crate::backup::{Plan, Scope};
    let target = crate::backup::list_targets(&state.db)?.into_iter().next().ok_or_else(|| AppError::bad("Önce bir yedek yeri ekleyin"))?;
    let owner = match user_id {
        Some(id) => id,
        None => state.db.call(|conn| conn.query_row("SELECT id FROM users WHERE role = 'admin' ORDER BY id LIMIT 1", [], |row| row.get(0)))?,
    };
    let existing = crate::backup::list_plans(&state.db)?;
    let mut name = "Günlük tam yedek".to_string();
    let mut suffix = 2;
    while existing.iter().any(|plan| plan.name == name) {
        name = format!("Günlük tam yedek {suffix}");
        suffix += 1;
    }
    let mut plan = Plan {
        id: 0,
        user_id: owner,
        name: name.clone(),
        target_id: target.id,
        scope: Scope { all_sites: true, all_databases: true, all_mail: true, panel: true, ..Scope::default() },
        frequency: "daily".into(),
        at_minute: 180,
        weekday: 1,
        month_day: 1,
        keep_daily: 7,
        keep_weekly: 4,
        keep_monthly: 6,
        enabled: true,
        next_run_at: None,
        last_run_at: None,
        last_status: String::new(),
        last_job_id: None,
        created_at: now(),
    };
    plan.validate()?;
    plan.next_run_at = plan.next_run(now(), crate::backup::local_offset().await);
    crate::backup::save_plan(&state.db, &plan)?;
    Ok(name)
}

/// Güvenlik güncellemelerinin kendiliğinden kurulmasını ayara göre açar ya da kapatır.
/// Yapılandırma zaten istenen durumdaysa hiçbir şey yapmaz.
pub async fn apply_security_updates(state: &AppState, enabled: bool) {
    let wanted = if enabled {
        "// PixPanel tarafından yazıldı (Genel bakış → Panelin kendiliğinden yaptıkları).\nAPT::Periodic::Update-Package-Lists \"1\";\nAPT::Periodic::Unattended-Upgrade \"1\";\n"
    } else {
        "// PixPanel tarafından yazıldı (Genel bakış → Panelin kendiliğinden yaptıkları).\nAPT::Periodic::Update-Package-Lists \"1\";\nAPT::Periodic::Unattended-Upgrade \"0\";\n"
    };
    let installed = exec::binary_exists("unattended-upgrade");
    if std::fs::read_to_string(AUTO_UPGRADES_FILE).ok().as_deref() == Some(wanted) && (installed || !enabled) {
        return;
    }
    if enabled && !installed {
        let _guard = state.apt_lock.lock().await;
        let install = exec::run_with_timeout(
            "apt-get",
            &["install", "-y", "--no-install-recommends", "-o", "DPkg::Lock::Timeout=180", "-o", "Dpkg::Options::=--force-confold", "--", "unattended-upgrades"],
            Duration::from_secs(600),
        )
        .await;
        if !matches!(&install, Ok(output) if output.success()) {
            tracing::warn!("unattended-upgrades kurulamadı; güvenlik güncellemeleri kendiliğinden kurulmayacak");
            return;
        }
    }
    if let Err(err) = crate::hosting::atomic_write(std::path::Path::new(AUTO_UPGRADES_FILE), wanted.as_bytes(), 0o644) {
        tracing::warn!(error = %err, "güvenlik güncellemesi ayarı yazılamadı");
    }
}

/// İşletim sistemi paketlerini günceller. Yapılandırma dosyalarında var olan hâl korunur.
fn start_upgrade_job(state: &AppState, user_id: Option<i64>) -> AppResult<i64> {
    let job_state = state.clone();
    state.jobs.start("system.upgrade", "Paket güncellemeleri", user_id, move |ctx| async move {
        if job_state.apt_lock.try_lock().is_err() {
            ctx.log("Başka bir kurulumun bitmesi bekleniyor");
        }
        let _guard = job_state.apt_lock.lock().await;
        ctx.exec("apt-get", &["update", "-o", "DPkg::Lock::Timeout=180"]).await?;
        ctx.exec(
            "apt-get",
            &["upgrade", "-y", "-o", "DPkg::Lock::Timeout=180", "-o", "Dpkg::Use-Pty=0", "-o", "Dpkg::Options::=--force-confold"],
        )
        .await?;
        job_state.health.forget_updates();
        ctx.log("Güncellemeler kuruldu");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_blocks_are_split_per_unit() {
        let output = "Id=nginx.service\nLoadState=loaded\nActiveState=failed\nUnitFileState=enabled\nResult=exit-code\n\nId=pdns.service\nLoadState=not-found\nActiveState=inactive\nUnitFileState=\nResult=success\n";
        let blocks = parse_unit_blocks(output);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["Id"], "nginx.service");
        assert_eq!(blocks[0]["ActiveState"], "failed");
        assert_eq!(blocks[1]["LoadState"], "not-found");
        assert_eq!(blocks[1]["UnitFileState"], "");
    }

    #[test]
    fn upgrades_are_counted_with_security_subset() {
        let simulation = "Reading package lists...\nInst libssl3 [3.0.1] (3.0.2 Debian-Security:13/stable-security [amd64])\nInst curl [8.1] (8.2 Debian:13.2/stable [amd64])\nConf libssl3 (3.0.2 Debian-Security:13/stable-security [amd64])\nConf curl (8.2 Debian:13.2/stable [amd64])\n";
        assert_eq!(count_upgrades(simulation), (2, 1));
        assert_eq!(count_upgrades("Reading package lists...\n0 upgraded, 0 newly installed\n"), (0, 0));
    }

    #[test]
    fn finding_key_combines_rule_and_target() {
        let finding = Finding::new("ssl.missing", 12, Severity::Warning, "başlık", "ayrıntı").fix("Sertifika al").auto(true);
        assert_eq!(finding.key, "ssl.missing:12");
        assert_eq!(finding.fix, Some("Sertifika al"));
        assert!(finding.auto);
    }

    #[test]
    fn many_similar_suggestions_collapse_into_one() {
        let make = |id: i64| Finding::new("php.eol", id, Severity::Info, "başlık", "ayrıntı").subject(format!("site{id}.test"));
        let mut few: Vec<Finding> = (1..=3).map(make).collect();
        collapse(&mut few, "php.eol", |count| format!("{count} site"), "Açıklama.");
        assert_eq!(few.len(), 3, "üç bulguya kadar ayrı ayrı gösterilir");

        let mut many: Vec<Finding> = (1..=8).map(make).collect();
        many.push(Finding::new("disk.full", "/", Severity::Critical, "disk", "dolu"));
        collapse(&mut many, "php.eol", |count| format!("{count} site"), "Açıklama.");
        assert_eq!(many.len(), 2);
        let merged = many.iter().find(|finding| finding.rule == "php.eol").unwrap();
        assert_eq!(merged.key, "php.eol:many");
        assert_eq!(merged.title, "8 site");
        assert!(merged.detail.contains("site1.test") && merged.detail.contains("ve 3 site daha"));
        assert!(many.iter().any(|finding| finding.rule == "disk.full"), "öteki kurallara dokunulmaz");
    }

    #[test]
    fn severity_orders_critical_highest() {
        assert!(Severity::Critical > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn certificate_retries_are_spaced_wider_than_service_retries() {
        assert!(retry_gap_secs("ssl.missing") > retry_gap_secs("service.failed"));
    }

    #[test]
    fn only_panel_issued_certificates_are_renewable() {
        assert!(renewable("letsencrypt"));
        assert!(renewable("letsencrypt_wildcard"));
        assert!(renewable("selfsigned"));
        assert!(!renewable("custom"));
    }
}
