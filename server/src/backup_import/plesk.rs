//! Plesk yedeği (`backup_*.tar` ya da açılmış yapı).
//!
//! Beklenen yerleşim (en çok birkaç klasör derinliğinde, `clients/<müşteri>/` önekiyle de olabilir):
//!
//! ```text
//! backup_info_<tarih>.xml                              üst veri: <domain name="..."> ve <database name="..." type="mysql">
//! domains/<alan>/backup_user-data_<tarih>.tgz          site içeriği (içinde httpdocs/)
//! domains/<alan>/backup_httpdocs_<tarih>.tgz           yalnızca yayın klasörü (varsa tercih edilir)
//! domains/<alan>/httpdocs/                             açılmış yapıda doğrudan klasör
//! databases/<veritabanı>/backup_sqldump_<tarih>.tgz    döküm (.sql, .sql.gz de olabilir); domains/<alan>/ altında da bulunabilir
//! ```
//!
//! Varsayımlar: alan adları XML'deki `domain` öğelerinin `name` özniteliğinden ve `domains/<alan>`
//! klasör adlarından birleştirilir; veritabanı türü `database` öğesinin `type` özniteliğidir (yoksa
//! MySQL); posta kutuları `mailuser`/`mailname` öğeleridir. Eski MIME biçimli Plesk yedekleri tanınmaz.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::manifest::{components, domain_entry, is_archive_name, is_dump_name, read_text, relative, valid_name, walk, BackupManifest, ManifestDatabase};
use super::xml::{self, Event};

fn is_info_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("backup_info") && lower.ends_with(".xml")
}

pub fn detect(base: &Path) -> bool {
    let (files, _) = walk(base, 3);
    files.iter().any(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(is_info_file))
}

#[derive(Debug, Default)]
struct Info {
    domains: Vec<String>,
    databases: Vec<InfoDatabase>,
    mailboxes: Vec<String>,
}

#[derive(Debug)]
struct InfoDatabase {
    name: String,
    engine: String,
    domain: Option<String>,
    users: Vec<String>,
}

fn engine_of(kind: &str) -> Option<&'static str> {
    match kind.to_lowercase().as_str() {
        "" | "mysql" | "mariadb" => Some("mysql"),
        "postgresql" | "postgres" | "pgsql" => Some("postgresql"),
        _ => None,
    }
}

/// `backup_info` XML'inden alan adları, veritabanları ve posta kutuları.
fn read_info(text: &str, info: &mut Info) {
    let mut stack: Vec<(String, Option<String>)> = Vec::new();
    let mut current_db: Option<usize> = None;
    for event in xml::scan(text) {
        match event {
            Event::Start { name, attrs, self_closing } => {
                let attr_name = Event::attr(&attrs, "name").map(str::to_string);
                let domain = stack.iter().rev().find(|(element, _)| element == "domain").and_then(|(_, value)| value.clone());
                match (name.as_str(), &attr_name) {
                    ("domain", Some(value)) if valid_name(value) => {
                        let value = value.to_lowercase();
                        if !info.domains.contains(&value) {
                            info.domains.push(value);
                        }
                    }
                    ("database", Some(value)) if valid_name(value) => {
                        current_db = None;
                        if let Some(engine) = engine_of(Event::attr(&attrs, "type").unwrap_or("")) {
                            if !info.databases.iter().any(|known| &known.name == value) {
                                info.databases.push(InfoDatabase { name: value.clone(), engine: engine.into(), domain: domain.clone(), users: Vec::new() });
                            }
                            current_db = info.databases.iter().position(|known| &known.name == value);
                        }
                    }
                    ("dbuser", Some(value)) if valid_name(value) => {
                        if let Some(index) = current_db {
                            if !info.databases[index].users.contains(value) {
                                info.databases[index].users.push(value.clone());
                            }
                        }
                    }
                    ("mailuser" | "mailname", Some(value)) if valid_name(value) => {
                        let address = if value.contains('@') { Some(value.clone()) } else { domain.map(|domain| format!("{value}@{domain}")) };
                        if let Some(address) = address {
                            if !info.mailboxes.contains(&address) {
                                info.mailboxes.push(address);
                            }
                        }
                    }
                    _ => {}
                }
                if !self_closing {
                    stack.push((name, attr_name.map(|value| value.to_lowercase())));
                }
            }
            Event::End { name } => {
                if let Some(position) = stack.iter().rposition(|(element, _)| *element == name) {
                    stack.truncate(position);
                }
                if name == "database" {
                    current_db = None;
                }
            }
        }
    }
}

pub fn parse(raw: &Path, base: &Path) -> BackupManifest {
    let mut manifest = BackupManifest { format: "plesk".into(), ..Default::default() };
    let (files, dirs) = walk(base, 10);

    let mut info = Info::default();
    for path in files.iter().filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(is_info_file)).take(4) {
        read_info(&read_text(&base.join(path)), &mut info);
    }

    // Site içeriği: alan başına (öncelik, yol). `backup_httpdocs` içeriği `backup_user-data`dan önce gelir.
    let mut content: BTreeMap<String, (u8, PathBuf)> = BTreeMap::new();
    let mut dumps: BTreeMap<String, PathBuf> = BTreeMap::new();
    fn note_content(content: &mut BTreeMap<String, (u8, PathBuf)>, domain: &str, priority: u8, path: PathBuf) {
        if valid_name(domain) {
            let key = domain.to_lowercase();
            if content.get(&key).is_none_or(|(known, _)| *known < priority) {
                content.insert(key, (priority, path));
            }
        }
    }
    for path in &files {
        let parts = components(path);
        let Some(file_name) = parts.last() else { continue };
        let lower = file_name.to_lowercase();
        if let Some(at) = parts.iter().rposition(|part| part.eq_ignore_ascii_case("domains")) {
            if at + 2 < parts.len() && is_archive_name(&lower) {
                if lower.starts_with("backup_httpdocs") {
                    note_content(&mut content, &parts[at + 1], 3, base.join(path));
                } else if lower.starts_with("backup_user-data") {
                    note_content(&mut content, &parts[at + 1], 2, base.join(path));
                }
            }
        }
        if let Some(at) = parts.iter().rposition(|part| part.eq_ignore_ascii_case("databases")) {
            if at + 2 < parts.len() && is_dump_name(&lower) && valid_name(&parts[at + 1]) {
                dumps.entry(parts[at + 1].clone()).or_insert_with(|| base.join(path));
            }
        }
    }
    // Açılmış yapıda yayın klasörü doğrudan klasör olabilir: domains/<alan>/httpdocs.
    for path in &dirs {
        let parts = components(path);
        if let Some(at) = parts.iter().rposition(|part| part.eq_ignore_ascii_case("domains")) {
            if at + 3 == parts.len() && parts[at + 2].eq_ignore_ascii_case("httpdocs") {
                note_content(&mut content, &parts[at + 1], 4, base.join(path));
            }
        }
    }
    let mut domain_names: Vec<String> = info.domains.clone();
    for name in content.keys() {
        if !domain_names.contains(name) {
            domain_names.push(name.clone());
        }
    }
    domain_names.sort();
    domain_names.dedup();
    for name in &domain_names {
        let path = content.get(name).map(|(_, path)| path.clone());
        let entry = domain_entry(raw, name, "domain", path.as_deref());
        if entry.docroot_path.is_none() {
            manifest.notes.push(format!("{name}: site içeriği yedekte bulunamadı."));
        }
        manifest.domains.push(entry);
    }

    // Veritabanları: XML'dekiler ve dökümü bulunanlar.
    let mut db_names: Vec<String> = info.databases.iter().map(|db| db.name.clone()).collect();
    for name in dumps.keys() {
        if !db_names.contains(name) {
            db_names.push(name.clone());
        }
    }
    db_names.sort();
    for name in db_names {
        let known = info.databases.iter().find(|db| db.name == name);
        let dump = dumps.get(&name).and_then(|path| relative(raw, path));
        if dump.is_none() {
            manifest.notes.push(format!("{name}: veritabanı dökümü yedekte bulunamadı."));
        }
        manifest.databases.push(ManifestDatabase {
            engine: known.map_or("mysql", |db| db.engine.as_str()).to_string(),
            users: known.map(|db| db.users.clone()).unwrap_or_default(),
            domain: known.and_then(|db| db.domain.clone()),
            dump_path: dump,
            name,
        });
    }
    manifest.mailboxes = info.mailboxes;
    manifest.mailboxes.sort();
    manifest.mailboxes.truncate(2000);
    if info.domains.is_empty() && content.is_empty() {
        manifest.notes.push("Yedekte alan adı ya da site içeriği bulunamadı.".into());
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::super::manifest::{parse_backup, testing::*};
    use super::*;
    use std::fs;

    const INFO: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Plesk -->
<migration-dump content-included-type="full">
  <dump-info><dump-format>panel</dump-format></dump-info>
  <domain name="Ornek.com" guid="x">
    <phosting>
      <sapp-installed><sapp-spec name="WordPress"/></sapp-installed>
    </phosting>
    <databases>
      <database name="ornek_wp" type="mysql">
        <db-server type="mysql"><host>localhost</host></db-server>
        <dbuser name="ornek_user" />
      </database>
      <database name="ornek_pg" type="postgresql"/>
      <database name="ornek_mssql" type="mssql"/>
    </databases>
    <mailsystem>
      <mailuser name="info"/>
      <mailname name="satis@ornek.com"/>
    </mailsystem>
  </domain>
  <domain name="ikinci.net"/>
</migration-dump>"#;

    #[test]
    fn xml_names_and_databases_are_extracted() {
        let mut info = Info::default();
        read_info(INFO, &mut info);
        assert_eq!(info.domains, ["ornek.com", "ikinci.net"]);
        let names: Vec<&str> = info.databases.iter().map(|db| db.name.as_str()).collect();
        assert_eq!(names, ["ornek_wp", "ornek_pg"]);
        assert_eq!(info.databases[0].users, ["ornek_user"]);
        assert_eq!(info.databases[0].domain.as_deref(), Some("ornek.com"));
        assert_eq!(info.databases[1].engine, "postgresql");
        assert_eq!(info.mailboxes, ["info@ornek.com", "satis@ornek.com"]);
    }

    #[test]
    fn plesk_backup_is_recognised_and_parsed() {
        let raw = fresh_dir("plesk");
        put(&raw, "backup_info_2610121200.xml", INFO);
        put(&raw, "domains/ornek.com/backup_user-data_2610121200.tgz", "sahte");
        put(&raw, "domains/ornek.com/backup_httpdocs_2610121200.tgz", "sahte2");
        put(&raw, "domains/ornek.com/databases/ornek_wp/backup_sqldump_2610121200.tgz", "dokum");
        put(&raw, "domains/ikinci.net/httpdocs/index.html", "x");
        put(&raw, "domains/ucuncu.org/backup_user-data_1.tgz", "sahte");
        put(&raw, "databases/ornek_pg/backup_sqldump_1.sql", "pg");
        let manifest = parse_backup(&raw).unwrap();
        assert_eq!(manifest.format, "plesk");
        let names: Vec<&str> = manifest.domains.iter().map(|domain| domain.name.as_str()).collect();
        assert_eq!(names, ["ikinci.net", "ornek.com", "ucuncu.org"]);
        let ornek = &manifest.domains[1];
        // backup_httpdocs, backup_user-data'ya tercih edilir.
        assert_eq!(ornek.docroot_path.as_deref(), Some("domains/ornek.com/backup_httpdocs_2610121200.tgz"));
        assert_eq!(manifest.domains[0].docroot_path.as_deref(), Some("domains/ikinci.net/httpdocs"));
        let wp = manifest.databases.iter().find(|db| db.name == "ornek_wp").unwrap();
        assert_eq!(wp.dump_path.as_deref(), Some("domains/ornek.com/databases/ornek_wp/backup_sqldump_2610121200.tgz"));
        assert_eq!(wp.domain.as_deref(), Some("ornek.com"));
        assert_eq!(manifest.databases.iter().find(|db| db.name == "ornek_pg").unwrap().engine, "postgresql");
        assert_eq!(manifest.mailboxes.len(), 2);
        fs::remove_dir_all(&raw).unwrap();
    }

    #[test]
    fn missing_content_is_reported() {
        let raw = fresh_dir("plesk-missing");
        put(&raw, "plesk-backup/backup_info_1.xml", INFO);
        let manifest = parse_backup(&raw).unwrap();
        assert_eq!(manifest.format, "plesk");
        assert!(manifest.domains.iter().all(|domain| domain.docroot_path.is_none()));
        assert!(manifest.notes.iter().any(|note| note.contains("ornek.com") && note.contains("bulunamadı")));
        assert!(manifest.notes.iter().any(|note| note.contains("ornek_wp") && note.contains("bulunamadı")));
        fs::remove_dir_all(&raw).unwrap();
    }
}
