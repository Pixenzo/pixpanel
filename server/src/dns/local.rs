//! Sunucudaki yetkili DNS: PowerDNS (SQLite arka ucu), HTTP API'si üzerinden yönetilir.

use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use super::Record;
use crate::auth;
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, sysuser};
use crate::sys::exec;

pub const SERVICE: &str = "pdns.service";
const API_BASE: &str = "http://127.0.0.1:8053/api/v1/servers/localhost";
const KEY_FILE: &str = "/etc/pixpanel/pdns-api.key";
const CONF_FILE: &str = "/etc/powerdns/pdns.d/pixpanel.conf";
const DB_FILE: &str = "/var/lib/powerdns/pdns.sqlite3";
const SCHEMA_FILE: &str = "/usr/share/pdns-backend-sqlite3/schema/schema.sqlite3.sql";

pub fn is_installed() -> bool {
    exec::binary_exists("pdns_server") && Path::new(SCHEMA_FILE).exists()
}

/// PowerDNS'in veritabanını ve yapılandırmasını hazırlar, servisi yeniden başlatır.
/// Ubuntu'da systemd-resolved 127.0.0.53:53 adresini dinler; PowerDNS bütün adreslerde
/// 53. portu açamaz. Yerel dinleyici kapatılır, sunucunun kendi ad çözümlemesi
/// resolved'ın yukarı akış sunucularını doğrudan kullanacak biçimde sürer.
async fn release_dns_port() -> AppResult<()> {
    const RESOLVED: &str = "systemd-resolved.service";
    const DROP_IN: &str = "/etc/systemd/resolved.conf.d/pixpanel.conf";
    const CONTENT: &str = "# PixPanel: 53. port DNS sunucusuna bırakılır.
[Resolve]
DNSStubListener=no
";
    if !crate::sys::services::is_active(RESOLVED).await {
        return Ok(());
    }
    if std::fs::read_to_string(DROP_IN).ok().as_deref() == Some(CONTENT) {
        return Ok(());
    }
    std::fs::create_dir_all("/etc/systemd/resolved.conf.d")?;
    atomic_write(Path::new(DROP_IN), CONTENT.as_bytes(), 0o644)?;
    // resolv.conf yerel dinleyiciyi gösteriyorsa gerçek sunucu listesine çevrilir.
    let resolv = Path::new("/etc/resolv.conf");
    let links_to_stub = std::fs::read_link(resolv).map(|target| target.ends_with("stub-resolv.conf")).unwrap_or(false);
    // Düz dosya da yerel dinleyiciyi (127.0.0.53) gösterebilir; dinleyici kapanınca ad çözümlemesi kesilirdi.
    let names_stub = std::fs::read_to_string(resolv)
        .map(|content| content.lines().any(|line| line.split_whitespace().collect::<Vec<_>>() == ["nameserver", "127.0.0.53"]))
        .unwrap_or(false);
    if links_to_stub || names_stub {
        let temporary = Path::new("/etc/resolv.conf.pixpanel");
        let _ = std::fs::remove_file(temporary);
        std::os::unix::fs::symlink("/run/systemd/resolve/resolv.conf", temporary)?;
        std::fs::rename(temporary, resolv)?;
    }
    exec::run_ok("systemctl", &["restart", "--", RESOLVED]).await?;
    Ok(())
}

/// Bölge aktarımı ayarları: bu sunucudan bölge çeken ikinciller ve bu sunucunun
/// (ikincil olarak) bölge çektiği birinciller. İkisi de boşsa sunucu tek başına çalışır.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Transfer {
    pub secondaries: Vec<IpAddr>,
    pub primaries: Vec<IpAddr>,
}

fn join(addresses: &[IpAddr]) -> String {
    addresses.iter().map(IpAddr::to_string).collect::<Vec<_>>().join(",")
}

/// Aktarım ayarlarının yapılandırma satırları.
pub fn transfer_conf(transfer: &Transfer) -> String {
    let mut conf = String::new();
    if !transfer.secondaries.is_empty() {
        let list = join(&transfer.secondaries);
        // Bildirim yalnızca tanımlı ikincillere gider; bölgedeki NS adlarına (çoğu bu sunucunun kendisi) gitmez.
        conf.push_str(&format!("primary=yes\nallow-axfr-ips=127.0.0.1,::1,{list}\nalso-notify={list}\nonly-notify={list}\n"));
    }
    if !transfer.primaries.is_empty() {
        conf.push_str(&format!("secondary=yes\nallow-notify-from={}\n", join(&transfer.primaries)));
    }
    conf
}

pub async fn setup(transfer: &Transfer) -> AppResult<()> {
    let pdns = sysuser::lookup("pdns").ok_or_else(|| AppError::internal("pdns kullanıcısı bulunamadı"))?;
    if !Path::new(DB_FILE).exists() {
        let schema = std::fs::read_to_string(SCHEMA_FILE)?;
        let conn = rusqlite::Connection::open(DB_FILE)?;
        conn.execute_batch(&schema)?;
    }
    for path in [Path::new(DB_FILE), Path::new("/var/lib/powerdns")] {
        std::os::unix::fs::chown(path, Some(pdns.uid), Some(pdns.gid))?;
    }
    let key = match std::fs::read_to_string(KEY_FILE) {
        Ok(key) if key.trim().len() >= 32 => key.trim().to_string(),
        _ => {
            let key = auth::random_token(24);
            atomic_write(Path::new(KEY_FILE), key.as_bytes(), 0o600)?;
            key
        }
    };
    let conf = format!(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         launch=gsqlite3\n\
         gsqlite3-database={DB_FILE}\n\
         gsqlite3-pragma-foreign-keys=yes\n\
         gsqlite3-dnssec=on\n\
         api=yes\n\
         api-key={key}\n\
         webserver=yes\n\
         webserver-address=127.0.0.1\n\
         webserver-port=8053\n\
         webserver-allow-from=127.0.0.1\n\
         default-soa-content=ns1.@ hostmaster.@ 0 10800 3600 604800 3600\n\
         {}",
        transfer_conf(transfer)
    );
    // Dosya API anahtarını içerdiğinden yalnızca PowerDNS okuyabilir.
    atomic_write(Path::new(CONF_FILE), conf.as_bytes(), 0o640)?;
    std::os::unix::fs::chown(CONF_FILE, Some(0), Some(pdns.gid))?;
    release_dns_port().await?;
    exec::run_ok("systemctl", &["enable", "--quiet", "--", SERVICE]).await?;
    exec::run_ok("systemctl", &["restart", "--", SERVICE]).await?;
    // API'nin isteklere yanıt vermesi kısa bir süre alır.
    for _ in 0..30 {
        if request(reqwest::Method::GET, "/zones", None).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(AppError::internal("PowerDNS başlatıldı ama API'si yanıt vermiyor"))
}

async fn request(method: reqwest::Method, path: &str, body: Option<Value>) -> AppResult<Value> {
    let key = std::fs::read_to_string(KEY_FILE).map_err(|_| AppError::bad("DNS sunucusu kurulu değil; önce Bileşenler sayfasından kurun"))?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().map_err(|err| AppError::internal(err.to_string()))?;
    let mut request = client.request(method, format!("{API_BASE}{path}")).header("X-API-Key", key.trim());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|_| AppError::bad("DNS sunucusuna ulaşılamadı; servisin çalıştığını denetleyin"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(value);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::NotFound);
    }
    let detail = value["error"].as_str().unwrap_or(&text).to_string();
    Err(AppError::bad(format!("DNS sunucusu kaydı kabul etmedi: {detail}")))
}

fn dotted(name: &str) -> String {
    format!("{}.", name.trim_end_matches('.'))
}

fn undotted(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// TXT değeri 255 baytlık tırnaklı parçalara bölünür.
fn txt_to_wire(content: &str) -> String {
    // Ham metin önce parçalanır, sonra her parça ayrı kaçışlanır; böylece kaçış
    // dizisi iki parçaya bölünemez. 120 bayt, kaçışlandıktan sonra da 255'in altında kalır.
    let mut parts = Vec::new();
    let mut start = 0;
    while start < content.len() {
        let mut end = (start + 120).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(format!("\"{}\"", content[start..end].replace('\\', "\\\\").replace('"', "\\\"")));
        start = end;
    }
    if parts.is_empty() { "\"\"".to_string() } else { parts.join(" ") }
}

fn txt_from_wire(content: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    let mut chars = content.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => inside = !inside,
            '\\' if inside => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            _ if inside => out.push(c),
            _ => {}
        }
    }
    // Tırnaksız yazılmış eski kayıtlar olduğu gibi gösterilir.
    if out.is_empty() && !content.contains('"') { content.to_string() } else { out }
}

/// Kaydın PowerDNS'teki içerik biçimi.
fn to_wire(record: &Record) -> String {
    match record.kind.as_str() {
        "CNAME" | "NS" => dotted(&record.content),
        "MX" => format!("{} {}", record.priority.unwrap_or(10), dotted(&record.content)),
        "SRV" => {
            // İçerik: "ağırlık port hedef"
            let mut parts = record.content.split_whitespace();
            let (weight, port, target) = (parts.next().unwrap_or("0"), parts.next().unwrap_or("0"), parts.next().unwrap_or("."));
            format!("{} {weight} {port} {}", record.priority.unwrap_or(0), dotted(target))
        }
        "TXT" => txt_to_wire(&record.content),
        _ => record.content.clone(),
    }
}

fn from_wire(kind: &str, name: &str, ttl: u32, content: &str) -> Record {
    let (content, priority) = match kind {
        "CNAME" | "NS" => (undotted(content), None),
        "MX" => {
            let (priority, target) = content.split_once(' ').unwrap_or(("10", content));
            (undotted(target), priority.parse().ok())
        }
        "SRV" => {
            let parts: Vec<&str> = content.split_whitespace().collect();
            match parts.as_slice() {
                [priority, weight, port, target] => (format!("{weight} {port} {}", undotted(target)), priority.parse().ok()),
                _ => (content.to_string(), None),
            }
        }
        "TXT" => (txt_from_wire(content), None),
        _ => (content.to_string(), None),
    };
    Record { id: String::new(), kind: kind.to_string(), name: undotted(name), content, ttl, priority, proxied: None }
}

pub async fn create_zone(domain: &str, nameservers: &[String]) -> AppResult<()> {
    let body = json!({
        "name": dotted(domain),
        // Birincil türündeki bölge, değiştikçe ikincillere bildirim gönderir; ikincil yokken zararsızdır.
        "kind": "Master",
        "nameservers": nameservers.iter().map(|ns| dotted(ns)).collect::<Vec<_>>(),
        "soa_edit_api": "DEFAULT",
    });
    request(reqwest::Method::POST, "/zones", Some(body)).await.map(|_| ()).map_err(|err| match err {
        AppError::BadRequest(detail) if detail.contains("already exists") || detail.contains("Conflict") => {
            AppError::conflict("Bu alan adı DNS sunucusunda zaten tanımlı")
        }
        other => other,
    })
}

pub async fn delete_zone(domain: &str) -> AppResult<()> {
    match request(reqwest::Method::DELETE, &format!("/zones/{}", dotted(domain)), None).await {
        Ok(_) | Err(AppError::NotFound) => Ok(()),
        Err(err) => Err(err),
    }
}

/// DNS sunucusundaki bir bölgenin aktarım açısından durumu.
#[derive(Debug, Clone)]
pub struct ZoneState {
    pub domain: String,
    /// Bölge başka bir sunucudan mı çekiliyor?
    pub secondary: bool,
    pub masters: Vec<String>,
}

/// Sunucudaki bütün bölgeler (panelin açtıkları ve başka sunucudan çekilenler).
pub async fn zones() -> AppResult<Vec<ZoneState>> {
    let list = request(reqwest::Method::GET, "/zones", None).await?;
    Ok(list
        .as_array()
        .into_iter()
        .flatten()
        .map(|zone| ZoneState {
            domain: undotted(zone["name"].as_str().unwrap_or("")),
            secondary: matches!(zone["kind"].as_str().unwrap_or("").to_ascii_lowercase().as_str(), "slave" | "secondary"),
            masters: zone["masters"].as_array().into_iter().flatten().filter_map(|master| master.as_str().map(str::to_string)).collect(),
        })
        .collect())
}

/// Panelin açtığı bölgeyi birincil türüne çevirir (eski kurulumlarda bölgeler bildirim göndermeyen türdeydi).
pub async fn make_primary(domain: &str) -> AppResult<()> {
    request(reqwest::Method::PUT, &format!("/zones/{}", dotted(domain)), Some(json!({ "kind": "Master" }))).await.map(|_| ())
}

/// Bölgenin ikincillerine değişiklik bildirimi gönderir.
pub async fn notify(domain: &str) -> AppResult<()> {
    request(reqwest::Method::PUT, &format!("/zones/{}/notify", dotted(domain)), None).await.map(|_| ())
}

/// Başka sunucudan çekilecek bölge açar ve ilk aktarımı başlatır.
pub async fn create_secondary_zone(domain: &str, primary: IpAddr) -> AppResult<()> {
    let body = json!({ "name": dotted(domain), "kind": "Slave", "masters": [primary.to_string()] });
    request(reqwest::Method::POST, "/zones", Some(body)).await?;
    // İlk aktarım kendi sırasını beklemeden başlar; başaramazsa sunucu sonra yeniden dener.
    let _ = request(reqwest::Method::PUT, &format!("/zones/{}/axfr-retrieve", dotted(domain)), None).await;
    Ok(())
}

/// Bölgedeki bütün kayıtlar (SOA hariç; onu sunucu yönetir).
pub async fn list(domain: &str) -> AppResult<Vec<Record>> {
    let zone = request(reqwest::Method::GET, &format!("/zones/{}", dotted(domain)), None).await?;
    let mut records = Vec::new();
    for rrset in zone["rrsets"].as_array().into_iter().flatten() {
        let (kind, name) = (rrset["type"].as_str().unwrap_or_default(), rrset["name"].as_str().unwrap_or_default());
        if kind == "SOA" {
            continue;
        }
        let ttl = rrset["ttl"].as_u64().unwrap_or(3600) as u32;
        for item in rrset["records"].as_array().into_iter().flatten() {
            records.push(from_wire(kind, name, ttl, item["content"].as_str().unwrap_or_default()));
        }
    }
    Ok(records)
}

/// Bir kayıt kümesinin API gövdesi; küme boşsa silme isteği olur.
fn rrset_json(name: &str, kind: &str, records: &[&Record]) -> Value {
    if records.is_empty() {
        json!({ "name": dotted(name), "type": kind, "changetype": "DELETE" })
    } else {
        json!({
            "name": dotted(name),
            "type": kind,
            "ttl": records[0].ttl,
            "changetype": "REPLACE",
            "records": records.iter().map(|record| json!({ "content": to_wire(record), "disabled": false })).collect::<Vec<_>>(),
        })
    }
}

/// Bir ad ve türe ait kayıt kümesini verilen kayıtlarla değiştirir; boşsa kümeyi siler.
async fn write_set(domain: &str, name: &str, kind: &str, records: &[&Record]) -> AppResult<()> {
    request(reqwest::Method::PATCH, &format!("/zones/{}", dotted(domain)), Some(json!({ "rrsets": [rrset_json(name, kind, records)] }))).await.map(|_| ())
}

/// Birçok kayıt kümesini tek istekte (tek işlemde) yazar; içe aktarma için. Her öğe: (ad, tür, kümenin son hâli).
pub async fn write_sets(domain: &str, sets: &[(String, String, Vec<Record>)]) -> AppResult<()> {
    let rrsets: Vec<Value> = sets.iter().map(|(name, kind, records)| rrset_json(name, kind, &records.iter().collect::<Vec<_>>())).collect();
    request(reqwest::Method::PATCH, &format!("/zones/{}", dotted(domain)), Some(json!({ "rrsets": rrsets }))).await.map(|_| ())
}

// ---- DNSSEC ----

/// Alan adı kayıt firmasına girilecek DS kaydı.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DsRecord {
    pub key_tag: u16,
    pub algorithm: u8,
    pub digest_type: u8,
    pub digest: String,
}

/// Bölgenin imzalama anahtarı (KSK ya da CSK; ZSK için DS gerekmez ve gösterilmez).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DnssecKey {
    pub id: u64,
    pub keytype: String,
    pub algorithm: String,
    /// `bayrak protokol algoritma açık-anahtar` (DNSKEY kaydının veri kısmı).
    pub dnskey: String,
    pub ds: Vec<DsRecord>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Dnssec {
    pub enabled: bool,
    pub keys: Vec<DnssecKey>,
}

/// PowerDNS'in `ds` dizisindeki bir satırı ayrıştırır: `anahtar-etiketi algoritma özet-türü özet`.
pub fn parse_ds(line: &str) -> Option<DsRecord> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let [tag, algorithm, digest_type, digest] = parts.as_slice() else { return None };
    if digest.is_empty() || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(DsRecord { key_tag: tag.parse().ok()?, algorithm: algorithm.parse().ok()?, digest_type: digest_type.parse().ok()?, digest: digest.to_ascii_uppercase() })
}

/// DNSKEY satırı yalnızca base64 ve sayılardan oluşmalı; arayüze ham metin olarak gider.
fn clean_dnskey(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let [flags, protocol, algorithm, key] = parts.as_slice() else { return None };
    let numbers = [flags, protocol, algorithm].iter().all(|part| part.bytes().all(|b| b.is_ascii_digit()));
    let base64 = !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
    (numbers && base64).then(|| format!("{flags} {protocol} {algorithm} {key}"))
}

/// Bölgedeki imzalama anahtarları. DNSSEC, etkin bir KSK/CSK varsa açık sayılır.
pub async fn dnssec_status(domain: &str) -> AppResult<Dnssec> {
    let list = request(reqwest::Method::GET, &format!("/zones/{}/cryptokeys", dotted(domain)), None).await?;
    let mut keys = Vec::new();
    let mut enabled = false;
    for item in list.as_array().into_iter().flatten() {
        let keytype = item["keytype"].as_str().unwrap_or_default().to_ascii_lowercase();
        if !matches!(keytype.as_str(), "ksk" | "csk") || !item["active"].as_bool().unwrap_or(false) {
            continue;
        }
        enabled = true;
        let mut ds: Vec<DsRecord> = item["ds"].as_array().into_iter().flatten().filter_map(|line| line.as_str().and_then(parse_ds)).collect();
        // SHA-1 özeti artık önerilmez; SHA-256 (2) ve SHA-384 (4) kalır. Hiçbiri yoksa eldeki gösterilir.
        if ds.iter().any(|record| record.digest_type != 1) {
            ds.retain(|record| record.digest_type != 1);
        }
        ds.sort_by_key(|record| record.digest_type);
        keys.push(DnssecKey {
            id: item["id"].as_u64().unwrap_or(0),
            keytype,
            algorithm: item["algorithm"].as_str().unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).collect(),
            dnskey: item["dnskey"].as_str().and_then(clean_dnskey).unwrap_or_default(),
            ds,
        });
    }
    Ok(Dnssec { enabled, keys })
}

/// Kayıt kümelerini aynen yeniden yazarak SOA seri numarasını artırır (`soa_edit_api`); ikincillerin
/// değişikliği (anahtar, imza) alması için seri numarası değişmelidir.
pub async fn bump_serial(domain: &str) -> AppResult<()> {
    let ns: Vec<Record> = list(domain).await?.into_iter().filter(|record| record.kind == "NS" && record.name == domain).collect();
    if ns.is_empty() {
        return Ok(());
    }
    write_set(domain, domain, "NS", &ns.iter().collect::<Vec<_>>()).await
}

/// DNSSEC'i açar: ECDSA P-256 (algoritma 13) tek imzalama anahtarı (CSK) üretilir, bölge her API
/// değişikliğinde düzeltilecek (`api_rectify`) biçimde işaretlenir ve bir kez düzeltilir.
/// NSEC3 kullanılmaz (düz NSEC).
pub async fn dnssec_enable(domain: &str) -> AppResult<()> {
    if dnssec_status(domain).await?.enabled {
        return Err(AppError::conflict("DNSSEC bu bölgede zaten açık"));
    }
    let zone_path = format!("/zones/{}", dotted(domain));
    let key = json!({ "keytype": "csk", "active": true, "published": true, "algorithm": "ecdsa256", "bits": 256 });
    request(reqwest::Method::POST, &format!("{zone_path}/cryptokeys"), Some(key)).await?;
    let finish = async {
        request(reqwest::Method::PUT, &zone_path, Some(json!({ "api_rectify": true }))).await?;
        request(reqwest::Method::PUT, &format!("{zone_path}/rectify"), None).await?;
        bump_serial(domain).await
    };
    if let Err(err) = finish.await {
        // Yarım kalmış imzalama (anahtar var, düzeltme yok) yanlış yanıtlara yol açar; anahtar geri alınır.
        let _ = remove_keys(domain).await;
        return Err(err);
    }
    Ok(())
}

async fn remove_keys(domain: &str) -> AppResult<()> {
    let zone_path = format!("/zones/{}", dotted(domain));
    let list = request(reqwest::Method::GET, &format!("{zone_path}/cryptokeys"), None).await?;
    for id in list.as_array().into_iter().flatten().filter_map(|item| item["id"].as_u64()) {
        request(reqwest::Method::DELETE, &format!("{zone_path}/cryptokeys/{id}"), None).await?;
    }
    Ok(())
}

/// DNSSEC'i kapatır: bölgenin bütün anahtarları silinir. Alan adı kayıt firmasında DS kaydı hâlâ
/// duruyorsa doğrulayan çözücüler alan adını çözemez; bu yüzden arayüz önce DS'nin kaldırılmasını ister.
pub async fn dnssec_disable(domain: &str) -> AppResult<()> {
    remove_keys(domain).await?;
    bump_serial(domain).await
}

fn same_set(a: &Record, b: &Record) -> bool {
    a.kind == b.kind && a.name == b.name
}

fn same_record(a: &Record, b: &Record) -> bool {
    same_set(a, b) && a.content == b.content && a.priority == b.priority
}

pub async fn create(domain: &str, record: &Record) -> AppResult<()> {
    let existing = list(domain).await?;
    if existing.iter().any(|other| same_record(other, record)) {
        return Err(AppError::conflict("Bu kayıt zaten var"));
    }
    let mut set: Vec<&Record> = existing.iter().filter(|other| same_set(other, record)).collect();
    set.push(record);
    write_set(domain, &record.name, &record.kind, &set).await
}

pub async fn delete(domain: &str, record: &Record) -> AppResult<()> {
    let existing = list(domain).await?;
    if !existing.iter().any(|other| same_record(other, record)) {
        return Err(AppError::NotFound);
    }
    let set: Vec<&Record> = existing.iter().filter(|other| same_set(other, record) && !same_record(other, record)).collect();
    write_set(domain, &record.name, &record.kind, &set).await
}

pub async fn update(domain: &str, old: &Record, new: &Record) -> AppResult<()> {
    let existing = list(domain).await?;
    if !existing.iter().any(|other| same_record(other, old)) {
        return Err(AppError::NotFound);
    }
    if same_set(old, new) {
        let mut set: Vec<&Record> = existing.iter().filter(|other| same_set(other, old) && !same_record(other, old)).collect();
        set.push(new);
        return write_set(domain, &new.name, &new.kind, &set).await;
    }
    // Ad ya da tür değiştiyse önce yeni kayıt yazılır; başarısız olursa eskisi yerinde kalır.
    let mut target: Vec<&Record> = existing.iter().filter(|other| same_set(other, new)).collect();
    target.push(new);
    write_set(domain, &new.name, &new.kind, &target).await?;
    let remaining: Vec<&Record> = existing.iter().filter(|other| same_set(other, old) && !same_record(other, old)).collect();
    write_set(domain, &old.name, &old.kind, &remaining).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_roundtrip_and_chunking() {
        let long = "v=DKIM1; k=rsa; p=".to_string() + &"A".repeat(400);
        let wire = txt_to_wire(&long);
        assert!(wire.split("\" \"").count() >= 2, "{wire}");
        assert!(wire.split("\" \"").all(|part| part.trim_matches('"').len() <= 255));
        assert_eq!(txt_from_wire(&wire), long);
        let tricky = r#"a "quoted" \ value"#;
        assert_eq!(txt_from_wire(&txt_to_wire(tricky)), tricky);
    }

    #[test]
    fn ds_lines_are_parsed_strictly() {
        let ds = parse_ds("12345 13 2 abcdef0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789").unwrap();
        assert_eq!((ds.key_tag, ds.algorithm, ds.digest_type), (12345, 13, 2));
        assert!(ds.digest.starts_with("ABCDEF01"));
        for bad in ["", "12345 13 2", "x 13 2 ab", "12345 13 2 zz", "12345 13 2 ab cd", "70000 13 2 ab"] {
            assert!(parse_ds(bad).is_none(), "{bad}");
        }
        assert_eq!(clean_dnskey("257 3 13 mdsswUyr3DPW132mOi8V9xESWE8jTo0dxCjjnopKl+GqJxpVXckHAeF+KkxLbxILfDLUT0rAK9iUzy1L53eKGQ=="), Some("257 3 13 mdsswUyr3DPW132mOi8V9xESWE8jTo0dxCjjnopKl+GqJxpVXckHAeF+KkxLbxILfDLUT0rAK9iUzy1L53eKGQ==".to_string()));
        assert!(clean_dnskey("257 3 13 a;b").is_none());
        assert!(clean_dnskey("257 3 13").is_none());
    }

    #[test]
    fn mx_and_srv_conversion() {
        let mx = Record { id: String::new(), kind: "MX".into(), name: "ornek.com".into(), content: "posta.ornek.com".into(), ttl: 3600, priority: Some(10), proxied: None };
        assert_eq!(to_wire(&mx), "10 posta.ornek.com.");
        let back = from_wire("MX", "ornek.com.", 3600, "10 posta.ornek.com.");
        assert_eq!((back.content.as_str(), back.priority), ("posta.ornek.com", Some(10)));
        let srv = from_wire("SRV", "_sip._tcp.ornek.com.", 300, "5 10 5060 sip.ornek.com.");
        assert_eq!((srv.content.as_str(), srv.priority), ("10 5060 sip.ornek.com", Some(5)));
        assert_eq!(to_wire(&srv), "5 10 5060 sip.ornek.com.");
    }
}
