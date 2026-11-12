//! Sunucu bileşenleri (nginx, PHP sürümleri, veritabanları, Docker...) kataloğu
//! ve bunların panelden kurulması.

use serde::Serialize;

use crate::db::Db;
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

const PHP_REPO_LIST: &str = "/etc/apt/sources.list.d/pixpanel-php.list";
const DOCKER_REPO_LIST: &str = "/etc/apt/sources.list.d/pixpanel-docker.list";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repo {
    /// Dağıtımın kendi deposu yeterli.
    Distro,
    /// packages.sury.org
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
            "python",
            "Python",
            "Python uygulamaları (Flask, Django, FastAPI) için sanal ortam desteği. Her site kendi bağımlılıklarını ayrı kurar.",
            "node",
            &["python3", "python3-venv", "python3-pip"],
            None,
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
            "fail2ban",
            "fail2ban",
            "Parola deneme saldırılarını kendiliğinden engeller (SSH, panel, posta, FTP).",
            "web",
            &["fail2ban", "python3-systemd", "nftables"],
            Some("fail2ban.service"),
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
            "restic",
            "Yedekleme (restic)",
            "Şifreli ve artımlı yedekler; yerel klasöre, SFTP ya da FTP sunucusuna, S3 uyumlu depoya.",
            "web",
            &["restic", "openssh-client"],
            None,
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
    // Bileşen, zorunlu paketlerinin hepsi kuruluysa kurulu sayılır: kimi sunucu kalıpları
    // tek tek paketlerle (ör. Postfix) gelir ve bu, bileşenin panelce kurulduğunu göstermez.
    let mut version = installed_version(&component.packages[0]).await;
    for package in &component.packages[1..] {
        if version.is_some() && installed_version(package).await.is_none() {
            version = None;
        }
    }
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
        // packages.sury.org Debian'ın yanında Ubuntu sürümlerini de yayınlar (eski PPA oraya taşındı).
        (Repo::Php, _) => {
            let list_path = PHP_REPO_LIST;
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
        (Repo::Docker, _) => {
            let list_path = DOCKER_REPO_LIST;
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

/// Ek deponun panelce yazılan kaynak dosyası.
fn repo_list(repo: Repo) -> Option<&'static str> {
    match repo {
        Repo::Distro => None,
        Repo::Php => Some(PHP_REPO_LIST),
        Repo::Docker => Some(DOCKER_REPO_LIST),
    }
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
    // Depo bu kurulumdan önce de varsa, başarısız bir güncelleme (geçici ağ hatası) onu silmemeli.
    let repo_existed = repo_list(component.repo).is_some_and(|path| std::path::Path::new(path).exists());
    ensure_repo(ctx, component.repo).await?;
    if let Err(err) = ctx.exec("apt-get", &["update", "-o", "DPkg::Lock::Timeout=180"]).await {
        // Yeni eklenen ek depo erişilemezse bırakılmaz; yoksa sonraki bütün kurulumlar da durur.
        if let Some(list) = repo_list(component.repo).filter(|path| !repo_existed && std::path::Path::new(path).exists()) {
            let _ = std::fs::remove_file(list);
            ctx.log(format!("Ek paket deposu kullanılamadığı için kaldırıldı: {list}"));
        }
        return Err(err);
    }

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
    // PowerDNS arka ucu yapılandırılmadan başlayamaz; onu yapılandırma adımı başlatır.
    if let Some(service) = component.service.as_ref().filter(|_| component.id != "dns") {
        ctx.exec("systemctl", &["enable", "--now", "--", service]).await?;
    }
    ctx.log(format!("{} kuruldu", component.name));
    Ok(())
}

// ---- Kaldırma ----

/// Panelden kaldırılabilen bileşenler. Veri tutanlar (MariaDB, PostgreSQL, posta, DNS, nginx)
/// kasıtlı olarak yok: kaldırmaları veri kaybına ya da siteleri durdurmaya yol açar.
pub fn removable(id: &str) -> bool {
    id.starts_with("php-") || matches!(id, "phpmyadmin" | "adminer" | "roundcube" | "redis" | "docker" | "ftp" | "fail2ban")
}

/// Bileşeni kullanımda tutan şey varsa nedenini döner. Kullanımdaki bileşen kaldırılmaz.
pub async fn removal_blocker(db: &Db, component: &Component) -> AppResult<Option<String>> {
    let id = component.id.as_str();
    if let Some(version) = id.strip_prefix("php-") {
        let sites = hosting::sites::list_by_php(db, version)?;
        if !sites.is_empty() {
            let names: Vec<&str> = sites.iter().take(3).map(|site| site.domain.as_str()).collect();
            let more = if sites.len() > 3 { format!(" ve {} site daha", sites.len() - 3) } else { String::new() };
            return Ok(Some(format!("{} site bu PHP sürümünü kullanıyor: {}{more}. Önce sitelerin PHP sürümünü değiştirin.", sites.len(), names.join(", "))));
        }
        // Veritabanı araçları ve web postası PHP ile çalışır; son sürüm kaldırılırsa çalışamazlar.
        let tools = crate::dbtools::installed();
        let remaining = hosting::php::installed_versions().into_iter().filter(|other| other != version).count();
        if remaining == 0 && (tools.phpmyadmin || tools.adminer || tools.roundcube) {
            return Ok(Some("Veritabanı araçları ya da web postası bu PHP sürümüyle çalışıyor; son PHP sürümü kaldırılamaz.".into()));
        }
        return Ok(None);
    }
    match id {
        "redis" => {
            let accounts: i64 = db.call(|conn| conn.query_row("SELECT COUNT(*) FROM redis_instances", [], |row| row.get(0)))?;
            if accounts > 0 {
                return Ok(Some(format!("{accounts} hesabın kendi Redis örneği var. Önce hesapların Redis örneklerini silin.")));
            }
            if status(&find("mail")?).await.installed {
                return Ok(Some("Posta sunucusu (Rspamd) Redis kullanıyor. Önce posta sunucusu kaldırılmalı.".into()));
            }
            Ok(None)
        }
        "ftp" => {
            let accounts: i64 = db.call(|conn| conn.query_row("SELECT COUNT(*) FROM ftp_accounts", [], |row| row.get(0)))?;
            Ok((accounts > 0).then(|| format!("{accounts} FTP hesabı var. Önce hesapları silin.")))
        }
        "docker" => {
            // Docker durmuşsa `docker ps` çalışmaz; bu durumda konteyner klasörüne bakılır.
            let containers = match exec::run("docker", &["ps", "-aq"]).await {
                Ok(output) if output.success() => output.stdout.lines().filter(|line| !line.trim().is_empty()).count(),
                _ => std::fs::read_dir("/var/lib/docker/containers").map(|entries| entries.flatten().count()).unwrap_or(0),
            };
            if containers > 0 {
                return Ok(Some(format!("{containers} konteyner var. Önce konteynerleri ve yığınları silin.")));
            }
            let stacks = std::fs::read_dir("/opt/pixpanel/stacks").map(|entries| entries.flatten().count()).unwrap_or(0);
            Ok((stacks > 0).then(|| format!("{stacks} Compose yığını tanımlı. Önce yığınları silin.")))
        }
        _ => Ok(None),
    }
}

/// Kaldırılacak paketler: bileşenin yalnızca ona ait paketleri. Başka bileşenlerle ortak paketler
/// (python3-systemd, nftables, redis-server...) bilerek dışarıda bırakılır.
async fn removal_packages(component: &Component) -> AppResult<Vec<String>> {
    let candidates: Vec<String> = if let Some(version) = component.id.strip_prefix("php-") {
        hosting::php::validate_version(version)?;
        // Sürüme ait bütün paketler (eklentiler dahil); yalnızca bu sürümün ön ekiyle eşleşenler.
        let prefix = format!("php{version}-");
        let listing = exec::run("dpkg-query", &["-W", "-f=${db:Status-Abbrev}\t${Package}\n", &format!("{prefix}*")]).await?;
        listing
            .stdout
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter(|(status, name)| status.starts_with("ii") && name.starts_with(&prefix))
            .map(|(_, name)| name.trim().to_string())
            .collect()
    } else if component.id == "fail2ban" {
        vec!["fail2ban".into()]
    } else if component.id == "redis" {
        vec!["redis-server".into()]
    } else {
        component.packages.clone()
    };
    let mut installed = Vec::new();
    for package in candidates {
        if !crate::sys::updates::valid_package_name(&package) {
            return Err(AppError::internal(format!("Beklenmeyen paket adı: {package}")));
        }
        if installed_version(&package).await.is_some() {
            installed.push(package);
        }
    }
    Ok(installed)
}

/// `apt-get -s remove` çıktısındaki `Remv` satırlarından kaldırılacak paketler.
fn parse_removals(simulation: &str) -> Vec<String> {
    simulation.lines().filter_map(|line| line.strip_prefix("Remv ")?.split_whitespace().next()).map(str::to_string).collect()
}

/// Bileşeni kaldırır (`apt-get remove`; yapılandırma ve veri dosyaları `purge` edilmez, yeniden
/// kurulumda yerinde bulunur). Önce benzetim yapılır: apt bileşene ait olmayan bir paketi de
/// kaldıracaksa (ör. başka bir bileşenin bağımlılığı) hiçbir şey yapılmadan durulur.
pub async fn remove(ctx: &JobCtx, component: &Component) -> AppResult<()> {
    ctx.log(format!("{} kaldırılıyor", component.name));
    let packages = removal_packages(component).await?;
    if packages.is_empty() {
        ctx.log("Kaldırılacak paket yok");
        return Ok(());
    }
    let mut simulate: Vec<&str> = vec!["-s", "-o", "Debug::NoLocking=true", "remove", "--"];
    simulate.extend(packages.iter().map(String::as_str));
    let plan = exec::run_with_timeout("apt-get", &simulate, std::time::Duration::from_secs(120)).await?;
    if !plan.success() {
        return Err(AppError::internal(format!("Kaldırma planı çıkarılamadı: {}", plan.stderr.trim())));
    }
    let extra: Vec<String> = parse_removals(&plan.stdout).into_iter().filter(|name| !packages.contains(name)).collect();
    if !extra.is_empty() {
        return Err(AppError::conflict(format!(
            "Kaldırma, başka yazılımların dayandığı şu paketleri de silerdi; hiçbir şey kaldırılmadı: {}",
            extra.join(", ")
        )));
    }
    let mut args: Vec<&str> = vec!["remove", "-y", "-o", "DPkg::Lock::Timeout=180", "-o", "Dpkg::Use-Pty=0", "--"];
    args.extend(packages.iter().map(String::as_str));
    ctx.exec("apt-get", &args).await?;
    ctx.log(format!("{} kaldırıldı (yapılandırma dosyaları korundu)", component.name));
    Ok(())
}

#[cfg(test)]
mod removal_tests {
    use super::*;

    #[test]
    fn only_safe_components_are_removable() {
        for id in ["php-8.3", "phpmyadmin", "adminer", "roundcube", "redis", "docker", "ftp", "fail2ban"] {
            assert!(removable(id), "{id}");
        }
        for id in ["nginx", "mariadb", "postgresql", "mail", "dns", "restic", "python", "node-22"] {
            assert!(!removable(id), "{id}");
        }
    }

    #[test]
    fn removal_plan_lists_packages_to_be_removed() {
        let simulation = "Reading package lists...\nRemv phpmyadmin [5.2.1]\nRemv php8.3-cli [8.3.1] []\nPurg x [1]\n";
        assert_eq!(parse_removals(simulation), vec!["phpmyadmin".to_string(), "php8.3-cli".to_string()]);
    }
}
