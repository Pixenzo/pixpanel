//! Python sürümleri: `uv` ile kurulan, bağımsız (python-build-standalone) yorumlayıcılar.
//!
//! Yönetici bir sürümü (`3.10` ... `3.14`) Bileşenler sayfasından kurar; dosyalar
//! `/opt/pixpanel/python-store` altına inip `/opt/pixpanel/python/<X.Y>` adlı kararlı bir
//! sembolik bağla gösterilir (uv'nin dizin adları yama sürümünü içerir). Siteler yorumlayıcıyı
//! bu kararlı yoldan seçer; sanal ortamlarını (`.venv`) o yorumlayıcıyla, kendi kullanıcılarıyla kurar.
//!
//! Güvenlik notları:
//! - `uv` ikilisi GitHub sürümlerinden indirilir; sürümü ve SHA-256 özetleri koda gömülüdür.
//!   Özeti doğrulanmayan ikili asla çalıştırılmaz; diskteki ikili her kullanımdan önce yeniden
//!   doğrulanır, bozulmuşsa yeniden indirilir.
//! - `uv`, Python derlemelerini kendisi indirir (astral-sh/python-build-standalone, GitHub) ve
//!   kendi gömülü özetleriyle doğrular. Bu ağ erişimi bilinçli olarak kabul edilmiştir.
//! - Sürüm dizesi beyaz listeyle doğrulanır; komutlar `sys::exec` ile argüman dizisi olarak çalışır.
//! - Kurulan dosyalar root'a aittir, herkes okuyup çalıştırabilir, kimse yazamaz.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::components;
use crate::error::{AppError, AppResult};
use crate::jobs::JobCtx;
use crate::sys::exec;

/// Panelden kurulabilen Python sürümleri (yeniden eskiye).
pub const PYTHON_VERSIONS: &[&str] = &["3.14", "3.13", "3.12", "3.11", "3.10"];

/// Kararlı yollar: `<kök>/<X.Y>/bin/python3`.
pub const PYTHON_ROOT: &str = "/opt/pixpanel/python";
/// uv'nin gerçek kurulum klasörü (dizin adları yama sürümü içerir).
const PYTHON_STORE: &str = "/opt/pixpanel/python-store";

/// Panelin kullandığı uv sürümü (astral-sh/uv sürüm sayfasından).
pub const UV_VERSION: &str = "0.13.6";
/// `uv-x86_64-unknown-linux-musl.tar.gz` dosyasının özeti (sürüm sayfasındaki `.sha256` ile aynı).
const UV_ARCHIVE_SHA256: &str = "5d61f26b6c7213779eda90edc6971ea6b0b796bfb8d49a574c8ad69b59838548";
/// Arşivden çıkan `uv` ikilisinin özeti; her çalıştırmadan önce diskteki dosya buna karşı denetlenir.
const UV_BINARY_SHA256: &str = "f56ca7e29e708e20ed6b96f9a8cb89ac45129239c6166481cc3ae3cbab286a0f";
const UV_DIR: &str = "/usr/local/lib/pixpanel";
pub const UV_PATH: &str = "/usr/local/lib/pixpanel/uv";

/// Sürüm dizesi `3.<1-2 rakam>` biçiminde ve izinli kümede olmalı (dosya yolunda kullanılır).
pub fn validate_version(version: &str) -> AppResult<()> {
    let shaped = version.strip_prefix("3.").is_some_and(|rest| (1..=2).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_digit()));
    if shaped && PYTHON_VERSIONS.contains(&version) { Ok(()) } else { Err(AppError::bad("Geçersiz Python sürümü")) }
}

/// Kurulu sürümün kararlı klasörü.
pub fn root_of(version: &str) -> PathBuf {
    Path::new(PYTHON_ROOT).join(version)
}

/// Kurulu sürümün yorumlayıcısı (sürüm doğrulanmış olmalı).
pub fn interpreter(version: &str) -> PathBuf {
    root_of(version).join("bin/python3")
}

/// Kurulu Python sürümleri (yeniden eskiye).
pub fn installed_versions() -> Vec<String> {
    PYTHON_VERSIONS.iter().filter(|version| interpreter(version).is_file()).map(|version| version.to_string()).collect()
}

/// Kurulu sürümün tam numarası (ör. `3.12.12`).
pub async fn full_version(version: &str) -> Option<String> {
    validate_version(version).ok()?;
    let binary = interpreter(version);
    if !binary.is_file() {
        return None;
    }
    let output = exec::run(&binary.to_string_lossy(), &["--version"]).await.ok()?;
    let success = output.success();
    let text = if output.stdout.trim().is_empty() { output.stderr } else { output.stdout };
    success.then(|| text.trim().trim_start_matches("Python").trim().to_string())
}

/// Dosyanın SHA-256 özeti (küçük harfli onaltılık); dosya okunamazsa `None`.
fn file_digest(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(hex::encode(hasher.finalize()))
}

/// uv ikilisini (yoksa ya da özeti tutmuyorsa) indirir; döndürülen yol özeti doğrulanmış ikiliyi gösterir.
async fn ensure_uv(ctx: &JobCtx) -> AppResult<PathBuf> {
    let path = Path::new(UV_PATH);
    if file_digest(path).as_deref() == Some(UV_BINARY_SHA256) {
        return Ok(path.to_path_buf());
    }
    if path.exists() {
        ctx.log("uv ikilisi beklenen özetle uyuşmuyor (bozulmuş ya da değiştirilmiş); yeniden indiriliyor");
    }
    ctx.log(format!("uv {UV_VERSION} indiriliyor"));
    std::fs::create_dir_all(UV_DIR)?;
    let archive = format!("{UV_DIR}/uv.tar.gz.new");
    let staging = Path::new(UV_DIR).join("uv.new");
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(&archive);
    let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-x86_64-unknown-linux-musl.tar.gz");
    let result = fetch_uv(ctx, &archive, &staging, &url).await;
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(path.to_path_buf())
}

async fn fetch_uv(ctx: &JobCtx, archive: &str, staging: &Path, url: &str) -> AppResult<()> {
    ctx.exec("curl", &["-fsSL", "--max-time", "300", "-o", archive, "--", url]).await?;
    if file_digest(Path::new(archive)).as_deref() != Some(UV_ARCHIVE_SHA256) {
        return Err(AppError::internal("İndirilen uv arşivinin özeti uyuşmuyor; kurulum durduruldu"));
    }
    std::fs::create_dir_all(staging)?;
    ctx.exec("tar", &["-xzf", archive, "-C", &staging.to_string_lossy(), "--strip-components=1", "--no-same-owner"]).await?;
    let extracted = staging.join("uv");
    if file_digest(&extracted).as_deref() != Some(UV_BINARY_SHA256) {
        return Err(AppError::internal("Arşivden çıkan uv ikilisinin özeti uyuşmuyor; kurulum durduruldu"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&extracted, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&extracted, UV_PATH)?;
    ctx.log("uv özeti doğrulandı");
    Ok(())
}

/// Kurulum klasöründeki gerçek (sembolik bağ olmayan) `cpython-<X.Y>.<yama>-...` dizinleri.
fn store_dirs(version: &str) -> Vec<(u64, PathBuf)> {
    let prefix = format!("cpython-{version}.");
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(PYTHON_STORE)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()))
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    let patch = name.strip_prefix(&prefix)?.split('-').next()?.to_string();
                    let digits: String = patch.chars().take_while(char::is_ascii_digit).collect();
                    let number = digits.parse::<u64>().ok()?;
                    entry.path().join("bin/python3").exists().then(|| (number, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
}

/// Sürümü uv ile kurar ve kararlı yolu hazırlar. Görev günlüğüne ilerleme yazar.
pub async fn install(ctx: &JobCtx, version: &str) -> AppResult<()> {
    validate_version(version)?;
    components::apt_install(ctx, &["curl".into(), "ca-certificates".into()]).await?;
    let uv = ensure_uv(ctx).await?;

    std::fs::create_dir_all(PYTHON_STORE)?;
    std::fs::create_dir_all(PYTHON_ROOT)?;
    ctx.log(format!("Python {version} uv ile kuruluyor (derleme python-build-standalone'dan indirilir)"));
    ctx.log(format!("$ uv python install {version}"));
    let mut command = exec::base_command(&uv.to_string_lossy());
    command
        .args(["--no-config", "--no-cache", "--no-progress", "python", "install", version])
        .env("UV_PYTHON_INSTALL_DIR", PYTHON_STORE)
        // uv'nin kullanıcı bin klasörüne (~/.local/bin) yürütülebilir bırakmasını engeller.
        .env("UV_PYTHON_INSTALL_BIN", "0")
        .env("UV_PYTHON_BIN_DIR", format!("{PYTHON_STORE}/.bin"))
        .env("UV_PYTHON_DOWNLOADS", "automatic")
        .env("HOME", "/root");
    ctx.run_streaming(&mut command, "uv", None).await?;

    let target = store_dirs(version).into_iter().next().map(|(_, path)| path).ok_or_else(|| AppError::internal("uv kurulumdan sonra Python klasörü bulunamadı"))?;
    // Dosyalar root'a ait, herkes okuyup çalıştırabilir, kimse yazamaz.
    ctx.exec("chown", &["-R", "root:root", "--", PYTHON_STORE]).await?;
    ctx.exec("chmod", &["-R", "u=rwX,go=rX", "--", PYTHON_STORE]).await?;
    ctx.exec("chmod", &["755", PYTHON_ROOT]).await?;

    // Kararlı yol: geçici bağ kurulup yerine taşınır; çalışan siteler hiçbir an eksik yol görmez.
    let link = root_of(version);
    let temporary = Path::new(PYTHON_ROOT).join(format!(".{version}.new"));
    let _ = std::fs::remove_file(&temporary);
    std::os::unix::fs::symlink(&target, &temporary)?;
    std::fs::rename(&temporary, &link)?;

    let shown = full_version(version).await.ok_or_else(|| AppError::internal("Kurulan Python çalıştırılamadı"))?;
    ctx.log(format!("Python {shown} kuruldu: {}", link.display()));
    Ok(())
}

/// Sürümü kaldırır (kararlı bağı ve uv kurulum klasörlerini siler). Sürümü kullanan siteler
/// çağıran tarafından denetlenmiş olmalıdır.
pub fn uninstall(version: &str) -> AppResult<()> {
    validate_version(version)?;
    let link = root_of(version);
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(&link)?,
        Ok(_) => return Err(AppError::internal("Python klasörü beklenmeyen biçimde (sembolik bağ değil); elle kaldırın")),
        Err(_) => {}
    }
    // uv'nin yama sürümlü klasörleri ve (yeni uv sürümlerinde) küçük sürüm bağları: yalnız bu sürüme ait olanlar.
    let (patch_prefix, link_prefix) = (format!("cpython-{version}."), format!("cpython-{version}-"));
    if let Ok(entries) = std::fs::read_dir(PYTHON_STORE) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if !(name.starts_with(&patch_prefix) || name.starts_with(&link_prefix)) {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() && !kind.is_symlink() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Bileşen kimliği (`python-3.12`) bir uv Python sürümünü gösteriyorsa sürümü döner.
pub fn version_of_component(id: &str) -> Option<&str> {
    id.strip_prefix("python-").filter(|version| PYTHON_VERSIONS.contains(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_whitelist() {
        for good in ["3.10", "3.11", "3.12", "3.13", "3.14"] {
            assert!(validate_version(good).is_ok(), "{good}");
        }
        for bad in ["", "3", "3.9", "3.1", "3.99", "2.7", "3.12.1", "3.12; id", "../x", "3.12/../..", " 3.12", "3.1\n2"] {
            assert!(validate_version(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn component_ids() {
        assert_eq!(version_of_component("python-3.12"), Some("3.12"));
        assert_eq!(version_of_component("python"), None);
        assert_eq!(version_of_component("python-3.9"), None);
        assert_eq!(version_of_component("node-22"), None);
    }
}
