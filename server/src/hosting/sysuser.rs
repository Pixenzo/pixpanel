//! Hosting hesaplarının Linux kullanıcıları.
//!
//! Her panel hesabının siteleri kendi Linux kullanıcısıyla çalışır; siteler
//! arası yalıtımı çekirdek (dosya izinleri ve uid) sağlar.
//!
//! Dizin düzeni:
//! ```text
//! /home/<kullanıcı>            kullanıcı:kullanıcı  0711
//! /home/<kullanıcı>/tmp        kullanıcı:kullanıcı  0700   PHP geçici dosyaları ve oturumlar
//! /home/<kullanıcı>/sites      root:root            0711   kullanıcı site klasörlerini değiştiremez
//! /home/<kullanıcı>/sites/<alan adı>  kullanıcı:www-data  2750
//! ```

use std::os::unix::fs::{lchown, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::sys::exec;

/// Panelin oluşturduğu kullanıcıların üye olduğu grup.
pub const MANAGED_GROUP: &str = "capsule-users";
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
fn validate_name(name: &str) -> AppResult<()> {
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

    // Kullanıcı henüz hiçbir süreç çalıştırmadığından bu klasörleri root
    // olarak oluşturmak güvenlidir.
    set_mode(&user.home, 0o711)?;
    let tmp = user.tmp_dir();
    std::fs::create_dir_all(&tmp)?;
    lchown(&tmp, Some(user.uid), Some(user.gid))?;
    set_mode(&tmp, 0o700)?;
    let sites = user.sites_dir();
    std::fs::create_dir_all(&sites)?;
    lchown(&sites, Some(0), Some(0))?;
    set_mode(&sites, 0o711)?;
    Ok(user)
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
