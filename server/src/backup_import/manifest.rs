//! Yedeğin içeriğini anlatan ortak model ve biçim tanıma.
//!
//! Her biçimin (cPanel, Plesk, HestiaCP) ayrı bir ayrıştırıcısı vardır; hepsi açılmış yedek
//! klasörünü okuyup aynı `BackupManifest`i üretir. Ayrıştırıcılar hoşgörülüdür: bulamadıkları
//! parçayı atlar ve `notes` içinde "bulunamadı" der. Manifestteki yollar yedeğin açıldığı klasöre
//! göre göreli, `/` ayraçlıdır; içe aktarma bu yolları yeniden doğrulayarak çözer.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BackupManifest {
    /// `cpanel`, `plesk` ya da `hestia`.
    pub format: String,
    pub domains: Vec<ManifestDomain>,
    pub databases: Vec<ManifestDatabase>,
    /// Yalnızca bilgi: yedekte bulunan posta kutusu adresleri (içe aktarılmaz).
    pub mailboxes: Vec<String>,
    /// Önizlemede gösterilen uyarılar ve bulunamayanlar.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestDomain {
    pub name: String,
    /// `main`, `addon`, `subdomain` ya da `domain`.
    pub kind: String,
    /// Site içeriği: klasör ya da arşiv (`.tgz`, `.tar.zst`...). Bulunamadıysa boş.
    pub docroot_path: Option<String>,
    /// Klasörse içeriğinin boyutu; arşivse arşivin boyutu.
    pub size_bytes: Option<u64>,
    /// Klasörün kökünde `wp-config.php` var mı (arşivlerde bilinmez, `false`).
    pub wordpress: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestDatabase {
    pub name: String,
    /// `mysql` ya da `postgresql`.
    pub engine: String,
    /// Döküm dosyası (`.sql`, `.sql.gz`, `.sql.zst`, `.tgz`...). Bulunamadıysa boş.
    pub dump_path: Option<String>,
    /// Veritabanı kullanıcı adları (bilgi amaçlı; kullanıcılar yeniden oluşturulur).
    pub users: Vec<String>,
    /// Yedekte veritabanı bir alan adıyla ilişkiliyse o alan adı.
    pub domain: Option<String>,
}

/// Arşivden gelen ad önizlemede ve kayıtlarda göründüğü için makul uzunlukta, denetim karakteri
/// içermeyen metin olmalı.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 253 && !name.chars().any(|ch| ch.is_control())
}

/// `path`ı `raw` köküne göre `/` ayraçlı göreli yola çevirir.
pub fn relative(raw: &Path, path: &Path) -> Option<String> {
    let stripped = path.strip_prefix(raw).ok()?;
    let mut parts = Vec::new();
    for component in stripped.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}

/// Klasörün (bağlar izlenmeden) toplam dosya boyutu; çok büyük ağaçlarda yaklaşık değer döner.
pub fn dir_size(path: &Path) -> u64 {
    fn walk(path: &Path, budget: &mut u32) -> u64 {
        let Ok(entries) = fs::read_dir(path) else { return 0 };
        let mut total = 0;
        for entry in entries.flatten() {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                total += walk(&entry.path(), budget);
            } else if kind.is_file() {
                total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            }
        }
        total
    }
    walk(path, &mut 300_000)
}

/// Klasörün doğrudan alt girdileri (ad sırasıyla); bağlar dosya/klasör sayılmaz.
pub fn children(path: &Path) -> Vec<(String, PathBuf, bool)> {
    let mut items: Vec<(String, PathBuf, bool)> = fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let kind = entry.file_type().ok()?;
                    if kind.is_symlink() || !(kind.is_dir() || kind.is_file()) {
                        return None;
                    }
                    Some((entry.file_name().to_str()?.to_string(), entry.path(), kind.is_dir()))
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

/// `base` altındaki bütün dosyalar ve klasörler (göreli yollar). Bağlar izlenmez.
pub fn walk(base: &Path, max_depth: usize) -> (Vec<PathBuf>, Vec<PathBuf>) {
    fn visit(base: &Path, dir: &Path, depth: usize, max_depth: usize, files: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) {
        if files.len() + dirs.len() > 300_000 {
            return;
        }
        for (_, path, is_dir) in children(dir) {
            let Ok(rel) = path.strip_prefix(base) else { continue };
            if is_dir {
                dirs.push(rel.to_path_buf());
                if depth < max_depth {
                    visit(base, &path, depth + 1, max_depth, files, dirs);
                }
            } else {
                files.push(rel.to_path_buf());
            }
        }
    }
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    visit(base, base, 1, max_depth, &mut files, &mut dirs);
    (files, dirs)
}

/// Yol bileşenleri (UTF-8 olmayanlar atılır).
pub fn components(path: &Path) -> Vec<String> {
    path.components().filter_map(|component| component.as_os_str().to_str().map(str::to_string)).collect()
}

/// Metin dosyasını okur (en çok 32 MB; fazlası kesilir). Okunamazsa boş metin.
pub fn read_text(path: &Path) -> String {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else { return String::new() };
    let mut bytes = Vec::new();
    let _ = file.take(32 * 1024 * 1024).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `ANAHTAR='değer'` ya da `ANAHTAR=değer` biçimindeki bir değeri (tırnaklar atılır) bulur.
/// Hestia yapılandırması satır başına birden çok çift içerebilir (`A='1' B='2'`).
pub fn conf_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let mut rest = line;
        while let Some(at) = rest.find(&format!("{key}=")) {
            let before_ok = at == 0 || rest[..at].ends_with(|ch: char| ch.is_whitespace());
            let value = &rest[at + key.len() + 1..];
            if before_ok {
                let value = value.trim_start();
                let parsed = match value.chars().next() {
                    Some(quote @ ('\'' | '"')) => value[1..].split(quote).next().unwrap_or("").to_string(),
                    _ => value.split_whitespace().next().unwrap_or("").to_string(),
                };
                return Some(parsed);
            }
            rest = value;
        }
    }
    None
}

/// Dosya adı bir arşive (tar/tgz/zst) mi işaret ediyor?
pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".tgz", ".tar.gz", ".tar", ".tar.zst", ".tzst", ".tar.bz2", ".tar.xz"].iter().any(|ext| lower.ends_with(ext))
}

/// Dosya adı bir SQL dökümüne (düz ya da sıkıştırılmış) ya da onu içeren arşive mi işaret ediyor?
pub fn is_dump_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".sql", ".sql.gz", ".sql.zst", ".sql.bz2", ".sql.xz"].iter().any(|ext| lower.ends_with(ext)) || (lower.contains("sqldump") && (is_archive_name(&lower) || lower.ends_with(".gz")))
}

/// Alan adı için bir klasörün yedekteki göreli yolunu ve boyut bilgisini üretir.
pub fn domain_entry(raw: &Path, name: &str, kind: &str, content: Option<&Path>) -> ManifestDomain {
    let found = content.filter(|path| path.exists());
    let is_dir = found.is_some_and(|path| path.is_dir());
    ManifestDomain {
        name: name.to_string(),
        kind: kind.to_string(),
        docroot_path: found.and_then(|path| relative(raw, path)),
        size_bytes: found.map(|path| if is_dir { dir_size(path) } else { fs::metadata(path).map(|meta| meta.len()).unwrap_or(0) }),
        wordpress: is_dir && found.is_some_and(|path| path.join("wp-config.php").is_file()),
    }
}

/// Açılmış yedeğin biçimini tanır ve manifestini üretir.
pub fn parse_backup(raw: &Path) -> Result<BackupManifest, String> {
    for base in candidate_bases(raw) {
        if super::plesk::detect(&base) {
            return Ok(super::plesk::parse(raw, &base));
        }
        if super::cpanel::detect(&base) {
            return Ok(super::cpanel::parse(raw, &base));
        }
        if super::hestia::detect(&base) {
            return Ok(super::hestia::parse(raw, &base));
        }
    }
    Err("Yedeğin biçimi tanınamadı. cPanel (cpmove-*.tar.gz), Plesk (backup_*.tar) ve HestiaCP (kullanici.tarih.tar) yedekleri desteklenir.".into())
}

/// Yedeğin kök klasörü: arşivin kendisi ya da tek bir üst klasör (`cpmove-kullanici/`) altındaki içerik.
fn candidate_bases(raw: &Path) -> Vec<PathBuf> {
    let mut bases = vec![raw.to_path_buf()];
    let items = children(raw);
    let dirs: Vec<&(String, PathBuf, bool)> = items.iter().filter(|item| item.2).collect();
    if dirs.len() == 1 {
        bases.push(dirs[0].1.clone());
    }
    bases
}

#[cfg(test)]
pub(crate) mod testing {
    use std::fs;
    use std::path::{Path, PathBuf};

    pub fn fresh_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("pixpanel-bimp-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `dir` altında göreli yola dosya yazar (üst klasörleri açar).
    pub fn put(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn config_values_are_read() {
        let text = "DOMAIN='ornek.com' IP='1.2.3.4' ALIAS='www.ornek.com,x.com'\nDBUSER=\"kul_1\" TYPE=mysql\nNOTDOMAIN='x'";
        assert_eq!(conf_value(text, "DOMAIN").as_deref(), Some("ornek.com"));
        assert_eq!(conf_value(text, "ALIAS").as_deref(), Some("www.ornek.com,x.com"));
        assert_eq!(conf_value(text, "DBUSER").as_deref(), Some("kul_1"));
        assert_eq!(conf_value(text, "TYPE").as_deref(), Some("mysql"));
        assert_eq!(conf_value(text, "YOK"), None);
    }

    #[test]
    fn names_and_paths_are_checked() {
        assert!(valid_name("ornek.com"));
        assert!(!valid_name("") && !valid_name("a\nb") && !valid_name(&"a".repeat(300)));
        assert!(is_archive_name("backup_user-data_2301.TGZ") && is_archive_name("domain_data.tar.zst") && !is_archive_name("a.sql"));
        assert!(is_dump_name("db.mysql.sql.zst") && is_dump_name("backup_sqldump_1.tgz") && !is_dump_name("backup_user-data.tgz"));
        let raw = Path::new("/x/raw");
        assert_eq!(relative(raw, &raw.join("a").join("b")).as_deref(), Some("a/b"));
        assert_eq!(relative(raw, Path::new("/baska/yer")), None);
    }

    #[test]
    fn unknown_layout_is_rejected() {
        let dir = fresh_dir("unknown");
        put(&dir, "rastgele/dosya.txt", "x");
        assert!(parse_backup(&dir).unwrap_err().contains("tanınamadı"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
