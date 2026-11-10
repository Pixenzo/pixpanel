//! Sistem komutlarını çalıştırmanın tek yolu.
//!
//! Komutlar ASLA kabuk üzerinden çalıştırılmaz; program ve argümanlar ayrı
//! verilir. Böylece kullanıcı girdisi komut enjeksiyonuna dönüşemez.

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{AppError, AppResult};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Bütün komutlar için ortak, öngörülebilir ortam.
pub fn base_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("LC_ALL", "C.UTF-8")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    cmd
}

/// Komutu çalıştırır; sıfırdan farklı çıkış kodu hata sayılmaz.
pub async fn run(program: &str, args: &[&str]) -> AppResult<CmdOutput> {
    run_with_timeout(program, args, DEFAULT_TIMEOUT).await
}

pub async fn run_with_timeout(program: &str, args: &[&str], timeout: Duration) -> AppResult<CmdOutput> {
    let mut cmd = base_command(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| AppError::internal(format!("komut zaman aşımına uğradı: {program}")))?
        .map_err(|err| AppError::internal(format!("komut başlatılamadı ({program}): {err}")))?;
    Ok(CmdOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Komutu çalıştırır; başarısız olursa hata çıktısıyla birlikte hata döner.
pub async fn run_ok(program: &str, args: &[&str]) -> AppResult<String> {
    let output = run(program, args).await?;
    if output.success() {
        return Ok(output.stdout);
    }
    let detail = if output.stderr.trim().is_empty() { output.stdout.trim() } else { output.stderr.trim() };
    Err(AppError::internal(format!("{program} başarısız oldu (kod {}): {detail}", output.code)))
}

/// Programın sistemde kurulu olup olmadığını denetler.
pub fn binary_exists(name: &str) -> bool {
    ["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"]
        .iter()
        .any(|dir| std::path::Path::new(dir).join(name).is_file())
}
