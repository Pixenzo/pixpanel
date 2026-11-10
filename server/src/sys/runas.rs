//! Komutları bir hosting hesabının Linux kullanıcısı adına çalıştırma.
//!
//! Kullanıcının dosyalarına dokunan her şey (git, yayın betikleri, anahtar
//! üretimi) root yerine o kullanıcının kimliğiyle yürür; böylece sembolik bağ
//! gibi hilelerle panelin yetkisi kötüye kullanılamaz.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::hosting::sysuser::SysUser;
use crate::sys::exec::CmdOutput;

pub const USER_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Kullanıcının kimliğine düşüp verilen programı çalıştıran komutu hazırlar.
/// Ortam sıfırdan kurulur; panelin ortam değişkenleri alt sürece sızmaz.
pub fn command(user: &SysUser, cwd: &Path, program: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new("setpriv");
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

/// Hazırlanmış komutu çalıştırıp çıktısını toplar.
pub async fn output(mut cmd: Command, timeout: Duration) -> AppResult<CmdOutput> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| AppError::internal("komut zaman aşımına uğradı"))?
        .map_err(|err| AppError::internal(format!("komut başlatılamadı: {err}")))?;
    Ok(CmdOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
