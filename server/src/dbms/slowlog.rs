//! MariaDB yavaş sorgu günlüğü: açma/kapama, eşik ayarı ve günlüğün son kayıtlarının okunması.
//!
//! Ayar iki yerde tutulur: `SET GLOBAL` ile anında uygulanır, yeniden başlatmaya dayanması için
//! panelin yazdığı küçük bir yapılandırma dosyasına da yazılır. Dosyanın içeriği yalnızca beyaz
//! listedeki tam sayıdan ve sabit yollardan üretilir; kullanıcı metni hiçbir yere girmez.
//!
//! Günlük başka hesapların sorgularını içerir; bu yüzden uçlar yalnızca yöneticiye açıktır.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;

use super::mariadb;
use crate::error::{AppError, AppResult};

/// Seçilebilen eşikler (saniye).
pub const THRESHOLDS: [u32; 4] = [1, 2, 5, 10];
/// Panelin yönettiği yapılandırma dosyası.
pub const CONFIG_PATH: &str = "/etc/mysql/mariadb.conf.d/99-pixpanel-slow.cnf";
const CONFIG_DIR: &str = "/etc/mysql/mariadb.conf.d";
/// Panelin yavaş sorgu günlüğü için seçtiği dosya.
pub const LOG_PATH: &str = "/var/log/mysql/mariadb-slow.log";
const LOG_DIR: &str = "/var/log/mysql";
/// Günlüğün sonundan okunan en çok bayt; dev dosyalar belleğe alınmaz.
const READ_WINDOW: u64 = 4 * 1024 * 1024;
/// Bir kaydın sorgu metninden saklanan en çok karakter.
const MAX_QUERY_CHARS: usize = 2000;

#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    pub enabled: bool,
    /// Eşik (saniye).
    pub threshold: f64,
    /// Sunucunun yazdığı günlük dosyası.
    pub file: String,
    /// `log_output` içinde `FILE` var mı (yoksa günlük tabloya yazılır, dosyadan okunamaz).
    pub file_output: bool,
    /// Ayar yeniden başlatmaya dayanıyor mu (panelin yapılandırma dosyası var mı).
    pub persistent: bool,
}

/// Sunucunun o anki yavaş sorgu ayarları.
pub async fn settings() -> AppResult<Settings> {
    let output = mariadb::query(
        "SHOW GLOBAL VARIABLES WHERE Variable_name IN ('slow_query_log', 'long_query_time', 'slow_query_log_file', 'log_output', 'datadir');\n",
    )
    .await?;
    let values: HashMap<&str, &str> = output.lines().filter_map(|line| line.split_once('\t')).collect();
    let get = |name: &str| values.get(name).copied().unwrap_or("").trim().to_string();
    Ok(Settings {
        enabled: matches!(get("slow_query_log").to_ascii_uppercase().as_str(), "ON" | "1"),
        threshold: get("long_query_time").parse().unwrap_or(10.0),
        // Göreli dosya adı veri klasörüne göredir (ör. `sunucu-slow.log`).
        file: match get("slow_query_log_file") {
            file if file.is_empty() || file.starts_with('/') => file,
            file => Path::new(&get("datadir")).join(file).to_string_lossy().into_owned(),
        },
        file_output: get("log_output").to_ascii_uppercase().contains("FILE"),
        persistent: Path::new(CONFIG_PATH).is_file(),
    })
}

/// Yapılandırma dosyasının içeriği. `threshold` beyaz listedendir.
fn render_config(threshold: u32, file: Option<&str>) -> String {
    let mut text = String::from(
        "# PixPanel tarafından yönetilir (Veritabanları > Yavaş sorgular); elle değiştirmeyin.\n[mysqld]\nslow_query_log = 1\n",
    );
    text.push_str(&format!("long_query_time = {threshold}\n"));
    if let Some(file) = file {
        text.push_str(&format!("slow_query_log_file = {file}\n"));
    }
    text
}

/// Yavaş sorgu günlüğünü açar ya da kapatır. Açarken önce sunucuya anında uygulanır, ardından
/// kalıcı dosya yazılır; dosya yazılamazsa önceki ayar geri konur.
pub async fn apply(enabled: bool, threshold: u32) -> AppResult<()> {
    if !THRESHOLDS.contains(&threshold) {
        return Err(AppError::bad("Eşik 1, 2, 5 ya da 10 saniye olmalı"));
    }
    let before = settings().await?;
    if !enabled {
        mariadb::query("SET GLOBAL slow_query_log = 0;\n").await?;
        match std::fs::remove_file(CONFIG_PATH) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(AppError::internal(format!("yapılandırma dosyası silinemedi: {err}"))),
        }
        return Ok(());
    }

    let file = Path::new(LOG_DIR).is_dir().then_some(LOG_PATH);
    let mut script = String::new();
    if let Some(file) = file {
        script.push_str(&format!("SET GLOBAL slow_query_log_file = {};\n", mariadb::quote(file)));
    }
    if !before.file_output {
        // Günlük yalnızca tabloya yazılıyorsa dosya çıktısı eklenir (tablo çıktısı korunur).
        script.push_str("SET GLOBAL log_output = 'FILE,TABLE';\n");
    }
    script.push_str(&format!("SET GLOBAL long_query_time = {threshold};\nSET GLOBAL slow_query_log = 1;\n"));
    mariadb::query(&script).await?;

    if Path::new(CONFIG_DIR).is_dir() {
        if let Err(err) = crate::hosting::atomic_write(Path::new(CONFIG_PATH), render_config(threshold, file).as_bytes(), 0o644) {
            let revert = format!(
                "SET GLOBAL slow_query_log = {};\nSET GLOBAL long_query_time = {};\n",
                u8::from(before.enabled),
                before.threshold
            );
            let _ = mariadb::query(&revert).await;
            return Err(err);
        }
    }
    Ok(())
}

/// Günlükteki tek kayıt.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Entry {
    /// Sorgunun başladığı an (Unix saniyesi); günlükte yoksa 0.
    pub time: i64,
    pub query_time: f64,
    pub lock_time: f64,
    pub rows_sent: u64,
    pub rows_examined: u64,
    pub user: String,
    pub host: String,
    pub database: String,
    pub query: String,
}

/// `Alan:` biçimli bir günlük satırında alanın değerini bulur (değer yoksa boş döner).
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == name {
            return Some(tokens.next().filter(|next| !next.ends_with(':')).unwrap_or(""));
        }
    }
    None
}

/// Sunucunun günlüğe yazdığı başlık satırları (açılışta ve günlük yenilenince); sorgu metnine karışmasın.
fn is_banner(line: &str) -> bool {
    (line.contains(", Version: ") && line.contains("started with")) || line.starts_with("Tcp port:") || (line.starts_with("Time ") && line.contains("Command"))
}

fn finish(entries: &mut Vec<Entry>, current: &mut Option<Entry>, body: &mut Vec<&str>) {
    if let Some(mut entry) = current.take() {
        let query = body.join("\n");
        let query = query.trim();
        entry.query = if query.chars().count() > MAX_QUERY_CHARS {
            let cut: String = query.chars().take(MAX_QUERY_CHARS).collect();
            format!("{cut}…")
        } else {
            query.to_string()
        };
        entries.push(entry);
    }
    body.clear();
}

/// Günlük metnini kayıtlara ayırır (eskiden yeniye). İlk kaydın başı kesilmişse o kayıt atlanır.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    let mut body: Vec<&str> = Vec::new();
    for line in text.lines() {
        // `# Time:` satırı bir önceki kaydı bitirir (zamanın kendisi `SET timestamp` satırından okunur).
        if is_banner(line) || line.starts_with("# Time:") {
            finish(&mut entries, &mut current, &mut body);
            continue;
        }
        if let Some(rest) = line.strip_prefix("# User@Host:") {
            finish(&mut entries, &mut current, &mut body);
            // `root[root] @ localhost []`
            let (user, host) = rest.split_once('@').unwrap_or((rest, ""));
            current = Some(Entry {
                user: user.split('[').next().unwrap_or("").trim().to_string(),
                host: host.trim().to_string(),
                ..Entry::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else { continue };
        // Üstbilgi satırları sorgu metninden önce gelir; metin başladıktan sonra `#` ile başlayan
        // satırlar sorgunun kendi yorumudur.
        if body.is_empty() {
            if line.starts_with("# ") || line == "#" {
                if let Some(schema) = field(line, "Schema:") {
                    entry.database = schema.to_string();
                }
                if let Some(value) = field(line, "Query_time:") {
                    entry.query_time = value.parse().unwrap_or(0.0);
                    entry.lock_time = field(line, "Lock_time:").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    entry.rows_sent = field(line, "Rows_sent:").and_then(|v| v.parse().ok()).unwrap_or(0);
                    entry.rows_examined = field(line, "Rows_examined:").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("SET timestamp=") {
                entry.time = value.trim_end_matches(';').trim().parse().unwrap_or(0);
                continue;
            }
            if let Some(value) = line.strip_prefix("use ") {
                if entry.database.is_empty() {
                    entry.database = value.trim_end_matches(';').trim().trim_matches('`').to_string();
                }
                continue;
            }
        }
        body.push(line);
    }
    finish(&mut entries, &mut current, &mut body);
    entries
}

fn read_tail(path: &str, limit: usize) -> AppResult<(u64, Vec<Entry>)> {
    let path = Path::new(path);
    // Yalnızca sıradan, mutlak yollu dosya okunur (simge bağı izlenmez).
    let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok((0, Vec::new())) };
    if !path.is_absolute() || !meta.is_file() {
        return Ok((0, Vec::new()));
    }
    let mut file = std::fs::File::open(path)?;
    let size = meta.len();
    if size > READ_WINDOW {
        file.seek(SeekFrom::Start(size - READ_WINDOW))?;
    }
    let mut bytes = Vec::new();
    file.take(READ_WINDOW).read_to_end(&mut bytes)?;
    let mut entries = parse(&String::from_utf8_lossy(&bytes));
    entries.reverse();
    entries.truncate(limit);
    Ok((size, entries))
}

/// Günlüğün son `limit` kaydı (yeniden eskiye) ve dosyanın boyutu.
pub async fn recent(path: &str, limit: usize) -> AppResult<(u64, Vec<Entry>)> {
    let path = path.to_string();
    tokio::task::spawn_blocking(move || read_tail(&path, limit))
        .await
        .map_err(|err| AppError::internal(format!("günlük okunamadı: {err}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
/usr/sbin/mariadbd, Version: 11.8.2-MariaDB-deb13 started with:
Tcp port: 3306  Unix socket: /run/mysqld/mysqld.sock
Time                 Id Command    Argument
# Time: 261112 12:00:00
# User@Host: root[root] @ localhost []
# Thread_id: 5  Schema: musteri_blog  QC_hit: No
# Query_time: 2.000123  Lock_time: 0.000010  Rows_sent: 1  Rows_examined: 4200
# Rows_affected: 0  Bytes_sent: 70
SET timestamp=1794484800;
SELECT SLEEP(2);
# Time: 261112 12:00:05
# User@Host: app[app] @ 10.0.0.5 [10.0.0.5]
# Thread_id: 6  Schema:   QC_hit: No
# Query_time: 5.500000  Lock_time: 0.000000  Rows_sent: 0  Rows_examined: 0
use test;
SET timestamp=1794484805;
SELECT *
# bu bir sorgu yorumu
FROM t;
";

    #[test]
    fn entries_are_parsed() {
        let entries = parse(SAMPLE);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].user, "root");
        assert_eq!(entries[0].host, "localhost []");
        assert_eq!(entries[0].database, "musteri_blog");
        assert_eq!(entries[0].time, 1_794_484_800);
        assert_eq!(entries[0].rows_examined, 4200);
        assert_eq!(entries[0].query, "SELECT SLEEP(2);");
        assert!((entries[0].query_time - 2.000123).abs() < 1e-9);
        assert_eq!(entries[1].user, "app");
        assert_eq!(entries[1].database, "test");
        assert_eq!(entries[1].query, "SELECT *\n# bu bir sorgu yorumu\nFROM t;");
    }

    #[test]
    fn truncated_head_and_banners_are_skipped() {
        let text = format!("lf 0.1\nSELECT 1;\n{SAMPLE}");
        assert_eq!(parse(&text).len(), 2);
        assert!(parse("").is_empty());
    }

    #[test]
    fn long_queries_are_shortened() {
        let text = format!("# User@Host: a[a] @ localhost []\n# Query_time: 1.0  Lock_time: 0.0  Rows_sent: 0  Rows_examined: 0\nSELECT '{}';\n", "x".repeat(5000));
        let entries = parse(&text);
        assert_eq!(entries[0].query.chars().count(), MAX_QUERY_CHARS + 1);
        assert!(entries[0].query.ends_with('…'));
    }

    #[test]
    fn config_contains_only_whitelisted_values() {
        let text = render_config(5, Some(LOG_PATH));
        assert!(text.contains("long_query_time = 5\n"));
        assert!(text.contains(&format!("slow_query_log_file = {LOG_PATH}\n")));
        assert!(!render_config(1, None).contains("slow_query_log_file"));
    }
}
