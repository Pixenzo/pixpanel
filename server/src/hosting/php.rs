//! PHP-FPM: kurulu sürümler ve site başına havuz yapılandırması.
//!
//! Her site kendi Linux kullanıcısıyla çalışan ayrı bir havuza sahiptir.
//! Havuzun soketine yalnızca nginx (www-data) bağlanabilir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PhpSettings {
    pub memory_limit: String,
    pub upload_max_filesize: String,
    pub post_max_size: String,
    pub max_execution_time: u32,
    pub max_input_vars: u32,
    pub display_errors: bool,
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
            memory_limit: "256M".into(),
            upload_max_filesize: "64M".into(),
            post_max_size: "64M".into(),
            max_execution_time: 60,
            max_input_vars: 3000,
            display_errors: false,
            max_children: 8,
            allow_exec: false,
            open_basedir: true,
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

impl PhpSettings {
    pub fn validate(&self) -> AppResult<()> {
        for (label, value) in [
            ("Bellek sınırı", &self.memory_limit),
            ("En büyük yükleme boyutu", &self.upload_max_filesize),
            ("En büyük POST boyutu", &self.post_max_size),
        ] {
            if !is_size(value) {
                return Err(AppError::bad(format!("{label} geçersiz: {value} (örnek: 256M)")));
            }
        }
        if !(1..=86400).contains(&self.max_execution_time) {
            return Err(AppError::bad("Çalışma süresi 1-86400 saniye arasında olmalı"));
        }
        if !(100..=100_000).contains(&self.max_input_vars) {
            return Err(AppError::bad("max_input_vars 100-100000 arasında olmalı"));
        }
        if !(1..=500).contains(&self.max_children) {
            return Err(AppError::bad("Süreç sayısı 1-500 arasında olmalı"));
        }
        if self.custom.len() > 50 {
            return Err(AppError::bad("En fazla 50 özel yönerge eklenebilir"));
        }
        for directive in &self.custom {
            let key = directive.key.as_str();
            let key_ok = !key.is_empty()
                && key.len() <= 64
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
            if !key_ok {
                return Err(AppError::bad(format!("Geçersiz yönerge adı: {key}")));
            }
            if FORBIDDEN_DIRECTIVES.contains(&key.to_ascii_lowercase().as_str()) {
                return Err(AppError::bad(format!("{key} yönergesi site yalıtımı nedeniyle değiştirilemez")));
            }
            // Değer havuz dosyasına yazıldığından satır sonu, tırnak ve yorum
            // karakterleri gibi dosyanın yapısını bozabilecek her şey reddedilir.
            let value_ok = directive.value.len() <= 255
                && directive.value.chars().all(|c| c.is_ascii_alphanumeric() || " _.,:/@=+~&|^!()%*-".contains(c));
            if !value_ok {
                return Err(AppError::bad(format!("{key} için geçersiz değer")));
            }
        }
        Ok(())
    }
}

/// Sürüm dizgesi yalnızca `8.3` biçiminde olabilir (dosya yolunda kullanılır).
pub fn validate_version(version: &str) -> AppResult<()> {
    let valid = version.len() <= 4
        && version.split_once('.').is_some_and(|(major, minor)| {
            !major.is_empty() && !minor.is_empty() && major.bytes().chain(minor.bytes()).all(|b| b.is_ascii_digit())
        });
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz PHP sürümü")) }
}

/// Sunucuda FPM'i kurulu PHP sürümleri (yeniden eskiye).
pub fn installed_versions() -> Vec<String> {
    let mut versions: Vec<String> = std::fs::read_dir("/etc/php")
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().join("fpm/pool.d").is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| validate_version(name).is_ok())
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
    format!("/run/php/capsule-{site_id}-php{version}.sock")
}

fn pool_path(version: &str, site_id: i64) -> PathBuf {
    PathBuf::from(format!("/etc/php/{version}/fpm/pool.d/capsule-{site_id}.conf"))
}

fn render_pool(
    site_id: i64,
    domain: &str,
    version: &str,
    user: &SysUser,
    site_root: &Path,
    settings: &PhpSettings,
) -> String {
    let tmp = user.tmp_dir();
    let tmp = tmp.display();
    let root = site_root.display();
    let name = &user.name;
    let mut out = format!(
        "; Capsule tarafından üretildi ({domain}). Elle yapılan değişiklikler kaybolur.\n\
         [capsule-{site_id}]\n\
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
        children = settings.max_children,
        memory = settings.memory_limit,
        upload = settings.upload_max_filesize,
        post = settings.post_max_size,
        vars = settings.max_input_vars,
        time = settings.max_execution_time,
        display = if settings.display_errors { "on" } else { "off" },
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
    "; Capsule tarafından yönetilir.\n",
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
    let path = PathBuf::from(format!("/etc/php/{version}/fpm/pool.d/000-capsule-global.conf"));
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

async fn reload(version: &str) -> AppResult<()> {
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
    atomic_write(&path, render_pool(site_id, domain, version, user, site_root, settings).as_bytes(), 0o644)?;

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
