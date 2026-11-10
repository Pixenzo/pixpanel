//! Sunucu bileşenleri (nginx, PHP sürümleri, veritabanları, Docker...) kataloğu
//! ve bunların panelden kurulması.

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::hosting;
use crate::jobs::JobCtx;
use crate::sys::{exec, services};

/// Panelden kurulabilen PHP sürümleri (yeniden eskiye).
pub const PHP_VERSIONS: &[&str] = &["8.5", "8.4", "8.3", "8.2", "8.1", "8.0", "7.4"];

/// Panelden kurulabilen Node.js ana sürümleri (yeniden eskiye).
pub const NODE_VERSIONS: &[&str] = &["24", "22", "20"];

/// Her PHP sürümüyle birlikte kurulmaya çalışılan eklentiler. Bir sürümde
/// bulunmayan eklenti (ör. 8.5'te yerleşik gelen opcache) sessizce atlanır.
const PHP_EXTENSIONS: &[&str] = &[
    "mysql", "pgsql", "sqlite3", "curl", "gd", "mbstring", "xml", "zip", "intl", "bcmath", "opcache",
    "soap", "readline", "redis", "imagick",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repo {
    /// Dağıtımın kendi deposu yeterli.
    Distro,
    /// packages.sury.org (Debian) ya da ppa:ondrej/php (Ubuntu).
    Php,
    /// download.docker.com
    Docker,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub description: &'static str,
    pub category: &'static str,
    /// Kurulması zorunlu paketler; ilki sürüm bilgisinin okunduğu ana pakettir.
    packages: Vec<String>,
    /// Depoda varsa kurulan, yoksa atlanan paketler.
    optional_packages: Vec<String>,
    pub service: Option<String>,
    repo: Repo,
}

#[derive(Debug, Serialize)]
pub struct ComponentStatus {
    pub id: String,
    pub name: String,
    pub description: &'static str,
    pub category: &'static str,
    pub installed: bool,
    pub version: Option<String>,
    pub service: Option<String>,
    pub running: bool,
}

fn component(
    id: &str,
    name: &str,
    description: &'static str,
    category: &'static str,
    packages: &[&str],
    service: Option<&str>,
    repo: Repo,
) -> Component {
    Component {
        id: id.to_string(),
        name: name.to_string(),
        description,
        category,
        packages: packages.iter().map(|p| p.to_string()).collect(),
        optional_packages: Vec::new(),
        service: service.map(str::to_string),
        repo,
    }
}

pub fn catalog() -> Vec<Component> {
    let mut list = vec![component(
        "nginx",
        "nginx",
        "Web sunucusu ve ters vekil. Siteleri yayınlamak için gereklidir.",
        "web",
        &["nginx"],
        Some("nginx.service"),
        Repo::Distro,
    )];
    for version in PHP_VERSIONS {
        list.push(Component {
            id: format!("php-{version}"),
            name: format!("PHP {version}"),
            description: "PHP-FPM ve yaygın eklentiler.",
            category: "php",
            packages: ["fpm", "cli", "common"].iter().map(|part| format!("php{version}-{part}")).collect(),
            optional_packages: PHP_EXTENSIONS.iter().map(|ext| format!("php{version}-{ext}")).collect(),
            service: Some(format!("php{version}-fpm.service")),
            repo: Repo::Php,
        });
    }
    for version in NODE_VERSIONS {
        // Node.js paket deposundan değil, nodejs.org'un resmî derlemelerinden kurulur;
        // böylece birden çok sürüm yan yana durabilir.
        list.push(Component {
            id: format!("node-{version}"),
            name: format!("Node.js {version}"),
            description: "Node.js çalışma ortamı ve npm. Siteler sürümü tek tek seçebilir.",
            category: "node",
            packages: Vec::new(),
            optional_packages: Vec::new(),
            service: None,
            repo: Repo::Distro,
        });
    }
    list.extend([
        component(
            "mariadb",
            "MariaDB",
            "MySQL uyumlu veritabanı sunucusu.",
            "database",
            &["mariadb-server", "mariadb-client"],
            Some("mariadb.service"),
            Repo::Distro,
        ),
        component(
            "postgresql",
            "PostgreSQL",
            "Gelişmiş ilişkisel veritabanı sunucusu.",
            "database",
            &["postgresql", "postgresql-client"],
            Some("postgresql.service"),
            Repo::Distro,
        ),
        component(
            "redis",
            "Redis",
            "Bellek içi önbellek ve kuyruk sunucusu.",
            "database",
            &["redis-server"],
            Some("redis-server.service"),
            Repo::Distro,
        ),
        component(
            "docker",
            "Docker",
            "Konteyner çalıştırma ortamı ve Compose eklentisi.",
            "container",
            &["docker-ce", "docker-ce-cli", "containerd.io", "docker-buildx-plugin", "docker-compose-plugin"],
            Some("docker.service"),
            Repo::Docker,
        ),
        component(
            "phpmyadmin",
            "phpMyAdmin",
            "MariaDB veritabanlarını tarayıcıdan yönetme aracı. Panelden tek tıkla, parola sormadan açılır.",
            "database",
            &["phpmyadmin"],
            None,
            Repo::Distro,
        ),
        component(
            "adminer",
            "Adminer",
            "PostgreSQL veritabanlarını tarayıcıdan yönetme aracı. Panelden tek tıkla, parola sormadan açılır.",
            "database",
            &["adminer"],
            None,
            Repo::Distro,
        ),
        component(
            "dns",
            "DNS sunucusu",
            "PowerDNS. Alan adlarının DNS kayıtlarını bu sunucuda barındırır.",
            "web",
            &["pdns-server", "pdns-backend-sqlite3", "bind9-dnsutils"],
            Some("pdns.service"),
            Repo::Distro,
        ),
        component(
            "ftp",
            "FTP sunucusu",
            "pure-ftpd. Site başına, o sitenin klasörüne hapsedilmiş FTP hesapları açılır.",
            "web",
            &["pure-ftpd"],
            Some("pure-ftpd.service"),
            Repo::Distro,
        ),
        component(
            "roundcube",
            "Roundcube web postası",
            "Posta kutularına tarayıcıdan erişim. Panel adresinin /webmail/ yolunda yayınlanır.",
            "mail",
            &["roundcube-core", "roundcube-sqlite3", "roundcube-plugins"],
            None,
            Repo::Distro,
        ),
        component(
            "mail",
            "Posta sunucusu",
            "Postfix (SMTP), Dovecot (IMAP, filtreler) ve Rspamd (istenmeyen posta süzgeci, DKIM).",
            "mail",
            &[
                "postfix", "dovecot-imapd", "dovecot-lmtpd", "dovecot-sieve", "dovecot-managesieved", "rspamd",
                "redis-server", "bind9-dnsutils",
            ],
            Some("postfix.service"),
            Repo::Distro,
        ),
    ]);
    list
}

pub fn find(id: &str) -> AppResult<Component> {
    catalog().into_iter().find(|c| c.id == id).ok_or(AppError::NotFound)
}

/// Paket kuruluysa sürümünü döner.
async fn installed_version(package: &str) -> Option<String> {
    let output = exec::run("dpkg-query", &["-W", "-f=${db:Status-Abbrev}\t${Version}", "--", package]).await.ok()?;
    let (status, version) = output.stdout.split_once('\t')?;
    status.starts_with("ii").then(|| version.trim().to_string())
}

/// Kurulu Node.js sürümünün tam numarası (ör. `22.11.0`).
async fn node_version(major: &str) -> Option<String> {
    let binary = hosting::node::bin_dir(major).join("node");
    if !binary.is_file() {
        return None;
    }
    let output = exec::run(&binary.to_string_lossy(), &["--version"]).await.ok()?;
    output.success().then(|| output.stdout.trim().trim_start_matches('v').to_string())
}

pub async fn status(component: &Component) -> ComponentStatus {
    if let Some(major) = component.id.strip_prefix("node-") {
        let version = node_version(major).await;
        return ComponentStatus {
            id: component.id.clone(),
            name: component.name.clone(),
            description: component.description,
            category: component.category,
            installed: version.is_some(),
            running: version.is_some(),
            version,
            service: None,
        };
    }
    let version = installed_version(&component.packages[0]).await;
    let running = match (&version, &component.service) {
        (Some(_), Some(service)) => services::is_active(service).await,
        _ => false,
    };
    ComponentStatus {
        id: component.id.clone(),
        name: component.name.clone(),
        description: component.description,
        category: component.category,
        installed: version.is_some(),
        version,
        service: component.service.clone(),
        running,
    }
}

pub async fn status_all() -> Vec<ComponentStatus> {
    let catalog = catalog();
    let mut list = Vec::with_capacity(catalog.len());
    for component in &catalog {
        list.push(status(component).await);
    }
    list
}

/// Dağıtım kimliği ve kod adı (ör. `debian`, `trixie`).
fn os_release() -> AppResult<(String, String)> {
    let raw = std::fs::read_to_string("/etc/os-release")?;
    let field = |key: &str| {
        raw.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim_matches('"').to_string())
    };
    let id = field("ID").ok_or_else(|| AppError::internal("işletim sistemi tanınamadı"))?;
    let codename = field("VERSION_CODENAME").ok_or_else(|| AppError::internal("dağıtım kod adı bulunamadı"))?;
    if id != "debian" && id != "ubuntu" {
        return Err(AppError::bad("Yalnızca Debian ve Ubuntu destekleniyor"));
    }
    Ok((id, codename))
}

/// `Dpkg::Use-Pty=0`, ilerleme çubuğunun günlüğü tek satırlık gürültüye çevirmesini önler.
const APT_OPTIONS: &[&str] = &[
    "-y", "-o", "DPkg::Lock::Timeout=180", "-o", "Dpkg::Use-Pty=0", "-o", "Dpkg::Options::=--force-confold",
];

pub async fn apt_install(ctx: &JobCtx, packages: &[String]) -> AppResult<()> {
    let mut args: Vec<&str> = vec!["install", "--no-install-recommends"];
    args.extend_from_slice(APT_OPTIONS);
    args.push("--");
    args.extend(packages.iter().map(String::as_str));
    ctx.exec("apt-get", &args).await
}

async fn ensure_repo(ctx: &JobCtx, repo: Repo) -> AppResult<()> {
    if repo == Repo::Distro {
        return Ok(());
    }
    let (os_id, codename) = os_release()?;
    apt_install(ctx, &["curl".into(), "ca-certificates".into(), "gnupg".into()]).await?;

    match (repo, os_id.as_str()) {
        (Repo::Php, "debian") => {
            let list_path = "/etc/apt/sources.list.d/pixpanel-php.list";
            if !std::path::Path::new(list_path).exists() {
                let keyring = "/var/lib/pixpanel/debsuryorg-archive-keyring.deb";
                ctx.exec("curl", &["-fsSLo", keyring, "https://packages.sury.org/debsuryorg-archive-keyring.deb"]).await?;
                ctx.exec("dpkg", &["-i", keyring]).await?;
                std::fs::write(
                    list_path,
                    format!(
                        "deb [signed-by=/usr/share/keyrings/debsuryorg-archive-keyring.gpg] https://packages.sury.org/php/ {codename} main\n"
                    ),
                )?;
            }
        }
        (Repo::Php, _) => {
            apt_install(ctx, &["software-properties-common".into()]).await?;
            ctx.exec("add-apt-repository", &["-y", "ppa:ondrej/php"]).await?;
        }
        (Repo::Docker, _) => {
            let list_path = "/etc/apt/sources.list.d/pixpanel-docker.list";
            if !std::path::Path::new(list_path).exists() {
                std::fs::create_dir_all("/etc/apt/keyrings")?;
                let key = "/etc/apt/keyrings/pixpanel-docker.asc";
                ctx.exec("curl", &["-fsSLo", key, &format!("https://download.docker.com/linux/{os_id}/gpg")]).await?;
                let arch = exec::run_ok("dpkg", &["--print-architecture"]).await?;
                std::fs::write(
                    list_path,
                    format!(
                        "deb [arch={} signed-by={key}] https://download.docker.com/linux/{os_id} {codename} stable\n",
                        arch.trim()
                    ),
                )?;
            }
        }
        (Repo::Distro, _) => {}
    }
    Ok(())
}

/// Paketin depoda kurulabilir bir adayı var mı?
async fn package_available(package: &str) -> bool {
    exec::run("apt-cache", &["policy", "--", package])
        .await
        .map(|out| out.stdout.lines().any(|line| line.trim_start().starts_with("Candidate:") && !line.contains("(none)")))
        .unwrap_or(false)
}

/// Bileşeni kurar; görev günlüğüne ilerleme yazar.
/// Node.js'in resmî derlemesini indirir, sağlama toplamını doğrular ve kurar.
async fn install_node(ctx: &JobCtx, major: &str) -> AppResult<()> {
    use sha2::{Digest, Sha256};
    hosting::node::validate_version(major)?;
    apt_install(ctx, &["curl".into(), "ca-certificates".into(), "xz-utils".into()]).await?;
    let arch = match exec::run_ok("dpkg", &["--print-architecture"]).await?.trim() {
        "amd64" => "x64",
        "arm64" => "arm64",
        other => return Err(AppError::bad(format!("Node.js bu işlemci mimarisi için kurulamıyor: {other}"))),
    };
    let base = format!("https://nodejs.org/dist/latest-v{major}.x");
    let sums = exec::run_ok("curl", &["-fsSL", "--max-time", "60", &format!("{base}/SHASUMS256.txt")]).await?;
    let suffix = format!("-linux-{arch}.tar.xz");
    let (expected, file) = sums
        .lines()
        .filter_map(|line| line.split_once("  "))
        .find(|(_, name)| name.ends_with(&suffix))
        .ok_or_else(|| AppError::internal("Node.js sürüm listesinde uygun derleme bulunamadı"))?;
    // Dosya adı indirme adresine ve yola girdiğinden biçimi ayrıca denetlenir.
    let name_ok = file.starts_with("node-v") && file.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    if !name_ok || expected.len() != 64 {
        return Err(AppError::internal("Node.js sürüm listesi beklenmeyen biçimde"));
    }

    let archive = format!("/var/lib/pixpanel/{file}");
    ctx.log(format!("{file} indiriliyor"));
    ctx.exec("curl", &["-fsSL", "--max-time", "600", "-o", &archive, &format!("{base}/{file}")]).await?;
    let actual = hex::encode(Sha256::digest(std::fs::read(&archive)?));
    if !actual.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(&archive);
        return Err(AppError::internal("İndirilen dosyanın sağlama toplamı uyuşmuyor; kurulum durduruldu"));
    }
    ctx.log("Sağlama toplamı doğrulandı");

    let target = std::path::Path::new(hosting::node::NODE_ROOT).join(major);
    let staging = std::path::Path::new(hosting::node::NODE_ROOT).join(format!("{major}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    ctx.exec("tar", &["-xJf", &archive, "-C", &staging.to_string_lossy(), "--strip-components=1", "--no-same-owner"]).await?;
    // Yeni sürüm hazır olduktan sonra eskisinin yerine geçer; çalışan uygulamalar yeniden başlatılana kadar eski dosyalarla sürer.
    let previous = std::path::Path::new(hosting::node::NODE_ROOT).join(format!("{major}.old"));
    let _ = std::fs::remove_dir_all(&previous);
    if target.exists() {
        std::fs::rename(&target, &previous)?;
    }
    std::fs::rename(&staging, &target)?;
    let _ = std::fs::remove_dir_all(&previous);
    let _ = std::fs::remove_file(&archive);
    ctx.log(format!("Node.js {} kuruldu", node_version(major).await.unwrap_or_default()));
    Ok(())
}

pub async fn install(ctx: &JobCtx, component: &Component) -> AppResult<()> {
    if let Some(major) = component.id.strip_prefix("node-") {
        ctx.log(format!("{} kuruluyor", component.name));
        return install_node(ctx, major).await;
    }
    ctx.log(format!("{} kuruluyor", component.name));
    ensure_repo(ctx, component.repo).await?;
    ctx.exec("apt-get", &["update", "-o", "DPkg::Lock::Timeout=180"]).await?;

    let mut packages = component.packages.clone();
    for package in &component.optional_packages {
        if package_available(package).await {
            packages.push(package.clone());
        } else {
            ctx.log(format!("{package} bu sürüm için depoda yok, atlanıyor"));
        }
    }
    apt_install(ctx, &packages).await?;

    if component.id == "nginx" {
        ctx.log("nginx temel yapılandırması yazılıyor");
        hosting::nginx::ensure_base().await?;
    }
    if let Some(service) = &component.service {
        ctx.exec("systemctl", &["enable", "--now", "--", service]).await?;
    }
    ctx.log(format!("{} kuruldu", component.name));
    Ok(())
}
