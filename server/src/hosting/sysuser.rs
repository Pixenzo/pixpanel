//! Hosting hesaplarının Linux kullanıcıları.
//!
//! Her panel hesabının siteleri kendi Linux kullanıcısıyla çalışır; siteler
//! arası yalıtımı çekirdek (dosya izinleri ve uid) sağlar.
//!
//! Dizin düzeni:
//! ```text
//! /home/<kullanıcı>            root:kullanıcı       0751   üst düzey kullanıcı tarafından değiştirilemez
//! /home/<kullanıcı>/tmp        kullanıcı:kullanıcı  0700   PHP geçici dosyaları ve oturumlar
//! /home/<kullanıcı>/.ssh ...   kullanıcı:kullanıcı  0700   araçların ayar ve önbellek klasörleri
//! /home/<kullanıcı>/sites      root:kullanıcı       0751   kullanıcı listeler ama site klasörü ekleyip çıkaramaz
//! /home/<kullanıcı>/sites/<alan adı>  kullanıcı:www-data  2750
//! ```
//!
//! Ev klasörünün üst düzeyi bilerek root'a aittir. Kullanıcıya ait olsaydı
//! `sites` klasörünü yeniden adlandırıp yerine başka bir yeri gösteren
//! sembolik bağ koyabilir, panel de root yetkisiyle o bağın ardında klasör
//! oluşturup sahiplik verirdi. Aynı düzen SFTP hapsinin (chroot) de şartıdır.

use std::os::unix::fs::{lchown, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::sys::exec;

/// Panelin oluşturduğu kullanıcıların üye olduğu grup.
pub const MANAGED_GROUP: &str = "pixpanel-users";
pub const WEB_GROUP: &str = "www-data";

#[derive(Debug, Clone)]
pub struct SysUser {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

impl SysUser {
    pub fn tmp_dir(&self) -> PathBuf {
        self.home.join("tmp")
    }

    pub fn sites_dir(&self) -> PathBuf {
        self.home.join("sites")
    }
}

/// Linux kullanıcı adı kuralı; panel kullanıcı adı kuralıyla aynıdır.
pub fn validate_name(name: &str) -> AppResult<()> {
    let mut chars = name.chars();
    let valid = (3..=32).contains(&name.len())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz sistem kullanıcı adı")) }
}

pub fn lookup(name: &str) -> Option<SysUser> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 6 || fields[0] != name {
            return None;
        }
        Some(SysUser {
            name: name.to_string(),
            uid: fields[2].parse().ok()?,
            gid: fields[3].parse().ok()?,
            home: PathBuf::from(fields[5]),
        })
    })
}

pub fn group_id(name: &str) -> AppResult<u32> {
    std::fs::read_to_string("/etc/group")?
        .lines()
        .find_map(|line| {
            let mut fields = line.split(':');
            (fields.next()? == name).then(|| fields.nth(1)?.parse().ok()).flatten()
        })
        .ok_or_else(|| AppError::internal(format!("{name} grubu bulunamadı")))
}

fn group_members(name: &str) -> Vec<String> {
    std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            (fields.len() >= 4 && fields[0] == name)
                .then(|| fields[3].split(',').filter(|m| !m.is_empty()).map(str::to_string).collect())
        })
        .unwrap_or_default()
}

fn set_mode(path: &Path, mode: u32) -> AppResult<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Kullanıcı yoksa oluşturur, varsa panelin yönettiği bir kullanıcı olduğunu doğrular.
pub async fn ensure(name: &str) -> AppResult<SysUser> {
    validate_name(name)?;
    if let Some(user) = lookup(name) {
        // Sistemde önceden var olan (root, www-data, postgres...) bir hesabın
        // panel tarafından ele geçirilmesine izin verilmez.
        if !group_members(MANAGED_GROUP).iter().any(|member| member == name) {
            return Err(AppError::conflict(format!(
                "“{name}” adında panelin yönetmediği bir sistem kullanıcısı zaten var; başka bir kullanıcı adı seçin"
            )));
        }
        harden_home(&user)?;
        return Ok(user);
    }

    exec::run_ok("groupadd", &["--force", "--system", MANAGED_GROUP]).await?;
    let home = format!("/home/{name}");
    exec::run_ok(
        "useradd",
        &[
            "--create-home",
            "--home-dir", &home,
            "--shell", "/usr/sbin/nologin",
            "--user-group",
            "--groups", MANAGED_GROUP,
            "--", name,
        ],
    )
    .await?;
    let user = lookup(name).ok_or_else(|| AppError::internal("kullanıcı oluşturuldu ama okunamadı"))?;

    harden_home(&user)?;
    // Hesaba paket (işlemci/bellek sınırı) zaten atanmışsa sınırlar yeni kullanıcının dilimine yazılır.
    crate::hosting::resource_limits::apply_stored(&user).await;
    Ok(user)
}

/// Panelin yönettiği Linux hesabını kilitler ya da açar. Kilitli (süresi dolmuş) hesapla SSH ve
/// SFTP oturumu açılamaz, zamanlı görevler çalışmaz. Sitelerin servisleri bundan etkilenmez.
pub async fn set_locked(name: &str, locked: bool) -> AppResult<()> {
    validate_name(name)?;
    if lookup(name).is_none() || !group_members(MANAGED_GROUP).iter().any(|member| member == name) {
        return Ok(());
    }
    // Hesabın geçerlilik tarihi: 1 = 2 Ocak 1970 (süresi dolmuş), boş = süresiz.
    exec::run_ok("usermod", &["--expiredate", if locked { "1" } else { "" }, "--", name]).await?;
    Ok(())
}

/// Silinen hesabın Linux kullanıcısını kaldırır. Ev klasöründe saklanmış site dosyaları varsa
/// (site "dosyaları kalsın" denerek silinmişse) kullanıcı silinmez, yalnızca kilitlenir; dosyalar
/// yöneticinin kararına kalır. Kaldırıldıysa `true` döner.
pub async fn remove(name: &str) -> AppResult<bool> {
    validate_name(name)?;
    let Some(user) = lookup(name) else { return Ok(true) };
    if !group_members(MANAGED_GROUP).iter().any(|member| member == name) {
        return Ok(false);
    }
    let has_files = std::fs::read_dir(user.sites_dir()).map(|mut entries| entries.next().is_some()).unwrap_or(false);
    if has_files {
        set_locked(name, true).await?;
        return Ok(false);
    }
    // Hesabın süren süreçleri varsa silme başarısız olur; önce sonlandırılırlar.
    // Açık oturumlar düzgünce kapatılır; kalan süreçler öldürülür.
    let _ = exec::run("loginctl", &["terminate-user", "--", name]).await;
    // Oturum yöneticisinin kapanması beklenir: kapanırken öldürülürse "başarısız birim" olarak kalır
    // (kapanış, aşağıdaki temizlikten sonra tamamlanıp durumu yeniden başarısıza çevirebilir).
    let uid = user.uid.to_string();
    let _ = exec::run("systemctl", &["stop", &format!("user@{uid}.service"), &format!("user-runtime-dir@{uid}.service")]).await;
    let _ = exec::run("pkill", &["-KILL", "-u", &uid]).await;
    exec::run_ok("userdel", &["--", name]).await?;
    let _ = exec::run("systemctl", &["reset-failed", &format!("user@{uid}.service"), &format!("user-runtime-dir@{uid}.service")]).await;
    // Ev klasörünün üst düzeyi root'a aittir; `userdel` ona dokunmaz. Geride kalırsa aynı adla
    // açılacak yeni hesap eskisinin klasörünü devralırdı. Site dosyası kalmadığı yukarıda doğrulandı.
    if user.home.starts_with("/home/") && user.home.is_dir() {
        std::fs::remove_dir_all(&user.home)?;
    }
    Ok(true)
}

/// Kullanıcının kendi yazabildiği, ev klasörünün üst düzeyindeki klasörler.
/// Üst düzey root'a ait olduğundan araçların beklediği klasörler önceden açılır.
const USER_DIRS: &[&str] = &["tmp", ".ssh", ".config", ".cache", ".local", ".composer", ".npm"];

/// Ev klasörünün sahipliğini ve izinlerini yukarıdaki düzene getirir.
/// Her çağrıda aynı sonucu verir; eski düzendeki hesapları da onarır.
fn harden_home(user: &SysUser) -> AppResult<()> {
    // Önce üst düzey kilitlenir: bundan sonra kullanıcı buradaki adları
    // değiştiremeyeceği için aşağıdaki işlemler sembolik bağ oyununa açık değildir.
    lchown(&user.home, Some(0), Some(user.gid))?;
    set_mode(&user.home, 0o751)?;

    let ensure_dir = |name: &str, uid: u32, gid: u32, mode: u32| -> AppResult<()> {
        let path = user.home.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            // Aynı adda dosya ya da sembolik bağ varsa (eski düzenden kalma) kaldırılır.
            Ok(_) => {
                std::fs::remove_file(&path)?;
                std::fs::create_dir(&path)?;
            }
            Err(_) => std::fs::create_dir(&path)?,
        }
        lchown(&path, Some(uid), Some(gid))?;
        set_mode(&path, mode)
    };
    for name in USER_DIRS {
        ensure_dir(name, user.uid, user.gid, 0o700)?;
    }
    // Kullanıcı (grup üzerinden) listeleyebilir, nginx (diğerleri) yalnızca içinden geçebilir.
    ensure_dir("sites", 0, user.gid, 0o751)
}

const WELCOME_PAGE: &str = include_str!("templates/welcome.html");

/// Site klasörünü oluşturur ve yolunu döner. Klasör zaten varsa dokunmaz.
pub fn create_site_dirs(user: &SysUser, domain: &str, public_dir: &str) -> AppResult<PathBuf> {
    let root = user.sites_dir().join(domain);
    if root.exists() {
        return Ok(root);
    }
    let web_gid = group_id(WEB_GROUP)?;
    // Üst klasör root'a ait olduğundan, sahiplik devredilene kadar kullanıcı
    // araya giremez; önce her şey oluşturulur, en son sahiplik verilir.
    let public = root.join(public_dir);
    let logs = root.join("logs");
    std::fs::create_dir_all(&public)?;
    std::fs::create_dir_all(&logs)?;
    let index = public.join("index.html");
    std::fs::write(&index, WELCOME_PAGE.replace("{{domain}}", domain))?;

    // `public_dir` iç içe olabilir (ör. `app/public`); aradaki her klasörün
    // sahipliği kökten yaprağa doğru verilir.
    let mut current = root.clone();
    let mut owned = vec![root.clone()];
    for part in Path::new(public_dir).components() {
        current = current.join(part);
        owned.push(current.clone());
    }
    for dir in &owned {
        lchown(dir, Some(user.uid), Some(web_gid))?;
        // setgid: içeride oluşturulan dosyalar www-data grubunu devralır,
        // böylece nginx okuyabilir ama diğer hesaplar erişemez.
        set_mode(dir, 0o2750)?;
    }
    lchown(&index, Some(user.uid), Some(web_gid))?;
    set_mode(&index, 0o640)?;
    lchown(&logs, Some(user.uid), Some(user.gid))?;
    set_mode(&logs, 0o750)?;
    Ok(root)
}

/// Site klasörünü içindekilerle birlikte siler.
pub fn remove_site_dirs(user: &SysUser, domain: &str) -> AppResult<()> {
    let root = user.sites_dir().join(domain);
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    Ok(())
}
