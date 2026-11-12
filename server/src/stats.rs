//! Site istatistikleri: nginx erişim günlüklerinden site başına günlük özet çıkarır
//! (istek, sayfa görüntüleme, tekil ziyaretçi, aktarılan veri, durum kodları, botlar,
//! en çok istenen sayfalar). Özetler veritabanında tutulur; ham günlük saklanmaz.
//!
//! Toplayıcı her günlüğün neresinde kaldığını (dosya kimliği ve konum) hatırlar; günlük
//! döndürüldüğünde eski dosyanın kalanını okuyup yenisine baştan başlar, böylece hiçbir
//! satır iki kez sayılmaz ya da atlanmaz.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::{now, Db};
use crate::error::AppResult;
use crate::hosting::nginx;
use crate::hosting::sites::Site;
use crate::state::AppState;

/// Panelin kendi site yoklamalarının kimliği; bu istekler istatistiğe sayılmaz.
pub const PROBE_USER_AGENT: &str = "PixPanel-Health/1";
/// Bir turda tek bir günlükten okunan en çok veri; çok büyük günlükler birkaç turda işlenir.
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;
/// Günlük özetler bu kadar gün saklanır.
const DAILY_RETENTION_DAYS: i64 = 400;
/// Ziyaretçi özetleri ve sayfa dökümü bu kadar gün saklanır.
const DETAIL_RETENTION_DAYS: i64 = 35;
/// Bir sitenin bir günü için tutulan farklı sayfa sayısı; fazlası "(diğer)" altında toplanır.
const MAX_PATHS_PER_DAY: usize = 300;
const OTHER_PATHS: &str = "(diğer)";
const SALT_KEY: &str = "stats_salt";

/// Sayfa sayılmayan dosya uzantıları (stil, betik, görsel, yazı tipi, medya).
const ASSET_EXTENSIONS: &[&str] = &[
    "css", "js", "mjs", "map", "jpg", "jpeg", "png", "gif", "webp", "avif", "svg", "ico", "woff", "woff2", "ttf", "otf", "eot", "mp4", "webm", "mp3", "pdf", "zip", "xml",
    "txt", "json",
];

const BOT_MARKERS: &[&str] = &[
    "bot", "crawl", "spider", "slurp", "curl/", "wget/", "python-requests", "python-urllib", "go-http-client", "monitor", "uptime", "headless", "scrapy", "httpclient",
    "facebookexternalhit", "preview", "lighthouse", "pingdom", "semrush", "ahrefs",
];

/// Günlükten çözülmüş tek bir istek.
#[derive(Debug, PartialEq)]
struct Hit<'a> {
    ip: &'a str,
    /// `YYYYMMDD` biçiminde gün (günlüğün kendi saat dilimiyle).
    day: i64,
    method: &'a str,
    path: &'a str,
    status: u16,
    bytes: u64,
    user_agent: &'a str,
}

fn month_number(name: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    MONTHS.iter().position(|month| *month == name).map(|index| index as i64 + 1)
}

/// nginx'in "combined" biçimindeki bir satırı çözer; biçime uymayan satır için `None`.
fn parse_line(line: &str) -> Option<Hit<'_>> {
    let (ip, rest) = line.split_once(' ')?;
    let rest = &rest[rest.find('[')? + 1..];
    let (stamp, rest) = rest.split_once(']')?;
    // 11/Nov/2026:07:45:38 +0000
    let mut date = stamp.split(['/', ':']);
    let (day, month, year) = (date.next()?.parse::<i64>().ok()?, month_number(date.next()?)?, date.next()?.parse::<i64>().ok()?);
    let rest = &rest[rest.find('"')? + 1..];
    let (request, rest) = rest.split_once('"')?;
    let mut request_parts = request.split(' ');
    let (method, target) = (request_parts.next()?, request_parts.next().unwrap_or(""));
    let mut numbers = rest.trim_start().split(' ');
    let status = numbers.next()?.parse::<u16>().ok()?;
    let bytes = numbers.next()?.parse::<u64>().unwrap_or(0);
    // Sonda: "yönlendiren" "tarayıcı"
    let user_agent = rest.rsplit('"').nth(1).unwrap_or("");
    let path = target.split(['?', '#']).next().unwrap_or("");
    Some(Hit { ip, day: year * 10000 + month * 100 + day, method, path, status, bytes, user_agent })
}

fn is_bot(user_agent: &str) -> bool {
    if user_agent.is_empty() || user_agent == "-" {
        return true;
    }
    let lower = user_agent.to_ascii_lowercase();
    BOT_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// İstek bir sayfa görüntülemesi mi? Başarılı (ya da yönlendiren) GET istekleri, dosya indirmeleri hariç.
fn is_page(hit: &Hit) -> bool {
    if hit.method != "GET" || !(200..400).contains(&hit.status) {
        return false;
    }
    let name = hit.path.rsplit('/').next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((_, extension)) => !ASSET_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()),
        None => true,
    }
}

/// Ziyaretçi adresinin geri döndürülemeyen özeti. Adresin kendisi saklanmaz; tuz her
/// kurulumda farklıdır, böylece özetlerden adres tahmin edilemez.
fn visitor_hash(salt: &str, ip: &str) -> i64 {
    let digest = Sha256::digest(format!("{salt}|{ip}").as_bytes());
    i64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]))
}

/// Bir günün birikmiş sayıları.
#[derive(Debug, Default, Clone, PartialEq)]
struct DayTotals {
    requests: i64,
    pages: i64,
    bytes: i64,
    bots: i64,
    s2xx: i64,
    s3xx: i64,
    s4xx: i64,
    s5xx: i64,
    visitors: HashSet<i64>,
    paths: HashMap<String, i64>,
}

/// Günlük metnini günlere göre toplar. Panelin kendi yoklamaları atlanır.
fn aggregate(text: &str, salt: &str) -> HashMap<i64, DayTotals> {
    let mut days: HashMap<i64, DayTotals> = HashMap::new();
    for line in text.lines() {
        let Some(hit) = parse_line(line) else { continue };
        if hit.user_agent == PROBE_USER_AGENT {
            continue;
        }
        let totals = days.entry(hit.day).or_default();
        totals.requests += 1;
        totals.bytes += hit.bytes as i64;
        match hit.status {
            200..=299 => totals.s2xx += 1,
            300..=399 => totals.s3xx += 1,
            400..=499 => totals.s4xx += 1,
            500..=599 => totals.s5xx += 1,
            _ => {}
        }
        if is_bot(hit.user_agent) {
            totals.bots += 1;
            continue;
        }
        totals.visitors.insert(visitor_hash(salt, hit.ip));
        if is_page(&hit) {
            totals.pages += 1;
            let path: String = hit.path.chars().take(200).collect();
            *totals.paths.entry(if path.is_empty() { "/".into() } else { path }).or_insert(0) += 1;
        }
    }
    days
}

#[cfg(unix)]
fn inode_of(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino() as i64
}

#[cfg(not(unix))]
fn inode_of(_meta: &std::fs::Metadata) -> i64 {
    0
}

/// Dosyayı `offset`ten okur; yalnızca tamamlanmış satırları döner ve yeni konumu verir.
fn read_from(path: &Path, offset: u64) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = Vec::new();
    file.take(MAX_READ_BYTES).read_to_end(&mut buffer)?;
    // Yazılmakta olan son satır bir sonraki tura bırakılır.
    let complete = buffer.iter().rposition(|byte| *byte == b'\n').map(|index| index + 1).unwrap_or(0);
    buffer.truncate(complete);
    Ok((String::from_utf8_lossy(&buffer).into_owned(), offset + complete as u64))
}

fn salt(db: &Db) -> AppResult<String> {
    if let Some(salt) = db.setting(SALT_KEY)? {
        return Ok(salt);
    }
    let salt = crate::auth::random_token(16);
    db.set_setting(SALT_KEY, &salt)?;
    Ok(salt)
}

/// Bir sitenin günlüğündeki yeni satırları okur: (metin, yeni dosya kimliği, yeni konum).
fn read_new_lines(db: &Db, site: &Site) -> AppResult<Option<(String, i64, u64)>> {
    let path_text = nginx::access_log_path(&site.domain);
    let path = Path::new(&path_text);
    let Ok(meta) = std::fs::metadata(path) else { return Ok(None) };
    let inode = inode_of(&meta);
    let state: Option<(i64, i64)> =
        db.call(|conn| conn.query_row("SELECT inode, offset FROM site_log_state WHERE site_id = ?1", params![site.id], |row| Ok((row.get(0)?, row.get(1)?))).optional())?;

    let mut text = String::new();
    let mut offset = 0u64;
    match state {
        Some((old_inode, old_offset)) if old_inode == inode && (old_offset as u64) <= meta.len() => offset = old_offset as u64,
        Some((old_inode, old_offset)) => {
            // Günlük döndürülmüş: eski dosyanın okunmamış kalanı `.1` adıyla durur.
            let rotated_text = format!("{path_text}.1");
            let rotated = Path::new(&rotated_text);
            if std::fs::metadata(rotated).map(|meta| inode_of(&meta) == old_inode).unwrap_or(false) {
                if let Ok((tail, _)) = read_from(rotated, old_offset as u64) {
                    text.push_str(&tail);
                }
            }
        }
        None => {}
    }
    let (fresh, new_offset) = read_from(path, offset)?;
    text.push_str(&fresh);
    Ok(Some((text, inode, new_offset)))
}

/// Toplayıcı ile "şimdi tazele" istekleri aynı günlüğü aynı anda okursa satırlar iki kez
/// sayılırdı; okuma ve yazma tek tek yürür.
static COLLECT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Bütün sitelerin günlüklerindeki yeni satırları işler.
pub fn collect(state: &AppState) -> AppResult<()> {
    for site in crate::backup::all_sites(&state.db)? {
        collect_site(&state.db, &site)?;
    }
    prune(&state.db)
}

/// Tek bir sitenin günlüğündeki yeni satırları işler.
pub fn collect_site(db: &Db, site: &Site) -> AppResult<()> {
    let _guard = COLLECT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let salt = salt(db)?;
    let Some((text, inode, offset)) = read_new_lines(db, site)? else { return Ok(()) };
    let days = aggregate(&text, &salt);
    db.call(|conn| {
        let tx = conn.transaction()?;
        for (day, totals) in &days {
            for hash in &totals.visitors {
                tx.execute("INSERT OR IGNORE INTO site_stats_visitors (site_id, day, hash) VALUES (?1, ?2, ?3)", params![site.id, day, hash])?;
            }
            for (path, hits) in &totals.paths {
                let known: i64 = tx.query_row("SELECT COUNT(*) FROM site_stats_paths WHERE site_id = ?1 AND day = ?2", params![site.id, day], |row| row.get(0))?;
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM site_stats_paths WHERE site_id = ?1 AND day = ?2 AND path = ?3)", params![site.id, day, path], |row| row.get(0))?;
                let path = if exists || (known as usize) < MAX_PATHS_PER_DAY { path.as_str() } else { OTHER_PATHS };
                tx.execute(
                    "INSERT INTO site_stats_paths (site_id, day, path, hits) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(site_id, day, path) DO UPDATE SET hits = hits + excluded.hits",
                    params![site.id, day, path, hits],
                )?;
            }
            let visitors: i64 = tx.query_row("SELECT COUNT(*) FROM site_stats_visitors WHERE site_id = ?1 AND day = ?2", params![site.id, day], |row| row.get(0))?;
            tx.execute(
                "INSERT INTO site_stats_daily (site_id, day, requests, pages, bytes, visitors, bots, s2xx, s3xx, s4xx, s5xx)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(site_id, day) DO UPDATE SET
                    requests = requests + excluded.requests, pages = pages + excluded.pages, bytes = bytes + excluded.bytes,
                    visitors = excluded.visitors, bots = bots + excluded.bots,
                    s2xx = s2xx + excluded.s2xx, s3xx = s3xx + excluded.s3xx, s4xx = s4xx + excluded.s4xx, s5xx = s5xx + excluded.s5xx",
                params![site.id, day, totals.requests, totals.pages, totals.bytes, visitors, totals.bots, totals.s2xx, totals.s3xx, totals.s4xx, totals.s5xx],
            )?;
        }
        tx.execute(
            "INSERT INTO site_log_state (site_id, inode, offset) VALUES (?1, ?2, ?3)
             ON CONFLICT(site_id) DO UPDATE SET inode = excluded.inode, offset = excluded.offset",
            params![site.id, inode, offset as i64],
        )?;
        tx.commit()
    })?;
    Ok(())
}

/// Unix anını `YYYYMMDD` gün anahtarına çevirir (UTC); saklama sınırları için yeterli kesinlikte.
fn day_key(unix: i64) -> i64 {
    let days = unix.div_euclid(86400);
    // Howard Hinnant'ın takvim algoritması (civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    year * 10000 + month * 100 + day
}

fn prune(db: &Db) -> AppResult<()> {
    let current = now();
    db.call(|conn| {
        conn.execute("DELETE FROM site_stats_daily WHERE day < ?1", params![day_key(current - DAILY_RETENTION_DAYS * 86400)])?;
        conn.execute("DELETE FROM site_stats_visitors WHERE day < ?1", params![day_key(current - DETAIL_RETENTION_DAYS * 86400)])?;
        conn.execute("DELETE FROM site_stats_paths WHERE day < ?1", params![day_key(current - DETAIL_RETENTION_DAYS * 86400)])
    })?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct DayStats {
    /// `YYYYMMDD`
    pub day: i64,
    pub requests: i64,
    pub pages: i64,
    pub visitors: i64,
    pub bytes: i64,
    pub bots: i64,
    pub s2xx: i64,
    pub s3xx: i64,
    pub s4xx: i64,
    pub s5xx: i64,
}

#[derive(Debug, Serialize)]
pub struct PathStats {
    pub path: String,
    pub hits: i64,
}

#[derive(Debug, Serialize)]
pub struct SiteStats {
    /// İstenen dönemin günleri, eskiden yeniye (veri olmayan günler yoktur).
    pub days: Vec<DayStats>,
    /// Dönemde en çok görüntülenen sayfalar (son 35 günle sınırlı).
    pub top_paths: Vec<PathStats>,
    /// İçinde bulunulan ayda aktarılan veri (bayt).
    pub month_bytes: i64,
}

pub fn site_stats(db: &Db, site_id: i64, days: i64) -> AppResult<SiteStats> {
    let current = now();
    let since = day_key(current - days * 86400);
    let month_start = day_key(current) / 100 * 100;
    db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT day, requests, pages, visitors, bytes, bots, s2xx, s3xx, s4xx, s5xx FROM site_stats_daily WHERE site_id = ?1 AND day > ?2 ORDER BY day",
        )?;
        let rows = stmt
            .query_map(params![site_id, since], |row| {
                Ok(DayStats {
                    day: row.get(0)?,
                    requests: row.get(1)?,
                    pages: row.get(2)?,
                    visitors: row.get(3)?,
                    bytes: row.get(4)?,
                    bots: row.get(5)?,
                    s2xx: row.get(6)?,
                    s3xx: row.get(7)?,
                    s4xx: row.get(8)?,
                    s5xx: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare("SELECT path, SUM(hits) AS total FROM site_stats_paths WHERE site_id = ?1 AND day > ?2 GROUP BY path ORDER BY total DESC LIMIT 15")?;
        let top_paths = stmt.query_map(params![site_id, since], |row| Ok(PathStats { path: row.get(0)?, hits: row.get(1)? }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let month_bytes: i64 = conn.query_row("SELECT COALESCE(SUM(bytes), 0) FROM site_stats_daily WHERE site_id = ?1 AND day > ?2", params![site_id, month_start], |row| row.get(0))?;
        Ok(SiteStats { days: rows, top_paths, month_bytes })
    })
}

/// Hesabın bütün sitelerinin içinde bulunulan ayda aktardığı veri (bayt).
/// Sitenin bu ayki trafiği (bayt).
pub fn site_month_bytes(db: &Db, site_id: i64) -> AppResult<i64> {
    let month_start = day_key(now()) / 100 * 100;
    Ok(db.call(|conn| conn.query_row("SELECT COALESCE(SUM(bytes), 0) FROM site_stats_daily WHERE site_id = ?1 AND day > ?2", params![site_id, month_start], |row| row.get(0)))?)
}

pub fn account_month_bytes(db: &Db, account_id: i64) -> AppResult<i64> {
    let month_start = day_key(now()) / 100 * 100;
    db.call(|conn| {
        conn.query_row(
            "SELECT COALESCE(SUM(d.bytes), 0) FROM site_stats_daily d JOIN sites s ON s.id = d.site_id WHERE s.user_id = ?1 AND d.day > ?2",
            params![account_id, month_start],
            |row| row.get(0),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36";

    fn line(ip: &str, request: &str, status: u16, bytes: u64, agent: &str) -> String {
        format!("{ip} - - [11/Nov/2026:07:45:38 +0300] \"{request}\" {status} {bytes} \"https://ornek.com/\" \"{agent}\"")
    }

    #[test]
    fn combined_line_is_parsed() {
        let text = line("203.0.113.7", "GET /blog/yazi?utm=1 HTTP/2.0", 200, 5120, BROWSER);
        let hit = parse_line(&text).unwrap();
        assert_eq!(hit, Hit { ip: "203.0.113.7", day: 20261111, method: "GET", path: "/blog/yazi", status: 200, bytes: 5120, user_agent: BROWSER });
    }

    #[test]
    fn malformed_lines_are_skipped() {
        for text in ["", "çöp", "1.2.3.4 - - [bozuk] \"GET / HTTP/1.1\" 200 1 \"-\" \"x\"", "1.2.3.4 - - [11/Nov/2026:07:45:38 +0000] \"GET / HTTP/1.1\" iki 1"] {
            assert!(parse_line(text).is_none(), "{text}");
        }
        // Tırnak içinde boş istek (bağlantı erken kapanmış): satır sayılır ama sayfa değildir.
        let empty = parse_line("1.2.3.4 - - [11/Nov/2026:07:45:38 +0000] \"\" 400 0 \"-\" \"-\"").unwrap();
        assert_eq!((empty.method, empty.path, empty.status), ("", "", 400));
    }

    #[test]
    fn pages_exclude_assets_errors_and_non_get() {
        let page = |request: &str, status: u16| is_page(&parse_line(&line("1.1.1.1", request, status, 1, BROWSER)).unwrap());
        assert!(page("GET / HTTP/1.1", 200));
        assert!(page("GET /urunler/kazak HTTP/1.1", 200));
        assert!(page("GET /index.php HTTP/1.1", 301));
        assert!(!page("GET /stil.css HTTP/1.1", 200));
        assert!(!page("GET /gorsel/FOTO.JPG HTTP/1.1", 200));
        assert!(!page("POST /giris HTTP/1.1", 200));
        assert!(!page("GET /yok HTTP/1.1", 404));
    }

    #[test]
    fn aggregation_counts_visitors_bots_and_statuses() {
        let text = [
            line("203.0.113.7", "GET / HTTP/1.1", 200, 1000, BROWSER),
            line("203.0.113.7", "GET /hakkinda HTTP/1.1", 200, 2000, BROWSER),
            line("203.0.113.8", "GET / HTTP/1.1", 200, 1000, BROWSER),
            line("203.0.113.8", "GET /stil.css HTTP/1.1", 200, 300, BROWSER),
            line("203.0.113.9", "GET /yok HTTP/1.1", 404, 150, BROWSER),
            line("66.249.66.1", "GET / HTTP/1.1", 200, 1000, "Mozilla/5.0 (compatible; Googlebot/2.1)"),
            line("127.0.0.1", "GET / HTTP/1.1", 200, 75, PROBE_USER_AGENT),
            "bozuk satır".to_string(),
        ]
        .join("\n");
        let days = aggregate(&text, "tuz");
        let totals = &days[&20261111];
        assert_eq!((totals.requests, totals.pages, totals.bots, totals.bytes), (6, 3, 1, 5450), "panelin yoklaması ve bozuk satır sayılmaz");
        assert_eq!(totals.visitors.len(), 3, "bot ziyaretçi sayılmaz");
        assert_eq!((totals.s2xx, totals.s4xx), (5, 1));
        assert_eq!(totals.paths.get("/"), Some(&2));
        assert_eq!(totals.paths.get("/hakkinda"), Some(&1));
        assert!(!totals.paths.contains_key("/stil.css"));
    }

    #[test]
    fn visitor_hash_depends_on_salt() {
        assert_eq!(visitor_hash("a", "203.0.113.7"), visitor_hash("a", "203.0.113.7"));
        assert_ne!(visitor_hash("a", "203.0.113.7"), visitor_hash("b", "203.0.113.7"));
        assert_ne!(visitor_hash("a", "203.0.113.7"), visitor_hash("a", "203.0.113.8"));
    }

    #[test]
    fn day_key_matches_calendar() {
        assert_eq!(day_key(0), 19700101);
        assert_eq!(day_key(1_794_357_000), 20261111);
        assert_eq!(day_key(951_782_400), 20000229, "artık yıl");
        assert_eq!(day_key(1_798_761_599), 20261231);
    }

    #[test]
    fn empty_user_agent_counts_as_bot() {
        assert!(is_bot("-") && is_bot("") && is_bot("curl/8.1") && !is_bot(BROWSER));
    }
}
