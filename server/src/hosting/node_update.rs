//! Node.js yama sürümü güncellemesi.
//!
//! Panel her ana sürümü (20, 22, 24...) nodejs.org'un resmî derlemelerinden kurar
//! (`components::fetch_node_build`). Burada kurulu yama sürümü, nodejs.org'un `index.json`
//! listesindeki en son yama sürümüyle karşılaştırılır; fark varsa güncelleme bir görev olarak
//! yürür: yeni derleme indirilip sağlama toplamı doğrulanır, yerinde değiştirilir, o sürümü
//! kullanan Node.js servisleri sırayla yeniden başlatılır. Bir servis yeni sürümle ayağa
//! kalkmazsa eski sürüme dönülür.
//!
//! Ağ hatasında sürüm bilgisi "bilinmiyor" sayılır; hiçbir şey bozulmaz.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

use crate::components;
use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::node;
use crate::hosting::sites::{self, Site};
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::exec;

/// Resmî sürüm listesi (derlemelerin indirildiği yerle aynı sunucu).
const INDEX_URL: &str = "https://nodejs.org/dist/index.json";
/// Başarılı sorgunun önbellekte kalma süresi.
const CACHE_OK_SECS: i64 = 3600;
/// Başarısız sorgudan sonra yeniden denemeden önce beklenen süre.
const CACHE_FAIL_SECS: i64 = 600;
/// Servis yeniden başlatıldıktan sonra ayakta kaldığı doğrulanmadan önce beklenen süre.
const SETTLE: Duration = Duration::from_secs(5);

// ---- Sürüm karşılaştırma ve liste ayrıştırma ----

/// `major.minor.patch`
pub type Version = (u32, u32, u32);

/// `v22.11.0` ya da `22.11.0` biçimindeki sürümü ayrıştırır; başka biçimi (ön sürüm eki gibi) reddeder.
pub fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let mut next = || -> Option<u32> {
        let part = parts.next()?;
        (!part.is_empty() && part.len() <= 6 && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok()).flatten()
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

fn format_version(version: Version) -> String {
    format!("{}.{}.{}", version.0, version.1, version.2)
}

/// İki sürüm metnini karşılaştırır; biri ayrıştırılamazsa `None`.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

/// `index.json` içindeki bir yayın.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    /// Yayın bir güvenlik düzeltmesi içeriyor.
    pub security: bool,
}

/// `index.json` içeriğini ayrıştırır. Biçimi bozuk satırlar atlanır; liste ayrıştırılamazsa boş döner.
pub fn parse_index(json: &str) -> Vec<Release> {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(json) else { return Vec::new() };
    entries
        .iter()
        .filter_map(|entry| {
            let version = parse_version(entry.get("version")?.as_str()?)?;
            Some(Release { version, security: entry.get("security").and_then(|value| value.as_bool()).unwrap_or(false) })
        })
        .collect()
}

/// Ana sürümün en son yama sürümü.
pub fn latest_for(releases: &[Release], major: u32) -> Option<&Release> {
    releases.iter().filter(|release| release.version.0 == major).max_by_key(|release| release.version)
}

/// Kurulu sürümden sonra, aynı ana sürümde güvenlik düzeltmesi içeren bir yayın var mı?
pub fn has_security_update(releases: &[Release], installed: Version) -> bool {
    releases.iter().any(|release| release.version.0 == installed.0 && release.version > installed && release.security)
}

/// Kurulu sürümün durumu: (en son sürüm, güncelleme var mı, bekleyen güncelleme güvenlik içeriyor mu).
/// Liste yoksa (ağ hatası) ya da kurulu sürüm okunamıyorsa güncelleme "yok" sayılır.
fn assess(installed: &str, releases: Option<&[Release]>) -> (Option<String>, bool, bool) {
    let (Some(releases), Some(installed)) = (releases, parse_version(installed)) else { return (None, false, false) };
    let Some(latest) = latest_for(releases, installed.0) else { return (None, false, false) };
    let update = latest.version > installed;
    (Some(format_version(latest.version)), update, update && has_security_update(releases, installed))
}

// ---- Önbellek ----

struct Cache {
    fetched_at: i64,
    /// Sorgu başarılı mıydı (başarısızsa `releases` boştur).
    ok: bool,
    releases: Vec<Release>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// En son yayın listesi ve listenin alındığı an. Sonuç saatlik önbelleğe alınır; ağ hatasında
/// `None` döner ve dakikalar içinde yeniden denenir.
async fn releases() -> Option<(Vec<Release>, i64)> {
    {
        let cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(cached) = cache.as_ref() {
            let ttl = if cached.ok { CACHE_OK_SECS } else { CACHE_FAIL_SECS };
            if now() - cached.fetched_at < ttl {
                return cached.ok.then(|| (cached.releases.clone(), cached.fetched_at));
            }
        }
    }
    let fetched = exec::run_with_timeout("curl", &["-fsSL", "--max-time", "20", "--max-filesize", "20000000", INDEX_URL], Duration::from_secs(30)).await;
    let list = fetched.ok().filter(|output| output.success()).map(|output| parse_index(&output.stdout)).unwrap_or_default();
    let fetched_at = now();
    let ok = !list.is_empty();
    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = Some(Cache { fetched_at, ok, releases: list.clone() });
    ok.then_some((list, fetched_at))
}

// ---- Durum ----

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    /// Ana sürüm (`22`).
    pub major: String,
    /// Kurulu tam sürüm (`22.11.0`).
    pub installed: String,
    /// En son yama sürümü; öğrenilemediyse `None` (güncelleme durumu bilinmiyor).
    pub latest: Option<String>,
    pub update_available: bool,
    /// Bekleyen güncellemelerden biri güvenlik düzeltmesi içeriyor.
    pub security: bool,
    /// Bu sürümü kullanan Node.js sitesi sayısı.
    pub sites: usize,
    /// Sürüm listesinin son alındığı an; listeye ulaşılamadıysa `None`.
    pub checked_at: Option<i64>,
}

/// Kurulu her Node.js ana sürümünün güncelleme durumu (yeniden eskiye).
pub async fn versions(db: &Db) -> AppResult<Vec<VersionInfo>> {
    let list = releases().await;
    let mut out = Vec::new();
    for major in node::installed_versions() {
        let installed = components::node_version(&major).await.unwrap_or_default();
        let (latest, update_available, security) = assess(&installed, list.as_ref().map(|(releases, _)| releases.as_slice()));
        out.push(VersionInfo {
            sites: sites::list_by_node(db, &major)?.len(),
            checked_at: list.as_ref().map(|(_, at)| *at),
            major,
            installed,
            latest,
            update_available,
            security,
        });
    }
    Ok(out)
}

// ---- Güncelleme görevi ----

/// Aynı ana sürümde aynı anda tek güncelleme yürür.
static RUNNING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Güncelleme işaretini tutar; düşünce işaret kalkar.
struct UpdateLock(String);

impl UpdateLock {
    fn acquire(major: &str) -> AppResult<Self> {
        let mut running = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
        if running.iter().any(|other| other == major) {
            return Err(AppError::conflict(format!("Node.js {major} için süren bir güncelleme var; bitmesini bekleyin")));
        }
        running.push(major.to_string());
        Ok(Self(major.to_string()))
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        RUNNING.lock().unwrap_or_else(|p| p.into_inner()).retain(|other| *other != self.0);
    }
}

/// Güncelleme görevini başlatır. Sürüm kurulu değilse hata döner; paket işlemleriyle aynı kilidi
/// kullanır (bir bileşen kurulumu ya da güncellemesi sürerken sıra bekler).
pub async fn start_job(state: &AppState, user_id: Option<i64>, major: &str) -> AppResult<i64> {
    node::validate_version(major)?;
    if components::node_version(major).await.is_none() {
        return Err(AppError::bad(format!("Node.js {major} kurulu değil")));
    }
    let lock = UpdateLock::acquire(major)?;
    let apt_lock = state.apt_lock.clone();
    let job_state = state.clone();
    let major = major.to_string();
    let title = format!("Node.js {major} güncellemesi");
    state.jobs.start("node.update", &title, user_id, move |ctx| async move {
        let _lock = lock;
        if apt_lock.try_lock().is_err() {
            ctx.log("Başka bir paket işleminin bitmesi bekleniyor");
        }
        let _guard = apt_lock.lock().await;
        update(&ctx, &job_state, &major).await
    })
}

/// Klasör adlarının üst klasörü verilerek çalışan, sınanabilir değiştirme yardımcıları.
fn swap_in(root: &Path, major: &str, staging: &Path) -> AppResult<PathBuf> {
    let target = root.join(major);
    let previous = root.join(format!("{major}.old"));
    let _ = std::fs::remove_dir_all(&previous);
    std::fs::rename(&target, &previous)?;
    if let Err(err) = std::fs::rename(staging, &target) {
        // Yeni derleme yerine konamadıysa eskisi geri alınır.
        let _ = std::fs::rename(&previous, &target);
        return Err(err.into());
    }
    Ok(previous)
}

/// `swap_in` işlemini geri alır: yeni sürüm kenara alınıp silinir, eskisi yerine döner.
fn swap_back(root: &Path, major: &str, previous: &Path) -> AppResult<()> {
    let target = root.join(major);
    let failed = root.join(format!("{major}.failed"));
    let _ = std::fs::remove_dir_all(&failed);
    std::fs::rename(&target, &failed)?;
    std::fs::rename(previous, &target)?;
    let _ = std::fs::remove_dir_all(&failed);
    Ok(())
}

/// Servis yeniden başlatıldıktan sonra ayakta kaldığını doğrular: bir süre beklenir, servis
/// çalışıyor olmalı ve bu sürede kendiliğinden yeniden başlamamış olmalıdır (çöküp dönen uygulama sağlam sayılmaz).
async fn wait_healthy(site_id: i64) -> Result<(), String> {
    let baseline = node::status(site_id).await.map(|status| status.restarts).unwrap_or(0);
    tokio::time::sleep(SETTLE).await;
    let status = node::status(site_id).await.map_err(|err| err.to_string())?;
    if status.state == "active" && status.sub_state == "running" && status.restarts == baseline {
        Ok(())
    } else {
        Err(format!("durum: {}/{}, kendiliğinden yeniden başlama: {}", status.state, status.sub_state, status.restarts.saturating_sub(baseline)))
    }
}

/// Yeni derlemenin çalıştığını (ve sürümünü) doğrular.
async fn build_version(staging: &Path) -> Option<String> {
    let binary = staging.join("bin/node");
    let output = exec::run_with_timeout(&binary.to_string_lossy(), &["--version"], Duration::from_secs(20)).await.ok()?;
    output.success().then(|| output.stdout.trim().trim_start_matches('v').to_string()).filter(|version| parse_version(version).is_some())
}

/// Ana sürümü yerinde günceller; çağıran `UpdateLock` ve paket kilidini tutar.
async fn update(ctx: &JobCtx, state: &AppState, major: &str) -> AppResult<()> {
    let root = Path::new(node::NODE_ROOT);
    let before = components::node_version(major).await.ok_or_else(|| AppError::bad(format!("Node.js {major} kurulu değil")))?;
    ctx.log(format!("Node.js {major}: kurulu sürüm {before}"));

    let staging = components::fetch_node_build(ctx, major).await?;
    let Some(after) = build_version(&staging).await else {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(AppError::internal("İndirilen Node.js derlemesi çalıştırılamadı; güncelleme yapılmadı"));
    };
    if compare_versions(&after, &before) != Some(Ordering::Greater) {
        let _ = std::fs::remove_dir_all(&staging);
        ctx.log(format!("Kurulu sürüm ({before}) zaten en son yama sürümü; yapılacak bir şey yok."));
        return Ok(());
    }
    ctx.log(format!("Yeni sürüm: {after}"));

    let previous = swap_in(root, major, &staging)?;
    ctx.log(format!("Node.js {major} {before} → {after} olarak değiştirildi"));

    // Sürümü kullanan servisler sırayla yeniden başlatılır; çalışmayanlara dokunulmaz.
    let users: Vec<Site> = sites::list_by_node(&state.db, major)?;
    let mut restarted: Vec<&Site> = Vec::new();
    for site in &users {
        if site.status != "active" || !node::is_running(site.id).await {
            ctx.log(format!("{}: servis çalışmıyor, yeniden başlatılmadı", site.domain));
            continue;
        }
        ctx.log(format!("{}: servis yeniden başlatılıyor", site.domain));
        restarted.push(site);
        let healthy = match node::action(site.id, "restart").await {
            Ok(()) => wait_healthy(site.id).await,
            Err(err) => Err(err.to_string()),
        };
        let Err(reason) = healthy else {
            ctx.log(format!("{}: servis ayakta", site.domain));
            continue;
        };
        ctx.log(format!("HATA: {} yeni sürümle ayağa kalkmadı ({reason}); eski sürüme dönülüyor", site.domain));
        return Err(roll_back(ctx, root, major, &previous, &restarted, &before, &site.domain, &reason).await);
    }

    let _ = std::fs::remove_dir_all(&previous);
    ctx.log(format!("Node.js {major} güncellendi: {before} → {after} ({} servis yeniden başlatıldı)", restarted.len()));
    Ok(())
}

/// Eski sürümü geri koyar ve yeniden başlatılmış servisleri ona döndürür; kullanıcıya gösterilecek hatayı döner.
#[allow(clippy::too_many_arguments)]
async fn roll_back(ctx: &JobCtx, root: &Path, major: &str, previous: &Path, restarted: &[&Site], before: &str, failed_domain: &str, reason: &str) -> AppError {
    if let Err(err) = swap_back(root, major, previous) {
        return AppError::internal(format!(
            "{failed_domain} yeni sürümle ayağa kalkmadı ({reason}) ve eski sürüm geri konamadı: {err}. {} klasörünü elle denetleyin.",
            root.join(format!("{major}.old")).display()
        ));
    }
    ctx.log(format!("Node.js {major} eski sürüme ({before}) döndürüldü"));
    for site in restarted {
        let outcome = match node::action(site.id, "restart").await {
            Ok(()) => wait_healthy(site.id).await,
            Err(err) => Err(err.to_string()),
        };
        match outcome {
            Ok(()) => ctx.log(format!("{}: eski sürümle ayakta", site.domain)),
            Err(err) => ctx.log(format!("UYARI: {} eski sürümle de ayağa kalkmadı ({err}); uygulamanın günlüğüne bakın", site.domain)),
        }
    }
    AppError::internal(format!("{failed_domain} Node.js'in yeni sürümüyle ayağa kalkmadı ({reason}). Node.js {major} eski sürüme ({before}) döndürüldü."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_parsed_strictly() {
        assert_eq!(parse_version("v22.11.0"), Some((22, 11, 0)));
        assert_eq!(parse_version(" 20.19.5\n"), Some((20, 19, 5)));
        for bad in ["", "v22", "22.11", "22.11.0.1", "22.11.x", "v22.11.0-rc.1", "22..0", "-1.0.0", "22.11.0\n23"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_compare_numerically_not_textually() {
        assert_eq!(compare_versions("22.9.0", "22.11.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("v22.11.1", "22.11.0"), Some(Ordering::Greater));
        assert_eq!(compare_versions("22.11.0", "v22.11.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("24.0.0", "20.19.5"), Some(Ordering::Greater));
        assert_eq!(compare_versions("x", "22.0.0"), None);
    }

    const INDEX: &str = r#"[
        {"version":"v24.1.0","date":"2026-02-01","security":false,"lts":false},
        {"version":"v22.12.0","date":"2026-01-10","security":true,"lts":"Jod"},
        {"version":"v22.11.1","date":"2026-01-02","security":false,"lts":"Jod"},
        {"version":"v22.11.0","date":"2025-12-01","security":false,"lts":"Jod"},
        {"version":"v20.19.5","date":"2025-11-01","security":true,"lts":"Iron"},
        {"version":"bozuk","security":true},
        {"security":true},
        {"version":"v21.0.0-rc.1","security":false}
    ]"#;

    #[test]
    fn index_is_parsed_and_bad_entries_skipped() {
        let list = parse_index(INDEX);
        assert_eq!(list.len(), 5);
        assert_eq!(list[1], Release { version: (22, 12, 0), security: true });
        assert!(parse_index("<html>hata</html>").is_empty());
        assert!(parse_index("{}").is_empty());
    }

    #[test]
    fn latest_patch_is_found_per_major() {
        let list = parse_index(INDEX);
        assert_eq!(latest_for(&list, 22).map(|r| r.version), Some((22, 12, 0)));
        assert_eq!(latest_for(&list, 20).map(|r| r.version), Some((20, 19, 5)));
        assert!(latest_for(&list, 18).is_none());
    }

    #[test]
    fn assessment_reports_update_and_security() {
        let list = parse_index(INDEX);
        // Eski yama, aradaki sürümlerden biri güvenlik düzeltmesi.
        assert_eq!(assess("22.11.0", Some(&list)), (Some("22.12.0".into()), true, true));
        // En son sürümdeyken güncelleme yok.
        assert_eq!(assess("22.12.0", Some(&list)), (Some("22.12.0".into()), false, false));
        assert_eq!(assess("20.19.5", Some(&list)), (Some("20.19.5".into()), false, false));
        // Aradaki yayınlar güvenlik içermiyorsa yalnızca güncelleme var.
        assert_eq!(assess("22.11.0", Some(&list[2..])), (Some("22.11.1".into()), true, false));
        // Liste yok (ağ hatası) ya da kurulu sürüm okunamıyor: bilinmiyor, güncelleme yok.
        assert_eq!(assess("22.11.0", None), (None, false, false));
        assert_eq!(assess("", Some(&list)), (None, false, false));
        // Listede olmayan ana sürüm.
        assert_eq!(assess("18.20.0", Some(&list)), (None, false, false));
    }

    #[test]
    fn swap_replaces_and_rolls_back() {
        let root = std::env::temp_dir().join(format!("pixpanel-node-test-{}-{}", std::process::id(), now()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, marker) in [("22", "eski"), ("22.new", "yeni")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("surum"), marker).unwrap();
        }
        let read = |dir: &str| std::fs::read_to_string(root.join(dir).join("surum")).unwrap();

        let previous = swap_in(&root, "22", &root.join("22.new")).unwrap();
        assert_eq!(read("22"), "yeni");
        assert_eq!(read("22.old"), "eski");
        assert!(!root.join("22.new").exists());

        swap_back(&root, "22", &previous).unwrap();
        assert_eq!(read("22"), "eski");
        assert!(!root.join("22.old").exists() && !root.join("22.failed").exists());

        // Kaynak klasör yoksa değiştirme başarısız olur ve kurulu sürüm yerinde kalır.
        assert!(swap_in(&root, "22", &root.join("yok")).is_err());
        assert_eq!(read("22"), "eski");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_lock_rejects_second_update_of_same_major() {
        let first = UpdateLock::acquire("99").unwrap();
        assert!(UpdateLock::acquire("99").is_err());
        assert!(UpdateLock::acquire("98").is_ok());
        drop(first);
        assert!(UpdateLock::acquire("99").is_ok());
    }
}
