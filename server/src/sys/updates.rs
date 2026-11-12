//! İşletim sistemi paket güncellemeleri: bekleyen paketler, çekirdek ve yeniden
//! başlatma durumu, güncellemelerin kurulumu (arka plan görevi) ve yeniden başlatma.
//!
//! Bütün komutlar `sys::exec` ile argüman dizisi olarak çalışır. Paket adları apt çıktısından
//! gelir ama yine de beyaz listeyle doğrulanmadan hiçbir komuta verilmez.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use serde::Serialize;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sys::exec;

const REBOOT_REQUIRED: &str = "/var/run/reboot-required";
const REBOOT_PACKAGES: &str = "/var/run/reboot-required.pkgs";
const SCHEDULED_SHUTDOWN: &str = "/run/systemd/shutdown/scheduled";
const AUTO_UPGRADES_FILE: &str = "/etc/apt/apt.conf.d/20auto-upgrades";
const APT_LISTS_DIR: &str = "/var/lib/apt/lists";
const UNATTENDED_STAMP: &str = "/var/lib/apt/periodic/unattended-upgrades-stamp";
const UNATTENDED_LOG: &str = "/var/log/unattended-upgrades/unattended-upgrades.log";
const UNATTENDED_DPKG_LOG: &str = "/var/log/unattended-upgrades/unattended-upgrades-dpkg.log";

/// Görev türü; aynı anda tek güncelleme görevi çalışsın diye de bununla aranır.
pub const JOB_KIND: &str = "system.upgrade";

/// Paket listesinin en son başarıyla yenilendiği an (Unix saniyesi); panel yeniden
/// başlayınca sıfırlanır, o zaman apt listelerinin dosya zamanına bakılır.
static LAST_CHECK: AtomicI64 = AtomicI64::new(0);

/// Paket listesinin şimdi yenilendiğini kaydeder.
pub fn mark_checked() {
    LAST_CHECK.store(now(), Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PendingPackage {
    pub name: String,
    /// Kurulu sürüm; pakete yeni bağımlılık olarak geliyorsa boş.
    pub installed: String,
    pub candidate: String,
    pub security: bool,
}

#[derive(Debug, Serialize)]
pub struct KernelInfo {
    pub running: String,
    /// Kurulu en yeni çekirdek paketinin sürümü (çekirdek paketi yoksa, ör. konteynerde, boş).
    pub latest: Option<String>,
    /// Kurulu çekirdek çalışandan yeni; yeniden başlatınca devreye girer.
    pub outdated: bool,
}

#[derive(Debug, Serialize)]
pub struct UnattendedInfo {
    /// `unattended-upgrade` programı kurulu mu?
    pub installed: bool,
    /// Yapılandırma dosyası günlük kurulumu açıyor mu?
    pub configured: bool,
    /// Son çalışma (kurulacak bir şey olmasa da çalıştığı an).
    pub last_run: Option<i64>,
    /// Paket kurduğu son an.
    pub last_install: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RebootInfo {
    pub required: bool,
    pub packages: Vec<String>,
    /// Zamanlanmış yeniden başlatmanın zamanı (Unix saniyesi).
    pub scheduled_at: Option<i64>,
    pub scheduled_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdatesView {
    pub checked_at: i64,
    pub total: usize,
    pub security: usize,
    pub packages: Vec<PendingPackage>,
    pub reboot: RebootInfo,
    pub kernel: KernelInfo,
    pub unattended: UnattendedInfo,
    /// Yeniden başlatılması gereken servisler; `needrestart` kurulu değilse `None`.
    pub services_to_restart: Option<Vec<String>>,
    /// Şu an bir güncelleme görevi çalışıyor mu?
    pub busy: bool,
    /// Paket listesi yenilenemediyse nedeni (yenileme isteğinde).
    pub refresh_error: Option<String>,
}

/// Paket adı: Debian kurallarının izin verdiği küçük harf, rakam ve `+ . : -`.
pub fn valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 128
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | ':' | '-'))
}

/// `apt-get -s upgrade` çıktısından bekleyen paketleri çıkarır. Satır biçimi:
/// `Inst libssl3 [3.0.1] (3.0.2 Debian-Security:13/stable-security [amd64])`
pub fn parse_pending(simulation: &str) -> Vec<PendingPackage> {
    let mut packages = Vec::new();
    for line in simulation.lines() {
        let Some(rest) = line.strip_prefix("Inst ") else { continue };
        let (name, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        if !valid_package_name(name) {
            continue;
        }
        let (before_origin, origin) = match rest.split_once('(') {
            Some((before, after)) => (before, after),
            None => (rest, ""),
        };
        let installed = before_origin
            .split_once('[')
            .and_then(|(_, tail)| tail.split_once(']'))
            .map(|(version, _)| version.trim().to_string())
            .unwrap_or_default();
        let candidate = origin.split_whitespace().next().unwrap_or_default().trim_end_matches(')').to_string();
        let security = origin.to_ascii_lowercase().contains("security");
        packages.push(PendingPackage { name: name.to_string(), installed, candidate, security });
    }
    packages.sort_by(|a, b| b.security.cmp(&a.security).then_with(|| a.name.cmp(&b.name)));
    packages
}

/// Bekleyen güncellemeleri benzetimle (sistemde değişiklik yapmadan) listeler.
/// Yeni bağımlılık gerektiren paketler de sayılır (`--with-new-pkgs`); yoksa çekirdek
/// güncellemeleri "tutuldu" diye hiç görünmez.
pub async fn scan() -> AppResult<Vec<PendingPackage>> {
    let output = exec::run_with_timeout(
        "apt-get",
        &["-s", "-o", "Debug::NoLocking=true", "upgrade", "--with-new-pkgs"],
        Duration::from_secs(120),
    )
    .await?;
    if !output.success() {
        let detail = if output.stderr.trim().is_empty() { output.stdout.trim() } else { output.stderr.trim() };
        return Err(AppError::internal(format!("Bekleyen paketler okunamadı: {detail}")));
    }
    Ok(parse_pending(&output.stdout))
}

fn mtime(path: &str) -> Option<i64> {
    let modified = std::fs::metadata(path).and_then(|meta| meta.modified()).ok()?;
    modified.duration_since(UNIX_EPOCH).ok().map(|age| age.as_secs() as i64)
}

/// Paket listesinin son yenilenme zamanı: panelin kaydı ile apt listelerinin en yeni dosya zamanından büyüğü.
pub fn checked_at() -> i64 {
    let lists = std::fs::read_dir(APT_LISTS_DIR)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
                .filter_map(|entry| entry.metadata().ok()?.modified().ok())
                .filter_map(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|age| age.as_secs() as i64)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    LAST_CHECK.load(Ordering::Relaxed).max(lists)
}

/// `6.12.48+deb13-amd64` gibi bir sürümden sayı öbeklerini çıkarır (karşılaştırma için).
fn version_key(release: &str) -> Vec<u64> {
    release.split(|c: char| !c.is_ascii_digit()).filter(|part| !part.is_empty()).filter_map(|part| part.parse().ok()).collect()
}

/// Çıktıdaki kurulu `linux-image-<sürüm>` paketlerinden en yenisinin sürümü.
fn latest_kernel(dpkg_output: &str) -> Option<String> {
    dpkg_output
        .lines()
        .filter_map(|line| {
            let (status, name) = line.split_once('\t')?;
            let release = name.trim().strip_prefix("linux-image-")?;
            (status.starts_with("ii") && release.starts_with(|c: char| c.is_ascii_digit())).then(|| release.to_string())
        })
        .max_by_key(|release| version_key(release))
}

async fn kernel_info() -> KernelInfo {
    let running = std::fs::read_to_string("/proc/sys/kernel/osrelease").map(|raw| raw.trim().to_string()).unwrap_or_default();
    let listing = exec::run("dpkg-query", &["-W", "-f=${db:Status-Abbrev}\t${Package}\n", "linux-image-*"]).await;
    let latest = listing.ok().and_then(|output| latest_kernel(&output.stdout));
    let outdated = latest.as_ref().is_some_and(|latest| version_key(latest) > version_key(&running));
    KernelInfo { running, latest, outdated }
}

fn unattended_info() -> UnattendedInfo {
    let configured = std::fs::read_to_string(AUTO_UPGRADES_FILE)
        .map(|raw| {
            raw.lines().any(|line| {
                let line = line.trim();
                !line.starts_with("//") && line.contains("Unattended-Upgrade") && line.contains("\"1\"")
            })
        })
        .unwrap_or(false);
    UnattendedInfo {
        installed: exec::binary_exists("unattended-upgrade"),
        configured,
        last_run: mtime(UNATTENDED_STAMP).or_else(|| mtime(UNATTENDED_LOG)),
        last_install: mtime(UNATTENDED_DPKG_LOG),
    }
}

/// `/run/systemd/shutdown/scheduled` içeriğinden (zaman, kip) çıkarır.
fn parse_scheduled(raw: &str) -> Option<(i64, String)> {
    let field = |key: &str| raw.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=')).map(str::trim);
    let micros: i64 = field("USEC")?.parse().ok()?;
    let mode = field("MODE").unwrap_or("reboot").to_string();
    Some((micros / 1_000_000, mode))
}

fn reboot_info() -> RebootInfo {
    let mut packages: Vec<String> = std::fs::read_to_string(REBOOT_PACKAGES)
        .map(|raw| raw.lines().map(str::trim).filter(|name| valid_package_name(name)).map(str::to_string).collect())
        .unwrap_or_default();
    packages.sort();
    packages.dedup();
    packages.truncate(50);
    let scheduled = std::fs::read_to_string(SCHEDULED_SHUTDOWN).ok().and_then(|raw| parse_scheduled(&raw));
    RebootInfo {
        required: Path::new(REBOOT_REQUIRED).exists(),
        packages,
        scheduled_at: scheduled.as_ref().map(|(at, _)| *at),
        scheduled_mode: scheduled.map(|(_, mode)| mode),
    }
}

/// `needrestart -b` çıktısından yeniden başlatılması gereken servis birimleri.
fn parse_needrestart(output: &str) -> Vec<String> {
    let mut units: Vec<String> = output
        .lines()
        .filter_map(|line| line.strip_prefix("NEEDRESTART-SVC:"))
        .map(|name| name.trim().to_string())
        .filter(|name| {
            !name.is_empty() && name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | ':'))
        })
        .collect();
    units.sort();
    units.dedup();
    units.truncate(100);
    units
}

async fn services_to_restart() -> Option<Vec<String>> {
    if !exec::binary_exists("needrestart") {
        return None;
    }
    // `-r l` yalnızca listeler; hiçbir servisi yeniden başlatmaz.
    let output = exec::run_with_timeout("needrestart", &["-b", "-r", "l"], Duration::from_secs(60)).await.ok()?;
    output.success().then(|| parse_needrestart(&output.stdout))
}

/// Güncelleme görevi çalışıyor mu?
pub fn job_running(state: &AppState) -> bool {
    state.jobs.list(30).map(|jobs| jobs.iter().any(|job| job.kind == JOB_KIND && job.status == "running")).unwrap_or(false)
}

/// Sayfanın tamamı için durum. Bekleyen paketler her çağrıda benzetimle yeniden okunur
/// (saniyeler sürmez); paket listesi ise yalnızca `refresh` ile ağdan yenilenir.
pub async fn view(state: &AppState, refresh_error: Option<String>) -> AppResult<UpdatesView> {
    let packages = scan().await?;
    let security = packages.iter().filter(|package| package.security).count();
    state.health.set_updates(packages.len(), security);
    let (kernel, services) = tokio::join!(kernel_info(), services_to_restart());
    Ok(UpdatesView {
        checked_at: checked_at(),
        total: packages.len(),
        security,
        packages,
        reboot: reboot_info(),
        kernel,
        unattended: unattended_info(),
        services_to_restart: services,
        busy: job_running(state),
        refresh_error,
    })
}

/// Paket listesini ağdan yeniler (`apt-get update`). Başka bir paket işlemi sürüyorsa
/// bekletmeden reddeder. Yenileme başarısızsa (ağ yok) nedenini döner; eldeki listeyle devam edilir.
pub async fn refresh(state: &AppState) -> AppResult<Option<String>> {
    let Ok(_guard) = state.apt_lock.try_lock() else {
        return Err(AppError::conflict("Başka bir paket işlemi sürüyor; bitince yeniden deneyin"));
    };
    let output =
        exec::run_with_timeout("apt-get", &["update", "-qq", "-o", "DPkg::Lock::Timeout=60"], Duration::from_secs(300)).await?;
    if output.success() {
        mark_checked();
        return Ok(None);
    }
    let detail = if output.stderr.trim().is_empty() { output.stdout.trim() } else { output.stderr.trim() };
    Ok(Some(detail.lines().take(3).collect::<Vec<_>>().join(" ")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Security,
    All,
}

impl Scope {
    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "security" => Ok(Self::Security),
            "all" => Ok(Self::All),
            _ => Err(AppError::bad("Geçersiz kapsam: security ya da all olmalı")),
        }
    }
}

/// Güncellemeleri kuran arka plan görevini başlatır. Paket kilidi görev boyunca tutulur;
/// kilit zaman aşımı ve yapılandırma dosyalarında var olan hâli koruma (`--force-confold`)
/// apt'a verilir. Çıktı canlı olarak görev günlüğüne akar.
pub fn start_install(state: &AppState, user_id: Option<i64>, scope: Scope) -> AppResult<i64> {
    if job_running(state) {
        return Err(AppError::conflict("Bir güncelleme görevi zaten çalışıyor"));
    }
    let job_state = state.clone();
    let title = match scope {
        Scope::Security => "Güvenlik güncellemeleri",
        Scope::All => "Paket güncellemeleri",
    };
    state.jobs.start(JOB_KIND, title, user_id, move |ctx| async move {
        if job_state.apt_lock.try_lock().is_err() {
            ctx.log("Başka bir paket işleminin bitmesi bekleniyor");
        }
        let _guard = job_state.apt_lock.lock().await;
        ctx.exec("apt-get", &["update", "-o", "DPkg::Lock::Timeout=180"]).await?;
        mark_checked();

        let pending = scan().await?;
        let apt_options = ["-y", "-o", "DPkg::Lock::Timeout=180", "-o", "Dpkg::Use-Pty=0", "-o", "Dpkg::Options::=--force-confold"];
        match scope {
            Scope::Security => {
                let names: Vec<&str> = pending.iter().filter(|package| package.security).map(|package| package.name.as_str()).collect();
                if names.is_empty() {
                    ctx.log("Kurulacak güvenlik güncellemesi yok");
                } else {
                    ctx.log(format!("{} güvenlik güncellemesi kurulacak", names.len()));
                    let mut args: Vec<&str> = vec!["install", "--only-upgrade", "--no-install-recommends"];
                    args.extend_from_slice(&apt_options);
                    args.push("--");
                    args.extend(names);
                    ctx.exec("apt-get", &args).await?;
                }
            }
            Scope::All => {
                if pending.is_empty() {
                    ctx.log("Sistem güncel, kurulacak paket yok");
                } else {
                    ctx.log(format!("{} paket güncellenecek", pending.len()));
                    let mut args: Vec<&str> = vec!["upgrade", "--with-new-pkgs"];
                    args.extend_from_slice(&apt_options);
                    ctx.exec("apt-get", &args).await?;
                }
            }
        }

        // Sonuç: kalan bekleyen sayısı yeniden ölçülür ki Genel bakış hemen güncellensin.
        match scan().await {
            Ok(left) => {
                let security = left.iter().filter(|package| package.security).count();
                job_state.health.set_updates(left.len(), security);
                ctx.log(format!("Kalan bekleyen güncelleme: {} ({security} güvenlik)", left.len()));
            }
            Err(err) => ctx.log(format!("Uyarı: kalan güncellemeler sayılamadı: {err}")),
        }
        if Path::new(REBOOT_REQUIRED).exists() {
            ctx.log("Sunucunun yeniden başlatılması gerekiyor");
        }
        ctx.log("Güncelleme görevi tamamlandı");
        Ok(())
    })
}

/// Sunucunun yeniden başlatılmasını bir dakika sonrasına zamanlar. Konteynerde (ör. yetkisiz
/// LXC) zamanlama çalışmayabilir; komutun hata iletisi olduğu gibi döner.
pub async fn schedule_reboot() -> AppResult<()> {
    let output = exec::run("shutdown", &["-r", "+1", "PixPanel: yönetici sunucuyu yeniden başlatıyor"]).await?;
    if output.success() {
        return Ok(());
    }
    let detail = if output.stderr.trim().is_empty() { output.stdout.trim() } else { output.stderr.trim() };
    Err(AppError::internal(format!("Yeniden başlatma zamanlanamadı: {detail}")))
}

/// Zamanlanmış yeniden başlatmayı iptal eder.
pub async fn cancel_reboot() -> AppResult<()> {
    let output = exec::run("shutdown", &["-c"]).await?;
    if output.success() {
        return Ok(());
    }
    let detail = if output.stderr.trim().is_empty() { output.stdout.trim() } else { output.stderr.trim() };
    Err(AppError::internal(format!("Yeniden başlatma iptal edilemedi: {detail}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_packages_are_parsed_with_security_flag() {
        let simulation = "Reading package lists...\n\
            Inst libssl3 [3.0.1] (3.0.2 Debian-Security:13/stable-security [amd64])\n\
            Inst curl [8.1] (8.2 Debian:13.2/stable [amd64])\n\
            Inst linux-image-6.12.57 (6.12.57-1 Debian:13.2/stable [amd64])\n\
            Conf libssl3 (3.0.2 Debian-Security:13/stable-security [amd64])\n";
        let packages = parse_pending(simulation);
        assert_eq!(packages.len(), 3);
        assert_eq!(packages[0], PendingPackage { name: "libssl3".into(), installed: "3.0.1".into(), candidate: "3.0.2".into(), security: true });
        assert_eq!(packages[1].name, "curl");
        assert!(!packages[1].security);
        assert_eq!(packages[2].installed, "", "yeni bağımlılığın kurulu sürümü yok");
        assert_eq!(packages[2].candidate, "6.12.57-1");
    }

    #[test]
    fn invalid_package_names_are_dropped() {
        let packages = parse_pending("Inst --evil [1] (2 Debian:13/stable [amd64])\nInst Bad_Name [1] (2 x [amd64])\nInst ok-pkg+2 [1] (2 x [amd64])\n");
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "ok-pkg+2");
        assert!(valid_package_name("libc6:amd64") && !valid_package_name("-x") && !valid_package_name("a b") && !valid_package_name(""));
    }

    #[test]
    fn kernel_versions_compare_numerically() {
        let listing = "ii \tlinux-image-6.12.9+deb13-amd64\nii \tlinux-image-6.12.57+deb13-amd64\nrc \tlinux-image-6.13.1+deb13-amd64\nii \tlinux-image-amd64\n";
        assert_eq!(latest_kernel(listing).as_deref(), Some("6.12.57+deb13-amd64"));
        assert!(version_key("6.12.57+deb13-amd64") > version_key("6.12.9+deb13-amd64"));
        assert_eq!(latest_kernel(""), None);
    }

    #[test]
    fn scheduled_shutdown_is_parsed() {
        let raw = "USEC=1794357060000000\nWARN_WALL=1\nMODE=reboot\nWALL_MESSAGE=x\n";
        assert_eq!(parse_scheduled(raw), Some((1_794_357_060, "reboot".to_string())));
        assert_eq!(parse_scheduled("MODE=reboot\n"), None);
    }

    #[test]
    fn needrestart_services_are_extracted() {
        let output = "NEEDRESTART-VER: 3.8\nNEEDRESTART-KCUR: 6.12\nNEEDRESTART-SVC: nginx.service\nNEEDRESTART-SVC: ssh.service\nNEEDRESTART-SVC: bad name;x\nNEEDRESTART-SVC: nginx.service\n";
        assert_eq!(parse_needrestart(output), vec!["nginx.service".to_string(), "ssh.service".to_string()]);
    }

    #[test]
    fn scope_accepts_only_known_values() {
        assert_eq!(Scope::parse("security").unwrap(), Scope::Security);
        assert_eq!(Scope::parse("all").unwrap(), Scope::All);
        assert!(Scope::parse("everything").is_err());
        assert!(Scope::parse("").is_err());
    }
}
