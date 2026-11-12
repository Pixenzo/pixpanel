//! Sitenin belge kökü altındaki WordPress kurulumlarını bulma (alt klasör kurulumları).
//!
//! Tarama hesabın kendi Linux kullanıcısıyla çalışan `find` ile yapılır; sembolik bağlar
//! izlenmez, `wp-content`/`node_modules`/`vendor` gibi klasörlerin içine girilmez. Kullanıcıdan
//! gelen kurulum yolu hem biçim kuralına uymalı hem de taramanın bulduğu kurulumlardan biri olmalıdır.

use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::hosting::sysuser;
use crate::sys::runas;

/// Belge kökünden en çok bu kadar klasör aşağıya inilir (`a/b/c` üç düzeydir).
pub const MAX_DEPTH: usize = 3;
/// Taramada okunan toplam girdi (klasör + `wp-config.php`) sınırı; aşılırsa tarama kesilir.
const MAX_SCAN_ENTRIES: usize = 5000;
/// Bir sitede listelenecek en çok kurulum.
const MAX_INSTALLS: usize = 20;
const SCAN_TIMEOUT: Duration = Duration::from_secs(20);
const QUICK: Duration = Duration::from_secs(10);

/// Taramanın içine girmediği klasör adları.
const SKIPPED_DIRS: [&str; 6] = ["wp-content", "wp-includes", "wp-admin", "node_modules", "vendor", ".git"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Install {
    /// Belge köküne göre klasör; kök kurulumda boş.
    pub path: String,
    pub version: String,
    /// Tahmini adres (alan adı + klasör); asıl adres kurulumun `siteurl` ayarıdır.
    pub url: String,
    pub is_root: bool,
}

/// Kurulum klasörünün biçimini doğrular ve yalın hâlini döner: boş ya da `.` kök demektir.
/// Mutlak yol, `..`, ters eğik çizgi, beklenmedik karakter ve üç düzeyden derin yollar reddedilir.
pub fn validate_install_path(raw: &str) -> AppResult<String> {
    let invalid = || AppError::bad("Geçersiz WordPress kurulum klasörü");
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(String::new());
    }
    if trimmed.len() > 200 || trimmed.starts_with('/') {
        return Err(invalid());
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() > MAX_DEPTH {
        return Err(invalid());
    }
    for segment in &segments {
        let plain = !segment.is_empty() && segment.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        // `.` ve `..` dizin gezintisidir; `-` ile başlayan ad komuta seçenek gibi görünür; atlanan klasörler tarama dışıdır.
        if !plain || segment.starts_with('-') || segment.chars().all(|c| c == '.') || SKIPPED_DIRS.contains(segment) {
            return Err(invalid());
        }
    }
    Ok(segments.join("/"))
}

/// `find` çıktısındaki satırlardan (`d <klasör>` / `f <klasör>/wp-config.php`) `wp-config.php`
/// bulunan klasörleri çıkarır. Biçimi bozuk ya da kurala uymayan satırlar atlanır.
pub fn parse_scan_output(output: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in output.lines() {
        let Some(path) = line.strip_prefix("f ") else { continue };
        let dir = match path.rsplit_once('/') {
            Some((dir, "wp-config.php")) => dir,
            None if path == "wp-config.php" => "",
            _ => continue,
        };
        let Ok(dir) = validate_install_path(dir) else { continue };
        if !found.contains(&dir) {
            found.push(dir);
        }
    }
    found
}

/// `wp-includes/version.php` içindeki `$wp_version = '6.5.2';` satırından sürümü okur.
pub fn parse_wp_version(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("$wp_version")?.trim_start().strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let value = rest[1..].split(quote).next()?;
    let valid = !value.is_empty() && value.len() <= 30 && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    valid.then(|| value.to_string())
}

/// Kökte önce, ardından klasör adına göre sıralar.
fn sort_installs(installs: &mut [Install]) {
    installs.sort_by(|a, b| b.is_root.cmp(&a.is_root).then_with(|| a.path.cmp(&b.path)));
}

fn guess_url(site: &Site, rel: &str) -> String {
    let scheme = if site.ssl_mode == "none" { "http" } else { "https" };
    if rel.is_empty() { format!("{scheme}://{}", site.domain) } else { format!("{scheme}://{}/{rel}", site.domain) }
}

/// Sitenin belge kökü altındaki WordPress kurulumlarını bulur (kök dahil).
pub async fn scan_installs(site: &Site) -> AppResult<Vec<Install>> {
    if site.kind != "php" {
        return Ok(Vec::new());
    }
    let user = sysuser::lookup(&site.owner).ok_or_else(|| AppError::internal("Hesabın Linux kullanıcısı bulunamadı"))?;
    let docroot = site.docroot();

    // Klasörler yalnızca girdi sayısını sınırlamak için, `wp-config.php` dosyaları sonuç için yazdırılır.
    // `find` sembolik bağları izlemez (`-P` öntanımlıdır); atlanan klasörler budanır.
    let mut args: Vec<String> = vec![".".into(), "-maxdepth".into(), (MAX_DEPTH + 1).to_string(), "(".into(), "-type".into(), "d".into(), "(".into()];
    for (index, name) in SKIPPED_DIRS.iter().enumerate() {
        if index > 0 {
            args.push("-o".into());
        }
        args.extend(["-name".into(), (*name).into()]);
    }
    args.extend([")".into(), "-prune".into(), ")".into(), "-o".into()]);
    args.extend(["(".into(), "-type".into(), "d".into(), "-printf".into(), "d %P\\n".into(), ")".into(), "-o".into()]);
    args.extend(["(".into(), "-type".into(), "f".into(), "-name".into(), "wp-config.php".into(), "-printf".into(), "f %P\\n".into(), ")".into()]);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let mut cmd = runas::command(&user, &docroot, "find", &arg_refs);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("tarama başlatılamadı: {err}")))?;
    let stdout = child.stdout.take().ok_or_else(|| AppError::internal("tarama çıktısı okunamadı"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut collected = String::new();
    let mut entries = 0usize;
    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            entries += 1;
            if entries > MAX_SCAN_ENTRIES {
                break;
            }
            collected.push_str(&line);
            collected.push('\n');
        }
    };
    // Zaman aşımında da o ana kadar toplananla devam edilir.
    let _ = tokio::time::timeout(SCAN_TIMEOUT, read).await;
    let _ = child.kill().await;

    let mut installs = Vec::new();
    for dir in parse_scan_output(&collected).into_iter().take(MAX_INSTALLS) {
        // Sürüm dosyası yoksa klasör bir WordPress kurulumu sayılmaz (ör. artakalan bir wp-config.php).
        let version_file = if dir.is_empty() { "wp-includes/version.php".to_string() } else { format!("{dir}/wp-includes/version.php") };
        let cmd = runas::command(&user, &docroot, "grep", &["-m1", "-E", r"^\$wp_version[[:space:]]*=", "--", &version_file]);
        let Ok(output) = runas::output(cmd, QUICK).await else { continue };
        if !output.success() {
            continue;
        }
        let Some(version) = output.stdout.lines().find_map(parse_wp_version) else { continue };
        installs.push(Install { is_root: dir.is_empty(), url: guess_url(site, &dir), path: dir, version });
    }
    sort_installs(&mut installs);
    Ok(installs)
}

/// İstekteki kurulum yolunu çözer: biçim doğrulanır, kök dışındaki yollar taramanın bulduğu
/// kurulumlardan biri olmalıdır. Dönen değer `Wp::for_install`a verilebilir (kök için boş).
pub async fn resolve_install(site: &Site, requested: Option<&str>) -> AppResult<String> {
    let rel = validate_install_path(requested.unwrap_or(""))?;
    if rel.is_empty() {
        return Ok(rel);
    }
    let installs = scan_installs(site).await?;
    if installs.iter().any(|install| install.path == rel) {
        Ok(rel)
    } else {
        Err(AppError::bad("Bu klasörde bir WordPress kurulumu bulunamadı"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_path_validation() {
        assert_eq!(validate_install_path("").unwrap(), "");
        assert_eq!(validate_install_path(" . ").unwrap(), "");
        assert_eq!(validate_install_path("blog").unwrap(), "blog");
        assert_eq!(validate_install_path("blog/").unwrap(), "blog");
        assert_eq!(validate_install_path("a/b/c").unwrap(), "a/b/c");
        for bad in ["/etc", "../x", "a/../b", "..", "a//b", "a/b/c/d", "wp-content", "blog/node_modules", "vendor/x", "-rf", "a b", "a\\b", "a;b", "blog/wp-admin", "$(x)", "./blog", "a/./b"] {
            assert!(validate_install_path(bad).is_err(), "{bad} reddedilmeli");
        }
    }

    #[test]
    fn scan_output_yields_config_directories() {
        let output = "d \nd blog\nf wp-config.php\nf blog/wp-config.php\nd shop\nf shop/en/wp-config.php\nf a/b/c/d/wp-config.php\nf deneme/wp-config.php.bak\nf ../kacak/wp-config.php\nsaçma satır\nf blog/wp-config.php\n";
        assert_eq!(parse_scan_output(output), vec!["", "blog", "shop/en"]);
        assert!(parse_scan_output("d a\nd b\n").is_empty());
    }

    #[test]
    fn version_is_read_from_version_php() {
        assert_eq!(parse_wp_version("$wp_version = '6.5.2';").as_deref(), Some("6.5.2"));
        assert_eq!(parse_wp_version("  $wp_version=\"6.6-beta1\";").as_deref(), Some("6.6-beta1"));
        assert_eq!(parse_wp_version("$wp_version = '';"), None);
        assert_eq!(parse_wp_version("$wp_db_version = 57155;"), None);
        assert_eq!(parse_wp_version("$wp_version = 'a b';"), None);
        assert_eq!(parse_wp_version("// $wp_version = '1.0';"), None);
    }

    #[test]
    fn installs_are_sorted_root_first() {
        let make = |path: &str| Install { path: path.into(), version: "6.5".into(), url: String::new(), is_root: path.is_empty() };
        let mut list = vec![make("shop"), make("blog"), make("")];
        sort_installs(&mut list);
        assert_eq!(list.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), vec!["", "blog", "shop"]);
    }
}
