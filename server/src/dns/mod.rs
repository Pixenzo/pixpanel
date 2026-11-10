//! DNS yönetimi: bölgeler ve kayıtlar.
//!
//! Bir bölge iki yerden birinde durur: bu sunucudaki PowerDNS'te (`local`) ya da
//! Cloudflare'de (`cloudflare`). Kayıtların aslı sağlayıcıdadır; panel onları
//! kendi veritabanında tutmaz, sağlayıcının API'siyle okur ve yazar. Site, posta
//! ve sertifika işlemleri gereken kayıtları bu modül üzerinden kendiliğinden ekler.

pub mod cloudflare;
pub mod local;

use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthUser, Role};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, domain as domain_rules};
use crate::state::AppState;
use crate::sys::exec;

pub const KINDS: &[&str] = &["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];
const SERVER_IP_KEY: &str = "dns.server_ip";
const NAMESERVERS_KEY: &str = "dns.nameservers";
const REAL_IP_KEY: &str = "cloudflare.real_ip";
const REAL_IP_CONF: &str = "/etc/nginx/conf.d/pixpanel-cloudflare.conf";

#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    pub id: i64,
    pub user_id: i64,
    pub owner: String,
    pub domain: String,
    /// `local` (bu sunucu) ya da `cloudflare`.
    pub provider: String,
    #[serde(skip)]
    pub cf_zone_id: String,
    pub created_at: i64,
}

/// Sağlayıcıdan bağımsız kayıt biçimi.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    /// Sağlayıcının kayıt kimliği (Cloudflare); yerel bölgelerde boştur.
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Tam ad, sonda nokta olmadan (`www.ornek.com`).
    pub name: String,
    /// MX için hedef sunucu, TXT için tırnaksız metin, SRV için `ağırlık port hedef`.
    pub content: String,
    pub ttl: u32,
    #[serde(default)]
    pub priority: Option<u32>,
    /// Yalnızca Cloudflare: kayıt Cloudflare üzerinden vekilleniyor mu?
    #[serde(default)]
    pub proxied: Option<bool>,
}

impl Record {
    pub fn new(kind: &str, name: &str, content: &str) -> Self {
        Self { id: String::new(), kind: kind.into(), name: name.into(), content: content.into(), ttl: 3600, priority: None, proxied: None }
    }
}

// ---- Doğrulama ----

/// Sunucu adı: etiketler harf, rakam, tire ve alt çizgiden oluşur (`_dmarc`, `_sip._tcp`).
fn valid_hostname(name: &str, allow_wildcard: bool) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').enumerate().all(|(index, label)| {
            (allow_wildcard && index == 0 && label == "*")
                || (!label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
        })
}

/// Kullanıcının yazdığı adı bölge içindeki tam ada çevirir: `@` → bölgenin kendisi,
/// `www` → `www.ornek.com`.
fn full_name(zone: &str, input: &str) -> AppResult<String> {
    let name = input.trim().trim_end_matches('.').to_ascii_lowercase();
    let full = if name.is_empty() || name == "@" {
        zone.to_string()
    } else if name == zone || name.ends_with(&format!(".{zone}")) {
        name
    } else {
        format!("{name}.{zone}")
    };
    if valid_hostname(&full, true) { Ok(full) } else { Err(AppError::bad(format!("Geçersiz kayıt adı: {input}"))) }
}

fn target_host(value: &str) -> AppResult<String> {
    let host = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if valid_hostname(&host, false) && host.contains('.') { Ok(host) } else { Err(AppError::bad(format!("Geçersiz hedef sunucu adı: {value}"))) }
}

/// Kullanıcıdan gelen kaydı doğrular ve sağlayıcıya yazılacak biçime getirir.
pub fn normalize(zone: &str, input: &Record) -> AppResult<Record> {
    let kind = input.kind.trim().to_ascii_uppercase();
    if !KINDS.contains(&kind.as_str()) {
        return Err(AppError::bad("Desteklenmeyen kayıt türü"));
    }
    let name = full_name(zone, &input.name)?;
    let raw = input.content.trim();
    let content = match kind.as_str() {
        "A" => raw.parse::<Ipv4Addr>().map(|ip| ip.to_string()).map_err(|_| AppError::bad("A kaydı bir IPv4 adresi olmalı (örnek: 203.0.113.10)"))?,
        "AAAA" => raw.parse::<Ipv6Addr>().map(|ip| ip.to_string()).map_err(|_| AppError::bad("AAAA kaydı bir IPv6 adresi olmalı"))?,
        "CNAME" | "NS" | "MX" => target_host(raw)?,
        "TXT" => {
            if raw.is_empty() || raw.len() > 2048 || raw.chars().any(char::is_control) {
                return Err(AppError::bad("TXT değeri boş olamaz, tek satır olmalı ve 2048 karakteri aşamaz"));
            }
            // Kullanıcı değeri tırnak içinde yapıştırdıysa tırnaklar atılır.
            raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')).filter(|inner| !inner.contains('"')).unwrap_or(raw).to_string()
        }
        "SRV" => {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            match parts.as_slice() {
                [weight, port, target] if weight.parse::<u16>().is_ok() && port.parse::<u16>().is_ok() => format!("{weight} {port} {}", target_host(target)?),
                _ => return Err(AppError::bad("SRV değeri “ağırlık port hedef” biçiminde olmalı (örnek: 10 5060 sip.ornek.com)")),
            }
        }
        _ => {
            // CAA: `0 issue "letsencrypt.org"`
            let mut parts = raw.splitn(3, ' ');
            let (flags, tag, value) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            let inner = value.trim_matches('"');
            let valid = flags.parse::<u8>().is_ok()
                && matches!(tag, "issue" | "issuewild" | "iodef")
                && !inner.is_empty()
                && inner.len() <= 255
                && inner.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\');
            if !valid {
                return Err(AppError::bad("CAA değeri `0 issue \"letsencrypt.org\"` biçiminde olmalı"));
            }
            format!("{flags} {tag} \"{inner}\"")
        }
    };
    if kind == "CNAME" && name == zone {
        return Err(AppError::bad("Bölgenin kök adına CNAME kaydı eklenemez; A kaydı kullanın"));
    }
    // Cloudflare'de 1 "otomatik" demektir.
    if input.ttl != 1 && !(60..=86400).contains(&input.ttl) {
        return Err(AppError::bad("Süre (TTL) 60 ile 86400 saniye arasında olmalı"));
    }
    let priority = match kind.as_str() {
        "MX" | "SRV" => Some(input.priority.unwrap_or(10).min(65535)),
        _ => None,
    };
    Ok(Record { id: input.id.clone(), kind, name, content, ttl: input.ttl, priority, proxied: input.proxied })
}

// ---- Bölge kayıtları (panel veritabanı) ----

const ZONE_COLUMNS: &str = "z.id, z.user_id, u.username, z.domain, z.provider, z.cf_zone_id, z.created_at";

fn read_zone(row: &rusqlite::Row<'_>) -> rusqlite::Result<Zone> {
    Ok(Zone {
        id: row.get(0)?,
        user_id: row.get(1)?,
        owner: row.get(2)?,
        domain: row.get(3)?,
        provider: row.get(4)?,
        cf_zone_id: row.get(5)?,
        created_at: row.get(6)?,
    })
}

pub fn list_zones(db: &Db, user: &AuthUser) -> AppResult<Vec<Zone>> {
    let owner = user.owner_id();
    let (all, include_children) = (user.is_admin(), user.role == Role::Reseller);
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {ZONE_COLUMNS} FROM dns_zones z JOIN users u ON u.id = z.user_id
             WHERE ?1 OR z.user_id = ?2 OR (?3 AND u.parent_id = ?2) ORDER BY z.domain"
        ))?;
        stmt.query_map(params![all, owner, include_children], read_zone)?.collect()
    })
}

pub fn get_zone(db: &Db, user: &AuthUser, id: i64) -> AppResult<Zone> {
    let found = db.call(|conn| {
        conn.query_row(
            &format!("SELECT {ZONE_COLUMNS}, u.parent_id FROM dns_zones z JOIN users u ON u.id = z.user_id WHERE z.id = ?1"),
            params![id],
            |row| Ok((read_zone(row)?, row.get::<_, Option<i64>>(7)?)),
        )
        .optional()
    })?;
    let (zone, owner_parent) = found.ok_or(AppError::NotFound)?;
    let allowed = user.is_admin() || zone.user_id == user.owner_id() || (user.role == Role::Reseller && owner_parent == Some(user.id));
    if allowed { Ok(zone) } else { Err(AppError::NotFound) }
}

/// Verilen adı kapsayan, aynı hesaba ait bölge (en uzun eşleşme).
pub fn zone_for(db: &Db, owner_id: i64, name: &str) -> AppResult<Option<Zone>> {
    let name = name.trim_start_matches("*.");
    let zones: Vec<Zone> = db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {ZONE_COLUMNS} FROM dns_zones z JOIN users u ON u.id = z.user_id WHERE z.user_id = ?1"))?;
        stmt.query_map(params![owner_id], read_zone)?.collect()
    })?;
    Ok(zones
        .into_iter()
        .filter(|zone| name == zone.domain || name.ends_with(&format!(".{}", zone.domain)))
        .max_by_key(|zone| zone.domain.len()))
}

pub fn cloudflare_token(db: &Db, user_id: i64) -> AppResult<Option<String>> {
    db.call(|conn| conn.query_row("SELECT token FROM cloudflare_tokens WHERE user_id = ?1", params![user_id], |row| row.get(0)).optional())
}

pub fn cloudflare_client(state: &AppState, user_id: i64) -> AppResult<cloudflare::Client> {
    let token = cloudflare_token(&state.db, user_id)?.ok_or_else(|| AppError::bad("Bu hesap için Cloudflare API anahtarı tanımlı değil"))?;
    cloudflare::Client::new(&state.cfg.cloudflare_api, &token)
}

// ---- Sağlayıcıya göre işlemler ----

pub async fn list_records(state: &AppState, zone: &Zone) -> AppResult<Vec<Record>> {
    let mut records = match zone.provider.as_str() {
        "cloudflare" => cloudflare_client(state, zone.user_id)?.list(&zone.cf_zone_id).await?,
        _ => local::list(&zone.domain).await?,
    };
    // Kök ad en üstte, sonra ada ve türe göre.
    records.sort_by(|a, b| (a.name != zone.domain, &a.name, &a.kind, &a.content).cmp(&(b.name != zone.domain, &b.name, &b.kind, &b.content)));
    Ok(records)
}

pub async fn create_record(state: &AppState, zone: &Zone, record: &Record) -> AppResult<()> {
    match zone.provider.as_str() {
        "cloudflare" => cloudflare_client(state, zone.user_id)?.create(&zone.cf_zone_id, record).await,
        _ => local::create(&zone.domain, record).await,
    }
}

pub async fn update_record(state: &AppState, zone: &Zone, old: &Record, new: &Record) -> AppResult<()> {
    match zone.provider.as_str() {
        "cloudflare" => cloudflare_client(state, zone.user_id)?.update(&zone.cf_zone_id, &old.id, new).await,
        _ => local::update(&zone.domain, old, new).await,
    }
}

pub async fn delete_record(state: &AppState, zone: &Zone, record: &Record) -> AppResult<()> {
    match zone.provider.as_str() {
        "cloudflare" => cloudflare_client(state, zone.user_id)?.delete(&zone.cf_zone_id, &record.id).await,
        _ => local::delete(&zone.domain, record).await,
    }
}

// ---- Genel ayarlar ----

/// Sitelerin A kayıtlarında kullanılacak adres: ayarlanmışsa o, yoksa sunucunun dışa çıkış adresi.
pub fn server_ip(db: &Db) -> String {
    if let Some(ip) = db.setting(SERVER_IP_KEY).ok().flatten().filter(|ip| ip.parse::<Ipv4Addr>().is_ok()) {
        return ip;
    }
    // Bağlantı kurulmaz; yalnızca çekirdeğin seçeceği kaynak adres öğrenilir.
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| socket.connect("1.1.1.1:53").and_then(|_| socket.local_addr()))
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

pub fn set_server_ip(db: &Db, value: &str) -> AppResult<()> {
    let ip = value.trim();
    if !ip.is_empty() && ip.parse::<Ipv4Addr>().is_err() {
        return Err(AppError::bad("Sunucu adresi bir IPv4 adresi olmalı"));
    }
    db.set_setting(SERVER_IP_KEY, ip)
}

/// Yönetici ortak ad sunucuları tanımladıysa onlar, yoksa bölgeye özel `ns1`/`ns2`.
pub fn nameservers(db: &Db, domain: &str) -> (Vec<String>, bool) {
    let configured: Vec<String> = db.setting(NAMESERVERS_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    if configured.len() >= 2 { (configured, false) } else { (vec![format!("ns1.{domain}"), format!("ns2.{domain}")], true) }
}

pub fn configured_nameservers(db: &Db) -> Vec<String> {
    db.setting(NAMESERVERS_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

pub fn set_nameservers(db: &Db, values: &[String]) -> AppResult<Vec<String>> {
    let list: Vec<String> = values.iter().filter(|value| !value.trim().is_empty()).map(|value| target_host(value)).collect::<AppResult<_>>()?;
    if list.len() == 1 || list.len() > 6 {
        return Err(AppError::bad("En az iki, en çok altı ad sunucusu girin (ya da hepsini boş bırakın)"));
    }
    db.set_setting(NAMESERVERS_KEY, &serde_json::to_string(&list).map_err(|err| AppError::internal(err.to_string()))?)?;
    Ok(list)
}

pub fn real_ip_enabled(db: &Db) -> bool {
    db.setting(REAL_IP_KEY).ok().flatten().as_deref() == Some("1")
}

/// Cloudflare arkasındaki sitelerde ziyaretçinin gerçek adresinin günlüklere ve
/// uygulamalara ulaşması için nginx'e Cloudflare'in adres aralıklarını tanıtır.
/// Yalnızca bu aralıklardan gelen bağlantıların bildirdiği adrese güvenilir.
pub async fn set_real_ip(state: &AppState, enabled: bool) -> AppResult<()> {
    let path = Path::new(REAL_IP_CONF);
    let previous = std::fs::read(path).ok();
    if enabled {
        let ranges = cloudflare::fetch_ranges(&state.cfg.cloudflare_api).await;
        let mut conf = String::from("# PixPanel tarafından üretildi. Cloudflare üzerinden gelen isteklerde ziyaretçinin gerçek adresi kullanılır.\n");
        for range in ranges.iter().filter(|range| cloudflare::is_cidr(range)) {
            conf.push_str(&format!("set_real_ip_from {range};\n"));
        }
        conf.push_str("real_ip_header CF-Connecting-IP;\n");
        atomic_write(path, conf.as_bytes(), 0o644)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    let test = exec::run("nginx", &["-t"]).await?;
    if !test.success() {
        match previous {
            Some(content) => atomic_write(path, &content, 0o644)?,
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
        return Err(AppError::internal(format!("nginx yapılandırması geçersiz: {}", test.stderr.trim())));
    }
    exec::run_ok("systemctl", &["reload", "nginx.service"]).await?;
    state.db.set_setting(REAL_IP_KEY, if enabled { "1" } else { "0" })
}

// ---- Kendiliğinden eklenen kayıtlar ----

/// Kaydın eklenmesi için hangi koşulun arandığı.
enum Policy {
    /// Aynı adda A, AAAA ya da CNAME yoksa eklenir; varsa kullanıcının kaydına dokunulmaz.
    IfNoAddress,
    /// Aynı ad ve türde, verilen önekle başlayan kayıt yoksa eklenir.
    IfNoneStartingWith(&'static str),
    /// Aynı ad ve türde kayıt yoksa eklenir.
    IfNoneOfKind,
    /// Aynı ad ve türdeki kayıtlar bu kayıtla değiştirilir (yalnızca panelin yönettiği adlar).
    Replace,
}

/// Kaydı ilkesine göre ekler; yapılan işi açıklayan kısa bir satır döner.
async fn ensure(state: &AppState, zone: &Zone, existing: &[Record], record: Record, policy: Policy) -> String {
    let same_name: Vec<&Record> = existing.iter().filter(|other| other.name == record.name).collect();
    let same_kind: Vec<&Record> = same_name.iter().copied().filter(|other| other.kind == record.kind).collect();
    let label = format!("{} {}", record.kind, record.name);
    if same_kind.iter().any(|other| other.content == record.content) {
        return format!("{label}: zaten doğru");
    }
    let blocked = match policy {
        Policy::IfNoAddress => same_name.iter().any(|other| matches!(other.kind.as_str(), "A" | "AAAA" | "CNAME")),
        Policy::IfNoneStartingWith(prefix) => same_kind.iter().any(|other| other.content.to_ascii_lowercase().starts_with(prefix)),
        Policy::IfNoneOfKind => !same_kind.is_empty(),
        Policy::Replace => false,
    };
    if blocked {
        return format!("{label}: farklı bir kayıt var, dokunulmadı");
    }
    if matches!(policy, Policy::Replace) {
        for old in &same_kind {
            if let Err(err) = delete_record(state, zone, old).await {
                return format!("{label}: eski kayıt silinemedi ({err})");
            }
        }
    }
    match create_record(state, zone, &record).await {
        Ok(()) => format!("{label}: eklendi"),
        Err(err) => format!("{label}: eklenemedi ({err})"),
    }
}

/// Bir sitenin (ve takma adlarının) A kayıtlarını ekler.
pub async fn add_site_records(state: &AppState, owner_id: i64, names: &[String]) -> Vec<String> {
    let ip = server_ip(&state.db);
    let mut report = Vec::new();
    for name in names {
        let Ok(Some(zone)) = zone_for(&state.db, owner_id, name) else { continue };
        let Ok(existing) = list_records(state, &zone).await else { continue };
        report.push(ensure(state, &zone, &existing, Record::new("A", name, &ip), Policy::IfNoAddress).await);
    }
    report
}

/// Posta için gereken MX, SPF, DKIM ve DMARC kayıtlarını ekler.
pub async fn add_mail_records(state: &AppState, owner_id: i64, domain: &str, mail_host: &str, dkim_selector: &str, dkim_record: &str) -> Vec<String> {
    let Ok(Some(zone)) = zone_for(&state.db, owner_id, domain) else { return Vec::new() };
    let Ok(existing) = list_records(state, &zone).await else { return vec!["Bölgenin kayıtları okunamadı".into()] };
    let mut mx = Record::new("MX", domain, mail_host);
    mx.priority = Some(10);
    let mut report = vec![
        ensure(state, &zone, &existing, mx, Policy::IfNoneOfKind).await,
        ensure(state, &zone, &existing, Record::new("TXT", domain, "v=spf1 mx ~all"), Policy::IfNoneStartingWith("v=spf1")).await,
        ensure(state, &zone, &existing, Record::new("TXT", &format!("{dkim_selector}._domainkey.{domain}"), dkim_record), Policy::Replace).await,
        ensure(
            state,
            &zone,
            &existing,
            Record::new("TXT", &format!("_dmarc.{domain}"), &format!("v=DMARC1; p=quarantine; rua=mailto:postmaster@{domain}")),
            Policy::IfNoneStartingWith("v=dmarc1"),
        )
        .await,
    ];
    // Posta sunucusunun adı bu bölgenin içindeyse ve adres kaydı yoksa o da eklenir.
    if mail_host == zone.domain || mail_host.ends_with(&format!(".{}", zone.domain)) {
        report.push(ensure(state, &zone, &existing, Record::new("A", mail_host, &server_ip(&state.db)), Policy::IfNoAddress).await);
    }
    report
}

/// Yeni açılan yerel bölgeye başlangıç kayıtlarını yazar.
pub async fn seed_local_zone(state: &AppState, zone: &Zone, vanity_nameservers: bool) -> Vec<String> {
    let ip = server_ip(&state.db);
    let existing = list_records(state, zone).await.unwrap_or_default();
    let mut names = vec![zone.domain.clone(), format!("www.{}", zone.domain)];
    if vanity_nameservers {
        names.extend([format!("ns1.{}", zone.domain), format!("ns2.{}", zone.domain)]);
    }
    let mut report = Vec::new();
    for name in names {
        report.push(ensure(state, zone, &existing, Record::new("A", &name, &ip), Policy::IfNoAddress).await);
    }
    report
}

// ---- Sertifika doğrulaması (DNS-01) ----

/// Let's Encrypt'in DNS doğrulaması için `_acme-challenge` kayıtlarını yazan ve
/// iş bitince temizleyen yardımcı.
pub struct AcmeSolver {
    state: AppState,
    owner_id: i64,
    published: Vec<(Zone, Record)>,
}

impl AcmeSolver {
    pub fn new(state: AppState, owner_id: i64) -> Self {
        Self { state, owner_id, published: Vec::new() }
    }

    /// Alan adının doğrulama değerini DNS'e yazar. `name` joker ise (`*.ornek.com`) üst ad kullanılır.
    pub async fn publish(&mut self, name: &str, value: &str) -> AppResult<()> {
        let base = name.trim_start_matches("*.");
        let zone = zone_for(&self.state.db, self.owner_id, base)?
            .ok_or_else(|| AppError::bad(format!("{base} için panelde yönetilen bir DNS bölgesi yok; DNS doğrulaması yapılamaz")))?;
        let mut record = Record::new("TXT", &format!("_acme-challenge.{base}"), value);
        record.ttl = 60;
        create_record(&self.state, &zone, &record).await?;
        self.published.push((zone, record));
        Ok(())
    }

    /// Kayıtların ad sunucularına yayılması için bekler.
    pub async fn wait(&self) {
        let remote = self.published.iter().any(|(zone, _)| zone.provider == "cloudflare");
        tokio::time::sleep(Duration::from_secs(if remote { 20 } else { 2 })).await;
    }

    pub async fn cleanup(&mut self) {
        for (zone, record) in std::mem::take(&mut self.published) {
            // Cloudflare'de silmek için kaydın kimliği gerekir; listeden bulunur.
            let target = match zone.provider.as_str() {
                "cloudflare" => list_records(&self.state, &zone)
                    .await
                    .ok()
                    .and_then(|records| records.into_iter().find(|other| other.kind == "TXT" && other.name == record.name && other.content == record.content)),
                _ => Some(record),
            };
            if let Some(target) = target {
                let _ = delete_record(&self.state, &zone, &target).await;
            }
        }
    }
}

/// Alan adı için yönetici dışındaki hesapların bölge açıp açamayacağı: yalnızca
/// kendi sitelerinin alan adı (ya da onun üst adı) için.
pub fn owns_domain(db: &Db, owner_id: i64, domain: &str) -> AppResult<bool> {
    let names: Vec<String> = db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT domain FROM sites WHERE user_id = ?1
             UNION SELECT a.name FROM site_aliases a JOIN sites s ON s.id = a.site_id WHERE s.user_id = ?1",
        )?;
        stmt.query_map(params![owner_id], |row| row.get(0))?.collect()
    })?;
    Ok(names.iter().any(|name| name == domain || name.ends_with(&format!(".{domain}"))))
}

pub fn normalize_domain(value: &str) -> AppResult<String> {
    domain_rules::normalize(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: &str, name: &str, content: &str) -> Record {
        Record::new(kind, name, content)
    }

    #[test]
    fn names_are_resolved_inside_zone() {
        assert_eq!(normalize("ornek.com", &input("A", "@", "1.2.3.4")).unwrap().name, "ornek.com");
        assert_eq!(normalize("ornek.com", &input("A", "www", "1.2.3.4")).unwrap().name, "www.ornek.com");
        assert_eq!(normalize("ornek.com", &input("A", "*.alt", "1.2.3.4")).unwrap().name, "*.alt.ornek.com");
        assert_eq!(normalize("ornek.com", &input("TXT", "_dmarc.ornek.com.", "v=DMARC1")).unwrap().name, "_dmarc.ornek.com");
    }

    #[test]
    fn rejects_malformed_records() {
        for (kind, name, content) in [
            ("A", "www", "999.1.1.1"),
            ("A", "a b", "1.2.3.4"),
            ("A", "www;x", "1.2.3.4"),
            ("AAAA", "www", "1.2.3.4"),
            ("CNAME", "@", "hedef.ornek.net"),
            ("CNAME", "www", "hedef ornek"),
            ("MX", "@", "posta"),
            ("TXT", "@", "satir\nsonu"),
            ("SRV", "_sip._tcp", "10 hedef.ornek.com"),
            ("CAA", "@", "0 issue letsencrypt.org\" ; x"),
            ("PTR", "@", "x.ornek.com"),
        ] {
            assert!(normalize("ornek.com", &input(kind, name, content)).is_err(), "{kind} {name} {content}");
        }
    }
}
