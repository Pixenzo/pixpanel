//! Olay kancaları: hesabın tanımladığı adreslere panel olaylarını HTTP POST ile iletir.
//!
//! Olay kaynakları yenilerini icat etmez, var olanlara bağlanır:
//! - denetim günlüğüne düşen her işlem (`auth::audit`) → `audit.<eylem>`,
//! - görev başlangıç/bitişleri (canlı olay yolundaki `Event::Job`) → `job.started|succeeded|failed`,
//! - bildirim olayları (`notify::send_event`) → `deploy_failed`, `site_down`...
//!
//! Güvenlik: kanca adresi müşteri girdisidir. Alan adı çözülür, çözülen adreslerden biri yerel
//! ya da özel ağdaysa reddedilir ve bağlantı, doğrulanmış adrese kurulur (DNS yeniden bağlama
//! saldırısına karşı); Host başlığı ve TLS SNI alan adıyla kalır. Yönlendirmeler izlenmez.
//! Yalnızca yönetici özel ağ adreslerine kanca tanımlayabilir (yerel, bağlantı-yerel ve
//! bulut üst verisi adresleri ona da yasaktır).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::{mpsc, Semaphore};
use url::{Host, Url};

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::events::{self, Event};
use crate::state::AppState;

/// Hesap başına en çok kanca sayısı.
pub const MAX_HOOKS: i64 = 10;
/// Bir kancanın geçmişinde tutulan en çok gönderim kaydı.
const HISTORY: i64 = 20;
/// Bir gönderim için en çok deneme sayısı (ilk deneme dahil).
const MAX_ATTEMPTS: u32 = 3;
/// Denemeler arasındaki bekleme (artan aralık).
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(20)];
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(8);
/// Aynı anda sürebilecek giden istek sayısı.
const CONCURRENCY: usize = 16;
/// Bekleyen (denemesi sürüp biten) gönderimlerin üst sınırı; aşılırsa yenileri düşer.
const MAX_PENDING: usize = 2000;
const MAX_URL_LEN: usize = 2048;
const MAX_ERROR_LEN: usize = 300;
/// Gizli anahtar önekü (arayüzde ve belgelerde tanınması için).
const SECRET_PREFIX: &str = "whsec_";

/// Tüm olaylar için kullanılan süzgeç değeri.
pub const ALL: &str = "*";

/// Kancaya bağlanabilen olay türleri ve arayüzde gösterilen adları. Denetim olayları alan
/// düzeyindedir: `audit.site` seçilirse `audit.site.create`, `audit.site.delete`... hepsi gelir.
pub const EVENTS: &[(&str, &str)] = &[
    ("deploy_success", "Yayınlama başarıyla bittiğinde"),
    ("deploy_failed", "Yayınlama başarısız olduğunda"),
    ("ssl_failed", "SSL sertifikası yenilenemediğinde"),
    ("backup_failed", "Yedekleme başarısız olduğunda"),
    ("site_down", "Siteniz yanıt vermediğinde ve yeniden yanıt vermeye başladığında"),
    ("quota_warning", "Disk kotanız dolmak üzereyken ya da dolduğunda"),
    ("login_new_ip", "Hesabınıza yeni bir adresten giriş yapıldığında"),
    ("health_alert", "Sunucuda dikkat gerektiren bir sorun çıktığında ya da panel bir sorunu kendiliğinden düzelttiğinde"),
    ("weekly_report", "Haftalık özet hazırlandığında"),
    ("job.started", "Bir görev başladığında"),
    ("job.succeeded", "Bir görev başarıyla bittiğinde"),
    ("job.failed", "Bir görev başarısız olduğunda"),
    ("audit.site", "Site işlemleri"),
    ("audit.ssl", "SSL işlemleri"),
    ("audit.git", "Git ile yayınlama işlemleri"),
    ("audit.database", "Veritabanı işlemleri"),
    ("audit.mail", "Posta işlemleri"),
    ("audit.dns", "DNS işlemleri"),
    ("audit.backup", "Yedekleme işlemleri"),
    ("audit.docker", "Docker işlemleri"),
    ("audit.redis", "Redis işlemleri"),
    ("audit.wordpress", "WordPress işlemleri"),
    ("audit.cron", "Zamanlı görev işlemleri"),
    ("audit.php", "PHP işlemleri"),
    ("audit.ftp", "FTP işlemleri"),
    ("audit.firewall", "Güvenlik duvarı işlemleri"),
    ("audit.user", "Hesap ve kullanıcı işlemleri"),
    ("audit.token", "API anahtarı işlemleri"),
    ("audit.package", "Paket işlemleri"),
    ("audit.webhook", "Web kancası işlemleri"),
];

// ---------------------------------------------------------------------------
// Kayıtlar
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Hook {
    pub id: i64,
    pub user_id: i64,
    pub url: String,
    pub events: Vec<String>,
    /// İmza gizli anahtarı; hiçbir yanıta ya da günlüğe yazılmaz.
    pub secret: String,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub id: i64,
    pub event: String,
    pub attempt: i64,
    /// Alıcının HTTP durum kodu; bağlantı kurulamadıysa yok.
    pub status: Option<i64>,
    pub duration_ms: i64,
    pub error: String,
    pub at: i64,
}

const HOOK_COLUMNS: &str = "id, user_id, url, events, secret, enabled, created_at";

fn read_hook(row: &rusqlite::Row<'_>) -> rusqlite::Result<Hook> {
    let events: String = row.get(3)?;
    Ok(Hook {
        id: row.get(0)?,
        user_id: row.get(1)?,
        url: row.get(2)?,
        events: serde_json::from_str(&events).unwrap_or_default(),
        secret: row.get(4)?,
        enabled: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn read_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        id: row.get(0)?,
        event: row.get(1)?,
        attempt: row.get(2)?,
        status: row.get(3)?,
        duration_ms: row.get(4)?,
        error: row.get(5)?,
        at: row.get(6)?,
    })
}

const DELIVERY_COLUMNS: &str = "id, event, attempt, status, duration_ms, error, created_at";

pub fn list(db: &Db, user_id: i64) -> AppResult<Vec<Hook>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {HOOK_COLUMNS} FROM webhooks WHERE user_id = ?1 ORDER BY id"))?;
        let rows = stmt.query_map(params![user_id], read_hook)?;
        rows.collect()
    })
}

/// Kancayı sahibine göre getirir; başkasının kancası için `NotFound` döner.
pub fn get(db: &Db, user_id: i64, id: i64) -> AppResult<Hook> {
    db.call(|conn| {
        conn.query_row(&format!("SELECT {HOOK_COLUMNS} FROM webhooks WHERE id = ?1 AND user_id = ?2"), params![id, user_id], read_hook)
    })
}

/// Üst sınırı aynı kilit altında denetleyerek ekler (eşzamanlı isteklerle aşılamaz).
pub fn create(db: &Db, user_id: i64, url: &str, events: &[String], secret: &str, enabled: bool) -> AppResult<Hook> {
    let encoded = serde_json::to_string(events).map_err(|err| AppError::internal(err.to_string()))?;
    let id = db.call(|conn| {
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM webhooks WHERE user_id = ?1", params![user_id], |row| row.get(0))?;
        if count >= MAX_HOOKS {
            return Ok(None);
        }
        conn.execute(
            "INSERT INTO webhooks (user_id, url, events, secret, enabled, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![user_id, url, encoded, secret, enabled, now()],
        )?;
        Ok(Some(conn.last_insert_rowid()))
    })?;
    match id {
        Some(id) => get(db, user_id, id),
        None => Err(AppError::conflict(format!("Hesap başına en çok {MAX_HOOKS} web kancası tanımlanabilir"))),
    }
}

pub fn update(db: &Db, hook: &Hook) -> AppResult<()> {
    let encoded = serde_json::to_string(&hook.events).map_err(|err| AppError::internal(err.to_string()))?;
    db.call(|conn| {
        conn.execute(
            "UPDATE webhooks SET url = ?1, events = ?2, enabled = ?3, secret = ?4 WHERE id = ?5 AND user_id = ?6",
            params![hook.url, encoded, hook.enabled, hook.secret, hook.id, hook.user_id],
        )
    })?;
    Ok(())
}

pub fn delete(db: &Db, user_id: i64, id: i64) -> AppResult<()> {
    let removed = db.call(|conn| conn.execute("DELETE FROM webhooks WHERE id = ?1 AND user_id = ?2", params![id, user_id]))?;
    if removed == 0 { Err(AppError::NotFound) } else { Ok(()) }
}

/// Son gönderimler, yeniden eskiye.
pub fn deliveries(db: &Db, hook_id: i64) -> AppResult<Vec<Delivery>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(&format!("SELECT {DELIVERY_COLUMNS} FROM webhook_deliveries WHERE webhook_id = ?1 ORDER BY id DESC LIMIT {HISTORY}"))?;
        let rows = stmt.query_map(params![hook_id], read_delivery)?;
        rows.collect()
    })
}

fn record_delivery(db: &Db, hook_id: i64, event: &str, attempt: u32, outcome: &Outcome) {
    let result = db.call(|conn| {
        conn.execute(
            "INSERT INTO webhook_deliveries (webhook_id, event, attempt, status, duration_ms, error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![hook_id, event, attempt, outcome.status, outcome.duration_ms, outcome.error, now()],
        )?;
        conn.execute(
            &format!(
                "DELETE FROM webhook_deliveries WHERE webhook_id = ?1
                   AND id NOT IN (SELECT id FROM webhook_deliveries WHERE webhook_id = ?1 ORDER BY id DESC LIMIT {HISTORY})"
            ),
            params![hook_id],
        )
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, hook_id, "web kancası gönderim kaydı yazılamadı");
    }
}

/// Kancayı ve sahibinin yönetici olup olmadığını okur (sahip silindiyse `None`).
fn load_with_role(db: &Db, id: i64) -> AppResult<Option<(Hook, bool)>> {
    db.call(|conn| {
        conn.query_row(
            "SELECT w.id, w.user_id, w.url, w.events, w.secret, w.enabled, w.created_at, u.role
               FROM webhooks w JOIN users u ON u.id = w.user_id WHERE w.id = ?1",
            params![id],
            |row| Ok((read_hook(row)?, row.get::<_, String>(7)? == "admin")),
        )
        .optional()
    })
}

fn enabled_with_role(db: &Db) -> AppResult<Vec<(Hook, bool)>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT w.id, w.user_id, w.url, w.events, w.secret, w.enabled, w.created_at, u.role
               FROM webhooks w JOIN users u ON u.id = w.user_id WHERE w.enabled = 1",
        )?;
        let rows = stmt.query_map([], |row| Ok((read_hook(row)?, row.get::<_, String>(7)? == "admin")))?;
        rows.collect()
    })
}

pub fn new_secret() -> String {
    format!("{SECRET_PREFIX}{}", crate::auth::random_token(24))
}

/// Kullanıcıdan gelen olay seçimini doğrular; `*` varsa yalnızca o kalır.
pub fn validate_events(events: &[String]) -> AppResult<Vec<String>> {
    if events.is_empty() {
        return Err(AppError::bad("En az bir olay seçin ya da tüm olayları seçin"));
    }
    if events.iter().any(|event| event == ALL) {
        return Ok(vec![ALL.to_string()]);
    }
    let mut chosen: Vec<String> = Vec::new();
    for event in events {
        // Listedeki bir olayın alt olayı da seçilebilir (ör. `audit.site.create`).
        let plain = event.len() <= 80 && event.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-'));
        let known = EVENTS.iter().any(|(id, _)| id == event || event.strip_prefix(id).is_some_and(|rest| rest.starts_with('.')));
        if !plain || !known {
            return Err(AppError::bad("Bilinmeyen olay türü"));
        }
        if !chosen.contains(event) {
            chosen.push(event.clone());
        }
    }
    Ok(chosen)
}

/// Olay, kancanın süzgecinden geçer mi? `audit.site` süzgeci `audit.site.create` olayını da kapsar.
pub fn matches(filter: &[String], event: &str) -> bool {
    filter.iter().any(|item| item == ALL || item == event || event.strip_prefix(item.as_str()).is_some_and(|rest| rest.starts_with('.')))
}

// ---------------------------------------------------------------------------
// Adres doğrulama (SSRF koruması)
// ---------------------------------------------------------------------------

/// IPv4 adresine gönderim yasak mı? `allow_private` yalnızca yöneticiler içindir ve özel
/// ağ aralıklarını (10/8, 172.16/12, 192.168/16, CGNAT) açar.
fn blocked_v4(ip: Ipv4Addr, allow_private: bool) -> bool {
    let o = ip.octets();
    if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() || ip.is_multicast() || ip.is_broadcast() {
        return true;
    }
    // 0.0.0.0/8, ayrılmış 240.0.0.0/4, protokol ataması 192.0.0.0/24, belge ve ölçüm aralıkları.
    if o[0] == 0 || o[0] >= 240 || (o[0] == 192 && o[1] == 0 && o[2] == 0) {
        return true;
    }
    if (o[0] == 192 && o[1] == 0 && o[2] == 2) || (o[0] == 198 && o[1] == 51 && o[2] == 100) || (o[0] == 203 && o[1] == 0 && o[2] == 113) {
        return true;
    }
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        return true;
    }
    let private = ip.is_private() || (o[0] == 100 && (o[1] & 0xc0) == 64);
    private && !allow_private
}

/// Adres yasak mı? IPv6 içine gömülü IPv4 adresleri (eşlenmiş, uyumlu, NAT64, 6to4) IPv4
/// kurallarıyla denetlenir; böylece `::ffff:127.0.0.1` gibi biçimler geçemez.
pub fn is_blocked(ip: IpAddr, allow_private: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4, allow_private),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4() {
                return blocked_v4(v4, allow_private);
            }
            let s = v6.segments();
            // NAT64 (64:ff9b::/96) ve 6to4 (2002::/16): gömülü IPv4 adresi aynı kurallara tabidir.
            if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6].iter().all(|part| *part == 0) {
                return blocked_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8), allow_private);
            }
            if s[0] == 0x2002 {
                return blocked_v4(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8), allow_private);
            }
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return true;
            }
            // Bağlantı-yerel fe80::/10, eski site-yerel fec0::/10, belge aralığı 2001:db8::/32.
            if (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xffc0) == 0xfec0 || (s[0] == 0x2001 && s[1] == 0x0db8) {
                return true;
            }
            // Benzersiz yerel fc00::/7.
            (s[0] & 0xfe00) == 0xfc00 && !allow_private
        }
    }
}

const BLOCKED_MESSAGE: &str = "Bu adrese istek gönderilemez: yerel ve özel ağ adresleri kullanılamaz";

/// Doğrulanmış bir hedef: bağlantı yalnızca `addrs` adreslerine kurulur.
pub struct Target {
    pub url: Url,
    /// Alan adıyla verildiyse adı (çözümü `addrs`'e sabitlenir); IP ile verildiyse `None`.
    pub domain: Option<String>,
    pub addrs: Vec<SocketAddr>,
}

/// Adresin biçimini denetler, alan adını çözer ve çözülen her adresi doğrular.
/// Çözülen adreslerden biri bile yasaksa hedef reddedilir.
pub async fn resolve_target(raw: &str, allow_private: bool) -> AppResult<Target> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_URL_LEN {
        return Err(AppError::bad("Adres boş ya da çok uzun"));
    }
    let url = Url::parse(raw).map_err(|_| AppError::bad("Geçersiz adres (örnek: https://ornek.com/kanca)"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::bad("Adres http:// ya da https:// ile başlamalı"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::bad("Adrese kullanıcı adı ve parola yazılamaz"));
    }
    let port = url.port_or_known_default().ok_or_else(|| AppError::bad("Geçersiz adres"))?;
    let host = url.host().ok_or_else(|| AppError::bad("Adreste sunucu adı yok"))?;

    let (domain, addrs): (Option<String>, Vec<SocketAddr>) = match host {
        Host::Ipv4(ip) => (None, vec![SocketAddr::new(IpAddr::V4(ip), port)]),
        Host::Ipv6(ip) => (None, vec![SocketAddr::new(IpAddr::V6(ip), port)]),
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            if name.is_empty() || name == "localhost" || name.ends_with(".localhost") {
                return Err(AppError::bad(BLOCKED_MESSAGE));
            }
            let resolved = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((name.as_str(), port)))
                .await
                .map_err(|_| AppError::bad("Adresin sunucu adı çözülemedi (zaman aşımı)"))?
                .map_err(|_| AppError::bad(format!("{name} adresi çözülemedi")))?;
            let mut addrs: Vec<SocketAddr> = Vec::new();
            for address in resolved {
                if !addrs.contains(&address) {
                    addrs.push(address);
                }
            }
            (Some(name), addrs)
        }
    };
    if addrs.is_empty() {
        return Err(AppError::bad("Adresin sunucu adı çözülemedi"));
    }
    if addrs.iter().any(|address| is_blocked(address.ip(), allow_private)) {
        return Err(AppError::bad(BLOCKED_MESSAGE));
    }
    Ok(Target { url, domain, addrs })
}

/// Denetim günlüğüne yazılacak güvenli ad: yol ve sorgu (Slack gibi hizmetlerde gizli anahtar
/// taşır) atılır, yalnızca sunucu kalır.
pub fn display_origin(raw: &str) -> String {
    Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(|host| format!("{}://{host}", url.scheme())))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Gönderim
// ---------------------------------------------------------------------------

/// Tek bir denemenin sonucu.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: Option<i64>,
    pub duration_ms: i64,
    pub error: String,
}

impl Outcome {
    fn ok(&self) -> bool {
        self.status.is_some_and(|status| (200..300).contains(&status)) && self.error.is_empty()
    }

    fn failed(started: Instant, status: Option<i64>, error: impl Into<String>) -> Self {
        Self { status, duration_ms: started.elapsed().as_millis() as i64, error: truncate(error.into()) }
    }
}

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_ERROR_LEN {
        return text;
    }
    text.chars().take(MAX_ERROR_LEN).collect()
}

/// Hata zincirini kısa bir metne çevirir; adres (gizli anahtar içerebilir) dahil edilmez.
fn describe_error(err: reqwest::Error) -> String {
    let err = err.without_url();
    if err.is_timeout() {
        return "Zaman aşımı (10 sn)".into();
    }
    let mut text = if err.is_connect() { "Bağlantı kurulamadı".to_string() } else { "İstek gönderilemedi".to_string() };
    let mut source = std::error::Error::source(&err);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// `sha256=<hex HMAC-SHA256(gizli, gövde)>`
pub fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC her uzunlukta anahtar kabul eder");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn build_body(event: &str, at: i64, account: Option<&str>, target: &str, detail: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "event": event, "at": at, "account": account, "target": target, "detail": detail })).unwrap_or_default()
}

/// Tek bir deneme: adresi yeniden çözüp doğrular, doğrulanmış adrese bağlanır ve gövdeyi gönderir.
async fn attempt(hook: &Hook, allow_private: bool, event: &str, body: &[u8]) -> Outcome {
    let started = Instant::now();
    let target = match resolve_target(&hook.url, allow_private).await {
        Ok(target) => target,
        Err(err) => return Outcome::failed(started, None, err.to_string()),
    };
    let mut builder = reqwest::Client::builder()
        .user_agent("PixPanel-Webhook")
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        // Ortam değişkenlerindeki vekil, doğrulanmış adresi atlatmasın.
        .no_proxy()
        .pool_max_idle_per_host(0);
    if let Some(domain) = &target.domain {
        // Bağlantı doğrulanmış adrese kurulur; Host başlığı ve TLS SNI alan adıyla kalır.
        builder = builder.resolve_to_addrs(domain, &target.addrs);
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(err) => return Outcome::failed(started, None, format!("İstemci kurulamadı: {}", err.without_url())),
    };
    let request = client
        .post(target.url.clone())
        .header("Content-Type", "application/json")
        .header("X-PixPanel-Event", event)
        .header("X-PixPanel-Signature", signature(&hook.secret, body))
        .body(body.to_vec());
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16() as i64;
            if response.status().is_success() {
                Outcome { status: Some(status), duration_ms: started.elapsed().as_millis() as i64, error: String::new() }
            } else {
                Outcome::failed(started, Some(status), format!("Alıcı {status} yanıtı verdi"))
            }
        }
        Err(err) => Outcome::failed(started, None, describe_error(err)),
    }
}

static SLOTS: Semaphore = Semaphore::const_new(CONCURRENCY);
static PENDING: AtomicUsize = AtomicUsize::new(0);

/// Bir kez izin alıp tek deneme yapar.
async fn attempt_limited(hook: &Hook, allow_private: bool, event: &str, body: &[u8]) -> Outcome {
    match SLOTS.acquire().await {
        Ok(_permit) => attempt(hook, allow_private, event, body).await,
        Err(_) => Outcome::failed(Instant::now(), None, "Gönderim kuyruğu kapalı"),
    }
}

/// Bir olayı bir kancaya iletir; başarısızlıkta artan aralıkla en çok `MAX_ATTEMPTS` deneme yapar.
/// Her denemenin sonucu geçmişe yazılır. Kanca bu sırada silinir ya da kapatılırsa durur.
async fn deliver(db: Db, hook_id: i64, event: String, body: Vec<u8>) {
    for attempt_no in 1..=MAX_ATTEMPTS {
        let Ok(Some((hook, admin))) = load_with_role(&db, hook_id) else { return };
        if !hook.enabled {
            return;
        }
        let outcome = attempt_limited(&hook, admin, &event, &body).await;
        record_delivery(&db, hook_id, &event, attempt_no, &outcome);
        if outcome.ok() {
            return;
        }
        match RETRY_DELAYS.get(attempt_no as usize - 1) {
            Some(delay) => tokio::time::sleep(*delay).await,
            None => return,
        }
    }
}

/// "Sınama gönderimi": tek deneme, sonucu hemen döner ve geçmişe yazılır.
pub async fn send_test(db: &Db, hook: &Hook) -> AppResult<Delivery> {
    let admin = load_with_role(db, hook.id)?.is_some_and(|(_, admin)| admin);
    let at = now();
    let account = account_name(db, Some(hook.user_id));
    let body = build_body("webhook.test", at, account.as_deref(), &hook.url_origin(), &json!({ "message": "Bu bir sınama gönderimidir" }));
    let outcome = attempt_limited(hook, admin, "webhook.test", &body).await;
    record_delivery(db, hook.id, "webhook.test", 1, &outcome);
    Ok(Delivery {
        id: 0,
        event: "webhook.test".into(),
        attempt: 1,
        status: outcome.status,
        duration_ms: outcome.duration_ms,
        error: outcome.error,
        at,
    })
}

impl Hook {
    fn url_origin(&self) -> String {
        display_origin(&self.url)
    }
}

// ---------------------------------------------------------------------------
// Olay kaynakları ve dağıtıcı
// ---------------------------------------------------------------------------

struct Emission {
    event: String,
    /// Olayın ilgilendirdiği hesap; `None` ise yalnızca yöneticilerin kancaları alır.
    account: Option<i64>,
    target: String,
    detail: Value,
}

static QUEUE: OnceLock<mpsc::UnboundedSender<Emission>> = OnceLock::new();

/// Bir olayı kancalara iletilmek üzere sıraya bırakır. Dağıtıcı çalışmıyorsa (ör. komut satırı
/// kipi, testler) sessizce düşer; çağıran hiçbir zaman beklemez ve hata almaz.
pub fn emit(event: &str, account: Option<i64>, target: &str, detail: Value) {
    if let Some(queue) = QUEUE.get() {
        let _ = queue.send(Emission { event: event.to_string(), account, target: target.to_string(), detail });
    }
}

/// Denetim günlüğüne düşen bir işlemi `audit.<eylem>` olarak yayınlar.
pub fn emit_audit(action: &str, account: i64, actor: &str, target: &str, detail: &str) {
    emit(&format!("audit.{action}"), Some(account), target, json!({ "action": action, "actor": actor, "detail": detail }));
}

fn account_name(db: &Db, account: Option<i64>) -> Option<String> {
    let id = account?;
    db.call(|conn| conn.query_row("SELECT username FROM users WHERE id = ?1", params![id], |row| row.get::<_, String>(0)).optional()).ok().flatten()
}

/// Dağıtıcıyı başlatır: canlı olay yolundaki görev olaylarını ve `emit` ile bırakılanları
/// eşleşen kancalara iletir.
pub fn spawn(state: AppState) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Emission>();
    if QUEUE.set(tx).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut bus = events::subscribe();
        loop {
            tokio::select! {
                queued = rx.recv() => {
                    let Some(emission) = queued else { break };
                    dispatch(&state.db, emission);
                }
                envelope = bus.recv() => match envelope {
                    Ok(envelope) => {
                        if let Event::Job { job } = envelope.event {
                            let name = match job.status.as_str() {
                                "running" => "job.started",
                                "success" => "job.succeeded",
                                "failed" => "job.failed",
                                _ => continue,
                            };
                            let detail = json!({
                                "id": job.id,
                                "kind": job.kind,
                                "status": job.status,
                                "created_at": job.created_at,
                                "finished_at": job.finished_at,
                            });
                            dispatch(&state.db, Emission { event: name.into(), account: envelope.account, target: job.title, detail });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    });
}

/// Olayı eşleşen kancalara dağıtır. Sahiplik süzgeci canlı olay akışınınkiyle aynıdır:
/// yönetici her olayı, diğer hesaplar yalnızca kendi hesaplarını ilgilendirenleri alır.
fn dispatch(db: &Db, emission: Emission) {
    let hooks = match enabled_with_role(db) {
        Ok(hooks) => hooks,
        Err(err) => {
            tracing::warn!(error = %err, "web kancaları okunamadı");
            return;
        }
    };
    let selected: Vec<i64> = hooks
        .iter()
        .filter(|(hook, admin)| (*admin || emission.account == Some(hook.user_id)) && matches(&hook.events, &emission.event))
        .map(|(hook, _)| hook.id)
        .collect();
    if selected.is_empty() {
        return;
    }
    let account = account_name(db, emission.account);
    let body = build_body(&emission.event, now(), account.as_deref(), &emission.target, &emission.detail);
    for hook_id in selected {
        if PENDING.fetch_add(1, Ordering::Relaxed) >= MAX_PENDING {
            PENDING.fetch_sub(1, Ordering::Relaxed);
            tracing::warn!(hook_id, event = %emission.event, "web kancası kuyruğu dolu, olay düşürüldü");
            continue;
        }
        let (db, event, body) = (db.clone(), emission.event.clone(), body.clone());
        tokio::spawn(async move {
            deliver(db, hook_id, event, body).await;
            PENDING.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn customers_cannot_reach_internal_addresses() {
        for blocked in [
            "127.0.0.1", "127.1.2.3", "0.0.0.0", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.25", "169.254.169.254",
            "100.64.0.1", "100.127.255.255", "224.0.0.1", "255.255.255.255", "240.0.0.1", "198.18.0.1", "192.0.0.8",
            "::1", "::", "fe80::1", "fc00::1", "fd12:3456::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1",
            "::ffff:169.254.169.254", "64:ff9b::7f00:1", "2002:7f00:1::", "2001:db8::1",
        ] {
            assert!(is_blocked(ip(blocked), false), "{blocked} yasak olmalı");
        }
        for allowed in ["8.8.8.8", "1.1.1.1", "172.15.0.1", "172.32.0.1", "100.63.0.1", "100.128.0.1", "93.184.216.34", "2606:4700::1111"] {
            assert!(!is_blocked(ip(allowed), false), "{allowed} açık olmalı");
        }
    }

    #[test]
    fn admin_may_reach_private_but_never_local_or_metadata() {
        for allowed in ["10.1.2.3", "172.16.0.1", "192.168.1.25", "100.64.0.1", "fd12:3456::1", "::ffff:192.168.1.25"] {
            assert!(!is_blocked(ip(allowed), true), "{allowed} yönetici için açık olmalı");
        }
        for blocked in ["127.0.0.1", "::1", "0.0.0.0", "169.254.169.254", "fe80::1", "::ffff:127.0.0.1", "224.0.0.1"] {
            assert!(is_blocked(ip(blocked), true), "{blocked} yönetici için de yasak olmalı");
        }
    }

    #[tokio::test]
    async fn rejects_bad_urls_without_network() {
        for url in [
            "file:///etc/passwd", "ftp://ornek.com/", "gopher://127.0.0.1/", "javascript:alert(1)", "http://127.0.0.1/", "http://[::1]/",
            "http://localhost/", "http://LOCALHOST:8080/", "http://a.localhost/", "http://169.254.169.254/latest/meta-data",
            "http://2130706433/", "http://0x7f.1/", "http://127.1/", "http://0/", "http://[::ffff:127.0.0.1]/", "http://10.0.0.5/",
            "https://kullanici:parola@8.8.8.8/", "", "ornek.com/kanca",
        ] {
            assert!(resolve_target(url, false).await.is_err(), "{url} reddedilmeli");
        }
        // Yönetici de loopback ve bulut üst verisi adresine gönderemez.
        for url in ["http://127.0.0.1/", "http://[::1]/", "http://localhost/", "http://169.254.169.254/"] {
            assert!(resolve_target(url, true).await.is_err(), "{url} yönetici için de reddedilmeli");
        }
        assert!(resolve_target("http://192.168.1.25:9000/x", true).await.is_ok());
        let target = resolve_target("https://8.8.8.8/kanca?x=1", false).await.unwrap();
        assert_eq!(target.addrs, vec!["8.8.8.8:443".parse().unwrap()]);
        assert!(target.domain.is_none());
    }

    #[test]
    fn filter_matching() {
        let all = vec![ALL.to_string()];
        assert!(matches(&all, "audit.site.create"));
        let some = vec!["audit.site".to_string(), "deploy_failed".to_string()];
        assert!(matches(&some, "audit.site.create"));
        assert!(matches(&some, "deploy_failed"));
        assert!(!matches(&some, "audit.sites.create"));
        assert!(!matches(&some, "audit.mail.create"));
        assert!(!matches(&some, "deploy_success"));
        assert!(!matches(&[], "deploy_failed"));
    }

    #[test]
    fn event_validation() {
        assert!(validate_events(&[]).is_err());
        assert!(validate_events(&["bilinmeyen".into()]).is_err());
        assert_eq!(validate_events(&["deploy_failed".into(), "*".into()]).unwrap(), vec!["*".to_string()]);
        assert_eq!(validate_events(&["deploy_failed".into(), "deploy_failed".into()]).unwrap(), vec!["deploy_failed".to_string()]);
    }

    #[test]
    fn signature_matches_known_vector() {
        // RFC 4231, test durumu 2: anahtar "Jefe", veri "what do ya want for nothing?"
        assert_eq!(
            signature("Jefe", b"what do ya want for nothing?"),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn origin_hides_path_and_query() {
        assert_eq!(display_origin("https://hooks.slack.com/services/T000/B000/XXXX?a=b"), "https://hooks.slack.com");
        assert_eq!(display_origin("bozuk"), "");
    }

    #[test]
    fn body_shape() {
        let body = build_body("site_down", 1700000000, Some("ali"), "ornek.com", &json!({ "k": 1 }));
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["event"], "site_down");
        assert_eq!(parsed["at"], 1700000000);
        assert_eq!(parsed["account"], "ali");
        assert_eq!(parsed["target"], "ornek.com");
        assert_eq!(parsed["detail"]["k"], 1);
    }
}
