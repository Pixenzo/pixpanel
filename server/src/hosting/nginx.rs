//! nginx yapılandırmasının üretilmesi ve güvenli biçimde uygulanması.
//!
//! Her site için tek bir dosya üretilir. Dosya yazıldıktan sonra `nginx -t`
//! ile sınanır; hata varsa eski hâli geri konur ve nginx'e dokunulmaz.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::sys::exec;

const BASE_CONF: &str = "/etc/nginx/conf.d/pixpanel.conf";
const SITES_DIR: &str = "/etc/nginx/pixpanel/sites";
pub const ACME_ROOT: &str = "/var/lib/pixpanel/acme";
pub const SSL_DIR: &str = "/etc/pixpanel/ssl";

/// Sitenin nginx yapılandırmasını üretmek için gereken her şey.
/// Buradaki bütün dizgeler çağıran tarafından doğrulanmış olmalıdır.
/// Sitenin bir yolunu başka bir adrese (ör. bir Docker konteynerine) ileten kural.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ProxyRule {
    /// İsteğin yolu bu önekle başlıyorsa kural uygulanır (ör. `/api/`).
    pub path: String,
    /// Hedefin kökü: `http://127.0.0.1:8080`.
    pub target: String,
    #[serde(default = "default_true")]
    pub websocket: bool,
    /// Açıksa önek hedefe iletilmez: `/api/kullanici` hedefte `/kullanici` olur.
    #[serde(default)]
    pub strip_prefix: bool,
}

fn default_true() -> bool {
    true
}

pub struct SiteConfig<'a> {
    pub domain: &'a str,
    pub aliases: &'a [String],
    pub backend: Backend<'a>,
    pub ssl: bool,
    pub force_https: bool,
    pub hsts: bool,
    pub suspended: bool,
    /// Yöneticinin eklediği ham nginx yönergeleri.
    pub custom: &'a str,
    pub proxy_rules: &'a [ProxyRule],
    /// Uygulamaya göre hazır ayar (bkz. `TEMPLATES`).
    pub template: &'a str,
}

/// Site şablonları: (kimlik, geçerli olduğu site türü). Şablon, sitenin sahibinin seçebildiği
/// hazır ve denetlenmiş bir ayar kümesidir; serbest metin içermez.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("default", "php"),
    ("wordpress", "php"),
    ("laravel", "php"),
    ("default", "static"),
    ("spa", "static"),
    ("clean_urls", "static"),
];

/// Şablon bu site türünde kullanılabilir mi?
pub fn template_allowed(kind: &str, template: &str) -> bool {
    template == "default" || TEMPLATES.iter().any(|(id, for_kind)| *id == template && *for_kind == kind)
}

/// Şablonun PHP sitelerine eklediği kurallar. PHP'yi çalıştıran konumdan önce yazılır:
/// nginx düzenli ifadeli konumlardan ilk eşleşeni kullanır.
fn php_template_rules(template: &str) -> &'static str {
    match template {
        "wordpress" => {
            "    # WordPress: yüklenen dosyalar arasına bırakılan PHP çalıştırılmaz.\n\
             \x20   location ~* ^/wp-content/(?:uploads|files)/.*\\.php$ {\n        return 403;\n    }\n\
             \x20   # Parola deneme ve yükseltme saldırılarının başlıca kapısı; modern eklentiler kullanmaz.\n\
             \x20   location = /xmlrpc.php {\n        return 403;\n    }\n\
             \x20   # Sürümü ve yapılandırmayı ele veren dosyalar.\n\
             \x20   location ~* ^/(?:wp-config-sample\\.php|readme\\.html|license\\.txt|wp-content/debug\\.log)$ {\n        return 404;\n    }\n"
        }
        "laravel" => {
            "    # Laravel: yayın klasörü yanlışlıkla proje kökü seçilirse uygulamanın iç klasörleri sunulmaz.\n\
             \x20   location ~* ^/(?:storage|vendor|bootstrap|database|config|resources|tests)/ {\n        return 404;\n    }\n\
             \x20   location ~* ^/(?:artisan|\\.env.*)$ {\n        return 404;\n    }\n"
        }
        _ => "",
    }
}

pub enum Backend<'a> {
    Static { docroot: &'a Path },
    Php { docroot: &'a Path, socket: &'a str },
    Proxy { target: &'a str, websocket: bool },
}

pub fn is_installed() -> bool {
    exec::binary_exists("nginx")
}

fn require_installed() -> AppResult<()> {
    if is_installed() {
        Ok(())
    } else {
        Err(AppError::bad("nginx kurulu değil; önce Bileşenler sayfasından kurun"))
    }
}

fn site_conf_path(domain: &str) -> PathBuf {
    Path::new(SITES_DIR).join(format!("{domain}.conf"))
}

pub fn certificate_paths(domain: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(SSL_DIR).join(domain);
    (dir.join("fullchain.pem"), dir.join("privkey.pem"))
}

pub fn access_log_path(domain: &str) -> String {
    format!("/var/log/nginx/{domain}.access.log")
}

pub fn error_log_path(domain: &str) -> String {
    format!("/var/log/nginx/{domain}.error.log")
}

fn ipv6_enabled() -> bool {
    std::fs::read_to_string("/proc/net/if_inet6").is_ok_and(|content| !content.trim().is_empty())
}

/// `http2 on;` yönergesi nginx 1.25.1 ile geldi; eski sürümlerde `listen ... http2` kullanılır.
async fn supports_http2_directive() -> bool {
    let Ok(output) = exec::run("nginx", &["-v"]).await else { return false };
    let version = output.stderr.rsplit('/').next().unwrap_or("").trim().to_string();
    let mut parts = version.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse::<u32>().ok());
    let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (major, minor, patch) >= (1, 25, 1)
}

fn listen_lines(out: &mut String, port: u16, flags: &str) {
    let _ = writeln!(out, "    listen {port}{flags};");
    if ipv6_enabled() {
        let _ = writeln!(out, "    listen [::]:{port}{flags};");
    }
}

const ACME_LOCATION: &str = "    location ^~ /.well-known/acme-challenge/ {\n        root /var/lib/pixpanel/acme;\n        default_type text/plain;\n        auth_basic off;\n    }\n";

/// Dosyanın şimdiki içeriğini saklar; `restore` ile eski hâline döndürülür.
struct Snapshot {
    path: PathBuf,
    previous: Option<Vec<u8>>,
}

impl Snapshot {
    fn take(path: &Path) -> Self {
        Self { path: path.to_path_buf(), previous: std::fs::read(path).ok() }
    }

    fn restore(&self) {
        let result = match &self.previous {
            Some(content) => atomic_write(&self.path, content, 0o644),
            None => std::fs::remove_file(&self.path).or_else(ignore_missing).map_err(Into::into),
        };
        if let Err(err) = result {
            tracing::error!(error = %err, path = %self.path.display(), "yapılandırma geri alınamadı");
        }
    }
}

fn ignore_missing(err: std::io::Error) -> std::io::Result<()> {
    if err.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(err) }
}

/// Panelin nginx'e eklediği genel ayarları ve tanımsız alan adlarını
/// karşılayan varsayılan sunucuyu yazar. Sınama başarısız olursa yazılanlar
/// geri alınır; nginx hiçbir zaman bozuk yapılandırmayla bırakılmaz.
pub async fn ensure_base() -> AppResult<()> {
    require_installed()?;
    std::fs::create_dir_all(SITES_DIR)?;
    std::fs::create_dir_all(format!("{ACME_ROOT}/.well-known/acme-challenge"))?;
    std::fs::create_dir_all(SSL_DIR)?;

    let base_path = Path::new(BASE_CONF);
    let default_path = Path::new(SITES_DIR).join("000-default.conf");
    let distro_default = Path::new("/etc/nginx/sites-enabled/default");
    let snapshots = [Snapshot::take(base_path), Snapshot::take(&default_path)];
    let distro_target = std::fs::read_link(distro_default).ok();

    // Dağıtımın nginx.conf dosyasında zaten bulunan yönergeler burada
    // yinelenmez; nginx yinelenen yönergeyi hata sayar.
    let base = "# PixPanel tarafından yönetilir. Elle yapılan değişiklikler kaybolur.
                client_max_body_size 128m;
                map $http_upgrade $pixpanel_connection {
    default upgrade;
    \"\" close;
}
                include /etc/nginx/pixpanel/sites/*.conf;
";
    atomic_write(base_path, base.as_bytes(), 0o644)?;

    // Sunucuya IP adresiyle ya da tanımlı olmayan bir adla gelen istekler
    // hiçbir siteye düşmez; bağlantı yanıtsız kapatılır.
    let mut default = String::from("# PixPanel: tanımsız alan adları
server {
");
    listen_lines(&mut default, 80, " default_server");
    listen_lines(&mut default, 443, " ssl default_server");
    default.push_str("    server_name _;
    ssl_reject_handshake on;
");
    default.push_str(ACME_LOCATION);
    default.push_str("    location / {
        return 444;
    }
}
");
    atomic_write(&default_path, default.as_bytes(), 0o644)?;

    // Dağıtımın örnek sitesi 80. portu sahiplenmesin.
    if distro_target.is_some() {
        std::fs::remove_file(distro_default)?;
    }

    if let Err(err) = test().await {
        for snapshot in &snapshots {
            snapshot.restore();
        }
        if let Some(target) = distro_target {
            let _ = std::os::unix::fs::symlink(target, distro_default);
        }
        return Err(err);
    }
    reload().await
}

async fn test() -> AppResult<()> {
    let output = exec::run("nginx", &["-t"]).await?;
    if output.success() {
        Ok(())
    } else {
        Err(AppError::bad(format!("nginx yapılandırması geçersiz: {}", output.stderr.trim())))
    }
}

async fn test_and_reload() -> AppResult<()> {
    test().await?;
    reload().await
}

/// Çalışan nginx işçi süreçlerinin (yeni istek kabul edenlerin) kimlikleri.
fn active_workers() -> Vec<u32> {
    let Some(master) = std::fs::read_to_string("/run/nginx.pid").ok().and_then(|raw| raw.trim().parse::<u32>().ok())
    else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let parent = stat.rsplit(')').next().and_then(|rest| rest.split_whitespace().nth(1)?.parse::<u32>().ok());
            // Kapanmakta olan işçinin başlığı "worker process is shutting down" olur.
            let title = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let title = String::from_utf8_lossy(&title);
            parent == Some(master) && title.trim_end_matches(' ').trim_end() == "nginx: worker process"
        })
        .collect()
}

/// nginx'i yeniden yükler ve yeni yapılandırma gerçekten devreye girene kadar bekler.
///
/// `systemctl reload` yalnızca sinyal gönderip döner; eski işçiler kısa bir
/// süre daha eski yapılandırmayla istek kabul eder. Bu beklenmeden eski
/// arka uç (ör. PHP havuzu) kapatılırsa o aralıktaki istekler 502 alır.
async fn reload() -> AppResult<()> {
    let old_workers = active_workers();
    exec::run_ok("systemctl", &["reload-or-restart", "nginx.service"]).await?;
    for _ in 0..100 {
        let current = active_workers();
        let replaced = !current.is_empty() && old_workers.iter().all(|pid| !current.contains(pid));
        if replaced {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tracing::warn!("nginx yeniden yüklemesi 5 saniyede tamamlanmadı");
    Ok(())
}

/// Vekil konum bloğunu yazar. `pass`, `proxy_pass` yönergesinin değeridir.
fn proxy_location(out: &mut String, location: &str, pass: &str, websocket: bool) {
    let _ = writeln!(out, "    location {location} {{");
    let _ = writeln!(out, "        proxy_pass {pass};");
    out.push_str(
        "        proxy_http_version 1.1;\n\
         \x20       proxy_set_header Host $host;\n\
         \x20       proxy_set_header X-Real-IP $remote_addr;\n\
         \x20       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n\
         \x20       proxy_set_header X-Forwarded-Proto $scheme;\n\
         \x20       proxy_read_timeout 300s;\n",
    );
    if websocket {
        out.push_str(
            "        proxy_set_header Upgrade $http_upgrade;\n\
             \x20       proxy_set_header Connection $pixpanel_connection;\n",
        );
    }
    out.push_str("    }\n");
}

pub fn render(site: &SiteConfig<'_>, http2_directive: bool) -> String {
    let mut names = site.domain.to_string();
    for alias in site.aliases {
        names.push(' ');
        names.push_str(alias);
    }
    let (cert, key) = certificate_paths(site.domain);
    let redirect_http = site.ssl && site.force_https;
    let mut out = format!("# PixPanel tarafından üretildi ({}). Elle yapılan değişiklikler kaybolur.\n", site.domain);

    if redirect_http {
        out.push_str("server {\n");
        listen_lines(&mut out, 80, "");
        let _ = writeln!(out, "    server_name {names};");
        out.push_str(ACME_LOCATION);
        out.push_str("    location / {\n        return 301 https://$host$request_uri;\n    }\n}\n\n");
    }

    out.push_str("server {\n");
    if !redirect_http {
        listen_lines(&mut out, 80, "");
    }
    if site.ssl {
        listen_lines(&mut out, 443, if http2_directive { " ssl" } else { " ssl http2" });
        if http2_directive {
            out.push_str("    http2 on;\n");
        }
        let _ = writeln!(out, "    ssl_certificate {};", cert.display());
        let _ = writeln!(out, "    ssl_certificate_key {};", key.display());
        if site.hsts {
            out.push_str("    add_header Strict-Transport-Security \"max-age=31536000\" always;\n");
        }
    }
    let _ = writeln!(out, "    server_name {names};");
    let _ = writeln!(out, "    access_log {};", access_log_path(site.domain));
    let _ = writeln!(out, "    error_log {} warn;", error_log_path(site.domain));
    out.push_str(ACME_LOCATION);

    if site.suspended {
        out.push_str("    location / {\n        return 503;\n    }\n}\n");
        return out;
    }

    // Yol bazlı vekil kuralları sitenin asıl içeriğinden önce eşleşir (`^~`).
    for rule in site.proxy_rules {
        // Hedefin sonuna `/` eklenirse nginx eşleşen öneki atar; eklenmezse yol olduğu gibi iletilir.
        let pass = if rule.strip_prefix { format!("{}/", rule.target) } else { rule.target.clone() };
        proxy_location(&mut out, &format!("^~ {}", rule.path), &pass, rule.websocket);
    }

    match &site.backend {
        Backend::Proxy { target, websocket } => proxy_location(&mut out, "/", target, *websocket),
        Backend::Static { docroot } | Backend::Php { docroot, .. } => {
            let php_socket = match &site.backend {
                Backend::Php { socket, .. } => Some(*socket),
                _ => None,
            };
            let _ = writeln!(out, "    root {};", docroot.display());
            out.push_str(if php_socket.is_some() {
                "    index index.php index.html index.htm;\n"
            } else {
                "    index index.html index.htm;\n"
            });
            // Başka bir hesabın dosyasına verilen sembolik bağlar izlenmez.
            out.push_str("    disable_symlinks if_not_owner;\n");
            // Nokta ile başlayan her şey (.git, .env, .htaccess) gizlenir.
            out.push_str("    location ~ /\\.(?!well-known/) {\n        return 404;\n    }\n");
            // Yanlışlıkla yayın klasöründe bırakılan döküm ve yapılandırma dosyaları.
            out.push_str(
                "    location ~* (?:\\.(?:sql|bak|old|orig|swp|dist)|/composer\\.(?:json|lock)|/package(?:-lock)?\\.json)$ {\n        return 404;\n    }\n",
            );
            match php_socket {
                Some(socket) => {
                    out.push_str(php_template_rules(site.template));
                    out.push_str("    location / {\n        try_files $uri $uri/ /index.php?$args;\n    }\n");
                    out.push_str("    location ~ [^/]\\.php(/|$) {\n        include snippets/fastcgi-php.conf;\n");
                    let _ = writeln!(out, "        fastcgi_pass unix:{socket};");
                    out.push_str("        fastcgi_read_timeout 300s;\n    }\n");
                }
                None => out.push_str(match site.template {
                    // Tek sayfalık uygulama: bilinmeyen her adres uygulamanın kendisine düşer.
                    "spa" => "    location / {\n        try_files $uri $uri/ /index.html;\n    }\n",
                    // Uzantısız adresler: /hakkinda isteği hakkinda.html dosyasını sunar.
                    "clean_urls" => "    location / {\n        try_files $uri $uri.html $uri/ =404;\n    }\n",
                    _ => "    location / {\n        try_files $uri $uri/ =404;\n    }\n",
                }),
            }
            out.push_str(
                "    location ~* \\.(?:css|js|mjs|jpe?g|png|gif|webp|avif|svg|ico|woff2?|ttf|otf|mp4|webm)$ {\n        expires 30d;\n    }\n",
            );
        }
    }

    if !site.custom.trim().is_empty() {
        out.push_str("    # Yöneticinin eklediği özel yönergeler\n");
        for line in site.custom.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    out.push_str("}\n");
    out
}

/// Sitenin yapılandırmasını yazar, sınar ve nginx'i yeniden yükler.
/// Sınama başarısız olursa önceki yapılandırma geri konur.
pub async fn apply(site: &SiteConfig<'_>) -> AppResult<()> {
    require_installed()?;
    if !Path::new(BASE_CONF).exists() {
        ensure_base().await?;
    }
    let content = render(site, supports_http2_directive().await);
    let path = site_conf_path(site.domain);
    let snapshot = Snapshot::take(&path);
    atomic_write(&path, content.as_bytes(), 0o644)?;

    if let Err(err) = test().await {
        snapshot.restore();
        return Err(err);
    }
    reload().await
}

pub async fn remove(domain: &str) -> AppResult<()> {
    let path = site_conf_path(domain);
    if path.exists() {
        std::fs::remove_file(&path)?;
        if is_installed() {
            test_and_reload().await?;
        }
    }
    Ok(())
}
