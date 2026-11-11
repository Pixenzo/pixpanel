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
