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
use crate::hosting::pyenv;
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

/// Python sitelerinde `python` alanının "dağıtımın kendi Python'u" anlamına gelen değeri (boş da aynı anlama gelir).
pub const SYSTEM_PYTHON: &str = "system";

/// Sistemdeki Python sürümü (ör. `3.13`); sanal ortam desteği (python3-venv) kurulu değilse boş.
pub fn python_versions() -> Vec<String> {
    let Ok(target) = std::fs::canonicalize("/usr/bin/python3") else { return Vec::new() };
    let Some(version) = target.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_prefix("python")).map(str::to_string) else { return Vec::new() };
    let valid = !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.');
    if valid && Path::new(&format!("/usr/lib/python{version}/ensurepip")).is_dir() { vec![version] } else { Vec::new() }
}

/// Python sitelerinin bağımlılık dosyası: `requirements.txt` önceliklidir, yoksa `pyproject.toml`.
pub fn dependency_file(root: &Path) -> Option<&'static str> {
    ["requirements.txt", "pyproject.toml"].into_iter().find(|name| root.join(name).is_file())
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
    /// Yalnız Python siteleri: boş ya da `system` dağıtımın Python'u (eski kayıtlarda alan yoktur,
    /// o da aynı anlama gelir), aksi hâlde yöneticinin uv ile kurduğu `X.Y` sürümü.
    pub python: String,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self { version: String::new(), command: "npm start".into(), env: Vec::new(), memory_mb: 0, python: String::new() }
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
    /// Seçilen uv Python sürümü; dağıtımın Python'u seçiliyse `None`.
    pub fn python_choice(&self) -> Option<&str> {
        match self.python.trim() {
            "" | SYSTEM_PYTHON => None,
            version => Some(version),
        }
    }

    /// Python ayarlarını çalışma ortamıyla tutarlı kılar: `system` boş metne çevrilir; uv sürümü seçiliyse
    /// `version` alanı onu yansıtır, değilse sistemin Python sürümünü gösterir. `was_python`: site zaten
    /// Python sitesiydi (tür yeni değiştiyse öteki çalışma ortamından kalan ayarlar başlangıç değerlerine döner).
    pub fn reconcile_python(&mut self, was_python: bool) -> AppResult<()> {
        if self.python.trim() == SYSTEM_PYTHON {
            self.python.clear();
        }
        if let Some(choice) = self.python_choice() {
            self.version = choice.to_string();
            return Ok(());
        }
        let system = python_versions();
        if system.contains(&self.version) {
            return Ok(());
        }
        if was_python {
            // Dağıtımın Python'u yükseldi ya da seçim uv sürümünden sisteme döndü: yalnız sürüm bilgisi düzelir.
            self.version = system.into_iter().next().ok_or_else(|| AppError::bad("Python kurulu değil; önce Bileşenler sayfasından kurun"))?;
        } else {
            *self = NodeSettings { env: self.env.clone(), memory_mb: self.memory_mb, ..python_defaults()? };
        }
        Ok(())
    }

    /// `kind`: sitenin türü (`node` ya da `python`); sürüm ona göre denetlenir.
    pub fn validate(&self, kind: &str) -> AppResult<()> {
        if kind == "python" {
            if let Some(choice) = self.python_choice() {
                // Yönetici uv ile kurduğu sürümlerden biri.
                pyenv::validate_version(choice)?;
                if !pyenv::installed_versions().iter().any(|installed| installed == choice) {
                    return Err(AppError::bad(format!("Python {choice} kurulu değil; önce Bileşenler sayfasından kurun")));
                }
            } else if !python_versions().contains(&self.version) {
                // Dağıtımın Python'u: ayardaki değer bilgi içindir ve kurulu olanla eşleşmelidir.
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
         SyslogIdentifier=pixpanel-node-{site_id}\n\
         {port_rules}{slice_line}",
        name = user.name,
        root = root.display(),
        port = port(site_id),
        home = user.home.display(),
        bin = bin.display(),
        runtime = runtime_name(kind),
        // Uygulama yalnızca kendi portunu açabilir; başka sitelerin portlarını bağlayamaz.
        port_rules = crate::hosting::portguard::unit_lines(port(site_id)),
        // Hesabın işlemci/bellek sınırı varsa uygulama da hesabın diliminde çalışır (koşulları `resource_limits` açıklar).
        slice_line = crate::hosting::resource_limits::app_slice_line(user.uid),
        env =config_file(site_id, "env").display(),
        script = config_file(site_id, "sh").display(),
    );
    if settings.memory_mb > 0 {
        unit.push_str(&format!("MemoryMax={}M\n", settings.memory_mb));
    }
    unit.push_str("\n[Install]\nWantedBy=multi-user.target\n");
    unit
}

/// Sitenin sanal ortamının hangi yorumlayıcıyla kurulduğunu (`system` ya da `X.Y`) tutan, yalnız root'un yazdığı dosya.
fn venv_marker(site_id: i64) -> PathBuf {
    config_file(site_id, "python")
}

/// Site silinirken sanal ortamın yorumlayıcı kaydı da silinir (tür değişiminde korunur:
/// klasördeki `.venv` kalır ve sitenin sonradan Python'a dönmesi halinde yorumlayıcı eşleşmesi denetlenir).
pub fn forget_interpreter(site_id: i64) {
    let _ = std::fs::remove_file(venv_marker(site_id));
}

/// Python sitesinin sanal ortamını seçili yorumlayıcıyla hazırlar. Ortam yoksa kurulur; yorumlayıcı
/// değişmişse (eski ortamın içindeki yollar ve kitaplıklar yeni sürüme uymaz) uygulama durdurulup
/// ortam sıfırdan kurulur. Her şey sitenin kendi kullanıcısıyla yapılır; uygulamanın bağımlılıkları
/// sistemin Python'una karışmaz. Kayıt dosyası olmayan (eski) siteler `system` sayılır.
async fn ensure_venv(site_id: i64, user: &SysUser, root: &Path, settings: &NodeSettings) -> AppResult<()> {
    let wanted = settings.python_choice().unwrap_or(SYSTEM_PYTHON);
    let built = std::fs::read_to_string(venv_marker(site_id)).map(|text| text.trim().to_string()).unwrap_or_else(|_| SYSTEM_PYTHON.to_string());
    let present = root.join(VENV_DIR).join("bin/python").exists();
    if present && built == wanted {
        return Ok(());
    }
    if present {
        let _ = exec::run("systemctl", &["stop", "--", &unit_name(site_id)]).await;
    }
    let program = match settings.python_choice() {
        Some(choice) => {
            pyenv::validate_version(choice)?;
            pyenv::interpreter(choice).to_string_lossy().into_owned()
        }
        None => "python3".to_string(),
    };
    // `--clear`: yarım ya da eski ortamın bütün içeriği silinip yeniden kurulur.
    let output = runas::output(runas::heavy_command(user, root, &program, &["-m", "venv", "--clear", VENV_DIR]), Duration::from_secs(180)).await?;
    if !output.success() {
        return Err(AppError::internal(format!("Python sanal ortamı kurulamadı: {}", output.stderr.trim())));
    }
    std::fs::create_dir_all(CONFIG_DIR)?;
    atomic_write(&venv_marker(site_id), wanted.as_bytes(), 0o644)?;
    Ok(())
}

/// Klasörde henüz bir uygulama yoksa, hemen çalışan küçük bir örnek bırakır.
async fn write_starter(site_id: i64, user: &SysUser, root: &Path, domain: &str, kind: &str, settings: &NodeSettings) -> AppResult<()> {
    let files: Vec<(&str, String)> = if kind == "python" {
        ensure_venv(site_id, user, root, settings).await?;
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
    write_starter(site_id, user, root, domain, kind, settings).await?;
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
    // Ayar art arda değişince systemd'nin "çok sık başlatıldı" sınırı dolabilir; sayaç sıfırlanır.
    let _ = exec::run("systemctl", &["reset-failed", "--", &unit]).await;
    exec::run_ok("systemctl", &["restart", "--", &unit]).await?;
    Ok(())
}

/// Var olan servisin birim dosyasını güncel ayarlarla yeniden yazar; servisi yeniden başlatmaz ve
/// `daemon-reload` çağırmaz (çağıran toplu yapar). Birim yoksa ya da içerik aynıysa `false` döner.
/// Kaynak sınırı değişince dilim satırını eklemek/kaldırmak için kullanılır; değişiklik servisin
/// bir sonraki başlatılışında geçerli olur.
pub fn rewrite_unit(site_id: i64, domain: &str, user: &SysUser, root: &Path, settings: &NodeSettings, kind: &str) -> AppResult<bool> {
    let path = unit_path(site_id);
    if !path.exists() {
        return Ok(false);
    }
    let wanted = render_unit(site_id, domain, user, root, settings, kind);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == wanted) {
        return Ok(false);
    }
    atomic_write(&path, wanted.as_bytes(), 0o644)?;
    Ok(true)
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
    /// Yalnız Python siteleri: bağımlılıkların kurulabileceği dosya (`requirements.txt` ya da `pyproject.toml`); yoksa `None`.
    pub dependencies: Option<&'static str>,
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
        dependencies: None,
    })
}

/// Sitenin servisi şu an çalışıyor mu?
pub async fn is_running(site_id: i64) -> bool {
    crate::sys::services::is_active(&unit_name(site_id)).await
}

pub async fn action(site_id: i64, action: &str) -> AppResult<()> {
    if !matches!(action, "start" | "stop" | "restart") {
        return Err(AppError::bad("Geçersiz işlem"));
    }
    if !unit_path(site_id).exists() {
        return Err(AppError::bad("Bu sitenin uygulama servisi henüz oluşturulmadı"));
    }
    if action != "stop" {
        let _ = exec::run("systemctl", &["reset-failed", "--", &unit_name(site_id)]).await;
    }
    exec::run_with_timeout("systemctl", &[action, "--", &unit_name(site_id)], Duration::from_secs(60)).await.and_then(|output| {
        if output.success() { Ok(()) } else { Err(AppError::bad(format!("Servis {action} işlemi başarısız: {}", output.stderr.trim()))) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_python_settings_mean_system() {
        // Eski kayıtlarda `python` alanı yoktur; dağıtımın Python'u sayılır.
        let legacy: NodeSettings = serde_json::from_str(r#"{"version":"3.13","command":"python app.py","env":[],"memory_mb":0}"#).unwrap();
        assert_eq!(legacy.python_choice(), None);
        let system = NodeSettings { python: "system".into(), ..NodeSettings::default() };
        assert_eq!(system.python_choice(), None);
        let chosen = NodeSettings { python: "3.12".into(), ..NodeSettings::default() };
        assert_eq!(chosen.python_choice(), Some("3.12"));
    }
}
