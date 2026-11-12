//! Komutları bir hosting hesabının Linux kullanıcısı adına çalıştırma.
//!
//! Kullanıcının dosyalarına dokunan her şey (git, yayın betikleri, anahtar
//! üretimi) root yerine o kullanıcının kimliğiyle yürür; böylece sembolik bağ
//! gibi hilelerle panelin yetkisi kötüye kullanılamaz.
//!
//! **Kaynak dilimi:** varsayılan olarak komut panelin kendi cgroup'unda çalışır. Ağır işler
//! (wp-cli, composer, git, arşiv, rsync, bağımlılık kurulumu) `heavy_command` ile hesabın
//! `user-<uid>.slice` diliminde çalıştırılır; böylece hesabın paketindeki işlemci/bellek/süreç
//! sınırı panelin onun adına yaptığı işlere de uygulanır. Dilime alma `systemd-run --scope`
//! iledir; kullanılamıyorsa (konteynerde D-Bus yok vb.) sessizce eski yola düşülür.

use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::hosting::sysuser::SysUser;
use crate::sys::exec::CmdOutput;

pub const USER_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Komutun hangi cgroup'ta koşacağı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Panelin kendi cgroup'unda (kısa, sık, salt okuma komutları: ek süreç maliyeti yok).
    Panel,
    /// Hesabın `user-<uid>.slice` diliminde (ağır işler).
    AccountSlice,
}

/// `systemd-run --scope` kullanılabiliyor mu? Açılışta bir kez sınanır (`probe_slice_support`); sınanmadıysa
/// ya da sınama başarısızsa `false` sayılır ve komutlar eski yoldan, panelin cgroup'unda çalışır.
static SLICE_SUPPORT: OnceLock<bool> = OnceLock::new();

pub fn slice_support() -> bool {
    SLICE_SUPPORT.get().copied().unwrap_or(false)
}

/// Dilime alma kararı. Hesabın diliminde uygulanacak bir sınır yoksa (drop-in yok) sarmalamanın
/// bir yararı olmadığından ek süreç başlatılmaz.
pub fn should_wrap(placement: Placement, scope_supported: bool, account_has_limits: bool) -> bool {
    placement == Placement::AccountSlice && scope_supported && account_has_limits
}

/// Komutun başına konacak `systemd-run` satırı. Kapsam (scope) kipi hizmet kipinden farklı olarak çağıran
/// süreci olduğu gibi bir cgroup kapsamına alır ve komutu aynı süreçte çalıştırır (`exec`): standart
/// girdi/çıktı/hata ve çıkış kodu aynen korunur, `--pipe`/`--wait` gerekmez. Kapsam iş bitince
/// kendiliğinden kalkar (`--collect`).
pub fn scope_prefix(uid: u32) -> Vec<String> {
    vec![
        "systemd-run".into(),
        "--scope".into(),
        format!("--slice={}", crate::hosting::resource_limits::slice_name(uid)),
        "--quiet".into(),
        "--collect".into(),
        "--no-ask-password".into(),
        "--".into(),
    ]
}

/// Dilimsiz kapsam satırı (yalnızca destek sınaması için).
fn probe_args() -> [&'static str; 6] {
    ["--scope", "--quiet", "--collect", "--no-ask-password", "--", "true"]
}

/// Açılışta bir kez: `systemd-run --scope` bu sunucuda çalışıyor mu (systemd PID 1 mi, D-Bus var mı)?
/// Sonuç önbelleğe alınır; çalışmıyorsa komutlar sessizce eski yola düşer.
pub async fn probe_slice_support() -> bool {
    if let Some(known) = SLICE_SUPPORT.get() {
        return *known;
    }
    let supported = if !Path::new("/run/systemd/system").is_dir() || !crate::sys::exec::binary_exists("systemd-run") {
        false
    } else {
        crate::sys::exec::run_with_timeout("systemd-run", &probe_args(), Duration::from_secs(15)).await.map(|out| out.success()).unwrap_or(false)
    };
    let _ = SLICE_SUPPORT.set(supported);
    tracing::info!(supported, "hesap diliminde komut çalıştırma (systemd-run --scope) sınandı");
    supported
}

/// Kullanıcının kimliğine düşüp verilen programı çalıştıran komutu hazırlar (panelin cgroup'unda).
/// Ortam sıfırdan kurulur; panelin ortam değişkenleri alt sürece sızmaz.
pub fn command(user: &SysUser, cwd: &Path, program: &str, args: &[&str]) -> Command {
    command_in(user, cwd, program, args, Placement::Panel)
}

/// Ağır işler için: komut hesabın kaynak diliminde çalışır (dilim kullanılamıyorsa `command` ile aynıdır).
pub fn heavy_command(user: &SysUser, cwd: &Path, program: &str, args: &[&str]) -> Command {
    command_in(user, cwd, program, args, Placement::AccountSlice)
}

pub fn command_in(user: &SysUser, cwd: &Path, program: &str, args: &[&str], placement: Placement) -> Command {
    let wrap = should_wrap(placement, slice_support(), crate::hosting::resource_limits::has_slice_limits(user.uid));
    let mut cmd = if wrap {
        let prefix = scope_prefix(user.uid);
        let mut cmd = Command::new(&prefix[0]);
        cmd.args(&prefix[1..]).arg("setpriv");
        cmd
    } else {
        Command::new("setpriv")
    };
    cmd.arg(format!("--reuid={}", user.uid))
        .arg(format!("--regid={}", user.gid))
        .arg("--init-groups")
        .arg("--")
        .arg(program)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", &user.home)
        .env("USER", &user.name)
        .env("LOGNAME", &user.name)
        .env("PATH", USER_PATH)
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    cmd
}

/// Kendi komut satırı olan ağır süreçler (dosya yardımcısı) için: dilime alma uygunsa komutu
/// `systemd-run --scope` ile sarar. Dönen `(program, ön argümanlar)` çiftinin ardına asıl programın
/// argümanları eklenir. Dilime alınmayacaksa program olduğu gibi döner.
pub fn wrap_for_account(uid: u32, program: std::path::PathBuf) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    let limited = uid != 0 && crate::hosting::resource_limits::has_slice_limits(uid);
    if should_wrap(Placement::AccountSlice, slice_support(), limited) {
        let prefix = scope_prefix(uid);
        let mut args: Vec<std::ffi::OsString> = prefix[1..].iter().map(Into::into).collect();
        args.push(program.into_os_string());
        (prefix[0].clone().into(), args)
    } else {
        (program.into_os_string(), Vec::new())
    }
}

/// Toplanan çıktının üst sınırı. Komut kullanıcının kodunu çalıştırabildiğinden (wp-cli,
/// yayın betikleri) sınırsız çıktı panelin belleğini tüketebilir.
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;

/// Hazırlanmış komutu çalıştırıp çıktısını toplar. Çıktı sınırı aşılırsa süreç
/// sonlandırılır ve hata dönülür.
pub async fn output(mut cmd: Command, timeout: Duration) -> AppResult<CmdOutput> {
    use tokio::io::AsyncReadExt;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|err| AppError::internal(format!("komut başlatılamadı: {err}")))?;
    let mut stdout_pipe = child.stdout.take().ok_or_else(|| AppError::internal("komut çıktısı okunamadı"))?;
    let mut stderr_pipe = child.stderr.take().ok_or_else(|| AppError::internal("komut çıktısı okunamadı"))?;

    let collect = async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        // Sınırdan bir bayt fazlası okunur; o bayt geldiyse çıktı sınırı aşmıştır.
        let mut limited_out = (&mut stdout_pipe).take(MAX_OUTPUT_BYTES + 1);
        let mut limited_err = (&mut stderr_pipe).take(MAX_OUTPUT_BYTES + 1);
        let (out, err) = tokio::join!(limited_out.read_to_end(&mut stdout), limited_err.read_to_end(&mut stderr));
        out.and(err).map_err(|err| AppError::internal(format!("komut çıktısı okunamadı: {err}")))?;
        if stdout.len() as u64 > MAX_OUTPUT_BYTES || stderr.len() as u64 > MAX_OUTPUT_BYTES {
            return Err(AppError::internal("komut çok fazla çıktı üretti"));
        }
        let status = child.wait().await.map_err(|err| AppError::internal(err.to_string()))?;
        Ok(CmdOutput {
            code: status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    };
    // Zaman aşımında ya da sınır aşımında `child` düşer; `kill_on_drop` süreci sonlandırır.
    tokio::time::timeout(timeout, collect).await.map_err(|_| AppError::internal("komut zaman aşımına uğradı"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_line_targets_the_account_slice() {
        let prefix = scope_prefix(1001);
        assert_eq!(prefix[0], "systemd-run");
        assert!(prefix.contains(&"--scope".to_string()));
        assert!(prefix.contains(&"--slice=user-1001.slice".to_string()));
        assert!(prefix.contains(&"--quiet".to_string()));
        assert!(prefix.contains(&"--collect".to_string()));
        // Hizmet kipi bayrakları karışmaz: standart girdi/çıktı ve çıkış kodu kapsam kipiyle olduğu gibi korunur.
        for forbidden in ["--pipe", "--wait", "--pty", "--uid", "--user"] {
            assert!(!prefix.iter().any(|arg| arg.starts_with(forbidden)), "{forbidden}");
        }
        assert_eq!(prefix.last().map(String::as_str), Some("--"), "program ayrı bir argümandır");
    }

    #[test]
    fn probe_uses_scope_mode_only() {
        let args = probe_args();
        assert!(args.contains(&"--scope"));
        assert!(!args.iter().any(|arg| arg.starts_with("--slice")));
        assert_eq!(args.last(), Some(&"true"));
    }

    #[test]
    fn wrap_decision() {
        // Yalnızca ağır çağrı, destek varsa ve hesabın diliminde sınır varsa sarılır.
        assert!(should_wrap(Placement::AccountSlice, true, true));
        assert!(!should_wrap(Placement::Panel, true, true), "hafif komutlar sarılmaz");
        assert!(!should_wrap(Placement::AccountSlice, false, true), "systemd-run yoksa eski yola düşülür");
        assert!(!should_wrap(Placement::AccountSlice, true, false), "sınırsız hesapta ek süreç gereksiz");
    }

    fn argv(cmd: &Command) -> Vec<String> {
        let std = cmd.as_std();
        std::iter::once(std.get_program()).chain(std.get_args()).map(|part| part.to_string_lossy().into_owned()).collect()
    }

    fn sample_user() -> SysUser {
        SysUser { name: "musteri".into(), uid: 1001, gid: 1001, home: "/home/musteri".into() }
    }

    #[test]
    fn command_line_is_unchanged_without_scope_support() {
        // Destek sınanmadığı sürece (birim testlerinde) her yerleşim eski yolu kullanır.
        let plain = argv(&command(&sample_user(), Path::new("/tmp"), "git", &["status"]));
        assert_eq!(plain, ["setpriv", "--reuid=1001", "--regid=1001", "--init-groups", "--", "git", "status"]);
        assert_eq!(argv(&heavy_command(&sample_user(), Path::new("/tmp"), "git", &["status"])), plain);
    }

    #[test]
    fn helper_wrapper_falls_back_without_support() {
        let (program, args) = wrap_for_account(1001, "/usr/bin/pixpanel".into());
        assert_eq!(program, std::ffi::OsString::from("/usr/bin/pixpanel"));
        assert!(args.is_empty());
    }
}
