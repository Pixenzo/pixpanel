//! Taşıma: başka bir sunucuda duran siteyi SSH ile bu sunucudaki bir siteye çeker.
//!
//! Dosyalar karşı sunucuda `tar` ile paketlenip bağlantı üzerinden akıtılır ve sitenin yayın
//! klasörüne açılır. Site WordPress ise veritabanı da karşıda dökülür, burada açılan yeni bir
//! veritabanına yüklenir ve ayar dosyası yeni veritabanını gösterecek biçimde düzeltilir.
//!
//! Güvenlik:
//! * Bütün bağlantılar ve dosya yazımı hesabın kendi Linux kullanıcısıyla yapılır; panel karşı
//!   sunucudan gelen hiçbir şeyi root olarak diske yazmaz.
//! * Karşı sunucunun anahtarı önce okunup kullanıcıya gösterilir; taşıma yalnızca onaylanan
//!   anahtara bağlanır.
//! * Parola ve özel anahtar saklanmaz, komut satırına ve görev günlüğüne yazılmaz.
//! * Karşı sunucuya giden komutlara yalnızca dar bir karakter kümesinden geçen değerler girer;
//!   veritabanı parolası komut satırı yerine bağlantının girdisinden verilir.
//! * Döküm, panelin içe aktarma yolundan (kısıtlı veritabanı hesabı) yüklenir.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::auth;
use crate::dbms::{self, Engine};
use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::hosting::sysuser::{self, SysUser};
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::runas;
use crate::wordpress::{self, ConfigDatabase};

/// Geçici bağlantı dosyalarının (bilinen sunucu anahtarı, özel anahtar) durduğu yer.
/// Üst klasör root'undur; her taşıma kendi rastgele adlı alt klasörünü kullanır.
const WORK_ROOT: &str = "/run/pixpanel-migrate";
/// `known_hosts` dosyasında karşı sunucunun adı yerine kullanılan sabit takma ad.
const HOST_ALIAS: &str = "pixpanel-migrate";
/// Parolayı `ssh`'a ileten ortam değişkeni (bkz. `askpass`).
pub const ASKPASS_ENV: &str = "PIXPANEL_ASKPASS";
const KEY_TYPES: [&str; 5] = ["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521", "ssh-rsa"];
const QUICK: Duration = Duration::from_secs(45);
const MAX_LOG_LINES: usize = 200;

/// Karşı sunucunun bağlantı bilgileri.
pub struct Source {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub private_key: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct Probe {
    /// Onaylandıktan sonra taşıma isteğinde geri gönderilen sunucu anahtarları.
    pub host_key: String,
    pub fingerprints: Vec<String>,
    pub address: String,
    pub path_found: bool,
    pub size_kb: Option<u64>,
    pub wordpress: Option<ProbeWordPress>,
    pub can_dump: bool,
}

#[derive(Serialize)]
pub struct ProbeWordPress {
    pub database: String,
    pub database_host: String,
}

fn bad(message: impl Into<String>) -> AppError {
    AppError::bad(message.into())
}

impl Source {
    /// Kullanıcıdan gelen bağlantı bilgilerini doğrular.
    pub fn validate(&mut self) -> AppResult<()> {
        self.host = self.host.trim().to_lowercase();
        self.username = self.username.trim().to_string();
        self.path = self.path.trim().trim_end_matches('/').to_string();
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && !self.host.starts_with('-')
            && self.host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'));
        if !host_ok {
            return Err(bad("Sunucu adresi geçersiz (alan adı ya da IP adresi yazın)"));
        }
        if self.port == 0 {
            return Err(bad("Bağlantı noktası geçersiz"));
        }
        let mut name = self.username.bytes();
        let name_ok = self.username.len() <= 64
            && name.next().is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && name.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if !name_ok {
            return Err(bad("SSH kullanıcı adı geçersiz"));
        }
        validate_remote_path(&self.path)?;
        if self.password.is_empty() == self.private_key.trim().is_empty() {
            return Err(bad("Parola ya da özel anahtardan yalnızca birini verin"));
        }
        if self.password.len() > 256 || self.password.chars().any(|ch| ch.is_control()) {
            return Err(bad("Parola geçersiz"));
        }
        if !self.private_key.trim().is_empty() {
            let key = self.private_key.replace("\r\n", "\n");
            let key = key.trim();
            let shape_ok = key.len() <= 16 * 1024
                && key.starts_with("-----BEGIN ")
                && key.ends_with("PRIVATE KEY-----")
                && key.bytes().all(|byte| byte == b'\n' || (0x20..0x7f).contains(&byte));
            if !shape_ok {
                return Err(bad("Özel anahtar tanınmadı; anahtar dosyasının bütün içeriğini (BEGIN ve END satırlarıyla) yapıştırın"));
            }
            self.private_key = format!("{key}\n");
        } else {
            self.private_key.clear();
        }
        Ok(())
    }
}

/// Karşı sunucudaki klasör yolu, oradaki kabuğun ayrıştırdığı komutun içine girer; bu yüzden
/// yalnızca kabukta özel anlamı olmayan karakterlere izin verilir.
fn validate_remote_path(path: &str) -> AppResult<()> {
    let rest = path.strip_prefix("~/").unwrap_or(path);
    let rooted = path.starts_with('/') || path.starts_with("~/");
    let chars_ok = rest.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+' | b'@'));
    let segments_ok = rest.split('/').all(|segment| segment != ".." && !segment.starts_with('-'));
    if path.is_empty() || path.len() > 512 || !rooted || !chars_ok || !segments_ok {
        return Err(bad(
            "Klasör yolu geçersiz. Tam yol yazın (ör. /var/www/vhosts/ornek.com/httpdocs ya da ~/public_html); yalnızca harf, rakam ve . _ - + @ / kullanılabilir",
        ));
    }
    Ok(())
}

/// Sunucu adresini çözer. Bu sunucunun kendisine ve yerel bağlantı adreslerine izin verilmez;
/// `ssh` sonradan adı yeniden çözmesin diye bağlantı çözülen adrese yapılır.
async fn resolve(source: &Source) -> AppResult<IpAddr> {
    let addresses: Vec<IpAddr> = match source.host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host((source.host.as_str(), source.port)))
            .await
            .map_err(|_| bad("Sunucu adresi çözülemedi (zaman aşımı)"))?
            .map_err(|_| bad(format!("{} adresi çözülemedi", source.host)))?
            .map(|address| address.ip())
            .collect(),
    };
    let ip = addresses.into_iter().next().ok_or_else(|| bad(format!("{} adresi çözülemedi", source.host)))?;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        other => other,
    };
    let forbidden = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified() || v4.is_link_local() || v4.is_multicast() || v4.is_broadcast(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80,
    };
    if forbidden {
        return Err(bad("Bu adrese bağlanılamaz; taşınacak sitenin bulunduğu başka bir sunucunun adresini yazın"));
    }
    Ok(ip)
}

/// `ssh-keyscan` çıktısından ya da istemcinin geri gönderdiği metinden sunucu anahtarlarını
/// ayıklar; yalnızca tanınan türde ve biçimi doğru anahtarlar alınır.
fn parse_host_keys(text: &str) -> Vec<(String, String)> {
    let mut keys = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_host), Some(kind), Some(key)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let key_ok = key.len() <= 2048 && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='));
        if KEY_TYPES.contains(&kind) && key_ok && base64::engine::general_purpose::STANDARD.decode(key).is_ok() && !keys.iter().any(|(k, _)| k == kind) {
            keys.push((kind.to_string(), key.to_string()));
        }
    }
    keys
}

fn known_hosts(keys: &[(String, String)]) -> String {
    keys.iter().map(|(kind, key)| format!("{HOST_ALIAS} {kind} {key}\n")).collect()
}

fn fingerprint(kind: &str, key: &str) -> String {
    let raw = base64::engine::general_purpose::STANDARD.decode(key).unwrap_or_default();
    let digest = base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&raw));
    let label = match kind {
        "ssh-ed25519" => "ED25519",
        "ssh-rsa" => "RSA",
        _ => "ECDSA",
    };
    format!("{label} SHA256:{digest}")
}

/// Bir taşımanın geçici bağlantı dosyaları. Düşünce klasör silinir.
struct Workspace {
    dir: PathBuf,
    user: SysUser,
    address: IpAddr,
}

impl Workspace {
    fn create(user: &SysUser, source: &Source, address: IpAddr, known_hosts: &str) -> AppResult<Self> {
        use std::os::unix::fs::{chown, DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o711).create(WORK_ROOT)?;
        let dir = Path::new(WORK_ROOT).join(auth::random_token(24));
        // Klasör root'un klasöründe ve rastgele adla açılır; içi doldurulduktan sonra hesaba devredilir.
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let workspace = Self { dir, user: user.clone(), address };
        let write = |name: &str, content: &str| -> AppResult<()> {
            use std::io::Write;
            let path = workspace.dir.join(name);
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
            file.write_all(content.as_bytes())?;
            chown(&path, Some(user.uid), Some(user.gid))?;
            Ok(())
        };
        write("known_hosts", known_hosts)?;
        if !source.private_key.is_empty() {
            write("key", &source.private_key)?;
        }
        chown(&workspace.dir, Some(user.uid), Some(user.gid))?;
        Ok(workspace)
    }

    /// Karşı sunucuda `remote_command`ı çalıştıran `ssh` komutu (hesabın kullanıcısıyla).
    fn ssh(&self, source: &Source, remote_command: &str) -> Command {
        let known = format!("UserKnownHostsFile={}", self.dir.join("known_hosts").display());
        let key = self.dir.join("key").to_string_lossy().into_owned();
        let port = source.port.to_string();
        let address = self.address.to_string();
        let mut args: Vec<&str> = vec!["-F", "/dev/null", "-p", &port, "-l", &source.username];
        for option in [
            "StrictHostKeyChecking=yes",
            known.as_str(),
            "GlobalKnownHostsFile=/dev/null",
            "HostKeyAlias=pixpanel-migrate",
            "ConnectTimeout=15",
            "ServerAliveInterval=30",
            "ServerAliveCountMax=4",
            "NumberOfPasswordPrompts=1",
            "LogLevel=ERROR",
            "ProxyCommand=none",
            "ControlMaster=no",
            "ForwardAgent=no",
            "ClearAllForwardings=yes",
            "RequestTTY=no",
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
        ] {
            args.extend(["-o", option]);
        }
        if source.private_key.is_empty() {
            args.extend(["-o", "PreferredAuthentications=password,keyboard-interactive", "-o", "PubkeyAuthentication=no"]);
        } else {
            args.extend(["-i", &key, "-o", "PreferredAuthentications=publickey", "-o", "PasswordAuthentication=no"]);
        }
        args.extend(["--", &address, remote_command]);
        let mut cmd = runas::command(&self.user, &self.dir, "ssh", &args);
        // Parola `ssh`'a, panelin kendi ikili dosyası üzerinden verilir (bkz. `askpass`);
        // anahtarla girişte değer boştur (parolalı anahtar desteklenmez).
        if let Ok(binary) = std::env::current_exe() {
            cmd.env("SSH_ASKPASS", binary.to_string_lossy().trim_end_matches(" (deleted)"));
        }
        cmd.env("SSH_ASKPASS_REQUIRE", "force").env(ASKPASS_ENV, &source.password);
        cmd
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `ssh`, parolayı sormak için `SSH_ASKPASS` programını çalıştırır; panelin ikili dosyası bu
/// ortam değişkeniyle çağrıldığında değeri yazıp çıkar. `main` en başta burayı dener.
pub fn askpass() -> Option<i32> {
    let secret = std::env::var(ASKPASS_ENV).ok()?;
    println!("{secret}");
    Some(0)
}

/// `ssh` hata çıktısını kullanıcının anlayacağı bir iletiye çevirir.
fn connection_error(stderr: &str) -> AppError {
    let text = stderr.trim();
    let lower = text.to_lowercase();
    let message = if lower.contains("permission denied") {
        "Karşı sunucu girişi reddetti: kullanıcı adı, parola ya da anahtar yanlış".to_string()
    } else if lower.contains("host key verification failed") || lower.contains("remote host identification has changed") {
        "Karşı sunucunun anahtarı onayladığınızla uyuşmuyor; bağlantıyı yeniden deneyin ve anahtarı karşılaştırın".to_string()
    } else if lower.contains("connection reset") || lower.contains("connection closed") || lower.contains("kex_exchange_identification") {
        "Karşı sunucu bağlantıyı kesti. Art arda başarısız giriş denemelerinden sonra sunucular geçici engel koyar; birkaç dakika bekleyip yeniden deneyin".to_string()
    } else if lower.contains("connection refused") {
        "Karşı sunucu bağlantıyı reddetti; SSH bağlantı noktasını denetleyin".to_string()
    } else if lower.contains("timed out") || lower.contains("no route to host") || lower.contains("unreachable") {
        "Karşı sunucuya ulaşılamadı; adresi, bağlantı noktasını ve karşı tarafın güvenlik duvarını denetleyin".to_string()
    } else {
        format!("Karşı sunucuya bağlanılamadı: {}", text.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("bilinmeyen hata").chars().take(200).collect::<String>())
    };
    bad(message)
}

fn account(site: &Site) -> AppResult<SysUser> {
    if !Path::new("/usr/bin/ssh").exists() {
        return Err(bad("Bu sunucuda SSH istemcisi kurulu değil (openssh-client paketi gerekir)"));
    }
    sysuser::lookup(&site.owner).ok_or_else(|| AppError::internal("Hesabın Linux kullanıcısı bulunamadı"))
}

/// Karşı sunucuya bağlanıp anahtarını, klasörü ve içinde WordPress olup olmadığını okur.
pub async fn probe(site: &Site, source: &Source) -> AppResult<Probe> {
    let user = account(site)?;
    let address = resolve(source).await?;
    let port = source.port.to_string();
    let scan = runas::command(&user, Path::new("/"), "ssh-keyscan", &["-T", "10", "-p", &port, "-t", "ed25519,ecdsa,rsa", "--", &address.to_string()]);
    let scanned = runas::output(scan, QUICK).await?;
    let keys = parse_host_keys(&scanned.stdout);
    if keys.is_empty() {
        return Err(bad(format!("{}:{} adresinde SSH sunucusu yanıt vermedi; adresi ve bağlantı noktasını denetleyin", source.host, source.port)));
    }
    let host_key = known_hosts(&keys);
    let workspace = Workspace::create(&user, source, address, &host_key)?;
    let path = &source.path;
    // Yol doğrulandığı için komuta olduğu gibi girer. Ayar dosyası en sona yazdırılır.
    let script = format!(
        "P={path}; command -v tar >/dev/null 2>&1 && echo pix:tar; \
         (command -v mariadb-dump || command -v mysqldump) >/dev/null 2>&1 && echo pix:dump; \
         if [ -d \"$P\" ]; then echo pix:dir; du -sk \"$P\" 2>/dev/null | awk '{{print \"pix:size \" $1}}'; \
         if [ -f \"$P/wp-config.php\" ]; then echo pix:config; head -c 65536 \"$P/wp-config.php\"; fi; fi"
    );
    let output = runas::output(workspace.ssh(source, &script), QUICK).await?;
    if !output.success() && !output.stdout.contains("pix:") {
        return Err(connection_error(&output.stderr));
    }
    let (head, config) = match output.stdout.split_once("pix:config\n") {
        Some((head, config)) => (head.to_string(), Some(config.to_string())),
        None => (output.stdout.clone(), None),
    };
    let has = |marker: &str| head.lines().any(|line| line.trim() == marker);
    if !has("pix:tar") {
        return Err(bad("Karşı sunucuda tar komutu bulunamadı; dosyalar bu yolla taşınamaz"));
    }
    let size_kb = head.lines().find_map(|line| line.strip_prefix("pix:size ")).and_then(|value| value.trim().parse().ok());
    let wordpress = config
        .as_deref()
        .and_then(wordpress::parse_config_database)
        .map(|database| ProbeWordPress { database: database.name, database_host: database.host });
    Ok(Probe {
        fingerprints: keys.iter().map(|(kind, key)| fingerprint(kind, key)).collect(),
        host_key,
        address: address.to_string(),
        path_found: has("pix:dir"),
        size_kb,
        wordpress,
        can_dump: has("pix:dump"),
    })
}

/// İki süreci birbirine bağlar: `producer`ın çıktısı `consumer`ın girdisine akar, ikisinin de
/// hata satırları görev günlüğüne yazılır. Çıkış kodlarını döner.
async fn pipe(ctx: &JobCtx, mut producer: Command, mut consumer: Command) -> AppResult<(i32, i32, String)> {
    let mut first = producer.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|err| AppError::internal(format!("ssh başlatılamadı: {err}")))?;
    let stream: Stdio = first.stdout.take().expect("stdout borusu").try_into().map_err(|err: std::io::Error| AppError::internal(err.to_string()))?;
    let mut second = consumer.stdin(stream).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|err| AppError::internal(format!("komut başlatılamadı: {err}")))?;
    let first_log = spawn_log(ctx, first.stderr.take().expect("stderr borusu"));
    let second_log = spawn_log(ctx, second.stderr.take().expect("stderr borusu"));
    let second_status = second.wait().await?;
    let first_status = first.wait().await?;
    let remote_errors = first_log.await.unwrap_or_default();
    let _ = second_log.await;
    Ok((first_status.code().unwrap_or(-1), second_status.code().unwrap_or(-1), remote_errors))
}

fn spawn_log(ctx: &JobCtx, stderr: tokio::process::ChildStderr) -> tokio::task::JoinHandle<String> {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let (mut count, mut all) = (0, String::new());
        while let Ok(Some(line)) = lines.next_line().await {
            if count < MAX_LOG_LINES {
                ctx.log(line.clone());
                all.push_str(&line);
                all.push('\n');
            }
            count += 1;
        }
        all
    })
}

/// Dosyaları karşı sunucudan yayın klasörüne çeker.
async fn pull_files(ctx: &JobCtx, workspace: &Workspace, source: &Source, site: &Site) -> AppResult<()> {
    let docroot = site.docroot();
    let user = &workspace.user;
    // Panelin karşılama sayfası tek başına duruyorsa kaldırılır; yoksa taşınan sitenin
    // index.php'sinin önüne geçerdi.
    let listing = runas::output(runas::command(user, &docroot, "find", &[".", "-mindepth", "1", "-maxdepth", "1", "-not", "-name", "index.html", "-print", "-quit"]), QUICK).await?;
    if listing.success() && listing.stdout.trim().is_empty() {
        let _ = runas::output(runas::command(user, &docroot, "rm", &["-f", "--", "index.html"]), QUICK).await;
    }
    ctx.log(format!("Dosyalar çekiliyor: {}:{} → {}", source.host, source.path, docroot.display()));
    let remote = format!("tar -C {} -czf - .", source.path);
    // Sahiplik hesabın kendisine geçer; arşivdeki yollar yayın klasörünün dışına çıkamaz
    // (tar mutlak yolları ve `..` içerenleri reddeder).
    let extract = runas::command(
        user,
        &docroot,
        "tar",
        &["-xzf", "-", "--no-same-owner", "--no-same-permissions", "--checkpoint=20000", "--checkpoint-action=echo=%T", "-C", "."],
    );
    let (remote_code, local_code, remote_errors) = pipe(ctx, workspace.ssh(source, &remote), extract).await?;
    // Karşıdaki tar, okurken değişen dosyalar için 1 ile çıkar; bu bir uyarıdır.
    if remote_code > 1 || remote_code < 0 {
        return Err(if remote_code == 255 { connection_error(&remote_errors) } else { bad("Karşı sunucudaki dosyalar okunamadı (ayrıntı yukarıda)") });
    }
    if local_code != 0 {
        return Err(bad("Dosyalar yayın klasörüne açılamadı (ayrıntı yukarıda); disk dolmuş ya da kota aşılmış olabilir"));
    }
    if let Ok(output) = runas::output(runas::command(user, &docroot, "du", &["-sh", "--", "."]), Duration::from_secs(300)).await {
        if let Some(total) = output.stdout.split_whitespace().next() {
            ctx.log(format!("Taşınan dosyaların boyutu: {total}"));
        }
    }
    Ok(())
}

/// Karşıdaki veritabanı sunucusunun adresi (`DB_HOST`): ad, ad:port ya da ad:/soket/yolu.
fn dump_target(host: &str) -> AppResult<Vec<String>> {
    let (name, extra) = match host.split_once(':') {
        Some((name, extra)) => (name, Some(extra)),
        None => (host, None),
    };
    let name = if name.is_empty() { "localhost" } else { name };
    let safe = |value: &str, allowed: &[u8]| !value.is_empty() && value.len() <= 255 && !value.starts_with('-') && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || allowed.contains(&byte));
    if !safe(name, b"._-") {
        return Err(bad("Karşı sitenin veritabanı sunucusu adresi (DB_HOST) tanınmadı"));
    }
    let mut args = vec!["-h".to_string(), name.to_string()];
    match extra {
        Some(port) if port.parse::<u16>().is_ok() => args.extend(["-P".to_string(), port.to_string()]),
        Some(socket) if socket.starts_with('/') && safe(socket, b"._-/") => args.extend(["-S".to_string(), socket.to_string()]),
        Some(_) => return Err(bad("Karşı sitenin veritabanı sunucusu adresi (DB_HOST) tanınmadı")),
        None => {}
    }
    Ok(args)
}

/// Döküm aracına parolayı taşıyan seçenek dosyasının içeriği (komut satırına yazılmaz).
fn client_options(database: &ConfigDatabase) -> AppResult<String> {
    let quote = |value: &str| -> AppResult<String> {
        if value.chars().any(|ch| ch == '\n' || ch == '\r' || ch == '\0') {
            return Err(bad("Karşı sitenin veritabanı bilgileri okunamadı"));
        }
        Ok(format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
    };
    Ok(format!("[client]\nuser={}\npassword={}\n", quote(&database.user)?, quote(&database.password)?))
}

/// Karşıdaki veritabanını döküp `database` adlı yerel veritabanına yükler.
async fn pull_database(ctx: &JobCtx, workspace: &Workspace, source: &Source, remote: &ConfigDatabase, database: &str) -> AppResult<()> {
    if remote.name.is_empty() || remote.name.len() > 64 || !remote.name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') || remote.name.starts_with('-') {
        return Err(bad("Karşı sitenin veritabanı adı desteklenmeyen karakterler içeriyor; veritabanını elle taşıyın"));
    }
    let target = dump_target(&remote.host)?.join(" ");
    let options = client_options(remote)?;
    let script = format!(
        "D=$(command -v mariadb-dump || command -v mysqldump) || {{ echo 'veritabani dokum araci bulunamadi' >&2; exit 127; }}; \
         \"$D\" --defaults-extra-file=/dev/stdin --single-transaction --quick --no-tablespaces --skip-lock-tables --default-character-set=utf8mb4 {target} -- {}",
        remote.name
    );
    let mut ssh = workspace.ssh(source, &script);
    let mut child = ssh.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|err| AppError::internal(format!("ssh başlatılamadı: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(options.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let stream: Stdio = child.stdout.take().expect("stdout borusu").try_into().map_err(|err: std::io::Error| AppError::internal(err.to_string()))?;
    let errors = spawn_log(ctx, child.stderr.take().expect("stderr borusu"));
    let imported = dbms::import::run(ctx, Engine::Mariadb, database, stream).await;
    let status = child.wait().await?;
    let remote_errors = errors.await.unwrap_or_default();
    if !status.success() {
        return Err(match status.code() {
            Some(255) => connection_error(&remote_errors),
            Some(127) => bad("Karşı sunucuda veritabanı döküm aracı (mysqldump) bulunamadı; veritabanını elle taşıyın"),
            _ => bad("Karşı sunucudaki veritabanı dökülemedi (ayrıntı yukarıda)"),
        });
    }
    imported
}

/// `.env` dosyasındaki bir anahtarın değerini değiştirir (anahtar yoksa metne dokunmaz).
fn replace_env(text: &str, key: &str, value: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let matches = body.trim_start().strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='));
        if matches {
            out.push_str(&format!("{key}={value}"));
            out.push_str(&line[body.len()..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// `.env` metnindeki bir anahtarın değeri (tırnaklar atılır).
fn env_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(key)?.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// WordPress dışındaki uygulamanın veritabanını taşır: bu hesapta yeni bir veritabanı açılır,
/// karşıdaki dökülüp yüklenir. Yayın klasöründe eski veritabanını gösteren bir `.env` varsa yeni
/// bilgilerle güncellenir; bağlantı bilgileri ayrıca hesabın `tmp` klasörüne yazılır (parola
/// görev günlüğüne düşmez).
async fn adopt_plain_database(ctx: &JobCtx, state: &AppState, site: &Site, workspace: &Workspace, source: &Source, remote: &ConfigDatabase) -> AppResult<()> {
    let user = &workspace.user;
    ctx.log(format!("Veritabanı taşınıyor: {}", remote.name));
    let (database, db_user, db_password) = wordpress::create_site_database(state, site, "db").await?;
    if let Err(err) = pull_database(ctx, workspace, source, remote, &database).await {
        wordpress::drop_database(state, &database, true).await;
        return Err(err);
    }
    ctx.log(format!("Veritabanı yüklendi: {database} (kullanıcı: {db_user})"));

    let docroot = site.docroot();
    let mut updated_env = false;
    let env = runas::output(runas::command(user, &docroot, "cat", &["--", ".env"]), QUICK).await;
    if let Ok(env) = env {
        if env.success() && env_value(&env.stdout, "DB_DATABASE").as_deref() == Some(remote.name.as_str()) {
            let mut text = env.stdout.clone();
            for (key, value) in [("DB_DATABASE", database.as_str()), ("DB_USERNAME", db_user.as_str()), ("DB_PASSWORD", db_password.as_str()), ("DB_HOST", "127.0.0.1"), ("DB_PORT", "3306")] {
                text = replace_env(&text, key, value);
            }
            if write_file(user, &docroot, ".env", &text).await.is_ok() {
                updated_env = true;
                ctx.log(".env dosyasındaki veritabanı ayarları yeni veritabanına çevrildi.");
            }
        }
    }

    let tmp = user.tmp_dir();
    std::fs::create_dir_all(&tmp).ok();
    let _ = std::os::unix::fs::chown(&tmp, Some(user.uid), Some(user.gid));
    let note_name = format!("veritabani-{database}.txt");
    let note = format!("Veritabanı: {database}\nKullanıcı: {db_user}\nParola: {db_password}\nSunucu: localhost\n");
    match write_file(user, &tmp, &note_name, &note).await {
        Ok(()) => {
            let _ = runas::output(runas::command(user, &tmp, "chmod", &["600", "--", &note_name]), QUICK).await;
            let hint = if updated_env { "Uygulama başka bir ayar dosyası kullanıyorsa oradan güncelleyin; işiniz bitince dosyayı silin." } else { "Uygulamanın ayar dosyasını bu bilgilerle güncelleyin, ardından dosyayı silin." };
            ctx.log(format!("Yeni veritabanının bağlantı bilgileri hesabın tmp/{note_name} dosyasına yazıldı. {hint}"));
        }
        Err(_) => ctx.log("UYARI: bağlantı bilgileri dosyaya yazılamadı; Veritabanları sayfasından kullanıcıya yeni bir parola verin."),
    }
    Ok(())
}

/// Dosyayı hesabın kullanıcısı olarak yazar (içerik `tee`nin girdisinden verilir).
async fn write_file(user: &SysUser, dir: &Path, name: &str, content: &str) -> AppResult<()> {
    let mut cmd = runas::command(user, dir, "tee", &["--", name]);
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("dosya yazılamadı: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes()).await.map_err(|err| AppError::internal(err.to_string()))?;
        let _ = stdin.shutdown().await;
    }
    let status = tokio::time::timeout(QUICK, child.wait()).await.map_err(|_| AppError::internal("dosya yazma zaman aşımına uğradı"))??;
    if status.success() { Ok(()) } else { Err(AppError::internal("dosya yazılamadı")) }
}

/// Taşıma görevinin gövdesi.
pub async fn run(ctx: &JobCtx, state: &AppState, site: &Site, source: &Source, host_key: &str, with_database: bool, manual: Option<ConfigDatabase>) -> AppResult<()> {
    let user = account(site)?;
    let keys = parse_host_keys(host_key);
    if keys.is_empty() {
        return Err(bad("Sunucu anahtarı onaylanmamış; önce bağlantıyı deneyin"));
    }
    let address = resolve(source).await?;
    let workspace = Workspace::create(&user, source, address, &known_hosts(&keys))?;
    pull_files(ctx, &workspace, source, site).await?;

    if let Some(remote) = manual {
        adopt_plain_database(ctx, state, site, &workspace, source, &remote).await?;
    } else if with_database {
        match wordpress::config_database(site).await? {
            None => ctx.log("Taşınan dosyalarda wp-config.php bulunamadı; veritabanı taşınmadı."),
            Some(remote) => {
                ctx.log(format!("WordPress veritabanı taşınıyor: {}", remote.name));
                let workspace = &workspace;
                wordpress::adopt_database(ctx, state, site, |database| async move { pull_database(ctx, &workspace, source, &remote, &database).await }).await?;
            }
        }
    }
    ctx.log(format!("{} taşındı. Alan adının DNS kayıtlarını bu sunucuya çevirmeden önce siteyi denetleyin.", site.domain));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: &str) -> Source {
        Source { host: "Eski.Ornek.com ".into(), port: 22, username: "musteri".into(), password: "gizli".into(), private_key: String::new(), path: path.into() }
    }

    #[test]
    fn remote_path_allows_only_plain_paths() {
        for path in ["/var/www/vhosts/ornek.com/httpdocs", "~/public_html", "/home/a_b/site-1/"] {
            assert!(source(path).validate().is_ok(), "{path}");
        }
        for path in ["public_html", "/var/www/$(id)", "/var/www/a b", "/var/www/a;id", "/var/../etc", "/var/-rf", "~root/x", "/a`id`", "/a'b", ""] {
            assert!(source(path).validate().is_err(), "{path}");
        }
    }

    #[test]
    fn connection_fields_are_validated() {
        let mut ok = source("/srv/site");
        ok.validate().unwrap();
        assert_eq!(ok.host, "eski.ornek.com");
        for host in ["-oProxyCommand=id", "a b", "ornek.com/x", ""] {
            let mut bad = source("/srv/site");
            bad.host = host.into();
            assert!(bad.validate().is_err(), "{host}");
        }
        for name in ["-l", "a b", "root;id", ""] {
            let mut bad = source("/srv/site");
            bad.username = name.into();
            assert!(bad.validate().is_err(), "{name}");
        }
        let mut both = source("/srv/site");
        both.private_key = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----".into();
        assert!(both.validate().is_err());
        both.password.clear();
        assert!(both.validate().is_ok());
        let mut neither = source("/srv/site");
        neither.password.clear();
        assert!(neither.validate().is_err());
    }

    #[test]
    fn host_keys_are_filtered() {
        let text = "1.2.3.4 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGJ1\n# yorum\n1.2.3.4 ssh-dss AAAA\n1.2.3.4 ssh-rsa bozuk!\n[1.2.3.4]:2222 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGJ2\n";
        let keys = parse_host_keys(text);
        assert_eq!(keys.len(), 1);
        assert_eq!(known_hosts(&keys), "pixpanel-migrate ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGJ1\n");
        assert!(fingerprint(&keys[0].0, &keys[0].1).starts_with("ED25519 SHA256:"));
        // İstemcinin geri gönderdiği metin aynı ayrıştırıcıdan geçer.
        assert_eq!(parse_host_keys(&known_hosts(&keys)), keys);
    }

    #[test]
    fn env_file_is_rewritten() {
        let text = "APP_NAME=Site\nDB_DATABASE=eski\n  DB_PASSWORD = \"gizli\"\r\nDB_PASSWORD_EXTRA=1\nSON=1";
        assert_eq!(env_value(text, "DB_DATABASE").as_deref(), Some("eski"));
        assert_eq!(env_value(text, "DB_PASSWORD").as_deref(), Some("gizli"));
        assert_eq!(env_value(text, "YOK"), None);
        let out = replace_env(&replace_env(text, "DB_DATABASE", "yeni"), "DB_PASSWORD", "Abc123");
        assert_eq!(out, "APP_NAME=Site\nDB_DATABASE=yeni\nDB_PASSWORD=Abc123\r\nDB_PASSWORD_EXTRA=1\nSON=1");
    }

    #[test]
    fn database_host_forms() {
        assert_eq!(dump_target("localhost").unwrap(), ["-h", "localhost"]);
        assert_eq!(dump_target("db.ic:3307").unwrap(), ["-h", "db.ic", "-P", "3307"]);
        assert_eq!(dump_target("localhost:/run/mysqld/mysqld.sock").unwrap(), ["-h", "localhost", "-S", "/run/mysqld/mysqld.sock"]);
        assert_eq!(dump_target(":/tmp/mysql.sock").unwrap(), ["-h", "localhost", "-S", "/tmp/mysql.sock"]);
        for host in ["a;id", "$(id)", "a:b c", "-h", "a:/tmp/$(id)"] {
            assert!(dump_target(host).is_err(), "{host}");
        }
    }

    #[test]
    fn client_options_escape_secrets() {
        let database = ConfigDatabase { name: "wp".into(), user: "wp_user".into(), password: "a\"b\\c#d".into(), host: "localhost".into() };
        assert_eq!(client_options(&database).unwrap(), "[client]\nuser=\"wp_user\"\npassword=\"a\\\"b\\\\c#d\"\n");
        let broken = ConfigDatabase { password: "a\nb".into(), ..database };
        assert!(client_options(&broken).is_err());
    }
}
