//! restic çağrıları.
//!
//! Depo adresi, parolası ve bulut anahtarları komut satırına değil ortam
//! değişkenlerine konur; böylece süreç listesinde görünmezler. Kullanıcıdan
//! gelen değerler (`TargetConfig`) kaydedilirken beyaz listeyle doğrulanmıştır.

use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{Target, KEY_DIR};
use crate::error::{AppError, AppResult};
use crate::jobs::JobCtx;
use crate::sys::exec::{base_command, CmdOutput};

const CACHE_DIR: &str = "/var/cache/pixpanel/restic";
const QUERY_TIMEOUT: Duration = Duration::from_secs(180);
/// `--stdin-from-command` ve `restore --delete` için gereken en düşük sürüm.
const MIN_VERSION: (u32, u32) = (0, 17);
/// restic'in "depo yok" çıkış kodu.
const REPOSITORY_MISSING: i32 = 10;

/// Kurulu restic sürümü (ana, alt).
pub async fn version() -> Option<(u32, u32)> {
    let output = crate::sys::exec::run("restic", &["version"]).await.ok()?;
    // "restic 0.18.0 compiled with go1.24 on linux/amd64"
    let number = output.stdout.split_whitespace().nth(1)?;
    let mut parts = number.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

pub async fn require_supported() -> AppResult<()> {
    match version().await {
        None => Err(AppError::bad("restic kurulu değil; önce Bileşenler sayfasından kurun")),
        Some(found) if found < MIN_VERSION => Err(AppError::bad(format!(
            "Kurulu restic sürümü ({}.{}) çok eski; en az {}.{} gerekir",
            found.0, found.1, MIN_VERSION.0, MIN_VERSION.1
        ))),
        Some(_) => Ok(()),
    }
}

/// Hedefin restic deposuna erişmek için gereken adres ve ortam.
fn command(target: &Target) -> Command {
    let config = &target.config;
    let mut cmd = base_command("restic");
    cmd.env("RESTIC_PASSWORD", &target.password)
        .env("RESTIC_CACHE_DIR", CACHE_DIR)
        .env("HOME", "/root")
        .env("RESTIC_PROGRESS_FPS", "0.02")
        .current_dir("/");
    let repository = match target.kind.as_str() {
        "sftp" => {
            let ssh_args = format!(
                "-i {} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile={KEY_DIR}/known_hosts -o ConnectTimeout=15",
                target.key_path().display()
            );
            cmd.arg("-o").arg(format!("sftp.args={ssh_args}"));
            // Mutlak yol `sftp://sunucu//yol`, göreli yol `sftp://sunucu/yol` biçimindedir.
            format!("sftp://{}@{}:{}/{}", config.user, config.host, config.port, config.path)
        }
        "s3" => {
            cmd.env("AWS_ACCESS_KEY_ID", &config.access_key).env("AWS_SECRET_ACCESS_KEY", &target.secret);
            if !config.region.is_empty() {
                cmd.env("AWS_DEFAULT_REGION", &config.region);
            }
            let prefix = if config.path.is_empty() { String::new() } else { format!("/{}", config.path) };
            format!("s3:{}/{}{}", config.endpoint, config.bucket, prefix)
        }
        "ftp" => {
            // restic FTP'yi kendisi konuşmaz; rclone aracılık eder. Bağlantı bilgisi rclone'a
            // ortam değişkenleriyle verilir; parola rclone'un beklediği gizlenmiş biçimde saklanır.
            cmd.env("RCLONE_FTP_HOST", &config.host)
                .env("RCLONE_FTP_PORT", config.port.to_string())
                .env("RCLONE_FTP_USER", &config.user)
                .env("RCLONE_FTP_PASS", &target.secret)
                .env("RCLONE_FTP_EXPLICIT_TLS", if config.tls { "true" } else { "false" })
                .env("RCLONE_FTP_NO_CHECK_CERTIFICATE", if config.tls_skip_verify { "true" } else { "false" })
                .env("RCLONE_CONFIG", "/dev/null")
                .env("RCLONE_CONTIMEOUT", "20s");
            format!("rclone::ftp:{}", config.path)
        }
        _ => config.path.clone(),
    };
    cmd.env("RESTIC_REPOSITORY", repository);
    cmd
}

async fn output(target: &Target, args: &[&str], timeout: Duration) -> AppResult<CmdOutput> {
    let mut cmd = command(target);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| AppError::internal("Yedek deposu zamanında yanıt vermedi"))?
        .map_err(|err| AppError::internal(format!("restic başlatılamadı: {err}")))?;
    Ok(CmdOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// restic'in hata çıktısından kullanıcıya gösterilecek kısa neden.
fn reason(output: &CmdOutput) -> String {
    let text = if output.stderr.trim().is_empty() { output.stdout.trim() } else { output.stderr.trim() };
    let last = text.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("bilinmeyen hata");
    // `--json` ile çalışan komutlar hatayı da JSON olarak yazar.
    let message = serde_json::from_str::<serde_json::Value>(last)
        .ok()
        .and_then(|value| value.get("message").and_then(|message| message.as_str()).map(str::to_string))
        .unwrap_or_else(|| last.to_string());
    let first = message.lines().next().unwrap_or("bilinmeyen hata");
    first.trim_start_matches("Fatal: ").chars().take(300).collect()
}

/// Depoya erişilebildiğini doğrular; depo yoksa oluşturur.
/// Başarısızlıkta kullanıcıya gösterilecek nedeni döner.
pub async fn check(target: &Target) -> Result<(), String> {
    let timeout = Duration::from_secs(60);
    let existing = output(target, &["cat", "config", "--no-lock"], timeout).await.map_err(|err| err.to_string())?;
    if existing.success() {
        return Ok(());
    }
    let created = output(target, &["init"], timeout).await.map_err(|err| err.to_string())?;
    if created.success() {
        return Ok(());
    }
    // Depo var ama açılamıyorsa (ör. yanlış parola) asıl neden ilk komutun hatasıdır.
    let init_reason = reason(&created);
    Err(if init_reason.contains("already") { reason(&existing) } else { init_reason })
}

// ---- Anlık görüntüler ----

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub id: String,
    pub short_id: String,
    /// Unix saniyesi.
    pub time: i64,
    /// `site`, `db`, `mail` ya da `panel`.
    pub kind: String,
    pub name: String,
    pub engine: String,
    pub owner: String,
    pub plan_id: Option<i64>,
    pub paths: Vec<String>,
    /// Yedeklenen verinin toplam boyutu (restic bildirdiyse).
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct RawSnapshot {
    id: String,
    short_id: String,
    time: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    summary: Option<RawSummary>,
}

#[derive(Deserialize)]
struct RawSummary {
    total_bytes_processed: Option<u64>,
}

/// `2026-11-10T14:03:22.123456789+03:00` biçimindeki zamanı Unix saniyesine çevirir.
fn parse_time(value: &str) -> i64 {
    let number = |range: std::ops::Range<usize>| value.get(range).and_then(|part| part.parse::<i64>().ok()).unwrap_or(0);
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let (hour, minute, second) = (number(11..13), number(14..16), number(17..19));
    // Gün sayısı (Howard Hinnant'ın takvim algoritması).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let zone = value.get(19..).unwrap_or("");
    let offset = match zone.rfind(['+', '-']) {
        Some(index) if zone.len() >= index + 6 => {
            let sign = if zone[index..].starts_with('-') { -1 } else { 1 };
            let hours: i64 = zone[index + 1..index + 3].parse().unwrap_or(0);
            let minutes: i64 = zone[index + 4..index + 6].parse().unwrap_or(0);
            sign * (hours * 3600 + minutes * 60)
        }
        _ => 0,
    };
    days * 86400 + hour * 3600 + minute * 60 + second - offset
}

fn tag<'a>(tags: &'a [String], key: &str) -> &'a str {
    tags.iter().find_map(|tag| tag.strip_prefix(key).and_then(|rest| rest.strip_prefix(':'))).unwrap_or("")
}

fn to_snapshot(raw: RawSnapshot) -> Snapshot {
    Snapshot {
        time: parse_time(&raw.time),
        kind: tag(&raw.tags, "kind").to_string(),
        name: tag(&raw.tags, "name").to_string(),
        engine: tag(&raw.tags, "engine").to_string(),
        owner: tag(&raw.tags, "owner").to_string(),
        plan_id: tag(&raw.tags, "plan").parse().ok(),
        size: raw.summary.and_then(|summary| summary.total_bytes_processed),
        id: raw.id,
        short_id: raw.short_id,
        paths: raw.paths,
    }
}

/// Depodaki, panelin aldığı bütün anlık görüntüler; en yenisi başta.
pub async fn snapshots(target: &Target) -> AppResult<Vec<Snapshot>> {
    let result = output(target, &["snapshots", "--json", "--no-lock"], QUERY_TIMEOUT).await?;
    // Depo henüz kurulmamışsa (ilk yedek alınmamışsa) yedek de yoktur.
    if result.code == REPOSITORY_MISSING {
        return Ok(Vec::new());
    }
    if !result.success() {
        return Err(AppError::bad(format!("Yedekler okunamadı: {}", reason(&result))));
    }
    let raw: Vec<RawSnapshot> = serde_json::from_str(result.stdout.trim()).map_err(|err| AppError::internal(format!("restic çıktısı anlaşılamadı: {err}")))?;
    let mut list: Vec<Snapshot> = raw.into_iter().map(to_snapshot).filter(|snapshot| !snapshot.kind.is_empty()).collect();
    list.sort_by(|a, b| b.time.cmp(&a.time));
    Ok(list)
}

/// Anlık görüntü kimliği yalnızca onaltılık rakamlardan oluşur.
pub fn validate_snapshot_id(value: &str) -> AppResult<&str> {
    if (8..=64).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(AppError::bad("Geçersiz yedek kimliği"))
    }
}

pub async fn find_snapshot(target: &Target, id: &str) -> AppResult<Snapshot> {
    let id = validate_snapshot_id(id)?;
    snapshots(target).await?.into_iter().find(|snapshot| snapshot.id.starts_with(id)).ok_or(AppError::NotFound)
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// `dir`, `file` ya da `symlink`.
    pub kind: String,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Deserialize)]
struct RawNode {
    #[serde(default)]
    struct_type: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime: String,
}

const MAX_ENTRIES: usize = 5000;

/// Anlık görüntüdeki bir klasörün içeriği (alt klasörlere inmeden).
pub async fn list_dir(target: &Target, snapshot: &str, path: &str) -> AppResult<Vec<Entry>> {
    let result = output(target, &["ls", "--json", "--no-lock", snapshot, "--", path], QUERY_TIMEOUT).await?;
    if !result.success() {
        return Err(AppError::bad(format!("Yedeğin içeriği okunamadı: {}", reason(&result))));
    }
    let mut entries = Vec::new();
    for line in result.stdout.lines() {
        let Ok(node) = serde_json::from_str::<RawNode>(line) else { continue };
        // İlk satır anlık görüntünün kendisi, ardından istenen klasör ve içindekiler gelir.
        if node.struct_type != "node" || node.path == path {
            continue;
        }
        entries.push(Entry {
            name: node.name,
            path: node.path,
            kind: match node.kind.as_str() {
                "dir" => "dir",
                "symlink" => "symlink",
                _ => "file",
            }
            .to_string(),
            size: node.size,
            mtime: parse_time(&node.mtime),
        });
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }
    entries.sort_by(|a, b| (a.kind != "dir").cmp(&(b.kind != "dir")).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// Anlık görüntüdeki küçük bir dosyanın içeriği (site bildirimi gibi).
pub async fn read_file(target: &Target, snapshot: &str, path: &str) -> AppResult<String> {
    let result = output(target, &["dump", "--no-lock", snapshot, "--", path], QUERY_TIMEOUT).await?;
    if !result.success() {
        return Err(AppError::bad(format!("Yedekteki dosya okunamadı: {}", reason(&result))));
    }
    Ok(result.stdout)
}

// ---- Yedek alma ----

fn tag_args(tags: &[String]) -> Vec<String> {
    vec!["--tag".to_string(), tags.join(",")]
}

/// restic `backup` çıkış kodunu yorumlar: 3, bazı dosyaların okunamadığı ama
/// anlık görüntünün yine de kaydedildiği anlamına gelir.
fn backup_outcome(ctx: &JobCtx, code: i32) -> AppResult<()> {
    match code {
        0 => Ok(()),
        3 => {
            ctx.log("UYARI: bazı dosyalar okunamadı; geri kalanı yedeklendi.");
            Ok(())
        }
        code => Err(AppError::internal(format!("restic yedeği kaydedemedi (kod {code})"))),
    }
}

/// Verilen klasör ve dosyaları tek bir anlık görüntü olarak yedekler.
pub async fn backup_paths(ctx: &JobCtx, target: &Target, paths: &[String], tags: &[String]) -> AppResult<()> {
    let mut cmd = command(target);
    cmd.args(["backup", "--one-file-system", "--exclude-caches"]).args(tag_args(tags)).arg("--").args(paths);
    let code = ctx.run_status(&mut cmd, "restic", None).await?;
    backup_outcome(ctx, code)
}

/// Bir komutun (veritabanı dökümü) çıktısını diske yazmadan yedekler. Komut
/// başarısız olursa restic anlık görüntüyü kaydetmez.
pub async fn backup_command(ctx: &JobCtx, target: &Target, file_name: &str, tags: &[String], source: &Command) -> AppResult<()> {
    let source = source.as_std();
    let mut cmd = command(target);
    cmd.args(["backup", "--stdin-filename", file_name]).args(tag_args(tags)).arg("--stdin-from-command").arg("--").arg(source.get_program()).args(source.get_args());
    let code = ctx.run_status(&mut cmd, "restic", None).await?;
    backup_outcome(ctx, code)
}

/// Saklama ilkesini uygular: planın eski anlık görüntülerini siler ve depoyu sıkıştırır.
pub async fn apply_retention(ctx: &JobCtx, target: &Target, plan_id: i64, daily: i64, weekly: i64, monthly: i64) -> AppResult<()> {
    let mut cmd = command(target);
    cmd.args(["forget", "--tag", &format!("plan:{plan_id}"), "--group-by", "tags", "--prune", "--compact"]);
    for (flag, count) in [("--keep-daily", daily), ("--keep-weekly", weekly), ("--keep-monthly", monthly)] {
        if count > 0 {
            cmd.args([flag, &count.to_string()]);
        }
    }
    ctx.run_streaming(&mut cmd, "restic", None).await
}

pub async fn forget(ctx: &JobCtx, target: &Target, snapshot: &str) -> AppResult<()> {
    let mut cmd = command(target);
    cmd.args(["forget", "--prune", "--compact", snapshot]);
    ctx.run_streaming(&mut cmd, "restic", None).await
}

// ---- Geri yükleme ----

/// Anlık görüntüyü (ya da `includes` ile seçilen yollarını) `destination` altına
/// geri yükler. `source` verilirse yalnızca o klasörün içeriği, kökü `destination`
/// olacak biçimde yüklenir. `delete`, hedefte olup yedekte olmayan dosyaları siler.
pub async fn restore(ctx: &JobCtx, target: &Target, snapshot: &str, source: Option<&str>, destination: &str, includes: &[String], delete: bool) -> AppResult<()> {
    let spec = match source {
        Some(source) => format!("{snapshot}:{source}"),
        None => snapshot.to_string(),
    };
    let mut cmd = command(target);
    cmd.args(["restore", &spec, "--target", destination]);
    if delete {
        cmd.arg("--delete");
    }
    for include in includes {
        cmd.args(["--include", include]);
    }
    ctx.run_streaming(&mut cmd, "restic", None).await
}

/// Anlık görüntüdeki tek dosyayı (SQL dökümü) verilen komutun girdisine akıtır.
pub async fn pipe_file(ctx: &JobCtx, target: &Target, snapshot: &str, path: &str, sink: &mut Command, sink_name: &str) -> AppResult<()> {
    let mut dump = command(target);
    dump.args(["dump", snapshot, "--", path]).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = dump.spawn().map_err(|err| AppError::internal(format!("restic başlatılamadı: {err}")))?;
    let stdout: Stdio = child.stdout.take().expect("stdout borusu").try_into().map_err(|err: std::io::Error| AppError::internal(err.to_string()))?;
    let stderr = child.stderr.take().expect("stderr borusu");
    let err_ctx = ctx.clone();
    let err_task = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            err_ctx.log(line);
        }
    });
    sink.stdin(stdout);
    let loaded = ctx.run_streaming(sink, sink_name, None).await;
    let status = child.wait().await?;
    let _ = err_task.await;
    if !status.success() {
        return Err(AppError::internal("Yedekteki döküm okunamadı"));
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restic_times_are_parsed() {
        assert_eq!(parse_time("2026-11-10T12:00:00Z"), 1_794_312_000);
        assert_eq!(parse_time("2026-11-10T15:00:00.123456789+03:00"), 1_794_312_000);
        assert_eq!(parse_time("2026-11-10T07:30:00.5-04:30"), 1_794_312_000);
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), 0);
    }

    #[test]
    fn snapshot_ids_are_validated() {
        assert!(validate_snapshot_id("0a1b2c3d").is_ok());
        assert!(validate_snapshot_id("latest").is_err());
        assert!(validate_snapshot_id("--help123").is_err());
        assert!(validate_snapshot_id("abc").is_err());
    }

    #[test]
    fn tags_are_read() {
        let tags = vec!["kind:db".to_string(), "name:blog".to_string(), "plan:7".to_string()];
        assert_eq!(tag(&tags, "kind"), "db");
        assert_eq!(tag(&tags, "plan"), "7");
        assert_eq!(tag(&tags, "owner"), "");
    }
}

/// FTP parolasını rclone'un yapılandırmada beklediği gizlenmiş biçime çevirir. Parola
/// komut satırına değil, `rclone obscure -` komutunun girdisine yazılır.
pub async fn obscure_ftp_password(password: &str) -> AppResult<String> {
    use tokio::io::AsyncWriteExt;
    if password.is_empty() || password.len() > 200 || password.chars().any(char::is_control) {
        return Err(AppError::bad("FTP parolası geçersiz"));
    }
    let mut cmd = base_command("rclone");
    cmd.args(["obscure", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|_| AppError::bad("rclone kurulu değil; FTP hedefi için gerekir"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(password.as_bytes()).await.map_err(|err| AppError::internal(err.to_string()))?;
        let _ = stdin.shutdown().await;
    }
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .map_err(|_| AppError::internal("rclone zaman aşımına uğradı"))?
        .map_err(|err| AppError::internal(err.to_string()))?;
    let obscured = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || obscured.is_empty() {
        return Err(AppError::internal("FTP parolası hazırlanamadı"));
    }
    Ok(obscured)
}
