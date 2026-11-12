//! Veritabanı sunucularına uzaktan erişim (MariaDB ve PostgreSQL).
//!
//! İki ayrı katman vardır ve ikisi de kapalı varsayılır:
//!
//! * **Sunucu ayarı (yalnızca yönetici):** veritabanı sunucusunun yerel olmayan arayüzlerde de
//!   dinlemesi. Panel bunun için küçük bir yapılandırma dosyası yazar (MariaDB: `bind-address`,
//!   PostgreSQL: `listen_addresses`) ve servisi yeniden başlatır. Yeniden başlatma başarısız olursa
//!   eski yapılandırma geri konup servis yeniden başlatılır; veritabanı kapalı bırakılmaz.
//! * **Kullanıcı başına adres:** MariaDB'de hesabın `host` kısmı; PostgreSQL'de `pg_hba.conf`
//!   içindeki panelin yönettiği işaretli blok. Blok, uzaktan erişimi olan her kullanıcı için yalnızca
//!   yetkili olduğu veritabanlarına izin veren satırlar içerir (mümkünse `hostssl`, yani TLS zorunlu).
//!   Dosyanın bloğun dışındaki satırlarına dokunulmaz.
//!
//! `pg_hba.conf` satırına yalnızca doğrulanmış (küçük harf, rakam, alt çizgi) adlar ve `std::net`
//! ile ayrıştırılmış adresler girer; adlar ayrıca çift tırnağa alınır, böylece `all` ya da
//! `replication` gibi özel sözcükler hiçbir zaman anahtar sözcük olarak yorumlanmaz.
//!
//! Güvenlik duvarı portlarına dokunulmaz; yönetici onları Güvenlik duvarı sayfasından açar.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use super::{mariadb, postgres, Engine};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sys::{exec, services};

/// Panelin MariaDB için yazdığı yapılandırma dosyası.
pub const MARIADB_CONFIG: &str = "/etc/mysql/mariadb.conf.d/99-pixpanel-remote.cnf";
const MARIADB_CONFIG_DIR: &str = "/etc/mysql/mariadb.conf.d";
/// PostgreSQL küme klasörü içindeki `conf.d` altında panelin dosyasının adı.
const PG_CONFIG_NAME: &str = "90-pixpanel.conf";
/// `pg_hba.conf` içinde panelin yönettiği bloğun işaretleri.
pub const BLOCK_START: &str = "# PixPanel başla";
pub const BLOCK_END: &str = "# PixPanel bitti";

// ---- Adres doğrulama ----

/// Bir veritabanı kullanıcısının bağlanabileceği yer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSpec {
    /// Yalnızca bu sunucu.
    Local,
    /// Her yer (`%`).
    Any,
    /// Tek IP adresi.
    Addr(IpAddr),
    /// Ağ aralığı; adres, ağ adresine indirgenmiştir (`10.0.0.5/24` → `10.0.0.0/24`).
    Cidr(IpAddr, u8),
    /// MariaDB'ye özgü `192.168.1.%` kalıbı.
    Pattern(String),
}

fn invalid_host() -> AppError {
    AppError::bad("Geçersiz bağlantı adresi (örnek: localhost, % , 203.0.113.10 ya da 203.0.113.0/24)")
}

/// Adresin ilk `bits` biti korunur, kalanı sıfırlanır.
fn network_of(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(ip) => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - u32::from(bits)) };
            IpAddr::V4(Ipv4Addr::from(u32::from(ip) & mask))
        }
        IpAddr::V6(ip) => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - u32::from(bits)) };
            IpAddr::V6(Ipv6Addr::from(u128::from(ip) & mask))
        }
    }
}

/// `10.%`, `192.168.%`, `192.168.1.%` biçimindeki MariaDB kalıbı mı?
fn is_pattern(host: &str) -> bool {
    let Some(prefix) = host.strip_suffix(".%") else { return false };
    let parts: Vec<&str> = prefix.split('.').collect();
    (1..=3).contains(&parts.len())
        && parts.iter().all(|part| !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit()) && part.parse::<u16>().is_ok_and(|n| n <= 255))
}

/// Adresi ayrıştırır. Önce karakter kümesi beyaz listeyle sınırlanır; satır sonu, boşluk, tırnak,
/// `#`, `;` gibi hiçbir karakter buradan geçemez.
pub fn parse_host(host: &str) -> AppResult<HostSpec> {
    if host == "localhost" {
        return Ok(HostSpec::Local);
    }
    if host == "%" {
        return Ok(HostSpec::Any);
    }
    if host.is_empty() || host.len() > 60 || !host.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b'.' | b':' | b'%' | b'/')) {
        return Err(invalid_host());
    }
    if let Some((address, prefix)) = host.split_once('/') {
        let ip: IpAddr = address.parse().map_err(|_| invalid_host())?;
        if prefix.is_empty() || prefix.len() > 3 || !prefix.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid_host());
        }
        let bits: u8 = prefix.parse().map_err(|_| invalid_host())?;
        let max = if ip.is_ipv4() { 32 } else { 128 };
        if bits == 0 {
            return Err(AppError::bad("/0 aralığı her yer demektir; bunun için % seçin"));
        }
        if bits > max {
            return Err(invalid_host());
        }
        return Ok(HostSpec::Cidr(network_of(ip, bits), bits));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_unspecified() {
            return Err(invalid_host());
        }
        return Ok(HostSpec::Addr(ip));
    }
    if is_pattern(host) {
        return Ok(HostSpec::Pattern(host.to_string()));
    }
    Err(invalid_host())
}

/// Adresin kayıtlı (kanonik) yazımı.
fn format_host(spec: &HostSpec) -> String {
    match spec {
        HostSpec::Local => "localhost".to_string(),
        HostSpec::Any => "%".to_string(),
        HostSpec::Addr(ip) => ip.to_string(),
        HostSpec::Cidr(ip, bits) => format!("{ip}/{bits}"),
        HostSpec::Pattern(pattern) => pattern.clone(),
    }
}

/// Kullanıcı oluşturma ve adres değiştirme uçlarında girdiyi doğrular ve kayıt biçimine çevirir.
/// Boş değer `localhost` sayılır.
pub fn normalize_host(engine: Engine, host: Option<&str>) -> AppResult<String> {
    // Kırpılmaz: baştaki/sondaki boşluk ya da satır sonu adresin geçersiz olduğunu gösterir.
    // Tümüyle boşluk "verilmedi" sayılır; içinde değer varsa kırpılmadan doğrulanır.
    let host = host.filter(|h| !h.trim().is_empty()).unwrap_or("localhost");
    let spec = parse_host(host)?;
    match (engine, &spec) {
        (Engine::Postgresql, HostSpec::Pattern(_)) => {
            Err(AppError::bad("PostgreSQL için localhost, %, bir IP adresi ya da CIDR aralığı (203.0.113.0/24) girin"))
        }
        (Engine::Mariadb, HostSpec::Cidr(IpAddr::V6(_), _)) => {
            Err(AppError::bad("MariaDB IPv6 aralıklarını (CIDR) desteklemez; tek bir IPv6 adresi ya da IPv4 aralığı girin"))
        }
        _ => Ok(format_host(&spec)),
    }
}

/// Kayıtlı adresin MariaDB hesap adındaki karşılığı: CIDR, MariaDB'nin `ağ/maske` yazımına çevrilir.
pub fn mariadb_account_host(host: &str) -> String {
    match parse_host(host) {
        Ok(HostSpec::Cidr(IpAddr::V4(net), bits)) => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - u32::from(bits)) };
            format!("{net}/{}", Ipv4Addr::from(mask))
        }
        _ => host.to_string(),
    }
}

// ---- pg_hba.conf bloğu ----

/// Bir PostgreSQL kullanıcısının uzaktan erişim kaydı.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HbaEntry {
    pub user: String,
    /// Kullanıcının panelde yetkili olduğu veritabanları.
    pub databases: Vec<String>,
    /// Kayıtlı adres (`%`, IP ya da CIDR).
    pub host: String,
}

/// `pg_hba.conf` satırına girecek adresler; yerel kayıtlar için boş.
fn hba_addresses(host: &str) -> Vec<String> {
    match parse_host(host) {
        Ok(HostSpec::Any) => vec!["0.0.0.0/0".to_string(), "::/0".to_string()],
        Ok(HostSpec::Addr(IpAddr::V4(ip))) => vec![format!("{ip}/32")],
        Ok(HostSpec::Addr(IpAddr::V6(ip))) => vec![format!("{ip}/128")],
        Ok(HostSpec::Cidr(ip, bits)) => vec![format!("{ip}/{bits}")],
        _ => Vec::new(),
    }
}

/// Ad, `pg_hba.conf` için çift tırnaklı biçime çevrilir. Yalnızca küçük harfle başlayan; küçük
/// harf, rakam ve alt çizgiden oluşan adlar geçer.
fn hba_ident(name: &str) -> Option<String> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| format!("\"{name}\""))
}

/// İşaretli bloğun metni; yazılacak satır yoksa `None` (blok hiç bulunmaz). `tls` doğruysa satırlar
/// `hostssl`, değilse `host` olur. Geçersiz adlı ya da adresli kayıtlar atlanır.
pub fn render_hba_block(entries: &[HbaEntry], tls: bool) -> Option<String> {
    let kind = if tls { "hostssl" } else { "host" };
    let mut sorted: Vec<&HbaEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| (&a.user, &a.host).cmp(&(&b.user, &b.host)));
    let mut lines = Vec::new();
    for entry in sorted {
        let Some(user) = hba_ident(&entry.user) else { continue };
        let mut databases: Vec<&String> = entry.databases.iter().collect();
        databases.sort();
        databases.dedup();
        let Some(databases) = databases.iter().map(|name| hba_ident(name)).collect::<Option<Vec<_>>>() else { continue };
        if databases.is_empty() {
            continue;
        }
        for address in hba_addresses(&entry.host) {
            lines.push(format!("{kind} {} {user} {address} scram-sha-256\n", databases.join(",")));
        }
    }
    if lines.is_empty() {
        return None;
    }
    let mut block = format!("{BLOCK_START}\n# PixPanel tarafından yönetilir (Veritabanları > Uzaktan erişim); elle değiştirmeyin.\n");
    for line in lines {
        block.push_str(&line);
    }
    block.push_str(BLOCK_END);
    block.push('\n');
    Some(block)
}

/// Dosya metnindeki işaretli bloğu `block` ile değiştirir (`None` ise kaldırır). Blok daha önce
/// vardıysa aynı yerde kalır, yoksa dosyanın sonuna eklenir; bloğun dışındaki satırlar bayt bayt aynı
/// kalır. İşaretler eşleşmiyorsa (yarım ya da iki blok) hiçbir şey yapılmaz, hata döner.
pub fn replace_block(existing: &str, block: Option<&str>) -> AppResult<String> {
    let broken = || AppError::bad("pg_hba.conf içindeki PixPanel bloğunun işaretleri bozuk; dosyayı elle düzeltin");
    let mut out = String::with_capacity(existing.len());
    let (mut inside, mut insert_at) = (false, None);
    for line in existing.split_inclusive('\n') {
        let marker = line.trim();
        if inside {
            if marker == BLOCK_START {
                return Err(broken());
            }
            if marker == BLOCK_END {
                inside = false;
            }
            continue;
        }
        if marker == BLOCK_START {
            if insert_at.is_some() {
                return Err(broken());
            }
            insert_at = Some(out.len());
            inside = true;
            continue;
        }
        if marker == BLOCK_END {
            return Err(broken());
        }
        out.push_str(line);
    }
    if inside {
        return Err(broken());
    }
    let Some(block) = block else { return Ok(out) };
    match insert_at {
        Some(at) => out.insert_str(at, block),
        None => {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(block);
        }
    }
    Ok(out)
}

// ---- Dosya yazma ve geri alma ----

/// Dosyayı geçici bir ada yazıp yerine taşır. Dosya varsa sahibi ve izinleri korunur (`pg_hba.conf`
/// `postgres` kullanıcısına aittir ve root'a ait olursa PostgreSQL onu okuyamaz); yoksa `mode` ile
/// root sahipli oluşturulur.
fn write_file(path: &Path, content: &[u8], mode: u32) -> AppResult<()> {
    use std::io::Write;
    use std::os::unix::fs::{chown, MetadataExt, OpenOptionsExt, PermissionsExt};
    let existing = std::fs::metadata(path).ok();
    let mode = existing.as_ref().map_or(mode, |meta| meta.mode() & 0o7777);
    let tmp = path.with_extension("pixpanel-tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
    let written = file.write_all(content).and_then(|()| file.sync_all());
    drop(file);
    let prepared = written.and_then(|()| {
        if let Some(meta) = &existing {
            chown(&tmp, Some(meta.uid()), Some(meta.gid()))?;
        }
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
    });
    if let Err(err) = prepared {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

fn remove_file(path: &Path) -> AppResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::internal(format!("{} silinemedi: {err}", path.display()))),
    }
}

/// Bir dosyanın değişiklikten önceki hâli; hata olursa buradan geri konur.
struct Backup {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

impl Backup {
    fn take(path: &Path) -> AppResult<Self> {
        let content = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(AppError::internal(format!("{} okunamadı: {err}", path.display()))),
        };
        Ok(Self { path: path.to_path_buf(), content })
    }

    fn restore(&self) -> AppResult<()> {
        match &self.content {
            Some(bytes) => write_file(&self.path, bytes, 0o644),
            None => remove_file(&self.path),
        }
    }
}

// ---- Durum ----

/// Panelin uzaktan erişim yapılandırmasını yazıp yazmadığı (ayar açık mı).
pub fn is_enabled(engine: Engine) -> bool {
    match engine {
        Engine::Mariadb => Path::new(MARIADB_CONFIG).is_file(),
        Engine::Postgresql => postgres_config_files().iter().any(|path| path.is_file()),
    }
}

/// `/etc/postgresql/<sürüm>/<küme>/conf.d/90-pixpanel.conf` yollarının hepsi (process başlatmadan).
fn postgres_config_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(versions) = std::fs::read_dir("/etc/postgresql") else { return found };
    for version in versions.flatten() {
        let Ok(clusters) = std::fs::read_dir(version.path()) else { continue };
        for cluster in clusters.flatten() {
            found.push(cluster.path().join("conf.d").join(PG_CONFIG_NAME));
        }
    }
    found
}

/// Çalışan sunucunun gerçek dinleme durumu.
#[derive(Debug, Clone)]
pub struct Runtime {
    /// MariaDB'de `bind_address`, PostgreSQL'de `listen_addresses`.
    pub listen: String,
    pub port: u16,
    /// PostgreSQL'de `ssl` açık mı (MariaDB'de yok).
    pub tls: Option<bool>,
}

impl Runtime {
    /// Yalnızca yerel arayüzlerde mi dinliyor?
    pub fn local_only(&self) -> bool {
        self.listen
            .split(',')
            .map(str::trim)
            .all(|address| matches!(address, "127.0.0.1" | "localhost" | "::1" | "[::1]"))
    }
}

pub async fn runtime(engine: Engine) -> AppResult<Runtime> {
    match engine {
        Engine::Mariadb => {
            let output = mariadb::query("SELECT @@global.bind_address, @@global.port;\n").await?;
            let line = output.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("");
            let (listen, port) = line.split_once('\t').unwrap_or((line, "3306"));
            Ok(Runtime { listen: listen.trim().to_string(), port: port.trim().parse().unwrap_or(3306), tls: None })
        }
        Engine::Postgresql => {
            let output = postgres::query("SELECT current_setting('listen_addresses'), current_setting('port'), current_setting('ssl');\n").await?;
            let line = output.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("");
            let mut fields = line.split('\t');
            let listen = fields.next().unwrap_or("").trim().to_string();
            let port = fields.next().and_then(|port| port.trim().parse().ok()).unwrap_or(5432);
            let tls = fields.next().map(|value| value.trim() == "on");
            Ok(Runtime { listen, port, tls })
        }
    }
}

// ---- MariaDB ----

/// Sunucuda kullanılabilir bir IPv6 yığını var mı? (`bind-address = ::` yalnızca o zaman çalışır.)
fn ipv6_available() -> bool {
    Path::new("/proc/net/if_inet6").is_file()
        && std::fs::read_to_string("/proc/sys/net/ipv6/conf/all/disable_ipv6").map_or(true, |value| value.trim() != "1")
}

/// Yapılandırma dosyasının içeriği; `bind` sabit adaylardan biridir (kullanıcı metni girmez).
fn render_mariadb_config(bind: &str) -> String {
    format!(
        "# PixPanel tarafından yönetilir (Veritabanları > Uzaktan erişim); elle değiştirmeyin.\n\
         [mysqld]\n\
         bind-address = {bind}\n"
    )
}

async fn set_mariadb(enabled: bool) -> AppResult<Outcome> {
    let path = Path::new(MARIADB_CONFIG);
    if enabled == path.is_file() {
        return Ok(Outcome { changed: false, tls: None });
    }
    if !Path::new(MARIADB_CONFIG_DIR).is_dir() {
        return Err(AppError::bad(format!("{MARIADB_CONFIG_DIR} klasörü yok; MariaDB yapılandırması beklenen yerde değil")));
    }
    let backup = Backup::take(path)?;
    let service = Engine::Mariadb.service();
    let mut last_error = None;
    if enabled {
        // `::` hem IPv4 hem IPv6'yı dinler; IPv6 yoksa ya da başlatılamazsa yalnızca IPv4'e düşülür.
        let candidates: &[&str] = if ipv6_available() { &["0.0.0.0,::", "0.0.0.0"] } else { &["0.0.0.0"] };
        for bind in candidates {
            if let Err(err) = write_file(path, render_mariadb_config(bind).as_bytes(), 0o644) {
                last_error = Some(err);
                break;
            }
            match services::action(service, "restart").await {
                Ok(()) => return Ok(Outcome { changed: true, tls: None }),
                Err(err) => last_error = Some(err),
            }
        }
    } else {
        match remove_file(path) {
            Ok(()) => match services::action(service, "restart").await {
                Ok(()) => return Ok(Outcome { changed: true, tls: None }),
                Err(err) => last_error = Some(err),
            },
            Err(err) => last_error = Some(err),
        }
    }
    // Bozuk yapılandırmayla kapalı veritabanı bırakılmaz: eski hâl geri konup servis yeniden başlatılır.
    let _ = backup.restore();
    let _ = services::action(service, "restart").await;
    let detail = last_error.map_or_else(String::new, |err| format!(": {err}"));
    Err(AppError::bad(format!("MariaDB yeni ayarla başlatılamadı, eski ayar geri konuldu{detail}")))
}

// ---- PostgreSQL ----

/// Bir PostgreSQL kümesi (Debian'ın `postgresql-common` düzeni).
#[derive(Debug, Clone)]
struct Cluster {
    version: String,
    name: String,
}

impl Cluster {
    fn dir(&self) -> PathBuf {
        Path::new("/etc/postgresql").join(&self.version).join(&self.name)
    }

    fn conf_dir(&self) -> PathBuf {
        self.dir().join("conf.d")
    }

    fn conf_file(&self) -> PathBuf {
        self.conf_dir().join(PG_CONFIG_NAME)
    }

    fn hba_file(&self) -> PathBuf {
        self.dir().join("pg_hba.conf")
    }

    fn unit(&self) -> String {
        format!("postgresql@{}-{}.service", self.version, self.name)
    }
}

fn safe_cluster_part(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) && !value.starts_with(['.', '-'])
}

/// Panelin sürdüğü küme: `pg_lsclusters`ta 5432 numaralı portu kullanan, yoksa ilk küme.
async fn pg_cluster() -> AppResult<Cluster> {
    let mut found: Vec<(Cluster, String)> = Vec::new();
    if let Ok(output) = exec::run("pg_lsclusters", &["--no-header"]).await {
        for line in output.stdout.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let [version, name, port, ..] = fields[..] {
                if safe_cluster_part(version) && safe_cluster_part(name) {
                    found.push((Cluster { version: version.to_string(), name: name.to_string() }, port.to_string()));
                }
            }
        }
    }
    if found.is_empty() {
        // `pg_lsclusters` yoksa klasör düzeninden bulunur (en yeni sürüm önde).
        if let Ok(versions) = std::fs::read_dir("/etc/postgresql") {
            let mut names: Vec<String> = versions.flatten().filter_map(|entry| entry.file_name().into_string().ok()).filter(|name| safe_cluster_part(name)).collect();
            names.sort_by(|a, b| b.cmp(a));
            for version in names {
                let candidate = Cluster { version, name: "main".to_string() };
                if candidate.dir().join("postgresql.conf").is_file() {
                    found.push((candidate, "5432".to_string()));
                }
            }
        }
    }
    let position = found.iter().position(|(_, port)| port == "5432").unwrap_or(0);
    found.into_iter().nth(position).map(|(cluster, _)| cluster).ok_or_else(|| AppError::bad("PostgreSQL kümesi bulunamadı (/etc/postgresql altında yapılandırma yok)"))
}

/// Sunucuda TLS açık mı? (Açıksa `hostssl` satırları kullanılır.)
async fn pg_tls() -> AppResult<bool> {
    let output = postgres::query("SHOW ssl;\n").await?;
    Ok(output.lines().any(|line| line.trim() == "on"))
}

/// `pg_hba.conf` içinde hata satırı var mı? Görünüm dosyayı sorgu anında okur, sunucunun yüklü
/// kurallarına bakmaz; bu yüzden yeniden yüklemeden önce de sınanabilir.
async fn check_hba() -> AppResult<()> {
    let output = postgres::query("SELECT line_number || ': ' || error FROM pg_hba_file_rules WHERE error IS NOT NULL;\n").await?;
    match output.lines().find(|line| !line.trim().is_empty()) {
        None => Ok(()),
        Some(first) => Err(AppError::bad(format!("pg_hba.conf geçersiz (satır {first}); değişiklik geri alındı"))),
    }
}

fn read_hba(path: &Path) -> AppResult<String> {
    std::fs::read_to_string(path).map_err(|err| AppError::internal(format!("{} okunamadı: {err}", path.display())))
}

fn load_hba_entries(db: &Db) -> AppResult<Vec<HbaEntry>> {
    let rows: Vec<(String, String, String)> = db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT du.name, du.host, d.name FROM database_users du
             JOIN database_grants g ON g.db_user_id = du.id
             JOIN databases d ON d.id = g.database_id
             WHERE du.engine = 'postgresql' AND du.host NOT IN ('', 'localhost')
             ORDER BY du.name, d.name",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    })?;
    let mut entries: Vec<HbaEntry> = Vec::new();
    for (user, host, database) in rows {
        match entries.iter_mut().find(|entry| entry.user == user && entry.host == host) {
            Some(entry) => entry.databases.push(database),
            None => entries.push(HbaEntry { user, databases: vec![database], host }),
        }
    }
    Ok(entries)
}

async fn restart_postgres(cluster: &Cluster) -> AppResult<()> {
    services::action(&cluster.unit(), "restart").await
}

async fn set_postgres(db: &Db, enabled: bool) -> AppResult<Outcome> {
    let cluster = pg_cluster().await?;
    let (conf, hba) = (cluster.conf_file(), cluster.hba_file());
    if enabled == conf.is_file() {
        return Ok(Outcome { changed: false, tls: None });
    }
    if !cluster.conf_dir().is_dir() {
        return Err(AppError::bad(format!("{} klasörü yok; PostgreSQL yapılandırması beklenen yerde değil", cluster.conf_dir().display())));
    }
    // TLS durumu için sunucu çalışıyor olmalı; çalışmıyorsa değişiklikten önce açık bir hata alınır.
    let tls = pg_tls().await?;
    let (conf_backup, hba_backup) = (Backup::take(&conf)?, Backup::take(&hba)?);

    let mut restart_tried = false;
    let result: AppResult<()> = async {
        let original = read_hba(&hba)?;
        let block = if enabled { render_hba_block(&load_hba_entries(db)?, tls) } else { None };
        let updated = replace_block(&original, block.as_deref())?;
        if updated != original {
            write_file(&hba, updated.as_bytes(), 0o640)?;
            check_hba().await?;
        }
        if enabled {
            write_file(&conf, "# PixPanel tarafından yönetilir (Veritabanları > Uzaktan erişim); elle değiştirmeyin.\nlisten_addresses = '*'\n".as_bytes(), 0o644)?;
        } else {
            remove_file(&conf)?;
        }
        restart_tried = true;
        restart_postgres(&cluster).await
    }
    .await;

    if let Err(err) = result {
        // Eski yapılandırma geri konur; yeniden başlatma denendiyse sunucu eski ayarla yeniden ayağa kaldırılır.
        let _ = hba_backup.restore();
        let _ = conf_backup.restore();
        if restart_tried {
            let _ = restart_postgres(&cluster).await;
        }
        return Err(AppError::bad(format!("PostgreSQL ayarı uygulanamadı, eski ayar geri konuldu: {err}")));
    }
    Ok(Outcome { changed: true, tls: Some(tls) })
}

/// Uzaktan erişimi olan PostgreSQL kullanıcılarına göre `pg_hba.conf` bloğunu yeniden üretir.
/// Sunucu ayarı kapalıysa ya da PostgreSQL kurulu değilse hiçbir şey yapmaz. Blok değişmediyse
/// dosyaya dokunulmaz; değiştiyse yazılır, `pg_hba_file_rules` ile sınanır (hata varsa eski dosya geri
/// konur) ve yapılandırma yeniden yüklenir (yeniden başlatma gerekmez).
pub async fn sync_postgres_hba(db: &Db) -> AppResult<()> {
    if !Engine::Postgresql.is_installed() || !is_enabled(Engine::Postgresql) {
        return Ok(());
    }
    let cluster = pg_cluster().await?;
    let hba = cluster.hba_file();
    let tls = pg_tls().await?;
    let original = read_hba(&hba)?;
    let updated = replace_block(&original, render_hba_block(&load_hba_entries(db)?, tls).as_deref())?;
    if updated == original {
        return Ok(());
    }
    let backup = Backup::take(&hba)?;
    write_file(&hba, updated.as_bytes(), 0o640)?;
    if let Err(err) = check_hba().await {
        let _ = backup.restore();
        return Err(err);
    }
    postgres::query("SELECT pg_reload_conf();\n").await?;
    Ok(())
}

/// Sunucu ayarını açar ya da kapatır.
pub struct Outcome {
    /// Gerçekten bir değişiklik yapıldı mı (zaten istenen durumdaysa `false`).
    pub changed: bool,
    /// PostgreSQL'de satırların `hostssl` (TLS zorunlu) olup olmadığı.
    pub tls: Option<bool>,
}

pub async fn set_enabled(db: &Db, engine: Engine, enabled: bool) -> AppResult<Outcome> {
    engine.require_installed()?;
    match engine {
        Engine::Mariadb => set_mariadb(enabled).await,
        Engine::Postgresql => set_postgres(db, enabled).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_whitelist() {
        for good in ["localhost", "%", "203.0.113.10", "2001:db8::1", "10.0.0.0/8", "192.168.1.0/24", "2001:db8::/32", "192.168.1.%", "10.%"] {
            assert!(parse_host(good).is_ok(), "{good} geçmeli");
        }
        for bad in [
            "", "1.2.3.4; DROP", "1.2.3.4\n", "%%", "1.2.3.4 ", " 1.2.3.4", "a b", "1.2.3.4'", "1.2.3.4\"", "1.2.3.4#", "0.0.0.0", "::", "999.1.1.1", "1.2.3.4/33", "1.2.3.4/0",
            "1.2.3.4/", "/24", "1.2.3.4/24/8", "1.2.%.4", "%.1.2.3", "1.2.3.4.5.%", "*", "example.com", "1.2.3.4,5.6.7.8", "fe80::1%eth0",
        ] {
            assert!(parse_host(bad).is_err(), "{bad:?} reddedilmeli");
        }
    }

    #[test]
    fn cidr_is_reduced_to_network() {
        assert_eq!(normalize_host(Engine::Postgresql, Some("10.1.2.3/24")).unwrap(), "10.1.2.0/24");
        assert_eq!(normalize_host(Engine::Mariadb, Some("10.1.2.3/8")).unwrap(), "10.0.0.0/8");
        assert_eq!(normalize_host(Engine::Postgresql, Some("2001:db8:1::5/32")).unwrap(), "2001:db8::/32");
        assert_eq!(normalize_host(Engine::Mariadb, None).unwrap(), "localhost");
        assert_eq!(normalize_host(Engine::Postgresql, Some("  ")).unwrap(), "localhost");
    }

    #[test]
    fn engine_specific_limits() {
        assert!(normalize_host(Engine::Postgresql, Some("10.%")).is_err());
        assert!(normalize_host(Engine::Mariadb, Some("10.%")).is_ok());
        assert!(normalize_host(Engine::Mariadb, Some("2001:db8::/32")).is_err());
        assert!(normalize_host(Engine::Mariadb, Some("2001:db8::1")).is_ok());
    }

    #[test]
    fn mariadb_cidr_uses_netmask() {
        assert_eq!(mariadb_account_host("192.168.1.0/24"), "192.168.1.0/255.255.255.0");
        assert_eq!(mariadb_account_host("10.0.0.0/8"), "10.0.0.0/255.0.0.0");
        assert_eq!(mariadb_account_host("203.0.113.5"), "203.0.113.5");
        assert_eq!(mariadb_account_host("%"), "%");
        assert_eq!(mariadb_account_host("10.0.0.0/21"), "10.0.0.0/255.255.248.0");
    }

    fn entry(user: &str, databases: &[&str], host: &str) -> HbaEntry {
        HbaEntry { user: user.to_string(), databases: databases.iter().map(|d| d.to_string()).collect(), host: host.to_string() }
    }

    #[test]
    fn hba_block_lines() {
        let block = render_hba_block(&[entry("ali_app", &["ali_blog", "ali_shop"], "203.0.113.10"), entry("veli", &["veli_db"], "%")], true).unwrap();
        assert!(block.starts_with(&format!("{BLOCK_START}\n")));
        assert!(block.ends_with(&format!("{BLOCK_END}\n")));
        assert!(block.contains("hostssl \"ali_blog\",\"ali_shop\" \"ali_app\" 203.0.113.10/32 scram-sha-256\n"));
        assert!(block.contains("hostssl \"veli_db\" \"veli\" 0.0.0.0/0 scram-sha-256\n"));
        assert!(block.contains("hostssl \"veli_db\" \"veli\" ::/0 scram-sha-256\n"));
        let plain = render_hba_block(&[entry("a", &["b"], "2001:db8::1")], false).unwrap();
        assert!(plain.contains("host \"b\" \"a\" 2001:db8::1/128 scram-sha-256\n"));
        assert!(!plain.contains("hostssl"));
        let cidr = render_hba_block(&[entry("a", &["b"], "10.0.0.0/24")], true).unwrap();
        assert!(cidr.contains("10.0.0.0/24 scram-sha-256"));
    }

    #[test]
    fn hba_block_skips_local_and_empty() {
        assert!(render_hba_block(&[], true).is_none());
        assert!(render_hba_block(&[entry("a", &["b"], "localhost")], true).is_none());
        assert!(render_hba_block(&[entry("a", &["b"], "")], true).is_none());
        // Yetkisi olmayan kullanıcıya satır yazılmaz.
        assert!(render_hba_block(&[entry("a", &[], "%")], true).is_none());
    }

    #[test]
    fn hba_block_rejects_injection() {
        let hostile = [
            entry("a b", &["db"], "%"),
            entry("a\nhost all all 0.0.0.0/0 trust", &["db"], "%"),
            entry("a", &["db\nhost all all 0.0.0.0/0 trust"], "%"),
            entry("a", &["db\" all"], "%"),
            entry("a#", &["db"], "%"),
            entry("A", &["db"], "%"),
            entry("a", &["+all"], "%"),
            entry("a", &["@file"], "%"),
            entry("a", &["db"], "1.2.3.4\nhost all all 0.0.0.0/0 trust"),
            entry("a", &["db"], "1.2.3.4 trust"),
            entry("a", &["db"], "1.2.3.4; DROP"),
        ];
        for item in &hostile {
            assert!(render_hba_block(std::slice::from_ref(item), true).is_none(), "{item:?} satıra girmemeli");
        }
        // Özel sözcük olan adlar çift tırnağa alınır; anahtar sözcük gibi yorumlanmaz.
        let block = render_hba_block(&[entry("replication", &["all"], "10.0.0.1")], true).unwrap();
        assert!(block.contains("hostssl \"all\" \"replication\" 10.0.0.1/32"));
    }

    #[test]
    fn block_replacement_keeps_outside_lines() {
        let base = "# başlık\nlocal all postgres peer\nhost all all 127.0.0.1/32 scram-sha-256\n";
        let block = "# PixPanel başla\nhostssl \"d\" \"u\" 10.0.0.1/32 scram-sha-256\n# PixPanel bitti\n";
        let added = replace_block(base, Some(block)).unwrap();
        assert_eq!(added, format!("{base}{block}"));
        // Kaldırınca dosya eski hâline döner.
        assert_eq!(replace_block(&added, None).unwrap(), base);
        // Bloğun ardından gelen satırlar korunur ve blok aynı yerde güncellenir.
        let tail = "host replication all ::1/128 scram-sha-256\n";
        let with_tail = format!("{base}{block}{tail}");
        let new_block = "# PixPanel başla\nhostssl \"x\" \"y\" 10.0.0.2/32 scram-sha-256\n# PixPanel bitti\n";
        assert_eq!(replace_block(&with_tail, Some(new_block)).unwrap(), format!("{base}{new_block}{tail}"));
        assert_eq!(replace_block(&with_tail, None).unwrap(), format!("{base}{tail}"));
        // Sonu satır sonuyla bitmeyen dosyaya ekleme satırı bozmaz.
        assert_eq!(replace_block("a", Some(block)).unwrap(), format!("a\n{block}"));
        assert_eq!(replace_block("", None).unwrap(), "");
    }

    #[test]
    fn broken_markers_are_refused() {
        let block = "# PixPanel başla\nx\n# PixPanel bitti\n";
        // Kapanmayan blok, iki blok, başlangıçsız bitiş: dosyaya dokunulmaz, hata döner.
        assert!(replace_block("a\n# PixPanel başla\nb\n", Some(block)).is_err());
        assert!(replace_block(&format!("{block}{block}"), Some(block)).is_err());
        assert!(replace_block("a\n# PixPanel bitti\n", None).is_err());
        assert!(replace_block("# PixPanel başla\n# PixPanel başla\n# PixPanel bitti\n", None).is_err());
    }

    #[test]
    fn runtime_locality() {
        let local = |listen: &str| Runtime { listen: listen.to_string(), port: 1, tls: None }.local_only();
        assert!(local("127.0.0.1"));
        assert!(local("localhost"));
        assert!(local("localhost, 127.0.0.1"));
        assert!(!local("*"));
        assert!(!local("0.0.0.0"));
        assert!(!local("::"));
        assert!(!local("127.0.0.1,10.0.0.5"));
    }

    #[test]
    fn mariadb_config_text() {
        let text = render_mariadb_config("0.0.0.0");
        assert!(text.contains("[mysqld]\nbind-address = 0.0.0.0\n"));
        assert!(render_mariadb_config("::").contains("bind-address = ::\n"));
    }

    #[test]
    fn cluster_names_are_whitelisted() {
        assert!(safe_cluster_part("17"));
        assert!(safe_cluster_part("main"));
        assert!(!safe_cluster_part("a b"));
        assert!(!safe_cluster_part("../x"));
        assert!(!safe_cluster_part("-x"));
        assert!(!safe_cluster_part(""));
    }

    /// Rollback: yapılandırma dosyasının yedeği, yeni dosya yazıldıktan sonra eski içeriği ve izinleri geri koyar.
    #[test]
    fn backup_restores_content_and_removes_new_files() {
        let dir = std::env::temp_dir().join(format!("pxremote-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("pg_hba.conf");
        std::fs::write(&existing, "eski\n").unwrap();
        let fresh = dir.join("yeni.conf");

        let (backup_existing, backup_fresh) = (Backup::take(&existing).unwrap(), Backup::take(&fresh).unwrap());
        write_file(&existing, b"yeni\n", 0o644).unwrap();
        write_file(&fresh, b"x\n", 0o644).unwrap();
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "yeni\n");
        assert!(fresh.is_file());

        backup_existing.restore().unwrap();
        backup_fresh.restore().unwrap();
        assert_eq!(std::fs::read_to_string(&existing).unwrap(), "eski\n");
        assert!(!fresh.exists());
        // Geçici dosya geride kalmaz.
        assert!(!existing.with_extension("pixpanel-tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
