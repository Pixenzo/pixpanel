//! Hesaplara SFTP ve kabuk (SSH) erişimi.
//!
//! Hesabın Linux kullanıcısı varsayılan olarak hiçbir biçimde SSH ile giremez
//! (kabuğu `nologin`). Erişim iki düzeyde açılır:
//!
//! - `sftp`: yalnızca dosya aktarımı; kullanıcı kendi ev klasörüne hapsedilir
//!   (`ChrootDirectory`), komut çalıştıramaz, port yönlendiremez.
//! - `shell`: tam kabuk. Paylaşımlı sunucuda risklidir; yalnızca yönetici verebilir.
//!
//! Açık anahtarlar kullanıcının kendi klasöründe değil, yalnızca root'un yazabildiği
//! `/etc/pixpanel/ssh/<kullanıcı>.keys` dosyasında durur: böylece sitesi ele geçirilen
//! bir hesap kendine anahtar ekleyemez. sshd ayarı panelin kendi dosyasında tutulur ve
//! `sshd -t` ile doğrulanmadan uygulanmaz.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::hosting::sysuser;
use crate::sys::exec;

pub const MODES: &[&str] = &["none", "sftp", "shell"];
const SFTP_GROUP: &str = "pixpanel-sftp";
const SHELL_GROUP: &str = "pixpanel-shell";
const KEYS_DIR: &str = "/etc/pixpanel/ssh";
const SSHD_DROP_IN: &str = "/etc/ssh/sshd_config.d/60-pixpanel.conf";
const SHELL: &str = "/bin/bash";
const NO_SHELL: &str = "/usr/sbin/nologin";
pub const MAX_KEYS: usize = 20;

/// Kabul edilen açık anahtar türleri.
const KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

pub fn is_available() -> bool {
    exec::binary_exists("sshd")
}

fn keys_path(username: &str) -> PathBuf {
    Path::new(KEYS_DIR).join(format!("{username}.keys"))
}

/// Panelin sshd ayarı. İki grup için ayrı kurallar; dosya `Match all` ile biter ki
/// buradaki kurallar bu dosyadan sonra okunan ayarlara taşmasın.
fn sshd_config() -> String {
    format!(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         Match Group {SFTP_GROUP}\n\
         \x20   ChrootDirectory /home/%u\n\
         \x20   ForceCommand internal-sftp -d /sites\n\
         \x20   AuthorizedKeysFile {KEYS_DIR}/%u.keys\n\
         \x20   PasswordAuthentication yes\n\
         \x20   AllowTcpForwarding no\n\
         \x20   AllowAgentForwarding no\n\
         \x20   X11Forwarding no\n\
         \x20   PermitTunnel no\n\
         Match Group {SHELL_GROUP}\n\
         \x20   AuthorizedKeysFile {KEYS_DIR}/%u.keys\n\
         \x20   PasswordAuthentication no\n\
         \x20   AllowTcpForwarding no\n\
         \x20   AllowAgentForwarding no\n\
         \x20   X11Forwarding no\n\
         \x20   PermitTunnel no\n\
         Match all\n"
    )
}

/// Grupları ve sshd ayarını hazırlar. Ayar değiştiyse doğrulanır; geçersizse eski hâli geri konur.
async fn ensure_sshd() -> AppResult<()> {
    if !is_available() {
        return Err(AppError::bad("Bu sunucuda SSH sunucusu (sshd) kurulu değil"));
    }
    for group in [SFTP_GROUP, SHELL_GROUP] {
        exec::run_ok("groupadd", &["--force", "--system", group]).await?;
    }
    std::fs::create_dir_all(KEYS_DIR)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // sshd anahtar dosyasını kullanıcı adına okur; klasör herkese geçilebilir, yalnızca root yazabilir olmalı.
        std::fs::set_permissions(KEYS_DIR, std::fs::Permissions::from_mode(0o755))?;
    }
    let wanted = sshd_config();
    let path = Path::new(SSHD_DROP_IN);
    let previous = std::fs::read_to_string(path).ok();
    if previous.as_deref() == Some(wanted.as_str()) {
        return Ok(());
    }
    atomic_write(path, wanted.as_bytes(), 0o644)?;
    let test = exec::run("sshd", &["-t"]).await?;
    if !test.success() {
        match previous {
            Some(content) => atomic_write(path, content.as_bytes(), 0o644)?,
            None => std::fs::remove_file(path)?,
        }
        return Err(AppError::internal(format!("SSH yapılandırması geçersiz, uygulanmadı: {}", test.stderr.trim())));
    }
    // Çalışan sshd yeni ayarı yeniden yükleyince görür; yeniden yükleme açık bağlantıları
    // (yöneticinin kendi oturumunu) kesmez. sshd hiç çalışmıyorsa (soketle, bağlantı geldikçe
    // başlatılıyor ve henüz bağlanan olmadıysa) yapılacak bir şey yoktur: ilk bağlantı ayarı okur.
    //
    // Soketle başlatılmış sshd yeniden yüklenemez (Debian 13: dinleme soketi systemd'den gelir,
    // sshd kendini yeniden çalıştırırken onu bulamaz ve servis hata verir). O durumda servis
    // yeniden başlatılır; açık oturumlar ayrı süreçlerde yürüdüğünden bundan etkilenmez.
    let socket_activated = crate::sys::services::is_active("ssh.socket").await;
    for unit in ["ssh.service", "sshd.service"] {
        if !crate::sys::services::is_active(unit).await {
            continue;
        }
        let action = if socket_activated { "restart" } else { "reload" };
        if exec::run_ok("systemctl", &[action, "--", unit]).await.is_err() {
            exec::run_ok("systemctl", &["restart", "--", unit]).await?;
        }
    }
    Ok(())
}

/// Hesabın erişim düzeyini uygular: grup üyeliği ve kabuk.
pub async fn apply_mode(username: &str, mode: &str) -> AppResult<()> {
    if !MODES.contains(&mode) {
        return Err(AppError::bad("Geçersiz erişim düzeyi"));
    }
    if mode == "none" && sysuser::lookup(username).is_none() {
        return Ok(());
    }
    ensure_sshd().await?;
    let user = sysuser::ensure(username).await?;
    for (group, wanted) in [(SFTP_GROUP, mode == "sftp"), (SHELL_GROUP, mode == "shell")] {
        // `gpasswd -d`, üye olmayan kullanıcıda hata döner; bu bir sorun değildir.
        let action = if wanted { "-a" } else { "-d" };
        let result = exec::run("gpasswd", &[action, &user.name, group]).await?;
        if wanted && !result.success() {
            return Err(AppError::internal(format!("Kullanıcı {group} grubuna eklenemedi")));
        }
    }
    let shell = if mode == "shell" { SHELL } else { NO_SHELL };
    exec::run_ok("usermod", &["--shell", shell, "--", &user.name]).await?;
    if mode == "none" {
        // Erişim kapatılınca parola da kilitlenir; yeniden açıldığında yeni parola verilmesi gerekir.
        let _ = exec::run("passwd", &["--lock", "--", &user.name]).await;
    }
    write_keys(&user.name, &load_key_lines(&user.name)).await?;
    Ok(())
}

/// Kullanıcının SFTP parolasını belirler. Parola komut satırına değil, `chpasswd`nin girdisine yazılır.
pub async fn set_password(username: &str, password: &str) -> AppResult<()> {
    validate_password(password)?;
    let user = sysuser::lookup(username).ok_or_else(|| AppError::bad("Önce bu hesap için SFTP erişimini açın"))?;
    use tokio::io::AsyncWriteExt;
    let mut cmd = exec::base_command("chpasswd");
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("chpasswd başlatılamadı: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(format!("{}:{password}\n", user.name).as_bytes()).await.map_err(|err| AppError::internal(err.to_string()))?;
        let _ = stdin.shutdown().await;
    }
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| AppError::internal("parola ayarlama zaman aşımına uğradı"))?
        .map_err(|err| AppError::internal(err.to_string()))?;
    if output.status.success() { Ok(()) } else { Err(AppError::internal("Parola ayarlanamadı")) }
}

/// Parola `chpasswd`ye `kullanıcı:parola` satırı olarak gider; satır sonu ve iki nokta
/// biçimi bozabileceğinden yalnızca yazdırılabilir ASCII kabul edilir.
pub fn validate_password(password: &str) -> AppResult<()> {
    if !(12..=128).contains(&password.len()) {
        return Err(AppError::bad("SFTP parolası 12-128 karakter olmalı"));
    }
    if !password.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(AppError::bad("SFTP parolası yalnızca harf, rakam ve yaygın işaretlerden oluşabilir (boşluk ve Türkçe karakter olmadan)"));
    }
    Ok(())
}

/// Doğrulanmış bir açık anahtar.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicKey {
    /// `tür anahtar` biçiminde, yorum ve seçenek içermeyen tek satır.
    pub line: String,
    /// `SHA256:…` parmak izi (ssh-keygen ile aynı biçim).
    pub fingerprint: String,
    /// Anahtarın sonundaki yorum (çoğunlukla `kullanıcı@makine`).
    pub comment: String,
}

/// Kullanıcının yapıştırdığı açık anahtarı doğrular. Yalnızca `tür anahtar [yorum]` biçimi
/// kabul edilir: başına `command="…"` gibi seçenek eklenmiş satırlar reddedilir, çünkü
/// `authorized_keys` seçenekleri sshd'nin davranışını değiştirir.
pub fn parse_key(input: &str) -> AppResult<PublicKey> {
    let text = input.trim();
    if text.is_empty() || text.len() > 8192 || text.chars().any(|c| c.is_control()) {
        return Err(AppError::bad("Açık anahtar tek satır olmalı"));
    }
    let mut parts = text.split_whitespace();
    let (kind, data) = match (parts.next(), parts.next()) {
        (Some(kind), Some(data)) => (kind, data),
        _ => return Err(AppError::bad("Açık anahtar “ssh-ed25519 AAAA…” biçiminde olmalı")),
    };
    if !KEY_TYPES.contains(&kind) {
        return Err(AppError::bad("Desteklenmeyen anahtar türü; ed25519, ECDSA ya da RSA açık anahtarı yapıştırın (özel anahtarı değil)"));
    }
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| AppError::bad("Açık anahtarın gövdesi bozuk"))?;
    // Gövde, anahtar türünün adını uzunluk önekiyle yineleyerek başlar; satırdaki türle tutmalıdır.
    let declared = blob.get(4..).and_then(|rest| {
        let length = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
        rest.get(..length)
    });
    if declared != Some(kind.as_bytes()) {
        return Err(AppError::bad("Açık anahtarın türü gövdesiyle uyuşmuyor"));
    }
    if kind == "ssh-rsa" && blob.len() < 270 {
        return Err(AppError::bad("RSA anahtarı çok kısa; en az 2048 bit olmalı"));
    }
    let digest = base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&blob));
    let comment: String = parts.collect::<Vec<_>>().join(" ").chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(100).collect();
    Ok(PublicKey { line: format!("{kind} {data}"), fingerprint: format!("SHA256:{digest}"), comment })
}

/// Diskteki anahtar dosyasının satırları (panelin kendi yazdığı, doğrulanmış satırlar).
fn load_key_lines(username: &str) -> Vec<String> {
    std::fs::read_to_string(keys_path(username)).map(|content| content.lines().filter(|line| !line.trim().is_empty() && !line.starts_with('#')).map(str::to_string).collect()).unwrap_or_default()
}

/// Kullanıcının anahtar dosyasını verilen satırlarla yeniden yazar. Her satır port
/// yönlendirmeyi ve benzeri yetenekleri kapatan seçeneklerle başlar.
pub async fn write_keys(username: &str, lines: &[String]) -> AppResult<()> {
    sysuser::validate_name(username)?;
    std::fs::create_dir_all(KEYS_DIR)?;
    let mut content = String::from("# PixPanel tarafından üretildi. Anahtarları panelden yönetin.\n");
    for line in lines {
        // Daha önce yazılmış satırda seçenekler zaten vardır; yalnızca anahtar kısmı alınır.
        let key = line.strip_prefix("restrict,pty ").unwrap_or(line);
        content.push_str("restrict,pty ");
        content.push_str(key);
        content.push('\n');
    }
    atomic_write(&keys_path(username), content.as_bytes(), 0o644)
}

/// Hesap silinirken anahtar dosyası da kaldırılır.
pub fn remove_keys(username: &str) {
    let _ = std::fs::remove_file(keys_path(username));
}

/// sshd'nin dinlediği port (bağlantı bilgisini göstermek için).
pub async fn port() -> u16 {
    exec::run("sshd", &["-T"])
        .await
        .ok()
        .and_then(|output| output.stdout.lines().find_map(|line| line.strip_prefix("port ")).and_then(|port| port.trim().parse().ok()))
        .unwrap_or(22)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHcF8nBqBh0sDpJ0o0yH3pYVZ0q0Uo1xk3o8W2b9v4Zt";

    #[test]
    fn valid_key_is_normalized() {
        let key = parse_key(&format!("  {ED25519}   ali@dizustu  \n")).unwrap();
        assert_eq!(key.line, ED25519);
        assert_eq!(key.comment, "ali@dizustu");
        assert!(key.fingerprint.starts_with("SHA256:") && key.fingerprint.len() == 50);
    }

    #[test]
    fn options_and_foreign_formats_are_rejected() {
        for input in [
            "",
            "ssh-ed25519",
            &format!("command=\"id\" {ED25519}"),
            &format!("no-pty,{ED25519}"),
            &format!("{ED25519}\nssh-ed25519 AAAA"),
            "ssh-dss AAAAB3NzaC1kc3MAAACBAP",
            "ssh-ed25519 bozuk!!govde",
            // Tür satırda ed25519, gövdede rsa.
            "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQC7",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
        ] {
            assert!(parse_key(input).is_err(), "{input}");
        }
    }

    #[test]
    fn password_must_be_printable_ascii() {
        assert!(validate_password("Uzun-Parola-12345").is_ok());
        for password in ["kisa", "parola iki nokta:olmaz ama bosluk", "satir\nsonu-123456", "türkçe-karakter-123"] {
            assert!(validate_password(password).is_err(), "{password}");
        }
        // İki nokta üst üste parolanın içinde olabilir: chpasswd ilk iki noktadan böler.
        assert!(validate_password("iki:nokta:olan-parola").is_ok());
    }

    #[test]
    fn sshd_rules_do_not_leak_past_the_file() {
        let config = sshd_config();
        assert!(config.trim_end().ends_with("Match all"));
        assert!(config.contains("ChrootDirectory /home/%u"));
        assert!(config.contains("ForceCommand internal-sftp"));
        // Kabuk erişimi yalnızca anahtarla.
        let shell_block = config.split("Match Group pixpanel-shell").nth(1).unwrap();
        assert!(shell_block.contains("PasswordAuthentication no"));
    }
}
