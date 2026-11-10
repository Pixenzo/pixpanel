//! Web terminali: tarayıcıdaki uçbirimi sunucudaki bir kabuğa bağlar.
//!
//! Yalnızca yöneticilere açıktır. Kabuk ya root olarak ya da seçilen hosting
//! hesabının kullanıcısı olarak başlatılır.
//!
//! İletişim kuralı (WebSocket): ikili çerçeveler kabuğa giden/kabuktan gelen
//! ham baytlardır; metin çerçeveleri JSON denetim iletileridir
//! (`{"type":"resize","cols":120,"rows":30}`).

use std::io::{Read, Write};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;

use crate::auth::{self, AuthUser};
use crate::error::{AppError, AppResult};
use crate::hosting::sysuser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/ws", get(connect))
}

#[derive(Deserialize)]
struct ConnectQuery {
    /// `root` ya da `user:<kimlik>`.
    scope: String,
}

/// Kabuğun kimin adına ve nerede açılacağı.
struct Target {
    label: String,
    /// `None` ise root.
    user: Option<sysuser::SysUser>,
    /// Verilirse kabuk bu Docker konteynerinin içinde açılır.
    container: Option<String>,
}

async fn resolve_target(state: &AppState, scope: &str) -> AppResult<Target> {
    if scope == "root" {
        return Ok(Target { label: "root".into(), user: None, container: None });
    }
    if let Some(container) = scope.strip_prefix("container:") {
        let mut chars = container.bytes();
        let valid = container.len() <= 128
            && chars.next().is_some_and(|b| b.is_ascii_alphanumeric())
            && chars.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if !valid {
            return Err(AppError::bad("Geçersiz konteyner"));
        }
        return Ok(Target { label: format!("konteyner {container}"), user: None, container: Some(container.to_string()) });
    }
    let id: i64 = scope.strip_prefix("user:").and_then(|id| id.parse().ok()).ok_or_else(|| AppError::bad("Geçersiz terminal hedefi"))?;
    let username: Option<String> = state.db.call(|conn| {
        conn.query_row("SELECT username FROM users WHERE id = ?1 AND role != 'member'", params![id], |row| row.get(0)).optional()
    })?;
    let username = username.ok_or(AppError::NotFound)?;
    let user = sysuser::lookup(&username)
        .ok_or_else(|| AppError::bad("Bu hesabın henüz bir Linux kullanıcısı yok; önce bir site oluşturun"))?;
    Ok(Target { label: username, user: Some(user), container: None })
}

/// WebSocket isteklerinde tarayıcılar çapraz kaynak korumasını uygulamaz; bu
/// yüzden isteğin panelin kendi sayfasından geldiği `Origin` ile doğrulanır.
/// (Oturum çerezi `SameSite=Strict` olduğundan bu ikinci bir savunma hattıdır.)
fn check_origin(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        // Tarayıcı olmayan istemciler (ör. API anahtarıyla bağlanan araçlar) Origin göndermez.
        return Ok(());
    };
    let host = headers.get(HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    let origin_host = origin.split_once("://").map(|(_, rest)| rest).unwrap_or(origin);
    if origin_host == host || state.cfg.allowed_origins.iter().any(|allowed| allowed == origin) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn connect(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Query(query): Query<ConnectQuery>,
    upgrade: WebSocketUpgrade,
) -> AppResult<Response> {
    user.require_admin()?;
    check_origin(&state, &headers)?;
    let target = resolve_target(&state, &query.scope).await?;
    auth::audit(&state.db, &user, "terminal.open", &target.label, "");
    Ok(upgrade.on_upgrade(move |socket| async move {
        if let Err(err) = session(socket, target).await {
            tracing::warn!(error = %err, "terminal oturumu hatayla sonlandı");
        }
    }))
}

fn shell_command(target: &Target) -> CommandBuilder {
    if let Some(container) = &target.container {
        // Konteynerde bash yoksa (ör. alpine) sh açılır. Buradaki kabuk dizesi
        // sabittir; kullanıcı girdisi içermez.
        let mut command = CommandBuilder::new("docker");
        command.args(["exec", "-it", "--", container, "/bin/sh", "-c", "command -v bash >/dev/null 2>&1 && exec bash || exec sh"]);
        command.cwd("/");
        command.env("TERM", "xterm-256color");
        command.env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
        return command;
    }
    let mut command = match &target.user {
        None => {
            let mut command = CommandBuilder::new("/bin/bash");
            command.arg("-l");
            command.cwd("/root");
            command.env("HOME", "/root");
            command.env("USER", "root");
            command
        }
        Some(user) => {
            // `setpriv` kimliği değiştirip doğrudan kabuğa dönüşür; arada üst süreç
            // kalmadığından oturum kapanırken gönderilen sinyal kabuğa ulaşır.
            // (`runuser` arada bekleyen bir süreç bırakır ve SIGHUP'ı engeller.)
            let mut command = CommandBuilder::new("setpriv");
            command.args([
                &format!("--reuid={}", user.uid),
                &format!("--regid={}", user.gid),
                "--init-groups",
                "--",
                "/bin/bash",
                "-l",
            ]);
            command.cwd(&user.home);
            command.env("HOME", &user.home);
            command.env("USER", &user.name);
            command.env("LOGNAME", &user.name);
            command
        }
    };
    command.env("TERM", "xterm-256color");
    command.env("LANG", "C.UTF-8");
    command.env("SHELL", "/bin/bash");
    command.env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
    command
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Control {
    Resize { cols: u16, rows: u16 },
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize { rows: rows.clamp(2, 500), cols: cols.clamp(2, 1000), pixel_width: 0, pixel_height: 0 }
}

async fn session(socket: WebSocket, target: Target) -> anyhow::Result<()> {
    let pair = native_pty_system().openpty(pty_size(80, 24))?;
    let mut command = shell_command(&target);
    // Panelin kendi ortamı (ör. yapılandırma değişkenleri) kabuğa sızmasın.
    command.env_remove("INVOCATION_ID");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut killer = child.clone_killer();
    let master = pair.master;
    let mut reader = master.try_clone_reader()?;
    let mut writer = master.take_writer()?;

    // PTY okuma ve yazması engelleyicidir; kendi iş parçacıklarında yürür.
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 || output_tx.blocking_send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(bytes) = input_rx.recv() {
            if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                break;
            }
        }
    });

    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            output = output_rx.recv() => match output {
                Some(bytes) => {
                    if sink.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                // Kabuk kapandı (ör. `exit`).
                None => break,
            },
            message = stream.next() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    if input_tx.send(bytes.to_vec()).is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if let Ok(Control::Resize { cols, rows }) = serde_json::from_str(text.as_str()) {
                        let _ = master.resize(pty_size(cols, rows));
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }

    // Sekme kapanınca kabuk ve başlattığı süreçler arkada kalmasın: önce
    // nazikçe (SIGHUP), kısa sürede kapanmazsa zorla (SIGKILL) sonlandırılır.
    let pid = child.process_id();
    let _ = killer.kill();
    let mut waiter = tokio::task::spawn_blocking(move || child.wait());
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut waiter).await.is_err() {
        if let Some(pid) = pid {
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), nix::sys::signal::Signal::SIGKILL);
        }
        let _ = waiter.await;
    }
    let _ = sink.close().await;
    Ok(())
}
