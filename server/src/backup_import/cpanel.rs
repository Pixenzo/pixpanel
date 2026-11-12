//! cPanel yedeği (`cpmove-<kullanıcı>.tar.gz`, `backup-<tarih>_<kullanıcı>.tar.gz`).
//!
//! Beklenen yerleşim (kök klasörün altında):
//!
//! ```text
//! homedir/public_html            ana alan adının sitesi
//! homedir/<klasör>               ek alan ve alt alan adı klasörleri
//! mysql/<veritabanı>.sql         döküm (mysql.sql: yetkiler)
//! psql/<veritabanı>.sql          PostgreSQL dökümü (içe aktarılmaz)
//! userdata/<alan>                documentroot ve servername (YAML benzeri); userdata/main: ana/ek/alt alanlar
//! cp/<kullanıcı>                 ANAHTAR=değer; DNS=ana alan adı
//! addons, sds                    eski sürümlerde ek/alt alan listeleri (userdata yoksa yedek olarak okunur)
//! homedir/etc/<alan>/passwd      posta kutuları (kullanıcı:parola özeti:...)
//! ```
//!
//! Varsayım: `userdata/` dosyaları `documentroot: /home/<kullanıcı>/...` satırı taşır. Yol, ev klasörü
//! öneki (`homedir:` satırı ya da `/home*/<kullanıcı>/`) atılarak `homedir/` altına eşlenir.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use super::manifest::{children, domain_entry, read_text, relative, valid_name, BackupManifest, ManifestDatabase};

pub fn detect(base: &Path) -> bool {
    base.join("homedir").is_dir() && ["cp", "userdata", "mysql", "psql", "version", "meta", "mysql.sql", "dnszones"].iter().any(|name| base.join(name).exists())
}

/// YAML benzeri dosyanın en üst düzey değerleri ve girintili alt satırları.
#[derive(Debug, Default)]
struct Yaml {
    scalars: HashMap<String, String>,
    blocks: HashMap<String, Vec<String>>,
}

fn parse_yaml(text: &str) -> Yaml {
    let mut yaml = Yaml::default();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-');
        if indented {
            if let Some(key) = &current {
                let item = line.trim().trim_start_matches('-').trim();
                yaml.blocks.entry(key.clone()).or_default().push(item.trim_matches(['"', '\'']).to_string());
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim().to_string(), value.trim().trim_matches(['"', '\'']).to_string());
        if value.is_empty() {
            current = Some(key.clone());
            yaml.blocks.entry(key).or_default();
        } else {
            current = None;
            yaml.scalars.insert(key, value);
        }
    }
    yaml
}

/// `documentroot` yolunu `homedir/` altındaki gerçek klasöre eşler (yoksa `None`).
fn map_docroot(home: &Path, documentroot: &str, home_prefix: Option<&str>) -> Option<PathBuf> {
    let documentroot = documentroot.trim().trim_end_matches('/');
    let rest = match home_prefix.map(|prefix| prefix.trim_end_matches('/')).filter(|prefix| !prefix.is_empty()) {
        Some(prefix) if documentroot == prefix => "",
        Some(prefix) if documentroot.starts_with(&format!("{prefix}/")) => &documentroot[prefix.len() + 1..],
        _ => {
            let pattern = regex::Regex::new(r"^/home[0-9]*/[^/]+(?:/(.*))?$").ok()?;
            pattern.captures(documentroot)?.get(1).map_or("", |found| found.as_str())
        }
    };
    safe_join(home, rest).filter(|path| path.is_dir())
}

/// `base` altında `rest`i (göreli, `..` içermeyen) birleştirir.
fn safe_join(base: &Path, rest: &str) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    for component in Path::new(rest).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(path)
}

/// Hesap adı: `cp/` altındaki dosya adı, yoksa kök klasör adından (`cpmove-kullanici`).
fn account_name(base: &Path) -> Option<String> {
    if let Some((name, _, _)) = children(&base.join("cp")).into_iter().find(|(_, _, is_dir)| !is_dir) {
        return Some(name);
    }
    let folder = base.file_name()?.to_str()?;
    folder.strip_prefix("cpmove-").map(str::to_string)
}

fn is_skipped_userdata(name: &str) -> bool {
    name == "main" || name == "cache" || name.ends_with(".cache") || name.ends_with("_SSL") || name.ends_with(".json") || name.ends_with(".yaml") || name.starts_with('.')
}

struct Candidate {
    name: String,
    kind: &'static str,
    docroot: Option<PathBuf>,
}

pub fn parse(raw: &Path, base: &Path) -> BackupManifest {
    let mut manifest = BackupManifest { format: "cpanel".into(), ..Default::default() };
    let home = base.join("homedir");
    let account = account_name(base);
    let cp = account.as_deref().map(|name| read_text(&base.join("cp").join(name))).unwrap_or_default();
    let cp_domain = cp.lines().find_map(|line| line.strip_prefix("DNS=")).map(|value| value.trim().to_lowercase()).filter(|value| valid_name(value));
    let main_yaml = parse_yaml(&read_text(&base.join("userdata").join("main")));
    let main_domain = main_yaml.scalars.get("main_domain").map(|value| value.to_lowercase()).filter(|value| valid_name(value)).or(cp_domain);
    let sub_domains: HashSet<String> = main_yaml.blocks.get("sub_domains").map(|list| list.iter().map(|item| item.to_lowercase()).collect()).unwrap_or_default();

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut home_prefix: Option<String> = None;
    for (file, path, is_dir) in children(&base.join("userdata")) {
        if is_dir || is_skipped_userdata(&file) {
            continue;
        }
        let yaml = parse_yaml(&read_text(&path));
        if home_prefix.is_none() {
            home_prefix = yaml.scalars.get("homedir").cloned();
        }
        let name = yaml.scalars.get("servername").cloned().unwrap_or_else(|| file.clone()).to_lowercase();
        if !valid_name(&name) || candidates.iter().any(|known| known.name == name) {
            continue;
        }
        let docroot = yaml.scalars.get("documentroot").and_then(|value| map_docroot(&home, value, home_prefix.as_deref()));
        let kind = if Some(&name) == main_domain.as_ref() {
            "main"
        } else if sub_domains.contains(&name) {
            "subdomain"
        } else {
            "addon"
        };
        candidates.push(Candidate { name, kind, docroot });
    }

    // Ana alan adı userdata'da görünmüyorsa: public_html ona aittir.
    let public_html = home.join("public_html");
    if let Some(main) = &main_domain {
        if !candidates.iter().any(|known| &known.name == main) {
            candidates.push(Candidate { name: main.clone(), kind: "main", docroot: Some(public_html.clone()).filter(|path| path.is_dir()) });
        }
    } else if candidates.is_empty() && public_html.is_dir() {
        manifest.notes.push("Ana alan adı yedekte bulunamadı; public_html klasörü \"public_html\" adıyla listelendi, içe aktarırken hedef alan adını yazın.".into());
        candidates.push(Candidate { name: "public_html".into(), kind: "main", docroot: Some(public_html.clone()) });
    }

    // Eski yedeklerde userdata yoksa addons/sds dosyalarından ek ve alt alan adları okunur.
    let mut used: HashSet<PathBuf> = candidates.iter().filter_map(|known| known.docroot.clone()).collect();
    for (file, kind) in [("addons", "addon"), ("sds", "subdomain"), ("sds2", "subdomain")] {
        for line in read_text(&base.join(file)).lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim().to_lowercase(), value.trim());
            let name = if key.contains('.') { key.clone() } else { key.replacen('_', ".", 1) };
            if !valid_name(&name) || !name.contains('.') || candidates.iter().any(|known| known.name == name) {
                continue;
            }
            let guesses = [value.trim_start_matches("public_html/").to_string(), format!("public_html/{name}"), name.clone(), value.to_string()];
            let docroot = guesses
                .iter()
                .filter(|guess| !guess.is_empty())
                .flat_map(|guess| [safe_join(&home, guess), safe_join(&public_html, guess)])
                .flatten()
                .find(|path| path.is_dir() && !used.contains(path));
            if let Some(path) = &docroot {
                used.insert(path.clone());
            }
            candidates.push(Candidate { name, kind, docroot });
        }
    }

    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for candidate in candidates {
        if seen.insert(candidate.name.clone(), ()).is_some() {
            continue;
        }
        let entry = domain_entry(raw, &candidate.name, candidate.kind, candidate.docroot.as_deref());
        if entry.docroot_path.is_none() {
            manifest.notes.push(format!("{}: site klasörü yedekte bulunamadı.", candidate.name));
        }
        manifest.domains.push(entry);
    }
    // Ana alan adı en üstte, ötekiler ada göre.
    manifest.domains.sort_by(|a, b| (a.kind != "main", &a.name).cmp(&(b.kind != "main", &b.name)));

    manifest.databases = databases(raw, base);
    manifest.mailboxes = mailboxes(&home);
    if manifest.domains.is_empty() {
        manifest.notes.push("Yedekte site klasörü bulunamadı.".into());
    }
    manifest
}

/// `mysql.sql` yetkilerinden veritabanı → kullanıcılar eşlemesi.
fn grant_users(text: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(pattern) = regex::Regex::new(r"(?i)GRANT\s.*?\sON\s+`([^`]+)`\.\*\s+TO\s+'([^']+)'") else { return map };
    for captures in pattern.captures_iter(text) {
        // cPanel `_` karakterini `\_` olarak kaçışlar.
        let database = captures[1].replace('\\', "");
        let users = map.entry(database).or_default();
        let user = captures[2].to_string();
        if !users.contains(&user) {
            users.push(user);
        }
    }
    map
}

fn databases(raw: &Path, base: &Path) -> Vec<ManifestDatabase> {
    let grants = grant_users(&read_text(&base.join("mysql.sql")));
    let mut list = Vec::new();
    for (folder, engine) in [("mysql", "mysql"), ("psql", "postgresql")] {
        for (file, path, is_dir) in children(&base.join(folder)) {
            if is_dir {
                continue;
            }
            let Some(name) = file.strip_suffix(".sql").map(str::to_string).filter(|name| valid_name(name)) else { continue };
            let users = if engine == "mysql" { grants.get(&name).cloned().unwrap_or_default() } else { Vec::new() };
            list.push(ManifestDatabase { name, engine: engine.into(), dump_path: relative(raw, &path), users, domain: None });
        }
    }
    list
}

fn mailboxes(home: &Path) -> Vec<String> {
    let mut addresses = Vec::new();
    for (domain, path, is_dir) in children(&home.join("etc")) {
        if !is_dir || !valid_name(&domain) || !domain.contains('.') {
            continue;
        }
        for line in read_text(&path.join("passwd")).lines() {
            if let Some(user) = line.split(':').next().map(str::trim).filter(|user| !user.is_empty() && valid_name(user)) {
                addresses.push(format!("{user}@{domain}"));
            }
        }
    }
    if addresses.is_empty() {
        for (domain, path, is_dir) in children(&home.join("mail")) {
            if !is_dir || !domain.contains('.') || !valid_name(&domain) {
                continue;
            }
            for (user, _, user_dir) in children(&path) {
                if user_dir && valid_name(&user) && !matches!(user.as_str(), "cur" | "new" | "tmp") && !user.starts_with('.') {
                    addresses.push(format!("{user}@{domain}"));
                }
            }
        }
    }
    addresses.sort();
    addresses.dedup();
    addresses.truncate(2000);
    addresses
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{parse_backup, testing::*};
    use super::*;
    use std::fs;

    fn build() -> PathBuf {
        let raw = fresh_dir("cpanel");
        let base = "cpmove-musteri";
        put(&raw, &format!("{base}/cp/musteri"), "USER=musteri\nDNS=ornek.com\nDNS1=ornek.net\n");
        put(&raw, &format!("{base}/userdata/main"), "main_domain: ornek.com\naddon_domains:\n  ek.org: ek.ornek.com\nsub_domains:\n- blog.ornek.com\nparked_domains: []\n");
        put(&raw, &format!("{base}/userdata/ornek.com"), "documentroot: /home/musteri/public_html\nhomedir: /home/musteri\nservername: ornek.com\nserveralias: www.ornek.com\n");
        put(&raw, &format!("{base}/userdata/ornek.com_SSL"), "documentroot: /home/musteri/public_html\nservername: ornek.com\n");
        put(&raw, &format!("{base}/userdata/ek.ornek.com"), "documentroot: /home/musteri/ek.org\nhomedir: /home/musteri\nservername: ek.org\n");
        put(&raw, &format!("{base}/userdata/blog.ornek.com"), "documentroot: /home/musteri/public_html/blog\nservername: blog.ornek.com\n");
        put(&raw, &format!("{base}/userdata/yok.ornek.com"), "documentroot: /home/musteri/olmayan\nservername: yok.ornek.com\n");
        put(&raw, &format!("{base}/homedir/public_html/wp-config.php"), "<?php");
        put(&raw, &format!("{base}/homedir/public_html/index.php"), "x");
        put(&raw, &format!("{base}/homedir/public_html/blog/index.html"), "b");
        put(&raw, &format!("{base}/homedir/ek.org/index.html"), "e");
        put(&raw, &format!("{base}/mysql/musteri_wp.sql"), "-- dokum");
        put(&raw, &format!("{base}/mysql/musteri_diger.sql"), "-- dokum");
        put(&raw, &format!("{base}/psql/musteri_pg.sql"), "-- pg");
        put(&raw, &format!("{base}/mysql.sql"), "GRANT USAGE ON *.* TO 'musteri_wp'@'localhost' IDENTIFIED BY PASSWORD 'x';\nGRANT ALL PRIVILEGES ON `musteri\\_wp`.* TO 'musteri_wp'@'localhost';\n");
        put(&raw, &format!("{base}/homedir/etc/ornek.com/passwd"), "info:$6$x:1:2\nsatis:$6$y\n");
        raw
    }

    #[test]
    fn cpanel_backup_is_recognised_and_parsed() {
        let raw = build();
        let manifest = parse_backup(&raw).unwrap();
        assert_eq!(manifest.format, "cpanel");
        let by_name = |name: &str| manifest.domains.iter().find(|domain| domain.name == name).unwrap_or_else(|| panic!("{name} yok: {:?}", manifest.domains));
        assert_eq!(manifest.domains[0].name, "ornek.com");
        assert_eq!(by_name("ornek.com").kind, "main");
        assert_eq!(by_name("ornek.com").docroot_path.as_deref(), Some("cpmove-musteri/homedir/public_html"));
        assert!(by_name("ornek.com").wordpress);
        assert_eq!(by_name("blog.ornek.com").kind, "subdomain");
        assert_eq!(by_name("ek.org").kind, "addon");
        assert_eq!(by_name("ek.org").docroot_path.as_deref(), Some("cpmove-musteri/homedir/ek.org"));
        // Klasörü olmayan alan listelenir ama "bulunamadı" notu düşer.
        assert_eq!(by_name("yok.ornek.com").docroot_path, None);
        assert!(manifest.notes.iter().any(|note| note.contains("yok.ornek.com") && note.contains("bulunamadı")));
        // _SSL dosyası tekrar sayılmaz.
        assert_eq!(manifest.domains.iter().filter(|domain| domain.name == "ornek.com").count(), 1);

        assert_eq!(manifest.databases.len(), 3);
        let wp = manifest.databases.iter().find(|db| db.name == "musteri_wp").unwrap();
        assert_eq!((wp.engine.as_str(), wp.users.as_slice(), wp.dump_path.as_deref()), ("mysql", &["musteri_wp".to_string()][..], Some("cpmove-musteri/mysql/musteri_wp.sql")));
        assert_eq!(manifest.databases.iter().find(|db| db.name == "musteri_pg").unwrap().engine, "postgresql");
        assert_eq!(manifest.mailboxes, ["info@ornek.com", "satis@ornek.com"]);
        fs::remove_dir_all(&raw).unwrap();
    }

    #[test]
    fn legacy_backup_without_userdata_uses_public_html() {
        let raw = fresh_dir("cpanel-old");
        put(&raw, "homedir/public_html/index.html", "x");
        put(&raw, "homedir/ek/index.html", "y");
        put(&raw, "cp/eski", "DNS=eski.com\n");
        put(&raw, "addons", "ek.net=ek\n");
        let manifest = parse_backup(&raw).unwrap();
        assert_eq!(manifest.format, "cpanel");
        let names: Vec<&str> = manifest.domains.iter().map(|domain| domain.name.as_str()).collect();
        assert_eq!(names, ["eski.com", "ek.net"]);
        assert_eq!(manifest.domains[1].docroot_path.as_deref(), Some("homedir/ek"));
        fs::remove_dir_all(&raw).unwrap();
    }

    #[test]
    fn documentroot_mapping_ignores_escapes() {
        let raw = fresh_dir("cpanel-map");
        put(&raw, "homedir/public_html/a.txt", "x");
        let home = raw.join("homedir");
        assert!(map_docroot(&home, "/home/u/public_html", Some("/home/u")).is_some());
        assert!(map_docroot(&home, "/home2/u/public_html", None).is_some());
        assert!(map_docroot(&home, "/home/u/../../etc", Some("/home/u")).is_none());
        assert!(map_docroot(&home, "/var/www/html", None).is_none());
        fs::remove_dir_all(&raw).unwrap();
    }

    #[test]
    fn yaml_blocks_are_read() {
        let yaml = parse_yaml("main_domain: a.com\naddon_domains:\n  b.org: x.a.com\nsub_domains:\n- s.a.com\n- t.a.com\n");
        assert_eq!(yaml.scalars["main_domain"], "a.com");
        assert_eq!(yaml.blocks["sub_domains"], ["s.a.com", "t.a.com"]);
        assert_eq!(yaml.blocks["addon_domains"], ["b.org: x.a.com"]);
    }
}
