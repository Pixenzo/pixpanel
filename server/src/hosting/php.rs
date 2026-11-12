//! PHP-FPM: kurulu sürümler ve site başına havuz yapılandırması.
//!
//! Her site kendi Linux kullanıcısıyla çalışan ayrı bir havuza sahiptir.
//! Havuzun soketine yalnızca nginx (www-data) bağlanabilir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::hosting::resource_limits::{self, Caps};
use crate::hosting::sysuser::SysUser;
use crate::sys::exec;

/// Kabuk komutu çalıştırabilen işlevler; site sahibi açıkça izin vermedikçe kapalıdır.
const EXEC_FUNCTIONS: &str = "exec,system,passthru,shell_exec,proc_open,popen,pcntl_exec";

/// Yalıtımı delebileceği için özel ayar olarak kabul edilmeyen yönergeler.
const FORBIDDEN_DIRECTIVES: &[&str] = &[
    "open_basedir", "disable_functions", "disable_classes", "extension", "zend_extension",
    "extension_dir", "sendmail_path", "error_log", "upload_tmp_dir", "session.save_path",
    "sys_temp_dir", "doc_root", "user_dir", "auto_prepend_file", "auto_append_file",
];

/// Site başına PHP ayarları. `None` olan alanlar o PHP sürümünün genel
/// ayarını (`PhpGlobal`) devralır.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PhpSettings {
    pub memory_limit: Option<String>,
    pub upload_max_filesize: Option<String>,
    pub post_max_size: Option<String>,
    pub max_execution_time: Option<u32>,
    pub max_input_vars: Option<u32>,
    pub display_errors: Option<bool>,
    /// Aynı anda çalışabilecek en fazla PHP süreci.
    pub max_children: u32,
    /// `exec`, `shell_exec` gibi işlevlere izin ver.
    pub allow_exec: bool,
    /// PHP'nin erişebileceği dizinleri site klasörüyle sınırla.
    pub open_basedir: bool,
    /// Ek php.ini yönergeleri.
    pub custom: Vec<Directive>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Directive {
    pub key: String,
    pub value: String,
}

impl Default for PhpSettings {
    fn default() -> Self {
        Self {
            memory_limit: None,
            upload_max_filesize: None,
            post_max_size: None,
            max_execution_time: None,
            max_input_vars: None,
            display_errors: None,
            max_children: 8,
            allow_exec: false,
            open_basedir: true,
            custom: Vec::new(),
        }
    }
}

/// Bir PHP sürümünün bütün sitelere ve komut satırına uygulanan genel ayarları.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PhpGlobal {
    pub memory_limit: String,
    pub upload_max_filesize: String,
    pub post_max_size: String,
    pub max_execution_time: u32,
    pub max_input_vars: u32,
    pub display_errors: bool,
    pub timezone: String,
    pub opcache: bool,
    /// OPcache belleği (MB).
    pub opcache_memory: u32,
    pub custom: Vec<Directive>,
}

impl Default for PhpGlobal {
    fn default() -> Self {
        let timezone = std::fs::read_to_string("/etc/timezone")
            .map(|zone| zone.trim().to_string())
            .ok()
            .filter(|zone| validate_timezone(zone).is_ok())
            .unwrap_or_else(|| "UTC".into());
        Self {
            memory_limit: "256M".into(),
            upload_max_filesize: "64M".into(),
            post_max_size: "64M".into(),
            max_execution_time: 60,
            max_input_vars: 3000,
            display_errors: false,
            timezone,
            opcache: true,
            opcache_memory: 128,
            custom: Vec::new(),
        }
    }
}

fn is_size(value: &str) -> bool {
    let digits = value.trim_end_matches(['K', 'M', 'G']);
    !digits.is_empty()
        && digits.len() <= 6
        && digits.bytes().all(|b| b.is_ascii_digit())
        && value.len() - digits.len() <= 1
}

fn validate_size(label: &str, value: &str) -> AppResult<()> {
    if is_size(value) { Ok(()) } else { Err(AppError::bad(format!("{label} geçersiz: {value} (örnek: 256M)"))) }
}

fn validate_limits(max_execution_time: u32, max_input_vars: u32) -> AppResult<()> {
    if !(1..=86400).contains(&max_execution_time) {
        return Err(AppError::bad("Çalışma süresi 1-86400 saniye arasında olmalı"));
    }
    if !(100..=100_000).contains(&max_input_vars) {
        return Err(AppError::bad("max_input_vars 100-100000 arasında olmalı"));
    }
    Ok(())
}

/// Özel yönergeleri doğrular. `isolated` verilirse site yalıtımını delebilecek
/// yönergeler de reddedilir (site sahipleri için).
fn validate_directives(custom: &[Directive], isolated: bool) -> AppResult<()> {
    if custom.len() > 50 {
        return Err(AppError::bad("En fazla 50 özel yönerge eklenebilir"));
    }
    for directive in custom {
        let key = directive.key.as_str();
        let key_ok = !key.is_empty()
            && key.len() <= 64
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
        if !key_ok {
            return Err(AppError::bad(format!("Geçersiz yönerge adı: {key}")));
        }
        if isolated && FORBIDDEN_DIRECTIVES.contains(&key.to_ascii_lowercase().as_str()) {
            return Err(AppError::bad(format!("{key} yönergesi site yalıtımı nedeniyle değiştirilemez")));
        }
        // Değer yapılandırma dosyasına yazıldığından satır sonu, tırnak ve yorum
        // karakterleri gibi dosyanın yapısını bozabilecek her şey reddedilir.
        let value_ok = directive.value.len() <= 255
            && directive.value.chars().all(|c| c.is_ascii_alphanumeric() || " _.,:/@=+~&|^!()%*-".contains(c));
        if !value_ok {
            return Err(AppError::bad(format!("{key} için geçersiz değer")));
        }
    }
    Ok(())
}

/// Saat dilimi `Europe/Istanbul` biçiminde ve sistemde tanımlı olmalıdır.
pub fn validate_timezone(zone: &str) -> AppResult<()> {
    let shaped = !zone.is_empty()
        && zone.len() <= 64
        && !zone.starts_with(['/', '.'])
        && !zone.contains("..")
        && zone.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-'));
    if shaped && Path::new("/usr/share/zoneinfo").join(zone).is_file() {
        Ok(())
    } else {
        Err(AppError::bad(format!("Geçersiz saat dilimi: {zone} (örnek: Europe/Istanbul)")))
    }
}

impl PhpSettings {
    pub fn validate(&self) -> AppResult<()> {
        for (label, value) in [
            ("Bellek sınırı", &self.memory_limit),
            ("En büyük yükleme boyutu", &self.upload_max_filesize),
            ("En büyük POST boyutu", &self.post_max_size),
        ] {
            if let Some(value) = value {
                validate_size(label, value)?;
            }
        }
        validate_limits(self.max_execution_time.unwrap_or(60), self.max_input_vars.unwrap_or(3000))?;
        if !(1..=500).contains(&self.max_children) {
            return Err(AppError::bad("Süreç sayısı 1-500 arasında olmalı"));
        }
        validate_directives(&self.custom, true)
    }
}

impl PhpGlobal {
    pub fn validate(&self) -> AppResult<()> {
        validate_size("Bellek sınırı", &self.memory_limit)?;
        validate_size("En büyük yükleme boyutu", &self.upload_max_filesize)?;
        validate_size("En büyük POST boyutu", &self.post_max_size)?;
        validate_limits(self.max_execution_time, self.max_input_vars)?;
        validate_timezone(&self.timezone)?;
        if !(8..=4096).contains(&self.opcache_memory) {
            return Err(AppError::bad("OPcache belleği 8-4096 MB arasında olmalı"));
        }
        validate_directives(&self.custom, false)
    }

    /// `conf.d` altına yazılan ini içeriği. Komut satırında bellek ve süre
    /// sınırı uygulanmaz (uzun süren betikler ve composer için).
    fn render_ini(&self, cli: bool) -> String {
        let mut out = String::from("; PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n");
        if !cli {
            out.push_str(&format!("memory_limit = {}\n", self.memory_limit));
            out.push_str(&format!("max_execution_time = {}\n", self.max_execution_time));
            out.push_str(&format!("display_errors = {}\n", if self.display_errors { "On" } else { "Off" }));
        }
        out.push_str(&format!("upload_max_filesize = {}\n", self.upload_max_filesize));
        out.push_str(&format!("post_max_size = {}\n", self.post_max_size));
        out.push_str(&format!("max_input_vars = {}\n", self.max_input_vars));
        out.push_str(&format!("date.timezone = \"{}\"\n", self.timezone));
        if !cli {
            out.push_str(&format!("opcache.enable = {}\n", u8::from(self.opcache)));
            out.push_str(&format!("opcache.memory_consumption = {}\n", self.opcache_memory));
        }
        for directive in &self.custom {
            out.push_str(&format!("{} = \"{}\"\n", directive.key, directive.value));
        }
        out
    }
}

fn global_store_path(version: &str) -> PathBuf {
    PathBuf::from(format!("/etc/pixpanel/php/{version}.json"))
}

fn ini_path(version: &str, sapi: &str) -> PathBuf {
    PathBuf::from(format!("/etc/php/{version}/{sapi}/conf.d/99-pixpanel.ini"))
}

/// Sürümün kayıtlı genel ayarları; hiç kaydedilmediyse varsayılanlar.
pub fn load_global(version: &str) -> PhpGlobal {
    std::fs::read(global_store_path(version))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// Genel ayarları kaydeder ve FPM ile komut satırı için ini dosyalarını yazar.
/// FPM sınamadan geçemezse eski dosyalar geri konur. Yeniden yükleme çağırana
/// aittir (havuzlar da yeniden yazılacağından tek seferde yapılır).
pub async fn save_global(version: &str, settings: &PhpGlobal) -> AppResult<()> {
    validate_version(version)?;
    settings.validate()?;
    let targets = [(ini_path(version, "fpm"), false), (ini_path(version, "cli"), true)];
    let previous: Vec<Option<Vec<u8>>> = targets.iter().map(|(path, _)| std::fs::read(path).ok()).collect();
    for (path, cli) in &targets {
        if path.parent().is_some_and(Path::is_dir) {
            atomic_write(path, settings.render_ini(*cli).as_bytes(), 0o644)?;
        }
    }
    if let Err(detail) = test_config(version).await? {
        for ((path, _), content) in targets.iter().zip(previous) {
            match content {
                Some(content) => atomic_write(path, &content, 0o644)?,
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        return Err(AppError::bad(format!("PHP ayarları geçersiz: {detail}")));
    }
    let encoded = serde_json::to_vec_pretty(settings).map_err(|err| AppError::internal(err.to_string()))?;
    atomic_write(&global_store_path(version), &encoded, 0o600)
}

/// Sürüm dizgesi yalnızca `8.3` biçiminde olabilir (dosya yolunda kullanılır).
pub fn validate_version(version: &str) -> AppResult<()> {
    let valid = version.len() <= 4
        && version.split_once('.').is_some_and(|(major, minor)| {
            !major.is_empty() && !minor.is_empty() && major.bytes().chain(minor.bytes()).all(|b| b.is_ascii_digit())
        });
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz PHP sürümü")) }
}

/// Sunucuda FPM'i kurulu PHP sürümleri (yeniden eskiye). Paket kaldırılınca `/etc/php/<sürüm>`
/// yapılandırması kalır (`remove` onu silmez); bu yüzden FPM ikilisinin varlığına da bakılır.
pub fn installed_versions() -> Vec<String> {
    let mut versions: Vec<String> = std::fs::read_dir("/etc/php")
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().join("fpm/pool.d").is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| validate_version(name).is_ok())
                .filter(|name| std::path::Path::new(&format!("/usr/sbin/php-fpm{name}")).is_file())
                .collect()
        })
        .unwrap_or_default();
    let key = |v: &String| -> (u32, u32) {
        let (major, minor) = v.split_once('.').unwrap_or(("0", "0"));
        (major.parse().unwrap_or(0), minor.parse().unwrap_or(0))
    };
    versions.sort_by_key(|v| std::cmp::Reverse(key(v)));
    versions
}

/// Soket yolu sürüme özgüdür: sürüm değişirken eski ve yeni havuz bir süre
/// yan yana çalışır, nginx yeni sokete geçtikten sonra eskisi kaldırılır.
/// (Ortak yol kullanılsaydı kapanan eski sürüm yeni soketi de silerdi.)
pub fn socket_path(site_id: i64, version: &str) -> String {
    format!("/run/php/pixpanel-{site_id}-php{version}.sock")
}

fn pool_path(version: &str, site_id: i64) -> PathBuf {
    PathBuf::from(format!("/etc/php/{version}/fpm/pool.d/pixpanel-{site_id}.conf"))
}

/// Havuzun gerçek `pm.max_children` değeri: site ayarı, paketin PHP işçisi sınırını aşamaz.
fn effective_max_children(setting: u32, caps: &Caps) -> u32 {
    caps.php_workers.and_then(|workers| u32::try_from(workers).ok()).map_or(setting, |workers| setting.min(workers.max(1)))
}

/// PHP boyut değerini (`256M`, `1G`, `512K`, bayt sayısı) bayta çevirir; anlaşılmazsa `None`.
fn size_bytes(value: &str) -> Option<u64> {
    let value = value.trim();
    let (digits, multiplier) = match value.chars().last()? {
        'K' | 'k' => (&value[..value.len() - 1], 1024),
        'M' | 'm' => (&value[..value.len() - 1], 1024 * 1024),
        'G' | 'g' => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(multiplier)
}

/// Paketin bellek sınırı (MB) varsa ve `memory_limit` onu aşıyorsa sınıra kırpar. Sınır yoksa değere dokunulmaz.
fn clamp_memory_limit(value: &str, cap_mb: Option<i64>) -> String {
    let Some(cap_mb) = cap_mb.and_then(|mb| u64::try_from(mb).ok()) else { return value.to_string() };
    match size_bytes(value) {
        Some(bytes) if bytes <= cap_mb * 1024 * 1024 => value.to_string(),
        // Aşıyor ya da anlaşılamıyor (ör. `-1` sınırsız): sınıra çekilir.
        _ => format!("{cap_mb}M"),
    }
}

fn render_pool(
    site_id: i64,
    domain: &str,
    version: &str,
    user: &SysUser,
    site_root: &Path,
    settings: &PhpSettings,
    global: &PhpGlobal,
    caps: &Caps,
) -> String {
    let tmp = user.tmp_dir();
    let tmp = tmp.display();
    let root = site_root.display();
    let name = &user.name;
    let mut out = format!(
        "; PixPanel tarafından üretildi ({domain}). Elle yapılan değişiklikler kaybolur.\n\
         [pixpanel-{site_id}]\n\
         user = {name}\n\
         group = {name}\n\
         listen = {socket}\n\
         listen.owner = www-data\n\
         listen.group = www-data\n\
         listen.mode = 0660\n\
         pm = ondemand\n\
         pm.max_children = {children}\n\
         pm.process_idle_timeout = 30s\n\
         pm.max_requests = 2000\n\
         chdir = /\n\
         catch_workers_output = yes\n\
         env[TMPDIR] = {tmp}\n\
         env[TMP] = {tmp}\n\
         php_admin_value[upload_tmp_dir] = {tmp}\n\
         php_admin_value[session.save_path] = {tmp}\n\
         php_admin_value[sys_temp_dir] = {tmp}\n\
         php_admin_value[error_log] = {root}/logs/php-error.log\n\
         php_admin_flag[log_errors] = on\n\
         php_admin_value[memory_limit] = {memory}\n\
         php_admin_value[upload_max_filesize] = {upload}\n\
         php_admin_value[post_max_size] = {post}\n\
         php_admin_value[max_input_vars] = {vars}\n\
         php_value[max_execution_time] = {time}\n\
         php_flag[display_errors] = {display}\n",
        socket = socket_path(site_id, version),
        children = effective_max_children(settings.max_children, caps),
        // Devralınan değerler de havuza yazılır: `php_admin_value` olmadan
        // site, genel sınırı `ini_set` ile aşabilirdi. Paketin bellek sınırı varsa
        // tek bir PHP sürecinin bellek sınırı onu aşamaz (aşıyorsa kırpılır).
        memory = clamp_memory_limit(settings.memory_limit.as_ref().unwrap_or(&global.memory_limit), caps.memory_mb),
        upload = settings.upload_max_filesize.as_ref().unwrap_or(&global.upload_max_filesize),
        post = settings.post_max_size.as_ref().unwrap_or(&global.post_max_size),
        vars = settings.max_input_vars.unwrap_or(global.max_input_vars),
        time = settings.max_execution_time.unwrap_or(global.max_execution_time),
        display = if settings.display_errors.unwrap_or(global.display_errors) { "on" } else { "off" },
    );
    if settings.open_basedir {
        out.push_str(&format!("php_admin_value[open_basedir] = {root}:{tmp}:/usr/share/php\n"));
    }
    if !settings.allow_exec {
        out.push_str(&format!("php_admin_value[disable_functions] = {EXEC_FUNCTIONS}\n"));
    }
    for directive in &settings.custom {
        out.push_str(&format!("php_value[{}] = \"{}\"\n", directive.key, directive.value));
    }
    out
}

/// Panelin her PHP sürümüne eklediği genel FPM ayarları.
///
/// `process_control_timeout` olmadan her yeniden yükleme, o sürümdeki bütün
/// sitelerin o an işlenen isteklerini yarıda keser; bu ayarla süren isteklerin
/// bitmesi 10 saniyeye kadar beklenir.
const GLOBAL_CONFIG: &str = concat!(
    "; PixPanel tarafından yönetilir.\n",
    "[global]\n",
    "process_control_timeout = 10s\n",
    "emergency_restart_threshold = 10\n",
    "emergency_restart_interval = 60s\n",
);

/// FPM yapılandırmasının bütününü sınar; hata varsa çıktısını döner.
async fn test_config(version: &str) -> AppResult<Result<(), String>> {
    let test = exec::run(&format!("php-fpm{version}"), &["--test"]).await?;
    Ok(if test.success() { Ok(()) } else { Err(test.stderr.trim().to_string()) })
}

/// Genel ayar dosyasını yazar. Bu dosya o sürümdeki bütün siteleri
/// etkilediğinden, sınamadan geçemezse hemen eski hâline döndürülür.
pub async fn ensure_global_config(version: &str) -> AppResult<()> {
    validate_version(version)?;
    let path = PathBuf::from(format!("/etc/php/{version}/fpm/pool.d/000-pixpanel-global.conf"));
    let previous = std::fs::read(&path).ok();
    if previous.as_deref() == Some(GLOBAL_CONFIG.as_bytes()) {
        return Ok(());
    }
    atomic_write(&path, GLOBAL_CONFIG.as_bytes(), 0o644)?;
    if let Err(detail) = test_config(version).await? {
        match previous {
            // Önceki içerik de bozuk olabilir (yarım kalmış bir yazım); o
            // durumda dosyayı kaldırmak FPM'i çalışır hâlde bırakır.
            Some(content) if content != GLOBAL_CONFIG.as_bytes() => {
                atomic_write(&path, &content, 0o644)?;
                if test_config(version).await?.is_err() {
                    std::fs::remove_file(&path)?;
                }
            }
            _ => std::fs::remove_file(&path)?,
        }
        return Err(AppError::internal(format!("PHP-FPM genel ayarları uygulanamadı: {detail}")));
    }
    Ok(())
}

pub async fn reload(version: &str) -> AppResult<()> {
    let service = format!("php{version}-fpm.service");
    exec::run_ok("systemctl", &["reload-or-restart", "--", &service]).await?;
    Ok(())
}

/// Sitenin havuzunu yazar. Başka sürümlerde kalmış eski havuz, nginx yeni
/// sokete geçtikten sonra `remove_pool_except` ile ayrıca temizlenir.
pub async fn apply_pool(
    site_id: i64,
    domain: &str,
    version: &str,
    user: &SysUser,
    site_root: &Path,
    settings: &PhpSettings,
) -> AppResult<()> {
    validate_version(version)?;
    settings.validate()?;
    if !installed_versions().iter().any(|v| v == version) {
        return Err(AppError::bad(format!("PHP {version} kurulu değil; önce Bileşenler sayfasından kurun")));
    }

    ensure_global_config(version).await?;
    let path = pool_path(version, site_id);
    let previous = std::fs::read(&path).ok();
    let caps = resource_limits::load_caps(&user.name);
    atomic_write(&path, render_pool(site_id, domain, version, user, site_root, settings, &load_global(version), &caps).as_bytes(), 0o644)?;

    // Hatalı bir havuz bütün PHP sitelerini düşürebileceğinden önce sınanır.
    if let Err(detail) = test_config(version).await? {
        match previous {
            Some(content) => atomic_write(&path, &content, 0o644)?,
            None => std::fs::remove_file(&path)?,
        }
        return Err(AppError::bad(format!("PHP ayarları geçersiz: {detail}")));
    }
    reload(version).await?;
    wait_for_socket(&socket_path(site_id, version)).await
}

/// Havuz dosyası güncel sınırlarla yazılacak içerikten farklıysa havuzu yeniden uygular (sınama, geri alma
/// ve yeniden yükleme `apply_pool` içindedir); aynıysa hiçbir şey yapmaz. Kaynak sınırları
/// (paketin PHP işçisi ve bellek sınırı) değişince kullanılır. Kurulu olmayan sürüm sessizce atlanır.
pub async fn sync_pool(site_id: i64, domain: &str, version: &str, user: &SysUser, site_root: &Path, settings: &PhpSettings) -> AppResult<()> {
    validate_version(version)?;
    if !installed_versions().iter().any(|v| v == version) {
        return Ok(());
    }
    let caps = resource_limits::load_caps(&user.name);
    let wanted = render_pool(site_id, domain, version, user, site_root, settings, &load_global(version), &caps);
    if std::fs::read(pool_path(version, site_id)).ok().as_deref() == Some(wanted.as_bytes()) {
        return Ok(());
    }
    apply_pool(site_id, domain, version, user, site_root, settings).await
}

/// `systemctl reload` PHP-FPM'e yalnızca sinyal gönderip döner; yeni havuzun
/// soketi kısa bir süre sonra oluşur. nginx o sokete yönlendirilmeden önce
/// gerçekten bağlantı kabul ettiğinden emin olunur.
async fn wait_for_socket(path: &str) -> AppResult<()> {
    const ATTEMPTS: u32 = 100;
    for _ in 0..ATTEMPTS {
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(AppError::internal(format!("PHP-FPM havuzu başlamadı: {path}")))
}

/// Sitenin havuzunu (verilen sürüm dışındaki) bütün sürümlerden kaldırır.
pub async fn remove_pool_except(site_id: i64, keep: Option<&str>) -> AppResult<()> {
    for version in installed_versions() {
        if Some(version.as_str()) == keep {
            continue;
        }
        let path = pool_path(&version, site_id);
        if path.exists() {
            std::fs::remove_file(&path)?;
            reload(&version).await?;
        }
    }
    Ok(())
}

/// Panelden kurulabilen yaygın eklentiler (paket adı `php<sürüm>-<ad>`).
pub const INSTALLABLE_EXTENSIONS: &[&str] = &[
    "apcu", "bcmath", "bz2", "curl", "gd", "gmp", "igbinary", "imagick", "imap", "intl", "ldap", "mbstring",
    "memcached", "mongodb", "msgpack", "mysql", "opcache", "pgsql", "redis", "soap", "sqlite3", "ssh2",
    "swoole", "xdebug", "xml", "xsl", "yaml", "zip",
];

#[derive(Debug, Serialize)]
pub struct Extension {
    pub name: String,
    pub enabled: bool,
}

pub fn validate_extension_name(name: &str) -> AppResult<()> {
    let valid = !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz eklenti adı")) }
}

/// Sürümde kurulu eklenti modülleri ve FPM için etkin olup olmadıkları.
pub fn extensions(version: &str) -> AppResult<Vec<Extension>> {
    validate_version(version)?;
    let available = PathBuf::from(format!("/etc/php/{version}/mods-available"));
    let enabled_dir = PathBuf::from(format!("/etc/php/{version}/fpm/conf.d"));
    let enabled: Vec<String> = std::fs::read_dir(&enabled_dir)
        .map(|entries| entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    let mut list: Vec<Extension> = std::fs::read_dir(&available)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter_map(|file| file.strip_suffix(".ini").map(str::to_string))
                .filter(|name| validate_extension_name(name).is_ok())
                .map(|name| {
                    // Etkin modüller `20-redis.ini` gibi öncelik önekiyle bağlanır.
                    let suffix = format!("-{name}.ini");
                    Extension { enabled: enabled.iter().any(|file| file.ends_with(&suffix)), name }
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}

/// Eklentiyi bütün çalışma biçimleri (FPM, komut satırı) için açar ya da kapatır.
pub async fn set_extension(version: &str, name: &str, enabled: bool) -> AppResult<()> {
    validate_version(version)?;
    validate_extension_name(name)?;
    if !extensions(version)?.iter().any(|extension| extension.name == name) {
        return Err(AppError::NotFound);
    }
    let tool = if enabled { "phpenmod" } else { "phpdismod" };
    exec::run_ok(tool, &["-v", version, "-s", "ALL", "--", name]).await?;
    if let Err(detail) = test_config(version).await? {
        // Açılan eklenti FPM'i bozduysa geri kapatılır.
        let undo = if enabled { "phpdismod" } else { "phpenmod" };
        exec::run_ok(undo, &["-v", version, "-s", "ALL", "--", name]).await?;
        return Err(AppError::bad(format!("Eklenti uygulanamadı: {detail}")));
    }
    reload(version).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_caps_limit_pool_workers() {
        let none = Caps::default();
        let capped = Caps { php_workers: Some(3), ..Caps::default() };
        assert_eq!(effective_max_children(8, &none), 8);
        assert_eq!(effective_max_children(8, &capped), 3);
        assert_eq!(effective_max_children(2, &capped), 2, "sınır, site ayarını yükseltmez");
    }

    #[test]
    fn memory_limit_is_clamped_to_package() {
        assert_eq!(size_bytes("256M"), Some(256 * 1024 * 1024));
        assert_eq!(size_bytes("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(size_bytes("512K"), Some(512 * 1024));
        assert_eq!(size_bytes("1048576"), Some(1_048_576));
        assert_eq!(size_bytes("-1"), None);
        assert_eq!(size_bytes(""), None);
        // Sınır yoksa dokunulmaz.
        assert_eq!(clamp_memory_limit("2G", None), "2G");
        assert_eq!(clamp_memory_limit("256M", Some(512)), "256M");
        assert_eq!(clamp_memory_limit("1G", Some(512)), "512M");
        assert_eq!(clamp_memory_limit("1G", Some(1024)), "1G");
        assert_eq!(clamp_memory_limit("-1", Some(512)), "512M");
    }
}
