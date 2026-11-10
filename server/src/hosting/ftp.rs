//! FTP: pure-ftpd ve site başına sanal hesaplar.
//!
//! Hesaplar sistem kullanıcısı değildir; pure-ftpd'nin kendi kullanıcı
//! veritabanında durur. Her hesap bir sitenin klasörüne (ya da alt klasörüne)
//! hapsedilir ve dosyalara sitenin sahibi olan Linux kullanıcısının kimliğiyle
//! erişir; böylece yüklenen dosyalar siteyle aynı sahipliği taşır.

use std::path::{Component as PathComponent, Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncWriteExt;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::hosting::sysuser::SysUser;
use crate::sys::exec;

const CONF_DIR: &str = "/etc/pure-ftpd/conf";
const AUTH_DIR: &str = "/etc/pure-ftpd/auth";
const CERT_PATH: &str = "/etc/ssl/private/pure-ftpd.pem";
pub const SERVICE: &str = "pure-ftpd.service";
/// Veri bağlantıları için açılan port aralığı (güvenlik duvarında izin verilmelidir).
pub const PASSIVE_PORTS: (u16, u16) = (30000, 30099);

pub fn is_installed() -> bool {
    exec::binary_exists("pure-ftpd") && exec::binary_exists("pure-pw")
}

/// Hesap adı: küçük harfle başlar; harf, rakam, alt çizgi, tire ve nokta içerir.
pub fn validate_name(value: &str) -> AppResult<String> {
    let name = value.trim().to_ascii_lowercase();
    let mut chars = name.bytes();
    let valid = (3..=32).contains(&name.len())
        && chars.next().is_some_and(|b| b.is_ascii_lowercase())
        && chars.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.'));
    if valid { Ok(name) } else { Err(AppError::bad("Geçersiz hesap adı (3-32 karakter; küçük harfle başlar; harf, rakam, _ - . kullanılabilir)")) }
}

/// Parola pure-pw'ye satır olarak verildiğinden satır sonu içeremez.
pub fn validate_password(password: &str) -> AppResult<()> {
    if password.len() < 10 || password.len() > 128 {
        return Err(AppError::bad("Parola 10-128 karakter olmalı"));
    }
    if password.chars().any(char::is_control) {
        return Err(AppError::bad("Parola geçersiz karakter içeriyor"));
    }
    Ok(())
}

/// Hesabın hapsedileceği klasör: site kökü ya da onun içindeki var olan bir alt klasör.
///
/// Alt klasör yalnızca düz adlardan oluşabilir; ayrıca gerçek yolun site kökünün
/// içinde kaldığı denetlenir (kök dışını gösteren sembolik bağ kabul edilmez).
pub fn resolve_directory(site_root: &Path, subdir: &str) -> AppResult<(String, PathBuf)> {
    let trimmed = subdir.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Ok((String::new(), site_root.to_path_buf()));
    }
    let invalid = || AppError::bad("Geçersiz klasör (örnek: public ya da public/yuklemeler)");
    let parts: Vec<&str> = Path::new(trimmed)
        .components()
        .map(|component| match component {
            PathComponent::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()
        .ok_or_else(invalid)?;
    let part_ok = |part: &&str| part.len() <= 64 && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if parts.len() > 8 || !parts.iter().all(part_ok) {
        return Err(invalid());
    }
    let normalized = parts.join("/");
    let target = site_root.join(&normalized);
    let (real_root, real_target) = (
        site_root.canonicalize().map_err(|_| AppError::bad("Site klasörü bulunamadı"))?,
        target.canonicalize().map_err(|_| AppError::bad("Bu klasör site içinde yok; önce dosya yöneticisinden oluşturun"))?,
    );
    if !real_target.starts_with(&real_root) || !real_target.is_dir() {
        return Err(invalid());
    }
    Ok((normalized, target))
}

/// `pure-pw` komutunu çalıştırır; `password` verilirse standart girdiden iki kez yazılır
/// (komut satırında parola görünmez).
async fn pure_pw(args: &[&str], password: Option<&str>) -> AppResult<()> {
    let mut cmd = exec::base_command("pure-pw");
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if password.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("pure-pw başlatılamadı: {err}")))?;
    if let (Some(password), Some(mut pipe)) = (password, child.stdin.take()) {
        pipe.write_all(format!("{password}\n{password}\n").as_bytes()).await?;
        pipe.shutdown().await?;
    }
    let output = child.wait_with_output().await?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(AppError::internal(format!("FTP hesabı güncellenemedi: {detail}")))
    }
}

pub async fn create_account(name: &str, password: &str, owner: &SysUser, directory: &Path) -> AppResult<()> {
    let (uid, gid, dir) = (owner.uid.to_string(), owner.gid.to_string(), directory.to_string_lossy().into_owned());
    pure_pw(&["useradd", name, "-u", &uid, "-g", &gid, "-d", &dir, "-m"], Some(password)).await
}

pub async fn set_password(name: &str, password: &str) -> AppResult<()> {
    pure_pw(&["passwd", name, "-m"], Some(password)).await
}

pub async fn set_directory(name: &str, directory: &Path) -> AppResult<()> {
    pure_pw(&["usermod", name, "-d", &directory.to_string_lossy(), "-m"], None).await
}

pub async fn remove_account(name: &str) -> AppResult<()> {
    pure_pw(&["userdel", name, "-m"], None).await
}

/// pure-ftpd'nin temel yapılandırmasını yazar ve servisi yeniden başlatır.
pub async fn setup(cfg: &Config) -> AppResult<()> {
    let settings: [(&str, String); 12] = [
        // Her hesap kendi klasörüne hapsedilir.
        ("ChrootEveryone", "yes".into()),
        ("NoAnonymous", "yes".into()),
        ("PureDB", "/etc/pure-ftpd/pureftpd.pdb".into()),
        // Sistem kullanıcıları FTP ile giremez; yalnızca panelin açtığı sanal hesaplar.
        ("UnixAuthentication", "no".into()),
        ("PAMAuthentication", "no".into()),
        ("PassivePortRange", format!("{} {}", PASSIVE_PORTS.0, PASSIVE_PORTS.1)),
        ("MinUID", "1000".into()),
        // 1: şifreli (FTPS) ve şifresiz bağlantının ikisi de kabul edilir.
        ("TLS", "1".into()),
        // Dosyalar 640, klasörler 750: web sunucusu grubu okuyabilir, başkaları erişemez.
        ("Umask", "137 027".into()),
        ("DisplayDotFiles", "yes".into()),
        ("MaxClientsPerIP", "10".into()),
        ("NoChmod", "no".into()),
    ];
    std::fs::create_dir_all(CONF_DIR)?;
    for (name, value) in &settings {
        atomic_write(&Path::new(CONF_DIR).join(name), format!("{value}\n").as_bytes(), 0o644)?;
    }
    std::fs::create_dir_all(AUTH_DIR)?;
    for stale in ["65unix", "70pam"] {
        let _ = std::fs::remove_file(Path::new(AUTH_DIR).join(stale));
    }
    let link = Path::new(AUTH_DIR).join("50pure");
    if std::fs::read_link(&link).is_err() {
        std::os::unix::fs::symlink("../conf/PureDB", &link)?;
    }
    // Kullanıcı veritabanı hiç oluşmadıysa boş hâliyle üretilir; aksi hâlde servis başlamaz.
    if !Path::new("/etc/pure-ftpd/pureftpd.passwd").exists() {
        std::fs::write("/etc/pure-ftpd/pureftpd.passwd", "")?;
    }
    exec::run_ok("pure-pw", &["mkdb"]).await?;

    write_certificate(cfg)?;
    exec::run_ok("systemctl", &["enable", "--quiet", "--", SERVICE]).await?;
    exec::run_ok("systemctl", &["restart", "--", SERVICE]).await?;
    Ok(())
}

/// TLS için panelin sertifikası kullanılır (pure-ftpd sertifika ve anahtarı tek dosyada ister).
fn write_certificate(cfg: &Config) -> AppResult<()> {
    let mut pem = std::fs::read(cfg.tls_dir().join("panel.crt"))?;
    pem.extend(std::fs::read(cfg.tls_dir().join("panel.key"))?);
    atomic_write(Path::new(CERT_PATH), &pem, 0o600)
}

/// Panelin sertifikası değiştiğinde FTP sunucusunu yeni sertifikaya geçirir.
pub async fn install_certificate(cfg: &Config) -> AppResult<()> {
    write_certificate(cfg)?;
    exec::run_ok("systemctl", &["restart", "--", SERVICE]).await?;
    Ok(())
}
