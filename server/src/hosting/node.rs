//! Uygulama siteleri (Node.js ve Python): kurulu sürümler ve site başına systemd servisi.
//!
//! İki tür aynı düzeni paylaşır; farkları çalışma ortamındadır: Node.js sitesi panelin
//! kurduğu Node.js sürümünü, Python sitesi site klasöründeki sanal ortamı (`.venv`) kullanır.
//!
//! Her Node.js sitesi, sitenin sahibi olan Linux kullanıcısıyla çalışan ayrı
//! bir servistir (`pixpanel-node-<kimlik>.service`). Uygulama panelin verdiği
//! porta (`PORT` ortam değişkeni) bağlanır; nginx istekleri o porta iletir.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::hosting::sysuser::SysUser;
use crate::sys::{exec, runas};

/// Panelin kurduğu Node.js sürümlerinin durduğu klasör (`<kök>/<ana sürüm>/bin/node`).
pub const NODE_ROOT: &str = "/opt/pixpanel/node";
const CONFIG_DIR: &str = "/etc/pixpanel/node";
/// Uygulamalara verilen portların başlangıcı; port = taban + site kimliği.
const PORT_BASE: i64 = 20000;

const STARTER_SERVER: &str = include_str!("templates/node-server.js");
const STARTER_PACKAGE: &str = include_str!("templates/node-package.json");
const STARTER_PYTHON: &str = include_str!("templates/python-app.py");
/// Python sitelerinin sanal ortamı; site klasörünün içindedir ve sitenin kullanıcısına aittir.
pub const VENV_DIR: &str = ".venv";
const PYTHON_DEFAULT_COMMAND: &str = "python app.py";

/// Site türü bir uygulama mı (kendi süreciyle çalışan, isteklerin porta iletildiği site)?
pub fn is_app(kind: &str) -> bool {
    matches!(kind, "node" | "python")
}

/// Türün kullanıcıya gösterilen adı.
pub fn runtime_name(kind: &str) -> &'static str {
    if kind == "python" { "Python" } else { "Node.js" }
}

/// Sistemdeki Python sürümü (ör. `3.13`); sanal ortam desteği (python3-venv) kurulu değilse boş.
pub fn python_versions() -> Vec<String> {
    let Ok(target) = std::fs::canonicalize("/usr/bin/python3") else { return Vec::new() };
    let Some(version) = target.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_prefix("python")).map(str::to_string) else { return Vec::new() };
    let valid = !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.');
    if valid && Path::new(&format!("/usr/lib/python{version}/ensurepip")).is_dir() { vec![version] } else { Vec::new() }
}

/// Yeni bir Python sitesinin başlangıç ayarları.
pub fn python_defaults() -> AppResult<NodeSettings> {
    let version = python_versions().into_iter().next().ok_or_else(|| AppError::bad("Python kurulu değil; önce Bileşenler sayfasından kurun"))?;
    Ok(NodeSettings { version, command: PYTHON_DEFAULT_COMMAND.into(), ..NodeSettings::default() })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NodeSettings {
    /// Ana sürüm numarası (ör. `22`).
    pub version: String,
    /// Uygulamayı başlatan komut; site klasöründe çalışır.
    pub command: String,
    pub env: Vec<EnvVar>,
    /// Bellek sınırı (MB); 0 sınırsız.
    pub memory_mb: u32,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self { version: String::new(), command: "npm start".into(), env: Vec::new(), memory_mb: 0 }
    }
}

/// Sürüm yalnızca rakamlardan oluşur (dosya yolunda kullanılır).
pub fn validate_version(version: &str) -> AppResult<()> {
    let valid = !version.is_empty() && version.len() <= 3 && version.bytes().all(|b| b.is_ascii_digit());
    if valid { Ok(()) } else { Err(AppError::bad("Geçersiz Node.js sürümü")) }
}

/// Kurulu Node.js ana sürümleri (yeniden eskiye).
pub fn installed_versions() -> Vec<String> {
    let mut versions: Vec<u32> = std::fs::read_dir(NODE_ROOT)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().join("bin/node").is_file())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter_map(|name| name.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    versions.sort_unstable_by(|a, b| b.cmp(a));
    versions.into_iter().map(|version| version.to_string()).collect()
}

pub fn bin_dir(version: &str) -> PathBuf {
    Path::new(NODE_ROOT).join(version).join("bin")
}

impl NodeSettings {
    /// `kind`: sitenin türü (`node` ya da `python`); sürüm ona göre denetlenir.
    pub fn validate(&self, kind: &str) -> AppResult<()> {
        if kind == "python" {
            // Python sürümü sistemden gelir; ayardaki değer bilgi içindir ve kurulu olanla eşleşmelidir.
            if !python_versions().contains(&self.version) {
                return Err(AppError::bad("Python kurulu değil; önce Bileşenler sayfasından kurun"));
            }
        } else {
            validate_version(&self.version)?;
            if !installed_versions().contains(&self.version) {
                return Err(AppError::bad(format!("Node.js {} kurulu değil; önce Bileşenler sayfasından kurun", self.version)));
            }
        }
        let command = self.command.trim();
        if command.is_empty() || command.len() > 500 || command.chars().any(char::is_control) {
            return Err(AppError::bad("Başlatma komutu boş olamaz, tek satır olmalı ve 500 karakteri aşamaz"));
        }
        if self.env.len() > 100 {
            return Err(AppError::bad("En fazla 100 ortam değişkeni tanımlanabilir"));
        }
        for variable in &self.env {
            let mut chars = variable.key.bytes();
            let key_ok = variable.key.len() <= 128
                && chars.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                && chars.all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if !key_ok {
                return Err(AppError::bad(format!("Geçersiz ortam değişkeni adı: {}", variable.key)));
            }
            if matches!(variable.key.as_str(), "PORT" | "HOST" | "PATH" | "HOME" | "VIRTUAL_ENV") {
                return Err(AppError::bad(format!("{} değişkenini panel belirler; elle tanımlanamaz", variable.key)));
            }
            if variable.value.len() > 4096 || variable.value.chars().any(char::is_control) {
                return Err(AppError::bad(format!("{} için geçersiz değer (tek satır, en fazla 4096 karakter)", variable.key)));
            }
        }
        if self.memory_mb != 0 && !(64..=1_048_576).contains(&self.memory_mb) {
            return Err(AppError::bad("Bellek sınırı en az 64 MB olmalı (0 sınırsız)"));
        }
        Ok(())
    }
}

/// Uygulamanın dinlemesi gereken yerel port.
pub fn port(site_id: i64) -> i64 {
    PORT_BASE + site_id
}

pub fn unit_name(site_id: i64) -> String {
    format!("pixpanel-node-{site_id}.service")
}

fn unit_path(site_id: i64) -> PathBuf {
    PathBuf::from(format!("/etc/systemd/system/{}", unit_name(site_id)))
}

fn config_file(site_id: i64, extension: &str) -> PathBuf {
    Path::new(CONFIG_DIR).join(format!("{site_id}.{extension}"))
}

/// systemd `EnvironmentFile` satırı: değer tırnak içine alınır ve kaçışlanır.
fn env_line(variable: &EnvVar) -> String {
    format!("{}=\"{}\"\n", variable.key, variable.value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn render_unit(site_id: i64, domain: &str, user: &SysUser, root: &Path, settings: &NodeSettings, kind: &str) -> String {
    // Çalışma ortamına özel satırlar: Node.js'te panelin kurduğu sürüm, Python'da sitenin sanal ortamı.
    let (bin, runtime_env) = if kind == "python" {
        let venv = root.join(VENV_DIR);
        (venv.join("bin"), format!("Environment=PYTHONUNBUFFERED=1\nEnvironment=VIRTUAL_ENV={}", venv.display()))
    } else {
        (bin_dir(&settings.version), "Environment=NODE_ENV=production".to_string())
    };
    let mut unit = format!(
        "# PixPanel tarafından üretildi ({domain}). Elle yapılan değişiklikler kaybolur.\n\
         [Unit]\n\
         Description=PixPanel {runtime} uygulaması: {domain}\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         User={name}\n\
         Group={name}\n\
         WorkingDirectory={root}\n\
         {runtime_env}\n\
         Environment=PORT={port}\n\
         Environment=HOST=127.0.0.1\n\
         Environment=HOME={home}\n\
         Environment=PATH={bin}:/usr/local/bin:/usr/bin:/bin\n\
         EnvironmentFile=-{env}\n\
         ExecStart=/bin/sh {script}\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\
         ProtectSystem=full\n\
         SyslogIdentifier=pixpanel-node-{site_id}\n",
        name = user.name,
        root = root.display(),
        port = port(site_id),
        home = user.home.display(),
        bin = bin.display(),
        runtime = runtime_name(kind),
        env = config_file(site_id, "env").display(),
        script = config_file(site_id, "sh").display(),
    );
    if settings.memory_mb > 0 {
        unit.push_str(&format!("MemoryMax={}M\n", settings.memory_mb));
    }
    unit.push_str("\n[Install]\nWantedBy=multi-user.target\n");
    unit
}

/// Klasörde henüz bir uygulama yoksa, hemen çalışan küçük bir örnek bırakır.
async fn write_starter(user: &SysUser, root: &Path, domain: &str, kind: &str) -> AppResult<()> {
    let files: Vec<(&str, String)> = if kind == "python" {
        // Sanal ortam yoksa kurulur; uygulamanın bağımlılıkları sistemin Python'una karışmaz.
        if !root.join(VENV_DIR).join("bin/python").exists() {
            let output = runas::output(runas::command(user, root, "python3", &["-m", "venv", VENV_DIR]), Duration::from_secs(180)).await?;
            if !output.success() {
                return Err(AppError::internal(format!("Python sanal ortamı kurulamadı: {}", output.stderr.trim())));
            }
        }
        let has_app = ["app.py", "main.py", "manage.py", "wsgi.py", "asgi.py", "requirements.txt", "pyproject.toml"].iter().any(|name| root.join(name).exists());
        if has_app { Vec::new() } else { vec![("app.py", STARTER_PYTHON.replace("{{domain}}", domain))] }
    } else if root.join("package.json").exists() || root.join("server.js").exists() {
        Vec::new()
    } else {
        vec![("server.js", STARTER_SERVER.replace("{{domain}}", domain)), ("package.json", STARTER_PACKAGE.replace("{{domain}}", domain))]
    };
    // Site klasörü kullanıcıya aittir; dosyalar onun kimliğiyle yazılır.
    for (name, content) in files {
        use tokio::io::AsyncWriteExt;
        let mut cmd = runas::command(user, root, "tee", &["--", name]);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let mut child = cmd.spawn()?;
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(content.as_bytes()).await?;
            pipe.shutdown().await?;
        }
        child.wait().await?;
    }
    Ok(())
}

/// Sitenin servisini yazar ve (yeniden) başlatır.
pub async fn apply(site_id: i64, domain: &str, user: &SysUser, root: &Path, settings: &NodeSettings, kind: &str) -> AppResult<()> {
    settings.validate(kind)?;
    write_starter(user, root, domain, kind).await?;
    std::fs::create_dir_all(CONFIG_DIR)?;
    // Ortam değişkenleri parola içerebildiğinden dosyayı yalnızca root okur (systemd root olarak okur).
    let env: String = settings.env.iter().map(env_line).collect();
    atomic_write(&config_file(site_id, "env"), env.as_bytes(), 0o600)?;
    let script = format!("#!/bin/sh\n# PixPanel tarafından üretildi ({domain}).\n{}\n", settings.command.trim());
    atomic_write(&config_file(site_id, "sh"), script.as_bytes(), 0o755)?;
    atomic_write(&unit_path(site_id), render_unit(site_id, domain, user, root, settings, kind).as_bytes(), 0o644)?;

    let unit = unit_name(site_id);
    exec::run_ok("systemctl", &["daemon-reload"]).await?;
    exec::run_ok("systemctl", &["enable", "--quiet", "--", &unit]).await?;
    exec::run_ok("systemctl", &["restart", "--", &unit]).await?;
    Ok(())
}

/// Sitenin servisini durdurur ve dosyalarını kaldırır (site silindiğinde ya da türü değiştiğinde).
pub async fn remove(site_id: i64) -> AppResult<()> {
    if !unit_path(site_id).exists() {
        return Ok(());
    }
    let unit = unit_name(site_id);
    let _ = exec::run("systemctl", &["disable", "--now", "--quiet", "--", &unit]).await;
    for path in [unit_path(site_id), config_file(site_id, "env"), config_file(site_id, "sh")] {
        let _ = std::fs::remove_file(path);
    }
    exec::run_ok("systemctl", &["daemon-reload"]).await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Status {
    /// `active`, `inactive`, `failed`, `activating`...
    pub state: String,
    pub sub_state: String,
    pub pid: u32,
    pub memory_bytes: Option<u64>,
    /// Kendiliğinden yeniden başlatma sayısı; yüksekse uygulama sürekli çöküyordur.
    pub restarts: u32,
    pub port: i64,
}

pub async fn status(site_id: i64) -> AppResult<Status> {
    let output =
        exec::run("systemctl", &["show", "--property=ActiveState,SubState,MainPID,MemoryCurrent,NRestarts", "--", &unit_name(site_id)]).await?;
    let field = |name: &str| output.stdout.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix('=')).unwrap_or_default().to_string();
    Ok(Status {
        state: field("ActiveState"),
        sub_state: field("SubState"),
        pid: field("MainPID").parse().unwrap_or(0),
        memory_bytes: field("MemoryCurrent").parse().ok(),
        restarts: field("NRestarts").parse().unwrap_or(0),
        port: port(site_id),
    })
}

pub async fn action(site_id: i64, action: &str) -> AppResult<()> {
    if !matches!(action, "start" | "stop" | "restart") {
        return Err(AppError::bad("Geçersiz işlem"));
    }
    if !unit_path(site_id).exists() {
        return Err(AppError::bad("Bu sitenin uygulama servisi henüz oluşturulmadı"));
    }
    exec::run_with_timeout("systemctl", &[action, "--", &unit_name(site_id)], Duration::from_secs(60)).await.and_then(|output| {
        if output.success() { Ok(()) } else { Err(AppError::bad(format!("Servis {action} işlemi başarısız: {}", output.stderr.trim()))) }
    })
}
