//! Panelin kendini güncellemesi.
//!
//! Sürümler sabit adlı dosyalar olarak yayınlanır (`update_url` altında):
//! `VERSION`, `SHA256SUMS`, `SHA256SUMS.sig` ve `pixpanel-linux-<mimari>.tar.gz`.
//! Özet dosyası sürüm anahtarıyla (ed25519) imzalıdır; imza doğrulanmadan
//! hiçbir dosya kullanılmaz. Yeni ikili yerine konduktan sonra ayrı bir
//! bekçi süreci paneli yeniden başlatır ve panel ayağa kalkmazsa eski ikiliyi
//! ve veritabanını geri koyar.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::exec;

/// Çalışan panelin sürümü. Sınama derlemelerinde `PIXPANEL_VERSION` ile değiştirilebilir.
pub const VERSION: &str = match option_env!("PIXPANEL_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// Sürümleri imzalayan anahtarın açık yarısı (ed25519, onaltılık).
const RELEASE_PUBLIC_KEY: &str = "b3c702845cbf0b7d9e2a0cdfe7b0ba2a6d469f30f905b2de7c83e00be82e0560";

pub const BINARY: &str = "/usr/local/bin/pixpanel";
const SERVICE: &str = "pixpanel.service";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_ARCHIVE_BYTES: usize = 200 * 1024 * 1024;
const STATE_KEY: &str = "update_state";

fn previous_binary() -> PathBuf {
    PathBuf::from(format!("{BINARY}.previous"))
}

fn work_dir(cfg: &Config) -> PathBuf {
    cfg.data_dir.join("update")
}

fn result_path(cfg: &Config) -> PathBuf {
    work_dir(cfg).join("result.json")
}

fn database_copy(cfg: &Config) -> PathBuf {
    work_dir(cfg).join("before.db")
}

/// Bu sunucunun mimarisine uyan arşivin adı.
fn archive_name() -> AppResult<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("pixpanel-linux-x86_64.tar.gz"),
        "aarch64" => Ok("pixpanel-linux-aarch64.tar.gz"),
        other => Err(AppError::bad(format!("Bu işlemci mimarisi için sürüm yayınlanmıyor: {other}"))),
    }
}

// ---- Sürüm karşılaştırma ----

/// `1.2.3` biçimindeki sürümü sayılara çevirir; başka biçimler reddedilir.
pub fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.trim().trim_start_matches('v').split('.');
    let numbers = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(numbers)
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

// ---- İndirme ve doğrulama ----

fn http() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .connect_timeout(Duration::from_secs(15))
        .user_agent(format!("pixpanel/{VERSION}"))
        .build()
        .map_err(|err| AppError::internal(err.to_string()))
}

async fn download(cfg: &Config, name: &str, limit: usize) -> AppResult<Vec<u8>> {
    let url = format!("{}/{name}", cfg.update_url.trim_end_matches('/'));
    let failed = |detail: String| AppError::bad(format!("Sürüm dosyası indirilemedi ({name}): {detail}"));
    let response = http()?.get(&url).send().await.map_err(|err| failed(err.without_url().to_string()))?;
    if !response.status().is_success() {
        return Err(failed(format!("sunucu {} döndü", response.status().as_u16())));
    }
    let bytes = response.bytes().await.map_err(|err| failed(err.without_url().to_string()))?;
    if bytes.len() > limit {
        return Err(failed("dosya beklenenden büyük".to_string()));
    }
    Ok(bytes.to_vec())
}

fn public_key(cfg: &Config) -> AppResult<Vec<u8>> {
    let text = cfg.update_public_key.as_deref().unwrap_or(RELEASE_PUBLIC_KEY);
    let key = hex::decode(text.trim()).map_err(|_| AppError::internal("sürüm anahtarı geçersiz"))?;
    if key.len() == 32 { Ok(key) } else { Err(AppError::internal("sürüm anahtarı geçersiz")) }
}

fn verify_signature(key: &[u8], message: &[u8], signature: &[u8]) -> AppResult<()> {
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(message, signature)
        .map_err(|_| AppError::bad("Sürümün imzası doğrulanamadı; güncelleme uygulanmadı"))
}

/// `sha256sum` çıktısından verilen dosyanın özetini bulur.
fn expected_hash<'a>(sums: &'a str, name: &str) -> AppResult<&'a str> {
    sums.lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, file)| file.trim().trim_start_matches('*') == name)
        .map(|(hash, _)| hash)
        .ok_or_else(|| AppError::bad(format!("Özet dosyasında {name} yok")))
}

fn check_hash(sums: &str, name: &str, data: &[u8]) -> AppResult<()> {
    let actual = hex::encode(Sha256::digest(data));
    if expected_hash(sums, name)?.eq_ignore_ascii_case(&actual) {
        Ok(())
    } else {
        Err(AppError::bad(format!("{name} bozuk ya da değiştirilmiş (özet uyuşmuyor)")))
    }
}

/// İmzası doğrulanmış özet dosyasını ve yayınlanan sürüm numarasını indirir.
async fn fetch_release(cfg: &Config) -> AppResult<(String, String)> {
    let sums = download(cfg, "SHA256SUMS", 64 * 1024).await?;
    let signature = download(cfg, "SHA256SUMS.sig", 1024).await?;
    verify_signature(&public_key(cfg)?, &sums, &signature)?;
    let sums = String::from_utf8(sums).map_err(|_| AppError::bad("Özet dosyası okunamadı"))?;
    let version = download(cfg, "VERSION", 64).await?;
    check_hash(&sums, "VERSION", &version)?;
    let version = String::from_utf8_lossy(&version).trim().to_string();
    if parse_version(&version).is_none() {
        return Err(AppError::bad("Yayınlanan sürüm numarası geçersiz"));
    }
    Ok((sums, version))
}

/// Arşivin içinden panel ikilisini çıkarır.
fn extract_binary(archive: &[u8]) -> AppResult<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let entry = entry?;
        let is_binary = entry.path()?.file_name().is_some_and(|name| name == "pixpanel") && entry.header().entry_type().is_file();
        if is_binary {
            let mut data = Vec::new();
            entry.take(MAX_ARCHIVE_BYTES as u64).read_to_end(&mut data)?;
            return Ok(data);
        }
    }
    Err(AppError::bad("Arşivde panel ikilisi yok"))
}

// ---- Durum ----

/// Son denetimin ve son güncellemenin sonucu.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct State {
    pub latest: String,
    pub checked_at: Option<i64>,
    pub check_error: String,
}

/// Bekçi sürecinin yazdığı güncelleme sonucu.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Outcome {
    pub from: String,
    pub to: String,
    /// `restarting`, `success` ya da `rolled_back`.
    pub status: String,
    pub detail: String,
    pub at: i64,
}

#[derive(Serialize)]
pub struct View {
    pub current: &'static str,
    pub latest: String,
    pub available: bool,
    pub checked_at: Option<i64>,
    pub check_error: String,
    pub last: Option<Outcome>,
    pub source: String,
}

fn load_state(state: &AppState) -> State {
    state.db.setting(STATE_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

fn read_outcome(cfg: &Config) -> Option<Outcome> {
    serde_json::from_str(&std::fs::read_to_string(result_path(cfg)).ok()?).ok()
}

fn write_outcome(cfg: &Config, outcome: &Outcome) {
    let _ = std::fs::create_dir_all(work_dir(cfg));
    if let Ok(json) = serde_json::to_vec_pretty(outcome) {
        let _ = std::fs::write(result_path(cfg), json);
    }
}

pub fn view(state: &AppState) -> View {
    let saved = load_state(state);
    View {
        current: VERSION,
        available: is_newer(&saved.latest, VERSION),
        latest: saved.latest,
        checked_at: saved.checked_at,
        check_error: saved.check_error,
        last: read_outcome(&state.cfg),
        source: state.cfg.update_url.clone(),
    }
}

/// Yayınlanan son sürümü denetler ve sonucu kaydeder.
pub async fn check(state: &AppState) -> AppResult<View> {
    let mut saved = load_state(state);
    saved.checked_at = Some(now());
    match fetch_release(&state.cfg).await {
        Ok((_, version)) => {
            saved.latest = version;
            saved.check_error.clear();
        }
        Err(err) => {
            // Denetlenemeyen kaynağın eski yanıtı "yeni sürüm var" diye gösterilmez.
            saved.latest.clear();
            saved.check_error = err.to_string();
        }
    }
    let json = serde_json::to_string(&saved).map_err(|err| AppError::internal(err.to_string()))?;
    state.db.set_setting(STATE_KEY, &json)?;
    Ok(view(state))
}

// ---- Uygulama ----

/// Yeni sürümü indirir, doğrular, yerine koyar ve bekçiyi başlatır. Panel birkaç
/// saniye sonra yeniden başlar; sonuç `result.json` dosyasından okunur.
pub async fn apply(state: &AppState, ctx: &JobCtx, config_path: &Path) -> AppResult<()> {
    let cfg = &state.cfg;
    ctx.log(format!("Çalışan sürüm: {VERSION}"));
    let (sums, latest) = fetch_release(cfg).await?;
    ctx.log(format!("Yayınlanan sürüm: {latest} (imza doğrulandı)"));
    if !is_newer(&latest, VERSION) {
        return Err(AppError::bad("Panel zaten güncel"));
    }

    let name = archive_name()?;
    ctx.log(format!("{name} indiriliyor…"));
    let archive = download(cfg, name, MAX_ARCHIVE_BYTES).await?;
    check_hash(&sums, name, &archive)?;
    ctx.log(format!("İndirildi ({} MB), özeti doğrulandı.", archive.len() / (1024 * 1024)));
    let binary = tokio::task::spawn_blocking(move || extract_binary(&archive)).await.map_err(|err| AppError::internal(err.to_string()))??;

    // Yeni ikili yerine konmadan önce gerçekten çalıştığı ve beklenen sürüm olduğu denetlenir.
    let staged = format!("{BINARY}.new");
    crate::hosting::atomic_write(Path::new(&staged), &binary, 0o755)?;
    let reported = exec::run(&staged, &["--version"]).await?;
    if !reported.success() || reported.stdout.split_whitespace().last() != Some(latest.as_str()) {
        let _ = std::fs::remove_file(&staged);
        return Err(AppError::bad("İndirilen ikili çalışmadı ya da beklenen sürüm değil; güncelleme uygulanmadı"));
    }

    std::fs::create_dir_all(work_dir(cfg))?;
    let copy = database_copy(cfg);
    let _ = std::fs::remove_file(&copy);
    let copy_str = copy.to_string_lossy().into_owned();
    state.db.call(|conn| conn.execute("VACUUM INTO ?1", [&copy_str]))?;
    std::fs::copy(BINARY, previous_binary())?;
    ctx.log("Eski sürüm ve veritabanı yedeklendi.");

    write_outcome(cfg, &Outcome { from: VERSION.to_string(), to: latest.clone(), status: "restarting".to_string(), detail: String::new(), at: now() });
    std::fs::rename(&staged, BINARY)?;

    // Bekçi eski ikiliyle, panelden bağımsız bir birimde çalışır; böylece yeni
    // sürüm hiç başlamasa bile geri dönüşü yapabilir.
    let unit = format!("pixpanel-update-{}", now());
    let previous = previous_binary().to_string_lossy().into_owned();
    let config = config_path.to_string_lossy().into_owned();
    exec::run_ok("systemd-run", &["--quiet", "--collect", "--unit", &unit, &previous, "--config", &config, "update-watchdog"]).await?;
    ctx.log("Panel birkaç saniye içinde yeni sürümle yeniden başlayacak. Başlamazsa eski sürüme kendiliğinden dönülür.");
    Ok(())
}

// ---- Bekçi (ayrı süreç, eski ikiliyle çalışır) ----

fn systemctl(action: &str) -> bool {
    std::process::Command::new("systemctl").args([action, SERVICE]).status().is_ok_and(|status| status.success())
}

fn healthy(cfg: &Config) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], cfg.listen_port));
    systemctl("is-active") && std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok()
}

/// Paneli yeniden başlatır; kararlı biçimde ayağa kalkmazsa eski ikiliyi ve
/// veritabanını geri koyar.
pub fn run_watchdog(cfg: &Config) -> anyhow::Result<()> {
    let mut outcome = read_outcome(cfg).unwrap_or_default();
    std::thread::sleep(Duration::from_secs(2));
    systemctl("restart");

    // Panel art arda 8 saniye sağlıklı kalırsa güncelleme başarılı sayılır.
    let mut streak = 0;
    for _ in 0..45 {
        std::thread::sleep(Duration::from_secs(1));
        streak = if healthy(cfg) { streak + 1 } else { 0 };
        if streak >= 8 {
            break;
        }
    }
    outcome.at = now();
    if streak >= 8 {
        outcome.status = "success".to_string();
        write_outcome(cfg, &outcome);
        let _ = std::fs::remove_file(database_copy(cfg));
        return Ok(());
    }

    systemctl("stop");
    std::fs::copy(previous_binary(), BINARY)?;
    let database = cfg.db_path();
    let copy = database_copy(cfg);
    if copy.exists() {
        // Yeni sürüm şemayı yarım değiştirmiş olabilir; güncelleme öncesi kopya geri konur.
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", database.display()));
        }
        std::fs::copy(&copy, &database)?;
    }
    systemctl("start");
    outcome.status = "rolled_back".to_string();
    outcome.detail = "Yeni sürüm başlamadı; eski sürüme dönüldü.".to_string();
    write_outcome(cfg, &outcome);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_compared_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("1.0", "0.1.0"));
        assert!(!is_newer("1.0.0-rc1", "0.1.0"));
    }

    #[test]
    fn hashes_are_looked_up_by_file_name() {
        let sums = "aaaa  VERSION\nbbbb *pixpanel-linux-x86_64.tar.gz\n";
        assert_eq!(expected_hash(sums, "VERSION").unwrap(), "aaaa");
        assert_eq!(expected_hash(sums, "pixpanel-linux-x86_64.tar.gz").unwrap(), "bbbb");
        assert!(expected_hash(sums, "baska").is_err());
        let data = b"0.2.0\n";
        let sums = format!("{}  VERSION\n", hex::encode(Sha256::digest(data)));
        assert!(check_hash(&sums, "VERSION", data).is_ok());
        assert!(check_hash(&sums, "VERSION", b"0.3.0\n").is_err());
    }

    #[test]
    fn signatures_must_match_the_release_key() {
        let key = hex::decode(RELEASE_PUBLIC_KEY).unwrap();
        assert_eq!(key.len(), 32);
        assert!(verify_signature(&key, b"icerik", &[0u8; 64]).is_err());
    }
}
