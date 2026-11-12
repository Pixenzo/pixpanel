//! Virüs taraması: ClamAV (isteğe bağlı bileşen) ve Rspamd'nin antivirüs modülüne bağlanması.
//!
//! Bileşen kurulunca (`setup`) imza veritabanı indirilir, `clamd` başlatılır ve Rspamd'ye
//! `local.d/antivirus.conf` yazılır: virüslü ileti SMTP aşamasında reddedilir. Rspamd'nin `_rspamd`
//! kullanıcısı `clamav` grubuna eklenir (soket izni). Bileşen kaldırılınca yapılandırma geri alınır
//! (`remove_config`).
//!
//! Karantina: virüs, Rspamd'de `reject` eylemiyle işaretlenir; "reddedileni karantinaya al" kipi
//! açıksa (`quarantine_on_reject`) virüslü ileti de puanla reddedilenler gibi bekletme kuyruğuna
//! düşer, kapalıysa SMTP'de reddedilir. Ayrı bir düzen gerekmez.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::MAIL_DIR;
use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::jobs::JobCtx;
use crate::sys::{exec, services};

pub const DAEMON_SERVICE: &str = "clamav-daemon.service";
pub const FRESHCLAM_SERVICE: &str = "clamav-freshclam.service";
/// Rspamd antivirüs modülünün panele ait yapılandırması.
const CONF_PATH: &str = "/etc/rspamd/local.d/antivirus.conf";
const RSPAMD_LOCAL_DIR: &str = "/etc/rspamd/local.d";
const CLAMD_CONF: &str = "/etc/clamav/clamd.conf";
const DEFAULT_SOCKET: &str = "/var/run/clamav/clamd.ctl";
const SIGNATURE_DIR: &str = "/var/lib/clamav";
const RSPAMD_USER: &str = "_rspamd";
const CLAM_GROUP: &str = "clamav";
/// Rspamd'nin virüs bulunca koyduğu simge.
pub const SYMBOL: &str = "CLAM_VIRUS";
/// Bunun altındaki bellekte ClamAV kurulmadan önce uyarılır (MB).
pub const MIN_MEMORY_MB: u64 = 2048;
/// "Son 7 günde yakalanan" penceresi.
const CATCH_WINDOW_SECS: i64 = 7 * 24 * 3600;

// ---- Saf yardımcılar (birim testli) ----

/// Rspamd `antivirus.conf`: ClamAV'ı unix soketiyle kullanır, virüste iletiyi reddeder.
pub fn render_config(socket: &str) -> String {
    format!(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         clamav {{\n\
         \x20 type = \"clamav\";\n\
         \x20 servers = \"{socket}\";\n\
         \x20 symbol = \"{SYMBOL}\";\n\
         \x20 action = \"reject\";\n\
         \x20 scan_mime_parts = true;\n\
         \x20 log_clean = false;\n\
         \x20 timeout = 20.0;\n\
         \x20 retransmits = 2;\n\
         }}\n"
    )
}

/// `clamd.conf` içindeki `LocalSocket` değeri.
pub fn parse_local_socket(clamd_conf: &str) -> Option<String> {
    clamd_conf
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let value = line.strip_prefix("LocalSocket")?;
            // `LocalSocketGroup` / `LocalSocketMode` ile karışmasın: ad ile değer arası boşluk olmalı.
            let value = value.strip_prefix(char::is_whitespace)?.trim();
            (value.starts_with('/') && !value.contains(char::is_whitespace)).then(|| value.to_string())
        })
}

/// `clamd --version` / `freshclam --version` çıktısı: `ClamAV 1.4.3/27800/Thu Nov 12 08:31:02 2026`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionInfo {
    pub engine: String,
    pub signature_version: Option<u32>,
    /// İmza veritabanının derlenme zamanı (Unix saniyesi).
    pub signatures_at: Option<i64>,
}

pub fn parse_version(output: &str) -> Option<VersionInfo> {
    let line = output.lines().map(str::trim).find(|line| line.starts_with("ClamAV "))?;
    let mut parts = line.splitn(3, '/');
    let engine = parts.next()?.trim_start_matches("ClamAV").trim().to_string();
    if engine.is_empty() {
        return None;
    }
    let signature_version = parts.next().and_then(|value| value.trim().parse().ok());
    let signatures_at = parts.next().and_then(|value| parse_build_time(value.trim()));
    Some(VersionInfo { engine, signature_version, signatures_at })
}

/// `Thu Nov 12 08:31:02 2026` (UTC) → Unix saniyesi.
fn parse_build_time(text: &str) -> Option<i64> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [_weekday, month, day, clock, year] = fields.as_slice() else { return None };
    let month = match *month {
        "Jan" => time::Month::January,
        "Feb" => time::Month::February,
        "Mar" => time::Month::March,
        "Apr" => time::Month::April,
        "May" => time::Month::May,
        "Jun" => time::Month::June,
        "Jul" => time::Month::July,
        "Aug" => time::Month::August,
        "Sep" => time::Month::September,
        "Oct" => time::Month::October,
        "Nov" => time::Month::November,
        "Dec" => time::Month::December,
        _ => return None,
    };
    let date = time::Date::from_calendar_date(year.parse().ok()?, month, day.parse().ok()?).ok()?;
    let mut clock = clock.split(':');
    let moment = time::Time::from_hms(clock.next()?.parse().ok()?, clock.next()?.parse().ok()?, clock.next()?.parse().ok()?).ok()?;
    Some(time::PrimitiveDateTime::new(date, moment).assume_utc().unix_timestamp())
}

/// `/proc/meminfo` içindeki toplam bellek (MB).
pub fn parse_meminfo_mb(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes / 1024)
}

pub fn total_memory_mb() -> Option<u64> {
    parse_meminfo_mb(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// Bellek ClamAV için azsa kurulumdan önce gösterilen uyarı.
pub fn memory_warning() -> Option<String> {
    low_memory_message(total_memory_mb()?)
}

fn low_memory_message(memory_mb: u64) -> Option<String> {
    (memory_mb < MIN_MEMORY_MB).then(|| {
        format!(
            "Bu sunucuda {memory_mb} MB bellek var; ClamAV imza veritabanını yüklerken yaklaşık 1,2 GB (en az 2 GB toplam bellek önerilir) ister. \
             Bellek yetmezse posta ya da siteler yavaşlayabilir, hizmetler bellek yetersizliğinden durdurulabilir."
        )
    })
}

/// Rspamd geçmişindeki (`/history` yanıtı) `since` sonrasında virüs simgesi taşıyan satırların sayısı.
pub fn count_virus_rows(history: &Value, since: i64) -> u32 {
    history["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["unix_time"].as_f64().is_some_and(|time| time as i64 >= since))
        .filter(|row| row["symbols"].get(SYMBOL).is_some())
        .count() as u32
}

/// Dizindeki dosya adlarından imza veritabanı var mı (`main`/`daily` `.cvd` ya da `.cld`).
fn has_signature_files(names: impl Iterator<Item = String>) -> bool {
    names.into_iter().any(|name| {
        ["main", "daily"].iter().any(|base| name == format!("{base}.cvd") || name == format!("{base}.cld"))
    })
}

fn signatures_present() -> bool {
    std::fs::read_dir(SIGNATURE_DIR).map(|entries| has_signature_files(entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()))).unwrap_or(false)
}

// ---- Durum ----

pub fn is_installed() -> bool {
    exec::binary_exists("clamd") && exec::binary_exists("freshclam")
}

/// Rspamd'ye virüs taraması yapılandırması yazılı mı?
pub fn is_configured() -> bool {
    Path::new(CONF_PATH).exists()
}

/// `clamd`'nin yerel soketi (`clamd.conf`'tan; okunamazsa Debian varsayılanı).
pub fn clamd_socket() -> String {
    std::fs::read_to_string(CLAMD_CONF).ok().and_then(|text| parse_local_socket(&text)).unwrap_or_else(|| DEFAULT_SOCKET.to_string())
}

#[derive(Debug, Serialize)]
pub struct AntivirusStatus {
    pub installed: bool,
    /// `clamd` çalışıyor mu?
    pub running: bool,
    /// İmza güncelleyicisi (`freshclam`) çalışıyor mu?
    pub updater_running: bool,
    /// Rspamd virüs taramasına bağlı mı (yapılandırma yazılı)?
    pub enabled: bool,
    pub engine_version: Option<String>,
    pub signature_version: Option<u32>,
    /// İmza veritabanının tarihi (Unix saniyesi).
    pub signatures_at: Option<i64>,
    /// Son 7 günde virüs nedeniyle yakalanan ileti sayısı (Rspamd geçmişinden; geçmiş sınırlıdır).
    pub caught_7d: Option<u32>,
    pub memory_mb: Option<u64>,
    pub memory_low: bool,
}

async fn read_version() -> Option<VersionInfo> {
    for program in ["clamd", "freshclam"] {
        if !exec::binary_exists(program) {
            continue;
        }
        if let Ok(output) = exec::run_with_timeout(program, &["--version"], Duration::from_secs(15)).await {
            if let Some(info) = parse_version(&output.stdout) {
                return Some(info);
            }
        }
    }
    None
}

/// Rspamd denetleyicisinin geçmişinden son 7 günde yakalanan virüslü ileti sayısı.
async fn caught_recently() -> Option<u32> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().ok()?;
    let history: Value = client.get("http://127.0.0.1:11334/history").send().await.ok()?.json().await.ok()?;
    Some(count_virus_rows(&history, crate::db::now() - CATCH_WINDOW_SECS))
}

pub async fn status() -> AntivirusStatus {
    let installed = is_installed();
    let version = if installed { read_version().await } else { None };
    let memory_mb = total_memory_mb();
    AntivirusStatus {
        installed,
        running: installed && services::is_active(DAEMON_SERVICE).await,
        updater_running: installed && services::is_active(FRESHCLAM_SERVICE).await,
        enabled: is_configured(),
        engine_version: version.as_ref().map(|info| info.engine.clone()),
        signature_version: version.as_ref().and_then(|info| info.signature_version),
        signatures_at: version.as_ref().and_then(|info| info.signatures_at),
        caught_7d: if installed && is_configured() { caught_recently().await } else { None },
        memory_mb,
        memory_low: memory_mb.is_some_and(|mb| mb < MIN_MEMORY_MB),
    }
}

// ---- Rspamd bağlantısı ----

/// `_rspamd` kullanıcısını `clamav` grubuna ekler (soket erişimi). Yeni eklendiyse `true` döner.
async fn ensure_group_membership() -> bool {
    if super::sysuser::lookup(RSPAMD_USER).is_none() || super::sysuser::group_id(CLAM_GROUP).is_err() {
        tracing::warn!("{RSPAMD_USER} kullanıcısı ya da {CLAM_GROUP} grubu bulunamadı; soket erişimi grup üyeliği olmadan denenecek");
        return false;
    }
    let member = exec::run("id", &["-nG", "--", RSPAMD_USER]).await.map(|out| out.stdout.split_whitespace().any(|group| group == CLAM_GROUP)).unwrap_or(false);
    if member {
        return false;
    }
    match exec::run_ok("usermod", &["-aG", CLAM_GROUP, "--", RSPAMD_USER]).await {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(error = %err, "{RSPAMD_USER} {CLAM_GROUP} grubuna eklenemedi");
            false
        }
    }
}

/// Rspamd yapılandırmasını yazar, `rspamadm configtest` ile sınar (hata olursa eski dosyayı geri koyar)
/// ve Rspamd'yi yeniden yükler. Rspamd kurulu değilse hiçbir şey yapmaz; tekrar çağrılabilir.
pub async fn apply_config() -> AppResult<()> {
    if !Path::new(RSPAMD_LOCAL_DIR).is_dir() || !is_installed() {
        return Ok(());
    }
    let group_added = ensure_group_membership().await;
    let content = render_config(&clamd_socket());
    let path = Path::new(CONF_PATH);
    let previous = std::fs::read(path).ok();
    let changed = previous.as_deref() != Some(content.as_bytes());
    if changed {
        atomic_write(path, content.as_bytes(), 0o644)?;
        let test = exec::run("rspamadm", &["configtest"]).await?;
        if !test.success() {
            match previous {
                Some(old) => atomic_write(path, &old, 0o644)?,
                None => {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(AppError::bad(format!("Rspamd virüs taraması ayarı geçersiz: {}", test.stdout.trim())));
        }
    }
    if changed || group_added {
        // Grup üyeliği yalnızca süreç yeniden başlayınca geçerli olur.
        let action = if group_added { "restart" } else { "reload-or-restart" };
        if services::is_active("rspamd.service").await {
            exec::run_ok("systemctl", &[action, "rspamd.service"]).await?;
        }
    }
    Ok(())
}

/// Bileşen kaldırılınca Rspamd'nin virüs taraması yapılandırmasını geri alır.
pub async fn remove_config() -> AppResult<()> {
    let path = Path::new(CONF_PATH);
    if path.exists() {
        std::fs::remove_file(path)?;
        if Path::new(RSPAMD_LOCAL_DIR).is_dir() && services::is_active("rspamd.service").await {
            let test = exec::run("rspamadm", &["configtest"]).await?;
            if test.success() {
                exec::run_ok("systemctl", &["reload-or-restart", "rspamd.service"]).await?;
            }
        }
    }
    // Grup üyeliği zararsızdır ama gereksiz yetki bırakılmaz (grup yoksa hata yok sayılır).
    let _ = exec::run("gpasswd", &["-d", RSPAMD_USER, CLAM_GROUP]).await;
    Ok(())
}

/// Posta sunucusu yapılandırılırken (`mail::setup`) ClamAV kuruluysa bağlantı korunur.
pub async fn sync_rspamd() {
    if is_installed() && is_configured() {
        if let Err(err) = apply_config().await {
            tracing::warn!(error = %err, "Rspamd virüs taraması yapılandırması yenilenemedi");
        }
    }
}

// ---- Kurulum sonrası ----

/// Paketler kurulduktan sonra: imza veritabanını hazırlar, `clamd`'yi başlatır, Rspamd'ye bağlar.
pub async fn setup(ctx: &JobCtx) -> AppResult<()> {
    if let Some(warning) = memory_warning() {
        ctx.log(format!("Uyarı: {warning}"));
    }
    ctx.log("İmza güncelleyicisi başlatılıyor");
    if let Err(err) = exec::run_ok("systemctl", &["enable", "--now", "--", FRESHCLAM_SERVICE]).await {
        ctx.log(format!("Uyarı: imza güncelleyicisi başlatılamadı: {err}"));
    }
    if !signatures_present() {
        ctx.log("İmza veritabanı indiriliyor (ilk indirme birkaç dakika sürebilir)");
        // Güncelleyici hizmeti aynı anda indiriyorsa bu çağrı hata verebilir; dosyalar beklenir.
        let _ = exec::run_with_timeout("freshclam", &["--quiet"], Duration::from_secs(420)).await;
        for _ in 0..30 {
            if signatures_present() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
    if !signatures_present() {
        return Err(AppError::internal(
            "İmza veritabanı indirilemedi (sunucunun internete erişimi var mı?). Bileşeni kaldırıp birkaç dakika sonra yeniden kurun ya da `freshclam` çıktısına bakın.",
        ));
    }
    ctx.log("Virüs tarayıcısı (clamd) başlatılıyor; imzaların belleğe yüklenmesi bir dakikaya yakın sürebilir");
    exec::run_ok("systemctl", &["enable", "--now", "--", DAEMON_SERVICE]).await?;
    for _ in 0..60 {
        if services::is_active(DAEMON_SERVICE).await && Path::new(&clamd_socket()).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    if !Path::new(&clamd_socket()).exists() {
        return Err(AppError::internal(format!("clamd soketi ({}) oluşmadı; `journalctl -u {DAEMON_SERVICE}` çıktısına bakın", clamd_socket())));
    }
    if Path::new(RSPAMD_LOCAL_DIR).is_dir() {
        ctx.log("Rspamd virüs taramasına bağlanıyor");
        apply_config().await?;
        ctx.log("Virüslü iletiler artık reddediliyor");
    } else {
        ctx.log(format!("Posta sunucusu kurulu değil; kurulunca Rspamd bu tarayıcıya otomatik bağlanır ({MAIL_DIR})"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_uses_socket_and_rejects() {
        let text = render_config("/run/clamav/clamd.ctl");
        assert!(text.contains("clamav {"));
        assert!(text.contains("type = \"clamav\";"));
        assert!(text.contains("servers = \"/run/clamav/clamd.ctl\";"));
        assert!(text.contains("action = \"reject\";"));
        assert!(text.contains("scan_mime_parts = true;"));
        assert!(text.contains("symbol = \"CLAM_VIRUS\";"));
    }

    #[test]
    fn socket_is_read_from_clamd_conf() {
        let conf = "# LocalSocket /yok\nLocalSocketGroup clamav\nLocalSocketMode 666\nLocalSocket /var/run/clamav/clamd.ctl\n";
        assert_eq!(parse_local_socket(conf).as_deref(), Some("/var/run/clamav/clamd.ctl"));
        assert_eq!(parse_local_socket("LocalSocketGroup clamav\n"), None);
        assert_eq!(parse_local_socket("LocalSocket relative/path\n"), None);
    }

    #[test]
    fn version_output_is_parsed() {
        let info = parse_version("ClamAV 1.4.3/27800/Thu Nov 12 08:31:02 2026\n").unwrap();
        assert_eq!(info.engine, "1.4.3");
        assert_eq!(info.signature_version, Some(27800));
        // 2026-11-12 08:31:02 UTC
        assert_eq!(info.signatures_at, Some(1_794_472_262));
        // Yalnızca motor sürümü bildiren çıktı.
        let bare = parse_version("ClamAV 1.0.7\n").unwrap();
        assert_eq!((bare.engine.as_str(), bare.signature_version, bare.signatures_at), ("1.0.7", None, None));
        assert!(parse_version("başka bir çıktı").is_none());
    }

    #[test]
    fn memory_is_parsed_and_warned() {
        assert_eq!(parse_meminfo_mb("MemTotal:        1011340 kB\nMemFree: 1 kB\n"), Some(987));
        assert!(low_memory_message(1024).unwrap().contains("1024 MB"));
        assert!(low_memory_message(2048).is_none());
        assert!(low_memory_message(8192).is_none());
    }

    #[test]
    fn virus_rows_in_window_are_counted() {
        let history = json!({"rows": [
            {"unix_time": 1000, "symbols": {"CLAM_VIRUS": {"score": 0}}},
            {"unix_time": 900, "symbols": {"CLAM_VIRUS": {}}},
            {"unix_time": 1100, "symbols": {"R_DKIM_ALLOW": {}}},
            {"unix_time": 1200, "symbols": {"CLAM_VIRUS": {}, "X": {}}},
        ]});
        assert_eq!(count_virus_rows(&history, 950), 2);
        assert_eq!(count_virus_rows(&history, 0), 3);
        assert_eq!(count_virus_rows(&json!({}), 0), 0);
    }

    #[test]
    fn signature_files_are_recognised() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>().into_iter();
        assert!(has_signature_files(names(&["freshclam.dat", "daily.cld"])));
        assert!(has_signature_files(names(&["main.cvd"])));
        assert!(!has_signature_files(names(&["freshclam.dat", "mirrors.dat", "daily.cvd.tmp"])));
    }
}
