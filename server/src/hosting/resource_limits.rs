//! Pakete bağlı işlemci ve bellek sınırı.
//!
//! Paket (`quota::Limits`) dört kaynak sınırı taşır: `cpu_percent`, `memory_mb`, `processes` ve
//! `php_workers`. Sınırlar hesabın Linux kullanıcısına şöyle uygulanır:
//!
//! - **Hesabın dilimi:** systemd'nin kendi `user-<uid>.slice` dilimine
//!   `/etc/systemd/system/user-<uid>.slice.d/50-pixpanel.conf` drop-in dosyası yazılır
//!   (`CPUQuota=`, `MemoryMax=`, `MemoryHigh=`, `TasksMax=`). SSH/SFTP oturumları ve zamanlı görevler
//!   zaten bu dilimde çalışır. Drop-in değişikliği `systemctl daemon-reload` ile çalışan dilime de
//!   uygulanır (systemd, yeniden yüklemede kaynak denetimi ayarlarını çalışan birimlere yeniden yazar).
//! - **Redis ve uygulama servisleri:** birim dosyalarına `Slice=user-<uid>.slice` eklenir; böylece
//!   hesabın toplam sınırı onları da kapsar. Servisler yeniden başlatılmaz: dilim değişikliği
//!   servisin bir sonraki başlatılışında geçerli olur. Node.js/Python uygulamaları ancak
//!   port koruması (`portguard`) çekirdekte işlemiyorsa dilime alınır (aşağıya bakın).
//! - **PHP-FPM:** havuzlar ortak servis cgroup'unda olduğundan dilim uygulanamaz; bunun yerine
//!   havuzdaki `pm.max_children` paketin `php_workers` değerini, `memory_limit` ise paketin
//!   `memory_mb` değerini aşamaz (`php::render_pool`).
//! - **Panelin hesap adına çalıştırdığı ağır komutlar:** `sys::runas::heavy_command` (wp-cli,
//!   composer/pip/npm, Git yayını, rsync/cp, WordPress yedek-geri dönüş) ve dosya yöneticisinin ağır
//!   işlemleri (kopyalama, sıkıştırma, arşiv açma; `api/files.rs`) `systemd-run --scope
//!   --slice=user-<uid>.slice` ile hesabın diliminde koşar. Dilimde uygulanacak sınır yoksa
//!   (drop-in dosyası yok) ya da `systemd-run` kullanılamıyorsa (konteynerde D-Bus yok vb.; açılışta bir
//!   kez sınanır, `sys::runas::probe_slice_support`) komut eskisi gibi panelin cgroup'unda çalışır.
//!   Kısa, sık çalışan salt okuma komutları (`runas::command`) sarılmaz.
//! - **Redis bellek sınırı:** paketteki `redis_memory_mb` hesabın Redis örneğinin `maxmemory` değeridir
//!   (`sync_redis`): paket atanınca/değişince örnek yeni sınırla yeniden yazılır; 0 ise Redis pakette yok
//!   sayılır (örnek durdurulur, yeniden açılamaz; veri silinmez).
//!
//! **Port korumasıyla çakışma:** `portguard`, uygulama port aralığını `user.slice` altında
//! yasaklar; cgroup-BPF kuralları üst dilimlerden de uygulanır ve bir üst dilimdeki yasağı alt
//! birimin `SocketBindAllow` satırı geçersiz kılamaz. Bu yüzden uygulama servisi `user-<uid>.slice`
//! içine alınırsa kendi portunu açamazdı. Port koruması çalışan sunucularda uygulama servisleri
//! dilime alınmaz (kendi `MemoryMax` sınırı sürer), çalışmayan sunucularda (ör. yetkisiz LXC) alınır.
//!
//! Durum, panel açılışında ve her paket/hesap değişikliğinde yeniden eşitlenir (`sync`): olması
//! gereken dosyalar yazılır, fazlalar silinir. Hesap için istenen sınırlar ayrıca
//! `/etc/pixpanel/limits/<hesap>.json` dosyasında tutulur; Linux kullanıcısı henüz yokken (ilk site
//! açılmadan) paket atanabilsin ve PHP havuzu veritabanına ihtiyaç duymadan sınırları okuyabilsin diye.
//!
//! cgroup v2 ve `cpu`/`memory` denetleyicilerinin kullanıcı dilimlerine devredilmiş olması gerekir
//! (yetkisiz LXC'de olmayabilir). Yoksa sınırlar yine yazılır ama "uygulanamıyor" uyarısı gösterilir.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::AppResult;
use crate::hosting::sysuser::{self, SysUser};
use crate::hosting::{atomic_write, node, php, redis_account, sites};
use crate::quota::{self, Limits};
use crate::state::AppState;
use crate::sys::exec;

const STATE_DIR: &str = "/etc/pixpanel/limits";
const SYSTEMD_DIR: &str = "/etc/systemd/system";
const DROPIN_NAME: &str = "50-pixpanel.conf";
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Bir hesabın kaynak sınırları; `None` sınırsız demektir.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caps {
    pub cpu_percent: Option<i64>,
    pub memory_mb: Option<i64>,
    pub php_workers: Option<i64>,
    pub processes: Option<i64>,
}

impl Caps {
    pub fn from_limits(limits: &Limits) -> Self {
        Self { cpu_percent: limits.cpu_percent, memory_mb: limits.memory_mb, php_workers: limits.php_workers, processes: limits.processes }
    }

    /// Hesabın dilimine yazılacak bir sınır var mı?
    pub fn needs_slice(&self) -> bool {
        self.cpu_percent.is_some() || self.memory_mb.is_some() || self.processes.is_some()
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Dilimin drop-in dosyası; sınır yoksa `None` (dosya olmamalı).
    pub fn render_dropin(&self) -> Option<String> {
        if !self.needs_slice() {
            return None;
        }
        let mut out = String::from("# PixPanel tarafından üretildi (hizmet paketi sınırları). Elle yapılan değişiklikler kaybolur.\n[Slice]\n");
        if let Some(cpu) = self.cpu_percent {
            out.push_str(&format!("CPUQuota={cpu}%\n"));
        }
        if let Some(memory) = self.memory_mb {
            // `MemoryHigh` yumuşak sınırdır: aşılınca süreçler yavaşlatılır, `MemoryMax` aşılınca öldürülür.
            out.push_str(&format!("MemoryMax={memory}M\nMemoryHigh={}M\n", memory * 90 / 100));
        }
        if let Some(tasks) = self.processes {
            out.push_str(&format!("TasksMax={tasks}\n"));
        }
        Some(out)
    }
}

/// Hesabın diliminde uygulanacak bir sınır (drop-in dosyası) var mı? `runas` ağır komutları yalnızca
/// o zaman dilime alır.
pub fn has_slice_limits(uid: u32) -> bool {
    dropin_path(uid).exists()
}

pub fn slice_name(uid: u32) -> String {
    format!("user-{uid}.slice")
}

fn dropin_path(uid: u32) -> PathBuf {
    Path::new(SYSTEMD_DIR).join(format!("{}.d", slice_name(uid))).join(DROPIN_NAME)
}

fn caps_path(username: &str) -> PathBuf {
    Path::new(STATE_DIR).join(format!("{username}.json"))
}

/// Hesap için kayıtlı sınırlar; kayıt yoksa ya da ad geçersizse sınırsız. PHP havuzu yazılırken kullanılır.
pub fn load_caps(username: &str) -> Caps {
    if sysuser::validate_name(username).is_err() {
        return Caps::default();
    }
    std::fs::read(caps_path(username)).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()
}

/// Redis birimine eklenecek satır: hesabın diliminde sınır varsa Redis de o dilimde çalışır.
pub fn redis_slice_line(uid: u32) -> String {
    if dropin_path(uid).exists() { format!("Slice={}\n", slice_name(uid)) } else { String::new() }
}

/// Uygulama (Node.js/Python) birimine eklenecek satır. Port koruması çekirdekte çalışıyorsa boş döner
/// (modül açıklamasındaki çakışma); sınır yoksa da boştur.
pub fn app_slice_line(uid: u32) -> String {
    if crate::hosting::portguard::supported() == Some(true) {
        return String::new();
    }
    redis_slice_line(uid)
}

// ---------------------------------------------------------------------------------------------
// Sunucu desteği

/// Sınırların bu sunucuda uygulanıp uygulanamayacağı; uygulanamıyorsa kullanıcıya gösterilecek neden.
pub fn enforcement_problem() -> Option<String> {
    let root = Path::new(CGROUP_ROOT);
    if !root.join("cgroup.controllers").is_file() {
        return Some("Bu sunucuda cgroup v2 yok; sınırlar kaydedildi ama uygulanamıyor.".into());
    }
    // Kullanıcı dilimlerinin üst dilimine devredilen denetleyiciler; dilim henüz yoksa köke bakılır.
    let user_slice = root.join("user.slice/cgroup.controllers");
    let available = if user_slice.is_file() { std::fs::read_to_string(&user_slice) } else { std::fs::read_to_string(root.join("cgroup.subtree_control")) }.unwrap_or_default();
    let has = |name: &str| available.split_whitespace().any(|controller| controller == name);
    // İleti sabit metinlerdir (arayüz çevirisi için); değişken birleştirilmez.
    match (has("cpu"), has("memory")) {
        (true, true) => None,
        (false, true) => Some("Bu sunucuda işlemci denetleyicisi kullanıcı dilimlerine devredilmemiş (yetkisiz konteynerde olabilir); sınırlar kaydedildi ama uygulanamıyor.".into()),
        (true, false) => Some("Bu sunucuda bellek denetleyicisi kullanıcı dilimlerine devredilmemiş (yetkisiz konteynerde olabilir); sınırlar kaydedildi ama uygulanamıyor.".into()),
        (false, false) => Some("Bu sunucuda işlemci ve bellek denetleyicileri kullanıcı dilimlerine devredilmemiş (yetkisiz konteynerde olabilir); sınırlar kaydedildi ama uygulanamıyor.".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// Dosya eşitleme

/// Dosyayı olması gereken içeriğe getirir: içerik verilmişse yazar (aynıysa dokunmaz), `None` ise siler.
/// İçerik değiştiyse `true` döner.
fn sync_file(path: &Path, wanted: Option<&str>, mode: u32) -> AppResult<bool> {
    let current = std::fs::read_to_string(path).ok();
    match (current.as_deref(), wanted) {
        (Some(have), Some(want)) if have == want => Ok(false),
        (None, None) => Ok(false),
        (_, Some(want)) => {
            atomic_write(path, want.as_bytes(), mode)?;
            Ok(true)
        }
        (Some(_), None) => {
            std::fs::remove_file(path)?;
            // Drop-in klasörü boş kalmışsa o da gider (doluysa `remove_dir` başarısız olur, sorun değil).
            if let Some(parent) = path.parent().filter(|parent| parent.extension().is_some_and(|ext| ext == "d")) {
                let _ = std::fs::remove_dir(parent);
            }
            Ok(true)
        }
    }
}

async fn daemon_reload() {
    if let Err(err) = exec::run_ok("systemctl", &["daemon-reload"]).await {
        tracing::warn!(error = %err, "systemd yeniden yüklenemedi (kaynak sınırları)");
    }
}

/// Hesabın istenen sınırlarını diske yazar: kayıt dosyası her zaman, dilimin drop-in dosyası Linux
/// kullanıcısı varsa. Systemd dosyası değiştiyse `true` döner (yeniden yükleme çağırana aittir).
fn write_account_files(username: &str, caps: &Caps) -> AppResult<bool> {
    sysuser::validate_name(username)?;
    let state = if caps.is_empty() { None } else { Some(serde_json::to_string(caps).map_err(|err| crate::error::AppError::internal(err.to_string()))?) };
    sync_file(&caps_path(username), state.as_deref(), 0o600)?;
    match sysuser::lookup(username) {
        Some(user) => sync_file(&dropin_path(user.uid), caps.render_dropin().as_deref(), 0o644),
        None => Ok(false),
    }
}

/// Linux kullanıcısı yeni oluşturulduğunda çağrılır: hesaba atanmış sınırlar varsa dilimine yazılır.
pub async fn apply_stored(user: &SysUser) {
    let caps = load_caps(&user.name);
    if !caps.needs_slice() {
        return;
    }
    match sync_file(&dropin_path(user.uid), caps.render_dropin().as_deref(), 0o644) {
        Ok(true) => daemon_reload().await,
        Ok(false) => {}
        Err(err) => tracing::warn!(account = %user.name, error = %err, "hesabın kaynak sınırları yazılamadı"),
    }
}

/// Hesap silinirken çağrılır (Linux kullanıcısı silinmeden önce): kayıt ve drop-in dosyası kalkar.
pub async fn forget(username: &str) {
    if sysuser::validate_name(username).is_err() {
        return;
    }
    let _ = sync_file(&caps_path(username), None, 0o600);
    if let Some(user) = sysuser::lookup(username) {
        if matches!(sync_file(&dropin_path(user.uid), None, 0o644), Ok(true)) {
            daemon_reload().await;
        }
    }
}

/// Hesaba bağlı servislerin birim dosyalarını yeniden yazar (dilim satırı eklenir ya da kalkar).
/// Servisler yeniden başlatılmaz. Değişen dosya olduysa `true` döner.
async fn rewrite_service_units(db: &Db, user_id: i64, user: &SysUser) -> AppResult<bool> {
    let mut changed = false;
    for site in sites::list_by_user(db, user_id)? {
        if node::is_app(&site.kind) {
            changed |= node::rewrite_unit(site.id, &site.domain, user, &site.root(), &site.node, &site.kind)?;
        }
    }
    if let Some(instance) = redis_account::get(db, user_id)? {
        changed |= redis_account::rewrite_unit(&instance)?;
    }
    Ok(changed)
}

/// PHP havuzlarını güncel sınırlarla yeniden yazar (içerik değişmediyse dokunmaz).
async fn refresh_pools(db: &Db, user_id: i64, user: &SysUser) {
    let sites = match sites::list_by_user(db, user_id) {
        Ok(sites) => sites,
        Err(err) => {
            tracing::warn!(account = %user.name, error = %err, "siteler okunamadı (PHP havuz sınırları)");
            return;
        }
    };
    for site in sites.into_iter().filter(|site| site.kind == "php") {
        let Some(version) = site.php_version.as_deref() else { continue };
        if let Err(err) = php::sync_pool(site.id, &site.domain, version, user, &site.root(), &site.php_settings).await {
            tracing::warn!(domain = %site.domain, error = %err, "PHP havuzu yeni sınırlarla yazılamadı");
        }
    }
}

/// Hesapların sınırlarını sisteme eşitler. `only`: yalnız bu hesaplar; `None` ise hepsi ve ayrıca
/// artık gerekmeyen dosyalar (fazlalar) silinir. Panel açılışında `None` ile, paket ve hesap
/// değişikliklerinde ilgili hesaplarla çağrılır. Hatalar günlüğe yazılır; panelin işini bozmaz.
pub async fn sync(state: &AppState, only: Option<&[i64]>) {
    let _guard = state.sys_lock.lock().await;
    if let Err(err) = sync_locked(&state.db, only).await {
        tracing::warn!(error = %err, "kaynak sınırları eşitlenemedi");
    }
}

/// Bir paketi kullanan bütün hesapların sınırlarını eşitler.
pub async fn sync_package(state: &AppState, package_id: i64) {
    let ids: AppResult<Vec<i64>> = state.db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE package_id = ?1")?;
        let rows = stmt.query_map(params![package_id], |row| row.get(0))?;
        rows.collect()
    });
    match ids {
        Ok(ids) if !ids.is_empty() => sync(state, Some(&ids)).await,
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "paketi kullanan hesaplar okunamadı"),
    }
}

async fn sync_locked(db: &Db, only: Option<&[i64]>) -> AppResult<()> {
    // Linux kullanıcısı olabilen hesaplar: alt hesaplar üst hesabın kullanıcısını paylaşır, yöneticinin sınırı yoktur.
    let accounts: Vec<(i64, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, username FROM users WHERE role IN ('user', 'reseller') ORDER BY id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })?;
    let mut reload = false;
    let mut wanted_state: HashSet<String> = HashSet::new();
    let mut wanted_dropins: HashSet<u32> = HashSet::new();
    let mut touched: Vec<(i64, String)> = Vec::new();
    for (id, username) in accounts {
        if sysuser::validate_name(&username).is_err() {
            continue;
        }
        let caps = quota::limits_for(db, id)?.map(|(_, limits)| Caps::from_limits(&limits)).unwrap_or_default();
        if !caps.is_empty() {
            wanted_state.insert(username.clone());
        }
        if caps.needs_slice() {
            if let Some(user) = sysuser::lookup(&username) {
                wanted_dropins.insert(user.uid);
            }
        }
        if only.is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        match write_account_files(&username, &caps) {
            Ok(changed) => reload |= changed,
            Err(err) => tracing::warn!(account = %username, error = %err, "hesabın kaynak sınırları yazılamadı"),
        }
        touched.push((id, username));
    }
    if only.is_none() {
        reload |= remove_stale(&wanted_state, &wanted_dropins);
    }
    if reload {
        daemon_reload().await;
    }
    // Birimler ve havuzlar, dilim dosyaları yerleştikten sonra güncellenir (dilim satırı drop-in'in varlığına bakar).
    let mut units_changed = false;
    for (id, username) in &touched {
        let Some(user) = sysuser::lookup(username) else { continue };
        match rewrite_service_units(db, *id, &user).await {
            Ok(changed) => units_changed |= changed,
            Err(err) => tracing::warn!(account = %username, error = %err, "servis birimleri güncellenemedi (kaynak sınırları)"),
        }
        refresh_pools(db, *id, &user).await;
        // Paketteki Redis bellek sınırı (ya da "Redis yok") hesabın örneğine uygulanır.
        if let Err(err) = redis_account::sync_package_limit(db, *id).await {
            tracing::warn!(account = %username, error = %err, "hesabın Redis sınırı paketle eşitlenemedi");
        }
    }
    if units_changed {
        daemon_reload().await;
    }
    Ok(())
}

/// Artık gerekmeyen kayıt ve drop-in dosyalarını siler; systemd dosyası silindiyse `true` döner.
fn remove_stale(wanted_state: &HashSet<String>, wanted_dropins: &HashSet<u32>) -> bool {
    if let Ok(entries) = std::fs::read_dir(STATE_DIR) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(account) = name.strip_suffix(".json") {
                if !wanted_state.contains(account) {
                    let _ = std::fs::remove_file(entry.path());
                }
            } else {
                // Yarım kalmış geçici dosya.
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let mut removed = false;
    if let Ok(entries) = std::fs::read_dir(SYSTEMD_DIR) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let uid = name.strip_prefix("user-").and_then(|rest| rest.strip_suffix(".slice.d")).and_then(|uid| uid.parse::<u32>().ok());
            let Some(uid) = uid else { continue };
            if !wanted_dropins.contains(&uid) && matches!(sync_file(&dropin_path(uid), None, 0o644), Ok(true)) {
                removed = true;
            }
        }
    }
    removed
}

// ---------------------------------------------------------------------------------------------
// Görünürlük

/// Hesabın sınırlarının durumu ve anlık kullanımı (Hesabım → kullanım görünümü).
#[derive(Debug, Serialize)]
pub struct Runtime {
    /// Sınırlar bu sunucuda gerçekten uygulanıyor mu?
    pub enforced: bool,
    /// Uygulanamıyorsa nedeni (kullanıcıya gösterilir).
    pub warning: Option<String>,
    /// Hesabın diliminin şu anki bellek kullanımı (bayt); dilim etkin değilse `None`.
    pub memory_bytes: Option<u64>,
    /// Dilimin toplam işlemci süresi (saniye).
    pub cpu_seconds: Option<f64>,
    /// Önceki ölçümden bu yana işlemci kullanımı (bir çekirdeğin yüzdesi); ilk ölçümde `None`.
    pub cpu_percent: Option<f64>,
    /// Dilimdeki süreç/iş parçacığı sayısı.
    pub tasks: Option<u64>,
}

/// systemd'nin "ayarlı değil" için döndüğü değer (u64 üst sınırı).
const UNSET: u64 = u64::MAX;

static CPU_SAMPLES: Mutex<Option<HashMap<u32, (Instant, u64)>>> = Mutex::new(None);

/// `systemctl show` çıktısından bir sayıyı okur; "[not set]" ve u64 üst sınırı `None` sayılır.
fn parse_property(output: &str, name: &str) -> Option<u64> {
    output.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('=')).and_then(|value| value.trim().parse::<u64>().ok()).filter(|value| *value != UNSET)
}

/// İki ölçüm arasındaki işlemci kullanımı (bir çekirdeğin yüzdesi).
fn cpu_percent_between(previous_nsec: u64, current_nsec: u64, elapsed_secs: f64) -> Option<f64> {
    if elapsed_secs <= 0.0 || current_nsec < previous_nsec {
        return None;
    }
    Some((current_nsec - previous_nsec) as f64 / 1e9 / elapsed_secs * 100.0)
}

/// Hesabın kaynak sınırı durumu; hesabın hiç kaynak sınırı yoksa `None`.
pub async fn runtime(db: &Db, account_id: i64) -> Option<Runtime> {
    let (_, limits) = quota::limits_for(db, account_id).ok().flatten()?;
    let caps = Caps::from_limits(&limits);
    if !caps.needs_slice() && caps.php_workers.is_none() {
        return None;
    }
    let username: String = db.call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![account_id], |row| row.get(0))).ok()?;
    let warning = enforcement_problem();
    let mut runtime = Runtime { enforced: warning.is_none(), warning, memory_bytes: None, cpu_seconds: None, cpu_percent: None, tasks: None };
    if !caps.needs_slice() {
        return Some(runtime);
    }
    let Some(user) = sysuser::validate_name(&username).ok().and_then(|()| sysuser::lookup(&username)) else { return Some(runtime) };
    let slice = slice_name(user.uid);
    let output = exec::run("systemctl", &["show", "-p", "MemoryCurrent", "-p", "CPUUsageNSec", "-p", "TasksCurrent", "--", &slice]).await.ok().filter(|out| out.success());
    if let Some(output) = output {
        runtime.memory_bytes = parse_property(&output.stdout, "MemoryCurrent");
        runtime.tasks = parse_property(&output.stdout, "TasksCurrent");
        if let Some(nsec) = parse_property(&output.stdout, "CPUUsageNSec") {
            runtime.cpu_seconds = Some(nsec as f64 / 1e9);
            let mut samples = CPU_SAMPLES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let samples = samples.get_or_insert_with(HashMap::new);
            let now = Instant::now();
            if let Some((then, previous)) = samples.get(&user.uid).copied() {
                let elapsed = now.duration_since(then).as_secs_f64();
                // Çok yakın iki isteğin ölçümü anlamsız olur; ilk örnek korunur.
                if elapsed >= 2.0 {
                    runtime.cpu_percent = cpu_percent_between(previous, nsec, elapsed);
                    samples.insert(user.uid, (now, nsec));
                }
            } else {
                samples.insert(user.uid, (now, nsec));
            }
        }
    }
    Some(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropin_contains_only_set_limits() {
        assert_eq!(Caps::default().render_dropin(), None);
        assert_eq!(Caps { php_workers: Some(4), ..Caps::default() }.render_dropin(), None, "PHP işçisi dilime yazılmaz");
        let full = Caps { cpu_percent: Some(150), memory_mb: Some(512), php_workers: Some(4), processes: Some(200) }.render_dropin().unwrap();
        assert!(full.contains("[Slice]\n"));
        assert!(full.contains("CPUQuota=150%\n"));
        assert!(full.contains("MemoryMax=512M\nMemoryHigh=460M\n"));
        assert!(full.contains("TasksMax=200\n"));
        let cpu_only = Caps { cpu_percent: Some(50), ..Caps::default() }.render_dropin().unwrap();
        assert!(!cpu_only.contains("Memory") && !cpu_only.contains("Tasks"));
    }

    #[test]
    fn slice_names_follow_systemd_convention() {
        assert_eq!(slice_name(1001), "user-1001.slice");
        assert_eq!(dropin_path(1001), PathBuf::from("/etc/systemd/system/user-1001.slice.d/50-pixpanel.conf"));
        assert_eq!(caps_path("musteri"), PathBuf::from("/etc/pixpanel/limits/musteri.json"));
    }

    #[test]
    fn unset_properties_are_ignored() {
        let output = "MemoryCurrent=12345\nCPUUsageNSec=[not set]\nTasksCurrent=18446744073709551615\n";
        assert_eq!(parse_property(output, "MemoryCurrent"), Some(12345));
        assert_eq!(parse_property(output, "CPUUsageNSec"), None);
        assert_eq!(parse_property(output, "TasksCurrent"), None);
        assert_eq!(parse_property(output, "Nope"), None);
    }

    #[test]
    fn cpu_percent_is_relative_to_one_core() {
        // 5 saniyede 2,5 saniye işlemci = bir çekirdeğin yarısı.
        assert_eq!(cpu_percent_between(0, 2_500_000_000, 5.0), Some(50.0));
        assert_eq!(cpu_percent_between(10, 5, 5.0), None);
        assert_eq!(cpu_percent_between(0, 5, 0.0), None);
    }

    #[test]
    fn caps_roundtrip_and_emptiness() {
        let caps = Caps { cpu_percent: Some(100), ..Caps::default() };
        let back: Caps = serde_json::from_str(&serde_json::to_string(&caps).unwrap()).unwrap();
        assert_eq!(caps, back);
        assert!(Caps::default().is_empty());
        assert!(!caps.is_empty());
    }
}
