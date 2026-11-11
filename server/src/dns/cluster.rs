//! DNS kümesi: bölgelerin ikincil ad sunucularına eşitlenmesi.
//!
//! İki katman vardır:
//!
//! 1. **Bölge aktarımı.** Bu sunucu birincildir; tanımlı ikincillere her değişiklikte bildirim
//!    (NOTIFY) gönderir ve onların bölgeyi çekmesine (AXFR) izin verir. Bu katman her DNS
//!    yazılımıyla ve ikincil DNS hizmetleriyle çalışır.
//! 2. **Bölge listesinin eşitlenmesi.** İkincil de bir PixPanel ise panel adresi ve bir API anahtarı
//!    verilir; birincil, bölge listesini ona iletir ve ikincil, bölgeleri kendiliğinden açıp kapatır.
//!    Bu katman olmadan ikincilde her bölgenin elle açılması gerekir.
//!
//! İkincil panelin sertifikası çoğu zaman kendinden imzalıdır; bu yüzden ilk bağlantıda görülen
//! sertifikanın parmak izi kaydedilir ve sonraki bağlantılarda yalnızca o sertifika (ya da
//! genel olarak geçerli bir sertifika) kabul edilir. API anahtarı doğrulanmamış bir karşı tarafa gönderilmez.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::local::{self, Transfer};
use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const SECONDARIES_KEY: &str = "dns.cluster.secondaries";
const STATUS_KEY: &str = "dns.cluster.status";
const PRIMARIES_KEY: &str = "dns.cluster.primaries";
const MAX_SECONDARIES: usize = 6;
/// Bir eşitleme isteğinde kabul edilen en çok bölge sayısı.
const MAX_ZONES: usize = 20_000;

/// Bu sunucudan bölge çeken bir ikincil ad sunucusu.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Secondary {
    /// İkincilin bölgeleri çekerken kullandığı adres (ve `ns2` kayıtlarında yayınlanan adres).
    pub address: String,
    /// İkincil bir PixPanel ise panelinin adresi; boşsa bölge listesi eşitlenmez.
    #[serde(default)]
    pub panel_url: String,
    /// İkincil panelde üretilmiş API anahtarı.
    #[serde(default)]
    pub token: String,
    /// İkincil panelin sertifikasının SHA-256 parmak izi (onaltılık).
    #[serde(default)]
    pub fingerprint: String,
}

/// Bir ikincile yapılan son eşitlemenin sonucu.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncStatus {
    pub at: i64,
    /// Boşsa eşitleme başarılıdır.
    pub error: String,
    pub zones: usize,
    /// İkincilin kendi bölgeleriyle çakıştığı için açamadığı alan adları.
    #[serde(default)]
    pub conflicts: Vec<String>,
}

pub fn secondaries(db: &Db) -> Vec<Secondary> {
    db.setting(SECONDARIES_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

pub fn statuses(db: &Db) -> BTreeMap<String, SyncStatus> {
    db.setting(STATUS_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

/// Bu sunucunun ikincil olarak bölge çektiği birinciller.
pub fn primaries(db: &Db) -> Vec<IpAddr> {
    let list: Vec<String> = db.setting(PRIMARIES_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    list.iter().filter_map(|text| text.parse().ok()).collect()
}

fn save_primaries(db: &Db, list: &[IpAddr]) -> AppResult<()> {
    let texts: Vec<String> = list.iter().map(IpAddr::to_string).collect();
    db.set_setting(PRIMARIES_KEY, &serde_json::to_string(&texts).unwrap_or_default())
}

/// DNS sunucusunun yapılandırmasına yazılacak aktarım ayarları.
pub fn transfer(db: &Db) -> Transfer {
    Transfer { secondaries: secondaries(db).iter().filter_map(|item| item.address.parse().ok()).collect(), primaries: primaries(db) }
}

/// Bölgeye özel `ns2` adının göstereceği adres: ilk ikincil (IPv4 ise), yoksa sunucunun kendisi.
pub fn second_nameserver_address(db: &Db) -> String {
    secondaries(db)
        .iter()
        .find_map(|item| item.address.parse::<std::net::Ipv4Addr>().ok())
        .map(|address| address.to_string())
        .unwrap_or_else(|| super::server_ip(db))
}

/// Panel adresini doğrular ve tek biçime indirger: yalnızca `https://ad[:port]`.
pub fn normalize_panel_url(value: &str) -> AppResult<String> {
    let text = value.trim().trim_end_matches('/');
    if text.is_empty() {
        return Ok(String::new());
    }
    let url = url::Url::parse(text).map_err(|_| AppError::bad("Panel adresi geçersiz; https://sunucu:8443 biçiminde girin"))?;
    if url.scheme() != "https" {
        return Err(AppError::bad("Panel adresi https ile başlamalı; API anahtarı şifresiz bağlantıdan gönderilmez"));
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
        return Err(AppError::bad("Panel adresi yalnızca sunucu adı ve porttan oluşmalı"));
    }
    let host = url.host_str().ok_or_else(|| AppError::bad("Panel adresinde sunucu adı yok"))?;
    Ok(match url.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

fn validate_address(value: &str) -> AppResult<IpAddr> {
    crate::addresses::normalize(value).map_err(|_| AppError::bad(format!("İkincil sunucu adresi geçerli bir IP adresi olmalı: {}", value.trim())))
}

// ---- Sertifika doğrulaması ----

/// Kaydedilmiş parmak iziyle eşleşen ya da genel olarak geçerli sertifikayı kabul eder.
/// `capture` kipinde her sertifikayı kabul edip parmak izini kaydeder; bu kip yalnızca
/// kimlik bilgisi taşımayan ilk tanışma isteğinde kullanılır.
#[derive(Debug)]
struct PinVerifier {
    pin: Option<[u8; 32]>,
    capture: bool,
    seen: Mutex<Option<[u8; 32]>>,
    public: Arc<WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
}

impl PinVerifier {
    fn new(pin: Option<[u8; 32]>, capture: bool) -> AppResult<Arc<Self>> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let public = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().map_err(|err| AppError::internal(err.to_string()))?;
        Ok(Arc::new(Self { pin, capture, seen: Mutex::new(None), public, provider }))
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        *self.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(digest);
        if self.capture || self.pin == Some(digest) {
            return Ok(ServerCertVerified::assertion());
        }
        self.public.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Yalnızca verilen parmak izini taşıyan sertifikayı (ya da genel olarak geçerli bir sertifikayı)
/// kabul eden istemci. Komut satırı aracı panelin kendi sertifikasını böyle sabitler.
pub fn pinned_client(fingerprint: [u8; 32], timeout: Duration) -> AppResult<reqwest::Client> {
    let verifier = PinVerifier::new(Some(fingerprint), false)?;
    let config = rustls::ClientConfig::builder_with_provider(verifier.provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|err| AppError::internal(err.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    reqwest::Client::builder().use_preconfigured_tls(config).redirect(reqwest::redirect::Policy::none()).timeout(timeout).build().map_err(|err| AppError::internal(err.to_string()))
}

fn client(verifier: Arc<PinVerifier>) -> AppResult<reqwest::Client> {
    let config = rustls::ClientConfig::builder_with_provider(verifier.provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|err| AppError::internal(err.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(config)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| AppError::internal(err.to_string()))
}

fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    hex::decode(text).ok()?.try_into().ok()
}

/// İkincil panele kimlik bilgisi göndermeden bağlanır ve sertifikasının parmak izini döner.
pub async fn probe_fingerprint(panel_url: &str) -> AppResult<String> {
    let verifier = PinVerifier::new(None, true)?;
    let http = client(verifier.clone())?;
    http.get(format!("{panel_url}/api/v1/auth/me"))
        .send()
        .await
        .map_err(|err| AppError::bad(format!("İkincil panele bağlanılamadı ({panel_url}): {}", root_cause(&err))))?;
    let seen = *verifier.seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    seen.map(hex::encode).ok_or_else(|| AppError::bad("İkincil panelin sertifikası okunamadı"))
}

fn root_cause(err: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = err;
    while let Some(next) = source.source() {
        source = next;
    }
    source.to_string()
}

// ---- Birincil taraf ----

/// Bu sunucudaki yerel bölgelerin alan adları.
fn local_zone_names(db: &Db) -> AppResult<Vec<String>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT domain FROM dns_zones WHERE provider = 'local' ORDER BY domain")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    })
}

/// Bölge listesini bir ikincil panele iletir.
async fn push(secondary: &Secondary, zones: &[String]) -> AppResult<SyncStatus> {
    let pin = parse_fingerprint(&secondary.fingerprint);
    let http = client(PinVerifier::new(pin, false)?)?;
    let response = http
        .put(format!("{}/api/v1/dns/replica", secondary.panel_url))
        .bearer_auth(&secondary.token)
        .json(&json!({ "zones": zones }))
        .send()
        .await
        .map_err(|err| {
            let cause = root_cause(&err);
            if cause.to_ascii_lowercase().contains("certificate") {
                AppError::bad("İkincil panelin sertifikası değişmiş. Değişikliği siz yaptıysanız yeni sertifikayı onaylayın; yapmadıysanız bağlantınız dinleniyor olabilir")
            } else {
                AppError::bad(format!("İkincil panele bağlanılamadı: {cause}"))
            }
        })?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let message = body.pointer("/error/message").and_then(Value::as_str).unwrap_or("bilinmeyen hata");
        return Err(AppError::bad(match status.as_u16() {
            401 => "İkincil panel API anahtarını kabul etmedi (yanlış, süresi dolmuş ya da bu sunucunun adresine izinli değil)".to_string(),
            403 => "İkincil paneldeki API anahtarı bir yöneticiye ait olmalı ve salt okunur olmamalı".to_string(),
            code => format!("İkincil panel isteği reddetti (HTTP {code}): {message}"),
        }));
    }
    let conflicts = body["conflicts"].as_array().into_iter().flatten().filter_map(|item| item.as_str().map(str::to_string)).collect();
    Ok(SyncStatus { at: now(), error: String::new(), zones: zones.len(), conflicts })
}

/// Bölge listesini paneli tanımlı bütün ikincillere iletir ve sonuçları kaydeder.
pub async fn sync(state: &AppState) -> AppResult<BTreeMap<String, SyncStatus>> {
    let list = secondaries(&state.db);
    let zones = local_zone_names(&state.db)?;
    let mut status = BTreeMap::new();
    for secondary in list.iter().filter(|item| !item.panel_url.is_empty()) {
        let outcome = match push(secondary, &zones).await {
            Ok(done) => done,
            Err(err) => {
                tracing::warn!(secondary = %secondary.address, error = %err, "DNS kümesi eşitlenemedi");
                SyncStatus { at: now(), error: err.to_string(), zones: zones.len(), conflicts: Vec::new() }
            }
        };
        status.insert(secondary.address.clone(), outcome);
    }
    state.db.set_setting(STATUS_KEY, &serde_json::to_string(&status).unwrap_or_default())?;
    Ok(status)
}

/// Bölge eklenip silindiğinde ikincilleri beklemeden arka planda eşitler.
pub fn sync_in_background(state: &AppState) {
    if secondaries(&state.db).iter().all(|item| item.panel_url.is_empty()) {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let _ = sync(&state).await;
    });
}

/// Bölgeye özel `ns2` kayıtlarını yeni adrese çevirir; yalnızca eski adresi gösterenlere dokunur.
async fn repoint_second_nameserver(zones: &[String], old: &str, new: &str) {
    if old == new {
        return;
    }
    for domain in zones {
        let name = format!("ns2.{domain}");
        let Ok(records) = local::list(domain).await else { continue };
        for record in records.iter().filter(|record| record.kind == "A" && record.name == name && record.content == old) {
            let mut updated = record.clone();
            updated.content = new.to_string();
            if let Err(err) = local::update(domain, record, &updated).await {
                tracing::warn!(domain = %domain, error = %err, "ns2 kaydı güncellenemedi");
            }
        }
    }
}

/// İstekle gelen ikincil tanımı; anahtar boşsa kayıtlı olan korunur.
#[derive(Debug, Deserialize)]
pub struct SecondaryInput {
    pub address: String,
    #[serde(default)]
    pub panel_url: String,
    #[serde(default)]
    pub token: String,
    /// İkincil panelin sertifikası değişti ve yönetici yenisini onaylıyor.
    #[serde(default)]
    pub repin: bool,
}

/// İkincil listesini kaydeder ve DNS sunucusuna uygular.
pub async fn save(state: &AppState, input: Vec<SecondaryInput>) -> AppResult<()> {
    if input.len() > MAX_SECONDARIES {
        return Err(AppError::bad(format!("En çok {MAX_SECONDARIES} ikincil sunucu tanımlanabilir")));
    }
    if !local::is_installed() {
        return Err(AppError::bad("DNS sunucusu kurulu değil; önce Bileşenler sayfasından kurun"));
    }
    let previous = secondaries(&state.db);
    let own: Vec<IpAddr> = crate::addresses::detect().await.into_iter().map(|found| found.address).collect();
    let mut list: Vec<Secondary> = Vec::new();
    for item in input {
        let address = validate_address(&item.address)?;
        if own.contains(&address) {
            return Err(AppError::bad(format!("{address} bu sunucunun kendi adresi; ikincil başka bir sunucu olmalı")));
        }
        let address = address.to_string();
        if list.iter().any(|other| other.address == address) {
            return Err(AppError::bad(format!("{address} iki kez girilmiş")));
        }
        let panel_url = normalize_panel_url(&item.panel_url)?;
        let old = previous.iter().find(|other| other.address == address);
        let token = match item.token.trim() {
            "" => old.filter(|other| other.panel_url == panel_url).map(|other| other.token.clone()).unwrap_or_default(),
            text => text.to_string(),
        };
        if !panel_url.is_empty() && token.is_empty() {
            return Err(AppError::bad(format!("{address} için ikincil panelin API anahtarını girin")));
        }
        // Sertifika yalnızca ikincil ilk eklendiğinde, panel adresi değiştiğinde ya da yönetici açıkça
        // yenisini onayladığında okunur. Başka bir ikincil eklenirken var olanların sabiti değişmez;
        // aksi hâlde araya giren biri o anda kendi sertifikasını sabitletip kayıtlı anahtarı alabilirdi.
        let kept = old.filter(|other| other.panel_url == panel_url && !other.fingerprint.is_empty() && !item.repin).map(|other| other.fingerprint.clone());
        let fingerprint = match kept {
            _ if panel_url.is_empty() => String::new(),
            Some(fingerprint) => fingerprint,
            None => probe_fingerprint(&panel_url).await?,
        };
        let token = if panel_url.is_empty() { String::new() } else { token };
        list.push(Secondary { address, panel_url, token, fingerprint });
    }

    let old_target = second_nameserver_address(&state.db);
    state.db.set_setting(SECONDARIES_KEY, &serde_json::to_string(&list).unwrap_or_default())?;
    let new_target = second_nameserver_address(&state.db);

    local::setup(&transfer(&state.db)).await?;
    let zones = local_zone_names(&state.db)?;
    for domain in &zones {
        // Eski kurulumlarda açılmış bölgeler bildirim göndermeyen türdeydi.
        if let Err(err) = local::make_primary(domain).await {
            tracing::warn!(domain = %domain, error = %err, "bölge birincil türüne çevrilemedi");
        }
    }
    repoint_second_nameserver(&zones, &old_target, &new_target).await;
    if !list.is_empty() {
        for domain in &zones {
            let _ = local::notify(domain).await;
        }
    }
    sync(state).await?;
    Ok(())
}

// ---- İkincil taraf ----

/// Bölge bu birincilden mi çekiliyor? Birincil adresi portlu da yazılmış olabilir.
fn pulled_from(zone: &local::ZoneState, primary: &str) -> bool {
    zone.secondary && zone.masters.iter().any(|master| master == primary || master.strip_suffix(":53") == Some(primary) || master == &format!("[{primary}]:53"))
}

/// İkincilin bir eşitleme isteğine verdiği yanıt.
#[derive(Debug, Default, Serialize)]
pub struct ReplicaReport {
    pub created: Vec<String>,
    pub removed: Vec<String>,
    /// Bu sunucuda aynı adla başka bir bölge bulunduğu için açılamayanlar.
    pub conflicts: Vec<String>,
    pub total: usize,
}

/// Bu sunucudaki ikincil bölgeleri, birincilin gönderdiği listeyle aynı hâle getirir.
/// Yalnızca o birincilden çekilen bölgelere dokunulur; panelin kendi bölgeleri korunur.
pub async fn replica_apply(state: &AppState, primary: IpAddr, wanted: &[String]) -> AppResult<ReplicaReport> {
    if !local::is_installed() {
        return Err(AppError::bad("Bu sunucuda DNS sunucusu kurulu değil; önce Bileşenler sayfasından kurun"));
    }
    if wanted.len() > MAX_ZONES {
        return Err(AppError::bad("Bölge listesi çok uzun"));
    }
    let own: Vec<IpAddr> = crate::addresses::detect().await.into_iter().map(|found| found.address).collect();
    if primary.is_loopback() || own.contains(&primary) {
        return Err(AppError::bad("Bir sunucu kendisinin ikincili olamaz"));
    }
    let mut domains = Vec::with_capacity(wanted.len());
    for name in wanted {
        domains.push(super::normalize_domain(name)?);
    }
    domains.sort();
    domains.dedup();

    // Birincil ilk kez görülüyorsa DNS sunucusu ondan bildirim kabul edecek şekilde yapılandırılır.
    let mut known = primaries(&state.db);
    if !known.contains(&primary) {
        known.push(primary);
        save_primaries(&state.db, &known)?;
        local::setup(&transfer(&state.db)).await?;
    }

    let primary_text = primary.to_string();
    let existing = local::zones().await?;
    let mut report = ReplicaReport { total: domains.len(), ..ReplicaReport::default() };
    for domain in &domains {
        match existing.iter().find(|zone| &zone.domain == domain) {
            Some(zone) if pulled_from(zone, &primary_text) => {}
            Some(_) => report.conflicts.push(domain.clone()),
            None => match local::create_secondary_zone(domain, primary).await {
                Ok(()) => report.created.push(domain.clone()),
                Err(err) => {
                    tracing::warn!(domain = %domain, error = %err, "ikincil bölge açılamadı");
                    report.conflicts.push(domain.clone());
                }
            },
        }
    }
    for zone in existing.iter().filter(|zone| pulled_from(zone, &primary_text) && !domains.contains(&zone.domain)) {
        if local::delete_zone(&zone.domain).await.is_ok() {
            report.removed.push(zone.domain.clone());
        }
    }
    Ok(report)
}

/// Bir birincille bağı koparır: ondan çekilen bölgeler silinir, bildirimleri artık kabul edilmez.
pub async fn replica_detach(state: &AppState, primary: IpAddr) -> AppResult<usize> {
    let primary_text = primary.to_string();
    let mut removed = 0;
    if local::is_installed() {
        for zone in local::zones().await?.iter().filter(|zone| pulled_from(zone, &primary_text)) {
            if local::delete_zone(&zone.domain).await.is_ok() {
                removed += 1;
            }
        }
    }
    let remaining: Vec<IpAddr> = primaries(&state.db).into_iter().filter(|known| *known != primary).collect();
    save_primaries(&state.db, &remaining)?;
    if local::is_installed() {
        local::setup(&transfer(&state.db)).await?;
    }
    Ok(removed)
}

/// Bu sunucunun ikincil olarak taşıdığı bölgelerin birincile göre sayısı.
pub async fn replica_summary(db: &Db) -> Vec<(String, usize)> {
    let known = primaries(db);
    if known.is_empty() || !local::is_installed() {
        return Vec::new();
    }
    let zones = local::zones().await.unwrap_or_default();
    known
        .iter()
        .map(|primary| {
            let text = primary.to_string();
            let count = zones.iter().filter(|zone| pulled_from(zone, &text)).count();
            (text, count)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_url_rules() {
        assert_eq!(normalize_panel_url(" https://ns2.ornek.com:8443/ ").unwrap(), "https://ns2.ornek.com:8443");
        assert_eq!(normalize_panel_url("https://203.0.113.5:8443").unwrap(), "https://203.0.113.5:8443");
        assert_eq!(normalize_panel_url("").unwrap(), "");
        for bad in ["http://ns2.ornek.com:8443", "https://kullanici:parola@ns2.ornek.com", "https://ns2.ornek.com/api", "https://ns2.ornek.com/?a=1", "ns2.ornek.com", "ftp://ns2.ornek.com"] {
            assert!(normalize_panel_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn transfer_lines() {
        let none = Transfer::default();
        assert_eq!(local::transfer_conf(&none), "");
        let both = Transfer { secondaries: vec!["203.0.113.5".parse().unwrap(), "2001:db8::5".parse().unwrap()], primaries: vec!["198.51.100.7".parse().unwrap()] };
        let conf = local::transfer_conf(&both);
        assert!(conf.contains("primary=yes\n"));
        assert!(conf.contains("allow-axfr-ips=127.0.0.1,::1,203.0.113.5,2001:db8::5\n"));
        assert!(conf.contains("also-notify=203.0.113.5,2001:db8::5\n"));
        assert!(conf.contains("only-notify=203.0.113.5,2001:db8::5\n"));
        assert!(conf.contains("secondary=yes\nallow-notify-from=198.51.100.7\n"));
    }

    #[test]
    fn fingerprint_parsing() {
        assert!(parse_fingerprint(&"ab".repeat(32)).is_some());
        assert!(parse_fingerprint("abcd").is_none());
        assert!(parse_fingerprint("").is_none());
    }
}
