//! Siteler: veritabanı kayıtları ve bunların sisteme (klasörler, PHP havuzu,
//! nginx) yansıtılması.

use std::path::{Component as PathComponent, Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::auth::{AuthUser, Role};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::nginx::ProxyRule;
use crate::hosting::node::{self, NodeSettings};
use crate::hosting::php::PhpSettings;
use crate::hosting::sysuser::{self, SysUser};
use crate::hosting::{domain, nginx, php, ssl};

pub const KINDS: &[&str] = &["php", "static", "proxy", "node", "python"];

#[derive(Debug, Clone, Serialize)]
pub struct Site {
    pub id: i64,
    pub user_id: i64,
    /// Sitenin sahibi olan panel hesabı (aynı zamanda Linux kullanıcı adı).
    pub owner: String,
    pub domain: String,
    pub aliases: Vec<String>,
    pub kind: String,
    /// Site klasörü içinde yayınlanan alt klasör (ör. `public`).
    pub public_dir: String,
    pub php_version: Option<String>,
    pub php_settings: PhpSettings,
    pub proxy_target: Option<String>,
    pub proxy_websocket: bool,
    pub ssl_mode: String,
    pub force_https: bool,
    pub hsts: bool,
    pub nginx_custom: String,
    pub status: String,
    pub created_at: i64,
    /// Node.js sitelerinin sürüm, başlatma komutu ve ortam değişkenleri.
    pub node: NodeSettings,
    /// Yol bazlı vekil kuralları (ör. `/api/` → bir Docker konteyneri).
    pub proxy_rules: Vec<ProxyRule>,
    /// Uygulamaya göre hazır web sunucusu ayarı (`default`, `wordpress`, `laravel`, `spa`, `clean_urls`).
    pub template: String,
    /// Sitenin bağlandığı sunucu adresi; boşsa site sunucunun bütün adreslerinden yanıt verir.
    pub ip: String,
    /// Yönlendirmeler, parola koruması, adres engelleme gibi kurallar. Arayüze parola özetleri gitmez.
    #[serde(serialize_with = "crate::hosting::rules::serialize_public")]
    pub rules: crate::hosting::rules::SiteRules,
    /// Site bir WordPress hazırlık ortamıysa asıl sitenin kimliği.
    pub staging_of: Option<i64>,
}

impl Site {
    pub fn root(&self) -> PathBuf {
        PathBuf::from(format!("/home/{}/sites/{}", self.owner, self.domain))
    }

    pub fn docroot(&self) -> PathBuf {
        self.root().join(&self.public_dir)
    }
}

const SITE_COLUMNS: &str = "s.id, s.user_id, u.username, s.domain, s.kind, s.public_dir, s.php_version,
     s.php_settings, s.proxy_target, s.proxy_websocket, s.ssl_mode, s.force_https, s.hsts,
     s.nginx_custom, s.status, s.created_at, s.node_settings, s.proxy_rules, s.template, s.ip, s.rules, s.staging_of";

fn read_site(row: &rusqlite::Row<'_>) -> rusqlite::Result<Site> {
    let php_settings: String = row.get(7)?;
    Ok(Site {
        id: row.get(0)?,
        user_id: row.get(1)?,
        owner: row.get(2)?,
        domain: row.get(3)?,
        aliases: Vec::new(),
        kind: row.get(4)?,
        public_dir: row.get(5)?,
        php_version: row.get(6)?,
        php_settings: serde_json::from_str(&php_settings).unwrap_or_default(),
        proxy_target: row.get(8)?,
        proxy_websocket: row.get(9)?,
        ssl_mode: row.get(10)?,
        force_https: row.get(11)?,
        hsts: row.get(12)?,
        nginx_custom: row.get(13)?,
        status: row.get(14)?,
        created_at: row.get(15)?,
        node: serde_json::from_str(&row.get::<_, String>(16)?).unwrap_or_default(),
        proxy_rules: serde_json::from_str(&row.get::<_, String>(17)?).unwrap_or_default(),
        template: row.get(18)?,
        ip: row.get(19)?,
        rules: serde_json::from_str(&row.get::<_, String>(20)?).unwrap_or_default(),
        staging_of: row.get(21)?,
    })
}

fn load_aliases(conn: &rusqlite::Connection, site_id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM site_aliases WHERE site_id = ?1 ORDER BY name")?;
    let rows = stmt.query_map(params![site_id], |row| row.get(0))?;
    rows.collect()
}

/// Kullanıcının görebildiği siteler: yönetici hepsini, bayi kendisinin ve
/// müşterilerinin, diğerleri yalnızca kendi (alt hesapsa üst hesabının) sitelerini görür.
pub fn list(db: &Db, user: &AuthUser) -> AppResult<Vec<Site>> {
    let owner = user.owner_id();
    let (all, include_children) = (user.is_admin(), user.role == Role::Reseller);
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SITE_COLUMNS} FROM sites s JOIN users u ON u.id = s.user_id
             WHERE ?1 OR s.user_id = ?2 OR (?3 AND u.parent_id = ?2)
             ORDER BY s.domain"
        ))?;
        let mut sites: Vec<Site> =
            stmt.query_map(params![all, owner, include_children], read_site)?.collect::<Result<_, _>>()?;
        for site in &mut sites {
            site.aliases = load_aliases(conn, site.id)?;
        }
        Ok(sites)
    })
}

/// Siteyi sahibinin üst hesabıyla birlikte okur (yetki denetimi yapmaz).
fn load_with_parent(db: &Db, id: i64) -> AppResult<Option<(Site, Option<i64>)>> {
    db.call(|conn| {
        let site = conn
            .query_row(
                // Ek sütun adıyla okunur: sıra numarası, site sütunlarına ekleme yapıldıkça kayar.
                &format!("SELECT {SITE_COLUMNS}, u.parent_id AS owner_parent_id FROM sites s JOIN users u ON u.id = s.user_id WHERE s.id = ?1"),
                params![id],
                |row| Ok((read_site(row)?, row.get::<_, Option<i64>>("owner_parent_id")?)),
            )
            .optional()?;
        match site {
            Some((mut site, parent_id)) => {
                site.aliases = load_aliases(conn, site.id)?;
                Ok(Some((site, parent_id)))
            }
            None => Ok(None),
        }
    })
}

/// Siteyi yetki denetimi olmadan okur. Yalnızca panelin kendi başlattığı
/// işlerde (arka plan görevleri, otomatik yenileme) kullanılmalıdır.
pub fn load(db: &Db, id: i64) -> AppResult<Option<Site>> {
    Ok(load_with_parent(db, id)?.map(|(site, _)| site))
}

/// Sertifikası olan bütün siteler (otomatik yenileme için).
pub fn list_with_ssl(db: &Db) -> AppResult<Vec<Site>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SITE_COLUMNS} FROM sites s JOIN users u ON u.id = s.user_id WHERE s.ssl_mode != 'none'"
        ))?;
        let mut sites: Vec<Site> = stmt.query_map([], read_site)?.collect::<Result<_, _>>()?;
        for site in &mut sites {
            site.aliases = load_aliases(conn, site.id)?;
        }
        Ok(sites)
    })
}

/// Verilen PHP sürümünü kullanan bütün siteler (genel ayar değişince havuzları yenilemek için).
pub fn list_by_php(db: &Db, version: &str) -> AppResult<Vec<Site>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SITE_COLUMNS} FROM sites s JOIN users u ON u.id = s.user_id
             WHERE s.kind = 'php' AND s.php_version = ?1 ORDER BY s.domain"
        ))?;
        stmt.query_map(params![version], read_site)?.collect()
    })
}

/// Verilen uv Python sürümünü seçmiş Python siteleri (sürüm kaldırılmadan önce denetlenir).
pub fn list_by_python(db: &Db, version: &str) -> AppResult<Vec<Site>> {
    let sites: Vec<Site> = db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {SITE_COLUMNS} FROM sites s JOIN users u ON u.id = s.user_id WHERE s.kind = 'python' ORDER BY s.domain"
        ))?;
        stmt.query_map([], read_site)?.collect()
    })?;
    Ok(sites.into_iter().filter(|site| site.node.python_choice() == Some(version)).collect())
}

/// Siteyi okur ve isteği yapanın ona erişimi olduğunu doğrular.
pub fn get(db: &Db, user: &AuthUser, id: i64) -> AppResult<Site> {
    let (site, owner_parent) = load_with_parent(db, id)?.ok_or(AppError::NotFound)?;
    let allowed = user.is_admin()
        || site.user_id == user.owner_id()
        || (user.role == Role::Reseller && owner_parent == Some(user.id));
    // Başkasının sitesinin varlığı da sızdırılmaz.
    if allowed { Ok(site) } else { Err(AppError::NotFound) }
}

/// Yayın klasörü site kökünün içinde kalmalıdır: `..`, mutlak yol ve
/// alışılmadık karakterler reddedilir.
pub fn validate_public_dir(value: &str) -> AppResult<String> {
    let trimmed = value.trim().trim_matches('/');
    let path = Path::new(trimmed);
    let parts: Vec<&str> = path
        .components()
        .map(|component| match component {
            PathComponent::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()
        .ok_or_else(|| AppError::bad("Geçersiz yayın klasörü"))?;
    let part_ok = |part: &&str| {
        !part.is_empty()
            && part.len() <= 64
            && !part.starts_with('.')
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    };
    if parts.is_empty() || parts.len() > 4 || !parts.iter().all(part_ok) || parts[0] == "logs" {
        return Err(AppError::bad("Geçersiz yayın klasörü (örnek: public ya da app/public)"));
    }
    Ok(parts.join("/"))
}

/// Yerel adreste müşteri sitelerinin vekilleyemeyeceği portlar: panel, veritabanı araçları, DNS ve
/// posta süzgeci yönetim uçları, veritabanları. Bu servisler yerel bağlantıya güvenir.
const INTERNAL_PORTS: &[u16] = &[8443, 8446, 8053, 11332, 11333, 11334, 3306, 5432, 6379, 953, 783, 9911];

/// Hedef, sunucunun içinden ulaşılıp dışarıya açılmaması gereken bir adres mi?
fn internal_target(url: &url::Url, panel_port: u16) -> bool {
    use std::net::IpAddr;
    let port = url.port_or_known_default().unwrap_or(80);
    let local_port_blocked = port < 1024 || port == panel_port || INTERNAL_PORTS.contains(&port) || (16400..17400).contains(&port);
    let address: Option<IpAddr> = match url.host() {
        Some(url::Host::Ipv4(address)) => Some(IpAddr::V4(address)),
        Some(url::Host::Ipv6(address)) => Some(IpAddr::V6(address)),
        Some(url::Host::Domain(name)) if name.eq_ignore_ascii_case("localhost") || name.to_ascii_lowercase().ends_with(".localhost") => {
            return local_port_blocked;
        }
        _ => None,
    };
    match address {
        Some(IpAddr::V4(v4)) if v4.is_loopback() => local_port_blocked,
        Some(IpAddr::V6(v6)) if v6.is_loopback() => local_port_blocked,
        // Bulut sağlayıcıların kimlik bilgisi sunan metaveri adresi (169.254.169.254) ve iç ağlar.
        Some(IpAddr::V4(v4)) => v4.is_link_local() || v4.is_private() || v4.is_unspecified() || v4.is_broadcast() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64),
        Some(IpAddr::V6(v6)) => v6.is_unspecified() || (v6.segments()[0] & 0xffc0) == 0xfe80 || (v6.segments()[0] & 0xfe00) == 0xfc00,
        None => false,
    }
}

/// Vekil hedefi yalnızca `http(s)://sunucu[:port][/yol]` biçiminde olabilir.
///
/// `trusted`: isteği yönetici yaptı. Öteki hesaplar iç servislere ve iç ağa yönlendiremez;
/// aksi hâlde site, o servisleri kendi alan adından dışarıya açardı.
pub fn validate_proxy_target(value: &str, trusted: bool, panel_port: u16) -> AppResult<String> {
    let checked = validate_proxy_target_form(value)?;
    if !trusted {
        let parsed = url::Url::parse(&checked).map_err(|_| AppError::bad("Geçersiz hedef adres"))?;
        if internal_target(&parsed, panel_port) {
            return Err(AppError::bad("Bu hedefe yönlendirme yapılamaz: sunucunun iç servisleri ve iç ağ adresleri yalnızca yönetici tarafından hedef gösterilebilir"));
        }
    }
    Ok(checked)
}

fn validate_proxy_target_form(value: &str) -> AppResult<String> {
    let invalid = || AppError::bad("Geçersiz hedef adres (örnek: http://127.0.0.1:3000)");
    let parsed = url::Url::parse(value.trim()).map_err(|_| invalid())?;
    let plain = matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    // Yeniden üretilen adres nginx dosyasına yazılır; yapıyı bozabilecek
    // karakter içermediği ayrıca denetlenir.
    let rendered = parsed.as_str().to_string();
    let safe = rendered.bytes().all(|b| b.is_ascii_alphanumeric() || b":/.-_~%[]".contains(&b));
    if plain && safe { Ok(rendered) } else { Err(invalid()) }
}

/// Vekil kuralının yolu: `/` ile başlar, yalnızca güvenli karakterler içerir.
/// Kök yol (`/`) kural olamaz; bütün siteyi iletmek için site türü kullanılır.
pub fn validate_proxy_rules(rules: &[ProxyRule], trusted: bool, panel_port: u16) -> AppResult<Vec<ProxyRule>> {
    if rules.len() > 30 {
        return Err(AppError::bad("Bir siteye en fazla 30 vekil kuralı eklenebilir"));
    }
    let mut normalized: Vec<ProxyRule> = Vec::new();
    for rule in rules {
        let path = rule.path.trim();
        let path_ok = path.starts_with('/')
            && path.len() > 1
            && path.len() <= 100
            && !path.contains("..")
            && !path.contains("//")
            && !path.starts_with("/.well-known")
            && path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.'));
        if !path_ok {
            return Err(AppError::bad(format!("Geçersiz yol: {path} (örnek: /api/ ; harf, rakam, / _ - . kullanılabilir)")));
        }
        if normalized.iter().any(|other| other.path == path) {
            return Err(AppError::bad(format!("{path} yolu için birden fazla kural var")));
        }
        // Hedefin yalnızca kökü (şema, sunucu, port) saklanır; yol kısmı kuraldan gelir.
        let parsed = url::Url::parse(&validate_proxy_target(&rule.target, trusted, panel_port)?).map_err(|_| AppError::bad("Geçersiz hedef adres"))?;
        if parsed.path() != "/" {
            return Err(AppError::bad("Hedef adres yol içeremez (örnek: http://127.0.0.1:8080)"));
        }
        let origin = parsed.as_str().trim_end_matches('/').to_string();
        normalized.push(ProxyRule { path: path.to_string(), target: origin, websocket: rule.websocket, strip_prefix: rule.strip_prefix });
    }
    // Daha özgül (uzun) yollar önce yazılır; okunaklı ve öngörülebilir bir sıra verir.
    normalized.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.path.cmp(&b.path)));
    Ok(normalized)
}

/// Alan adlarının (ana ad ve takma adlar) başka bir sitede kullanılmadığını denetler.
/// Yönetici olmayan bir hesabın bu adlarla site açıp açamayacağını denetler.
///
/// * Sunucunun kendi adları (ana bilgisayar adı, posta sunucusunun adı) site adı olamaz: o adla
///   açılan sitenin sertifikası posta servislerine verilir ve panelin kimliğiyle karışır.
/// * Başka bir hesabın alan adının altındaki adlar (`giris.baskasi.com`) kullanılamaz: o hesabın
///   ziyaretçilerini kendi sitesine çekebilir, o ad için sertifika alabilirdi. Hesabın kendi alan
///   adlarının ve bayisinin alan adlarının altı serbesttir.
pub fn check_names_allowed(db: &Db, acting: &AuthUser, owner_id: i64, names: &[String]) -> AppResult<()> {
    if acting.is_admin() {
        return Ok(());
    }
    let mut reserved = vec![crate::mail::system_hostname(), crate::mail::hostname(db)];
    if let Ok(short) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        reserved.push(short.trim().to_string());
    }
    let reserved: Vec<String> = reserved.into_iter().map(|name| name.to_ascii_lowercase()).filter(|name| name.contains('.')).collect();
    for name in names {
        if reserved.iter().any(|host| host == name || name.strip_prefix("www.") == Some(host.as_str())) {
            return Err(AppError::bad(format!("{name} bu sunucunun kendi adı; site adı olarak yalnızca yönetici kullanabilir")));
        }
    }
    // Hesabın kendisi, isteği yapan ve hesabın bayisi "yabancı" sayılmaz.
    let parent: Option<i64> = db.call(|conn| conn.query_row("SELECT parent_id FROM users WHERE id = ?1", params![owner_id], |row| row.get(0)).optional())?.flatten();
    let own = [Some(owner_id), Some(acting.owner_id()), parent];
    let foreign: Vec<(String, i64)> = db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT domain, user_id FROM sites
             UNION SELECT a.name, s.user_id FROM site_aliases a JOIN sites s ON s.id = a.site_id
             UNION SELECT domain, user_id FROM dns_zones
             UNION SELECT domain, user_id FROM mail_domains",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })?;
    for name in names {
        if let Some((taken, _)) = foreign.iter().find(|(other, user_id)| !own.contains(&Some(*user_id)) && name.ends_with(&format!(".{other}"))) {
            return Err(AppError::bad(format!("{name} adı, başka bir hesaba ait {taken} alan adının altında; kullanılamaz")));
        }
    }
    Ok(())
}

fn ensure_names_free(conn: &rusqlite::Connection, names: &[String], except_site: Option<i64>) -> rusqlite::Result<Option<String>> {
    for name in names {
        let taken: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sites WHERE domain = ?1 AND id IS NOT ?2)
                 OR EXISTS(SELECT 1 FROM site_aliases WHERE name = ?1 AND site_id IS NOT ?2)",
            params![name, except_site],
            |row| row.get(0),
        )?;
        if taken {
            return Ok(Some(name.clone()));
        }
    }
    Ok(None)
}

pub fn normalize_aliases(domain_name: &str, aliases: &[String]) -> AppResult<Vec<String>> {
    let mut normalized = Vec::new();
    for alias in aliases {
        let alias = domain::normalize(alias)?;
        if alias != domain_name && !normalized.contains(&alias) {
            normalized.push(alias);
        }
    }
    if normalized.len() > 50 {
        return Err(AppError::bad("Bir siteye en fazla 50 takma ad eklenebilir"));
    }
    Ok(normalized)
}

pub struct NewSite {
    pub user_id: i64,
    pub domain: String,
    pub aliases: Vec<String>,
    pub kind: String,
    pub php_version: Option<String>,
    pub proxy_target: Option<String>,
    pub node: NodeSettings,
}

/// Site kaydını ekler ve kimliğini döner. Girdiler çağıran tarafından doğrulanmış olmalıdır.
pub fn insert(db: &Db, new: &NewSite) -> AppResult<i64> {
    let mut names = vec![new.domain.clone()];
    names.extend(new.aliases.iter().cloned());
    let node_settings = serde_json::to_string(&new.node).map_err(|err| AppError::internal(err.to_string()))?;
    let outcome = db.call(|conn| {
        let tx = conn.transaction()?;
        if let Some(taken) = ensure_names_free(&tx, &names, None)? {
            return Ok(Err(taken));
        }
        tx.execute(
            "INSERT INTO sites (user_id, domain, kind, php_version, proxy_target, node_settings, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![new.user_id, new.domain, new.kind, new.php_version, new.proxy_target, node_settings, crate::db::now()],
        )?;
        let id = tx.last_insert_rowid();
        for alias in &new.aliases {
            tx.execute("INSERT INTO site_aliases (name, site_id) VALUES (?1, ?2)", params![alias, id])?;
        }
        tx.commit()?;
        Ok(Ok(id))
    })?;
    outcome.map_err(|taken| AppError::conflict(format!("{taken} başka bir sitede zaten kullanılıyor")))
}

/// Sitenin değiştirilebilir alanlarını (ve takma adlarını) kaydeder.
pub fn save(db: &Db, site: &Site) -> AppResult<()> {
    let to_json = |err: serde_json::Error| AppError::internal(err.to_string());
    let settings = serde_json::to_string(&site.php_settings).map_err(to_json)?;
    let node_settings = serde_json::to_string(&site.node).map_err(to_json)?;
    let proxy_rules = serde_json::to_string(&site.proxy_rules).map_err(to_json)?;
    // Saklanan biçim parola özetlerini içerir (arayüze giden görünüm içermez).
    let rules = serde_json::to_string(&site.rules).map_err(to_json)?;
    let outcome = db.call(|conn| {
        let tx = conn.transaction()?;
        if let Some(taken) = ensure_names_free(&tx, &site.aliases, Some(site.id))? {
            return Ok(Err(taken));
        }
        tx.execute(
            "UPDATE sites SET kind = ?1, public_dir = ?2, php_version = ?3, php_settings = ?4, proxy_target = ?5,
                    proxy_websocket = ?6, ssl_mode = ?7, force_https = ?8, hsts = ?9, nginx_custom = ?10, status = ?11,
                    node_settings = ?12, proxy_rules = ?13, template = ?14, ip = ?15, rules = ?16
             WHERE id = ?17",
            params![
                site.kind, site.public_dir, site.php_version, settings, site.proxy_target, site.proxy_websocket,
                site.ssl_mode, site.force_https, site.hsts, site.nginx_custom, site.status, node_settings, proxy_rules,
                site.template, site.ip, rules, site.id
            ],
        )?;
        tx.execute("DELETE FROM site_aliases WHERE site_id = ?1", params![site.id])?;
        for alias in &site.aliases {
            tx.execute("INSERT INTO site_aliases (name, site_id) VALUES (?1, ?2)", params![alias, site.id])?;
        }
        tx.commit()?;
        Ok(Ok(()))
    })?;
    outcome.map_err(|taken| AppError::conflict(format!("{taken} başka bir sitede zaten kullanılıyor")))
}

/// Siteyi bir asıl sitenin hazırlık ortamı olarak işaretler (ya da bağı koparır).
pub fn set_staging_of(db: &Db, id: i64, live: Option<i64>) -> AppResult<()> {
    db.call(|conn| conn.execute("UPDATE sites SET staging_of = ?1 WHERE id = ?2", params![live, id]))?;
    Ok(())
}

/// Sitenin hazırlık ortamları: (kimlik, alan adı).
pub fn stagings_of(db: &Db, id: i64) -> AppResult<Vec<(i64, String)>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, domain FROM sites WHERE staging_of = ?1 ORDER BY domain")?;
        let rows = stmt.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })
}

pub fn delete_row(db: &Db, id: i64) -> AppResult<()> {
    db.call(|conn| conn.execute("DELETE FROM sites WHERE id = ?1", params![id]))?;
    Ok(())
}

/// Sitenin sertifikası diskte hazır mı?
pub fn ssl_ready(site: &Site) -> bool {
    let (cert, key) = nginx::certificate_paths(&site.domain);
    site.ssl_mode != "none" && cert.exists() && key.exists()
}

/// Site kaydını sisteme uygular: kullanıcı, klasörler, PHP havuzu ve nginx.
/// Tekrar tekrar çağrılabilir; her seferinde aynı sonucu üretir.
pub async fn provision(site: &Site) -> AppResult<()> {
    let user: SysUser = sysuser::ensure(&site.owner).await?;
    let root = sysuser::create_site_dirs(&user, &site.domain, &site.public_dir)?;
    let docroot = site.docroot();
    let socket = site.php_version.as_deref().map(|version| php::socket_path(site.id, version)).unwrap_or_default();
    let node_target = format!("http://127.0.0.1:{}", node::port(site.id));
    if !node::is_app(&site.kind) {
        // Tür değiştiyse eski uygulama servisi kaldırılır.
        node::remove(site.id).await?;
    }

    let backend = match site.kind.as_str() {
        "php" => {
            let version = site.php_version.as_deref().ok_or_else(|| AppError::bad("PHP sürümü seçilmedi"))?;
            php::apply_pool(site.id, &site.domain, version, &user, &root, &site.php_settings).await?;
            nginx::Backend::Php { docroot: &docroot, socket: &socket }
        }
        "proxy" => {
            let target = site.proxy_target.as_deref().ok_or_else(|| AppError::bad("Hedef adres girilmedi"))?;
            nginx::Backend::Proxy { target, websocket: site.proxy_websocket }
        }
        "node" | "python" => {
            // Askıdaki sitenin uygulaması çalıştırılmaz.
            if site.status == "active" {
                node::apply(site.id, &site.domain, &user, &root, &site.node, &site.kind).await?;
            } else {
                let _ = node::action(site.id, "stop").await;
            }
            nginx::Backend::Proxy { target: &node_target, websocket: true }
        }
        _ => nginx::Backend::Static { docroot: &docroot },
    };

    // Kuralların dayandığı dosyalar yapılandırmadan önce yerinde olmalı.
    crate::hosting::rules::write_password_files(site.id, &site.rules)?;
    crate::hosting::rules::ensure_pages()?;
    let live = site.status == "active";
    let cache_wanted = live && site.rules.server_cache.enabled && matches!(site.kind.as_str(), "php" | "proxy" | "node" | "python");
    if cache_wanted {
        crate::hosting::rules::ensure_cache_dir(site.id)?;
    }
    if live && site.rules.cloudflare_only {
        crate::hosting::rules::ensure_cloudflare_ranges().await?;
    }
    nginx::apply(&nginx::SiteConfig {
        id: site.id,
        rules: &site.rules,
        bind: crate::addresses::bind_for(&site.ip).await,
        domain: &site.domain,
        aliases: &site.aliases,
        backend,
        ssl: ssl_ready(site),
        force_https: site.force_https,
        hsts: site.hsts,
        suspended: site.status != "active",
        custom: &site.nginx_custom,
        proxy_rules: &site.proxy_rules,
        // Türü değişmiş bir sitede eski türün şablonu kalmış olabilir; geçersizse varsayılan uygulanır.
        template: if nginx::template_allowed(&site.kind, &site.template) { &site.template } else { "default" },
    })
    .await?;
    // Önbellek kapandıysa klasörü nginx bölgeyi bıraktıktan (yeniden yükleme tamamlandıktan) sonra silinir.
    if !cache_wanted {
        crate::hosting::rules::remove_cache_dir(site.id);
    }

    // nginx artık yeni sokete baktığına göre eski sürümün havuzu kaldırılabilir.
    let keep = if site.kind == "php" { site.php_version.as_deref() } else { None };
    php::remove_pool_except(site.id, keep).await
}

/// Siteyi sistemden kaldırır; `delete_files` verilirse dosyalarını da siler.
pub async fn deprovision(site: &Site, delete_files: bool) -> AppResult<()> {
    nginx::remove(&site.domain).await?;
    crate::hosting::rules::remove_password_files(site.id, 0);
    crate::hosting::rules::remove_cache_dir(site.id);
    node::remove(site.id).await?;
    node::forget_interpreter(site.id);
    php::remove_pool_except(site.id, None).await?;
    let ssl_dir = Path::new(nginx::SSL_DIR).join(&site.domain);
    if ssl_dir.exists() {
        std::fs::remove_dir_all(&ssl_dir)?;
    }
    if delete_files {
        if let Some(user) = sysuser::lookup(&site.owner) {
            sysuser::remove_site_dirs(&user, &site.domain)?;
        }
    }
    Ok(())
}

/// Sertifikayı kurar ve siteyi HTTPS ile yayınlar. nginx yeni sertifikayı
/// kabul etmezse (ör. anahtar uyuşmuyorsa) eski sertifika ve ayarlar geri gelir.
pub async fn activate_ssl(db: &Db, site: &Site, mode: &str, certificate: &str, private_key: &str) -> AppResult<Site> {
    let installed = ssl::install(&site.domain, certificate, private_key)?;
    let mut updated = site.clone();
    updated.ssl_mode = mode.to_string();
    let outcome = async {
        save(db, &updated)?;
        provision(&updated).await
    }
    .await;
    if let Err(err) = outcome {
        installed.rollback();
        save(db, site)?;
        if let Err(restore) = provision(site).await {
            tracing::error!(error = %restore, domain = %site.domain, "site eski sertifikasına döndürülemedi");
        }
        return Err(err);
    }
    Ok(updated)
}
