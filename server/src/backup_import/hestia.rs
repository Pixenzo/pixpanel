//! HestiaCP yedeği (`<kullanıcı>.<tarih>.tar`).
//!
//! Beklenen yerleşim (kök klasörün altında):
//!
//! ```text
//! web/<alan>/domain_data.tar.zst|.tar.gz   site içeriği (içinde public_html/); açılmış yapıda domain_data/ klasörü
//! web/<alan>/hestia/web.conf               DOMAIN='..' ALIAS='..' ... (yalnızca ipucu; alan adı klasör adından alınır)
//! db/<veritabanı>/<veritabanı>.mysql.sql.zst|.gz   döküm; .pgsql. içerenler PostgreSQL'dir (içe aktarılmaz)
//! db/<veritabanı>/**/*.conf                DBUSER='..' TYPE='mysql'  (varsa)
//! mail/<alan>/**/*.conf                    ACCOUNT='..' satırları: posta kutuları
//! user/, pam/, cron/, dns/                 tanıma ipucu; içe aktarılmaz
//! ```
//!
//! Varsayım: Hestia sürümleri arasında yapılandırma dosyalarının adı değişebildiği için `.conf`
//! dosyaları adına bakılmadan taranır; bulunamayan bilgi atlanır.

use std::path::Path;

use super::manifest::{children, conf_value, domain_entry, is_archive_name, is_dump_name, read_text, relative, valid_name, walk, BackupManifest, ManifestDatabase};

pub fn detect(base: &Path) -> bool {
    let has_data = base.join("web").is_dir() || base.join("db").is_dir() || base.join("mail").is_dir();
    let hint = ["user", "pam", "cron", "dns"].iter().any(|name| base.join(name).is_dir())
        || children(&base.join("web")).iter().any(|(_, path, is_dir)| *is_dir && children(path).iter().any(|(name, _, _)| name.starts_with("domain_data")));
    has_data && hint
}

/// `.conf` dosyalarının metinleri (en çok iki klasör derinliğinde).
fn conf_texts(dir: &Path) -> Vec<String> {
    let (files, _) = walk(dir, 2);
    files.iter().filter(|path| path.extension().is_some_and(|ext| ext == "conf")).map(|path| read_text(&dir.join(path))).collect()
}

pub fn parse(raw: &Path, base: &Path) -> BackupManifest {
    let mut manifest = BackupManifest { format: "hestia".into(), ..Default::default() };

    for (folder, path, is_dir) in children(&base.join("web")) {
        let name = folder.to_lowercase();
        if !is_dir || !valid_name(&name) {
            continue;
        }
        let items = children(&path);
        let archive = items.iter().find(|(file, _, is_dir)| !is_dir && file.starts_with("domain_data") && is_archive_name(file));
        let data_dir = items.iter().find(|(file, _, is_dir)| *is_dir && (file == "domain_data" || file == "public_html"));
        let content = archive.or(data_dir).map(|(_, path, _)| path.as_path());
        let entry = domain_entry(raw, &name, "domain", content);
        if entry.docroot_path.is_none() {
            manifest.notes.push(format!("{name}: site içeriği (domain_data) yedekte bulunamadı."));
        }
        manifest.domains.push(entry);
    }

    for (name, path, is_dir) in children(&base.join("db")) {
        if !is_dir || !valid_name(&name) {
            continue;
        }
        let dump = children(&path).into_iter().find(|(file, _, is_dir)| !is_dir && is_dump_name(file));
        let confs = conf_texts(&path);
        let conf_type = confs.iter().find_map(|text| conf_value(text, "TYPE")).unwrap_or_default().to_lowercase();
        let file_hint = dump.as_ref().map(|(file, _, _)| file.to_lowercase()).unwrap_or_default();
        let postgres = file_hint.contains(".pgsql.") || file_hint.contains(".postgres") || conf_type.starts_with("pg") || conf_type.starts_with("postgres");
        let mut users: Vec<String> = Vec::new();
        for user in confs.iter().filter_map(|text| conf_value(text, "DBUSER")).filter(|user| valid_name(user)) {
            if !users.contains(&user) {
                users.push(user);
            }
        }
        if dump.is_none() {
            manifest.notes.push(format!("{name}: veritabanı dökümü yedekte bulunamadı."));
        }
        manifest.databases.push(ManifestDatabase {
            name,
            engine: if postgres { "postgresql" } else { "mysql" }.into(),
            dump_path: dump.and_then(|(_, path, _)| relative(raw, &path)),
            users,
            domain: None,
        });
    }

    for (domain, path, is_dir) in children(&base.join("mail")) {
        if !is_dir || !valid_name(&domain) || !domain.contains('.') {
            continue;
        }
        for text in conf_texts(&path) {
            for line in text.lines() {
                if let Some(account) = conf_value(line, "ACCOUNT").filter(|account| valid_name(account)) {
                    manifest.mailboxes.push(format!("{account}@{}", domain.to_lowercase()));
                }
            }
        }
    }
    manifest.mailboxes.sort();
    manifest.mailboxes.dedup();
    manifest.mailboxes.truncate(2000);
    if manifest.domains.is_empty() {
        manifest.notes.push("Yedekte web alan adı bulunamadı.".into());
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{parse_backup, testing::*};
    use std::fs;

    #[test]
    fn hestia_backup_is_recognised_and_parsed() {
        let raw = fresh_dir("hestia");
        put(&raw, "web/ornek.com/domain_data.tar.zst", "sahte");
        put(&raw, "web/ornek.com/hestia/web.conf", "DOMAIN='ornek.com' ALIAS='www.ornek.com'\n");
        put(&raw, "web/eski.net/domain_data.tar.gz", "sahte");
        put(&raw, "web/bos.org/hestia/web.conf", "DOMAIN='bos.org'\n");
        put(&raw, "db/kul_wp/kul_wp.mysql.sql.zst", "dokum");
        put(&raw, "db/kul_wp/hestia/db.conf", "DB='kul_wp' DBUSER='kul_wpuser' TYPE='mysql'\n");
        put(&raw, "db/kul_pg/kul_pg.pgsql.sql.gz", "pg");
        put(&raw, "mail/ornek.com/accounts.conf", "ACCOUNT='info' ALIAS=''\nACCOUNT='satis' ALIAS=''\n");
        put(&raw, "user/user.conf", "FNAME='x'\n");
        put(&raw, "pam/passwd", "x");
        let manifest = parse_backup(&raw).unwrap();
        assert_eq!(manifest.format, "hestia");
        let names: Vec<&str> = manifest.domains.iter().map(|domain| domain.name.as_str()).collect();
        assert_eq!(names, ["bos.org", "eski.net", "ornek.com"]);
        assert_eq!(manifest.domains[2].docroot_path.as_deref(), Some("web/ornek.com/domain_data.tar.zst"));
        assert_eq!(manifest.domains[0].docroot_path, None);
        assert!(manifest.notes.iter().any(|note| note.contains("bos.org") && note.contains("bulunamadı")));
        let wp = manifest.databases.iter().find(|db| db.name == "kul_wp").unwrap();
        assert_eq!((wp.engine.as_str(), wp.users.clone()), ("mysql", vec!["kul_wpuser".to_string()]));
        assert_eq!(wp.dump_path.as_deref(), Some("db/kul_wp/kul_wp.mysql.sql.zst"));
        assert_eq!(manifest.databases.iter().find(|db| db.name == "kul_pg").unwrap().engine, "postgresql");
        assert_eq!(manifest.mailboxes, ["info@ornek.com", "satis@ornek.com"]);
        fs::remove_dir_all(&raw).unwrap();
    }

    #[test]
    fn wrapped_in_single_folder_is_found() {
        let raw = fresh_dir("hestia-wrap");
        put(&raw, "admin.2026-10-12/web/ornek.com/domain_data.tar.zst", "sahte");
        put(&raw, "admin.2026-10-12/user/user.conf", "x");
        assert_eq!(parse_backup(&raw).unwrap().format, "hestia");
        fs::remove_dir_all(&raw).unwrap();
    }
}
