//! Giden posta sınırı: panel, Postfix için küçük bir "policy servisi" olur.
//!
//! Ele geçirilmiş bir kutunun spam basmasını sınırlamak için kimlik doğrulamalı gönderim
//! servislerinde (submission, smtps) her alıcı (RCPT) için panele sorulur; panel kutunun ve
//! alan adının son bir saatteki alıcı sayısını sayar, sınır aşıldıysa geçici ret (450) döner.
//!
//! - Protokol: Postfix policy delegation (`name=value` satırları, boş satırla biter, yanıt
//!   `action=...` + boş satır). Dinleme Postfix'in kuyruk klasöründeki (`private/`) unix
//!   soketindedir: Postfix'in chroot'undan erişilir ve soket dosyasına yalnızca `postfix`
//!   kullanıcısı yazabilir; böylece sunucudaki bir müşteri sahte istekle başkasının sayacını
//!   dolduramaz (127.0.0.1 üzerinde bir port bunu engelleyemezdi).
//! - Panel kapalıysa posta durmaz: `smtpd_policy_service_default_action = DUNNO`.
//! - Sayaçlar bellektedir (dakikalık kovalarla kayan pencere); panel yeniden başlayınca sıfırlanır.
//! - Sunucunun içinden (`sendmail`, PHP `mail()`) gönderilen posta smtpd'den geçmez; bu sınır
//!   onları kapsamaz.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::params;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::sysuser;
use crate::state::AppState;
use crate::sys::exec;

/// Sunucu geneli saatlik sınırın varsayılanı (0 sınırsız demektir).
pub const DEFAULT_HOURLY_LIMIT: i64 = 200;
/// Alan adı sınırı, ayrıca belirtilmemişse kutu varsayılanının bu kadar katıdır.
pub const DOMAIN_FACTOR: i64 = 5;
pub const MAX_LIMIT: i64 = 1_000_000;
const SETTING_KEY: &str = "mail.send_limit";
const SOCKET_NAME: &str = "pixpanel-policy";
const DEFAULT_QUEUE_DIR: &str = "/var/spool/postfix";
/// Postfix'in policy servisine verdiği ad (kuyruk klasörüne göreli).
const POSTFIX_ENDPOINT: &str = "unix:private/pixpanel-policy";
const BUCKET_SECS: u64 = 60;
const WINDOW_BUCKETS: u64 = 60;
const LIMITS_TTL: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(900);
const MAX_LINE: u64 = 8192;
const MAX_ATTRIBUTES: usize = 64;
const HIT_RETENTION_SECS: i64 = 30 * 24 * 3600;

// ---- Sınırlar ----

/// Geçerli sınırların bir anlık görüntüsü.
#[derive(Debug, Default)]
pub struct Limits {
    pub default_limit: i64,
    /// Kutu adresi → kutuya özel sınır (`None`: varsayılan).
    pub mailboxes: HashMap<String, Option<i64>>,
    /// Alan adı → alan adına özel sınır (`None`: varsayılanın katı).
    pub domains: HashMap<String, Option<i64>>,
}

impl Limits {
    pub fn mailbox_limit(&self, address: &str) -> i64 {
        self.mailboxes.get(address).copied().flatten().unwrap_or(self.default_limit)
    }

    pub fn domain_limit(&self, domain: &str) -> i64 {
        self.domains.get(domain).copied().flatten().unwrap_or(self.default_limit.saturating_mul(DOMAIN_FACTOR))
    }
}

/// Sunucu geneli varsayılan sınır.
pub fn default_limit(db: &Db) -> i64 {
    db.setting(SETTING_KEY).ok().flatten().and_then(|raw| raw.trim().parse::<i64>().ok()).filter(|value| (0..=MAX_LIMIT).contains(value)).unwrap_or(DEFAULT_HOURLY_LIMIT)
}

pub fn validate_limit(value: i64) -> AppResult<i64> {
    if (0..=MAX_LIMIT).contains(&value) { Ok(value) } else { Err(AppError::bad("Saatlik sınır 0 (sınırsız) ile 1.000.000 arasında olmalı")) }
}

pub fn save_default_limit(db: &Db, value: i64) -> AppResult<()> {
    db.set_setting(SETTING_KEY, &validate_limit(value)?.to_string())?;
    invalidate();
    Ok(())
}

fn load_limits(db: &Db) -> AppResult<Limits> {
    let mailboxes = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT m.local || '@' || d.domain, m.send_limit FROM mailboxes m JOIN mail_domains d ON d.id = m.domain_id")?;
        stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))?.collect::<rusqlite::Result<HashMap<_, _>>>()
    })?;
    let domains = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT domain, send_limit FROM mail_domains")?;
        stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))?.collect::<rusqlite::Result<HashMap<_, _>>>()
    })?;
    Ok(Limits { default_limit: default_limit(db), mailboxes, domains })
}

// ---- Sayaçlar ----

#[derive(Default)]
struct Usage {
    /// Anahtar (`m:adres` ya da `d:alan`) → (dakika numarası, alıcı sayısı) kovaları.
    windows: HashMap<String, VecDeque<(u64, u32)>>,
}

impl Usage {
    fn prune(window: &mut VecDeque<(u64, u32)>, minute: u64) {
        while window.front().is_some_and(|(bucket, _)| bucket + WINDOW_BUCKETS <= minute) {
            window.pop_front();
        }
    }

    fn count(&mut self, key: &str, now_secs: u64) -> u32 {
        let minute = now_secs / BUCKET_SECS;
        match self.windows.get_mut(key) {
            Some(window) => {
                Self::prune(window, minute);
                window.iter().map(|(_, count)| *count).sum()
            }
            None => 0,
        }
    }

    fn add(&mut self, key: &str, now_secs: u64) {
        let minute = now_secs / BUCKET_SECS;
        let window = self.windows.entry(key.to_string()).or_default();
        Self::prune(window, minute);
        match window.back_mut() {
            Some((bucket, count)) if *bucket == minute => *count += 1,
            _ => window.push_back((minute, 1)),
        }
    }

    /// Boşalmış pencereleri atar (bellek sınırlı kalsın).
    fn collect_garbage(&mut self, now_secs: u64) {
        let minute = now_secs / BUCKET_SECS;
        self.windows.retain(|_, window| {
            Self::prune(window, minute);
            !window.is_empty()
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Mailbox,
    Domain,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Mailbox => "mailbox",
            Scope::Domain => "domain",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny { scope: Scope, subject: String, limit: i64 },
}

/// Alıcı isteğini sayar ve izin verip vermeyeceğine karar verir. Reddedilen istek sayılmaz;
/// böylece pencere kayarken sınır kendiliğinden açılır.
fn decide(usage: &mut Usage, limits: &Limits, user: &str, now_secs: u64) -> Verdict {
    let domain = user.rsplit_once('@').map(|(_, domain)| domain).unwrap_or("");
    let mailbox_limit = limits.mailbox_limit(user);
    let domain_limit = if domain.is_empty() { 0 } else { limits.domain_limit(domain) };
    let (mailbox_key, domain_key) = (format!("m:{user}"), format!("d:{domain}"));
    if mailbox_limit > 0 && i64::from(usage.count(&mailbox_key, now_secs)) >= mailbox_limit {
        return Verdict::Deny { scope: Scope::Mailbox, subject: user.to_string(), limit: mailbox_limit };
    }
    if domain_limit > 0 && i64::from(usage.count(&domain_key, now_secs)) >= domain_limit {
        return Verdict::Deny { scope: Scope::Domain, subject: domain.to_string(), limit: domain_limit };
    }
    usage.add(&mailbox_key, now_secs);
    if !domain.is_empty() {
        usage.add(&domain_key, now_secs);
    }
    Verdict::Allow
}

#[derive(Default)]
struct Shared {
    usage: Usage,
    limits: Option<(Instant, Arc<Limits>)>,
    /// `tür:ad` → en son bildirildiği saat (saat başına en çok bir kayıt için).
    reported: HashMap<String, u64>,
    requests: u64,
}

fn shared() -> &'static Mutex<Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, Shared> {
    shared().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sınırlar değişti: bir sonraki istekte veritabanından yeniden okunur.
pub fn invalidate() {
    lock().limits = None;
}

fn current_limits(db: &Db) -> Arc<Limits> {
    let mut guard = lock();
    if let Some((loaded_at, limits)) = &guard.limits {
        if loaded_at.elapsed() < LIMITS_TTL {
            return limits.clone();
        }
    }
    match load_limits(db) {
        Ok(limits) => {
            let limits = Arc::new(limits);
            guard.limits = Some((Instant::now(), limits.clone()));
            limits
        }
        Err(err) => {
            // Veritabanı okunamazsa eski sınırlar (yoksa hiçbiri) kullanılır; posta durmaz.
            tracing::warn!(error = %err, "gönderim sınırları okunamadı");
            guard.limits.as_ref().map(|(_, limits)| limits.clone()).unwrap_or_default()
        }
    }
}

fn unix_now() -> u64 {
    now().max(0) as u64
}

/// Kutunun son bir saatteki alıcı sayısı.
pub fn sent_last_hour(address: &str) -> u32 {
    lock().usage.count(&format!("m:{address}"), unix_now())
}

/// Alan adının son bir saatteki toplam alıcı sayısı.
pub fn domain_sent_last_hour(domain: &str) -> u32 {
    lock().usage.count(&format!("d:{domain}"), unix_now())
}

/// Kutu listesine gönderim sayısını ve geçerli sınırı işler.
pub fn decorate_mailboxes(db: &Db, mailboxes: &mut [super::Mailbox]) {
    let limits = current_limits(db);
    for mailbox in mailboxes {
        mailbox.sent_last_hour = sent_last_hour(&mailbox.address);
        mailbox.effective_limit = limits.mailbox_limit(&mailbox.address);
    }
}

/// Alan adı listesine gönderim sayısını ve geçerli sınırı işler.
pub fn decorate_domains(db: &Db, domains: &mut [super::MailDomain]) {
    let limits = current_limits(db);
    for domain in domains {
        domain.sent_last_hour = domain_sent_last_hour(&domain.domain);
        domain.effective_limit = limits.domain_limit(&domain.domain);
    }
}

// ---- Sınıra takılanlar ----

#[derive(Debug, Serialize)]
pub struct LimitHit {
    pub id: i64,
    /// `mailbox` ya da `domain`.
    pub kind: String,
    pub address: String,
    pub domain: String,
    pub limit_value: i64,
    pub hit_at: i64,
}

pub fn list_hits(db: &Db, limit: i64) -> AppResult<Vec<LimitHit>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT id, kind, address, domain, limit_value, hit_at FROM mail_limit_hits ORDER BY hit_at DESC, id DESC LIMIT ?1")?;
        stmt.query_map(params![limit], |row| {
            Ok(LimitHit { id: row.get(0)?, kind: row.get(1)?, address: row.get(2)?, domain: row.get(3)?, limit_value: row.get(4)?, hit_at: row.get(5)? })
        })?
        .collect()
    })
}

/// Sınıra takılmayı kaydeder ve kutunun sahibiyle yöneticilere haber verir.
async fn report_hit(state: AppState, scope: Scope, subject: String, limit: i64) {
    let domain = match scope {
        Scope::Mailbox => subject.rsplit_once('@').map(|(_, domain)| domain.to_string()).unwrap_or_default(),
        Scope::Domain => subject.clone(),
    };
    let cutoff = now() - HIT_RETENTION_SECS;
    let owner: Option<i64> = {
        let (kind, subject, domain) = (scope.as_str(), subject.clone(), domain.clone());
        let result = state.db.call(|conn| {
            conn.execute("DELETE FROM mail_limit_hits WHERE hit_at < ?1", params![cutoff])?;
            conn.execute(
                "INSERT INTO mail_limit_hits (kind, address, domain, limit_value, hit_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind, subject, domain, limit, now()],
            )?;
            let detail = format!("{kind}: saatlik sınır {limit}");
            conn.execute(
                "INSERT INTO audit_log (user_id, username, action, target, detail, ip, created_at) VALUES (NULL, 'system', 'mail.limit.hit', ?1, ?2, '', ?3)",
                params![subject, detail, now()],
            )?;
            use rusqlite::OptionalExtension;
            conn.query_row("SELECT user_id FROM mail_domains WHERE domain = ?1", params![domain], |row| row.get(0)).optional()
        });
        match result {
            Ok(owner) => owner,
            Err(err) => {
                tracing::warn!(error = %err, "gönderim sınırı kaydı yazılamadı");
                None
            }
        }
    };
    tracing::warn!(scope = scope.as_str(), subject = %subject, limit, "saatlik gönderim sınırı aşıldı");
    crate::events::publish(crate::events::Event::Changed { area: "mail".into() }, owner);

    let what = match scope {
        Scope::Mailbox => format!("{subject} kutusu"),
        Scope::Domain => format!("{subject} alan adı"),
    };
    let subject_line = format!("PixPanel: {what} saatlik gönderim sınırına takıldı");
    let body = format!(
        "{what} son bir saatte {limit} alıcıya ulaştı ve yeni gönderimler geçici olarak reddediliyor (sınır pencere kaydıkça kendiliğinden açılır). \
         Kutunun parolası ele geçirilmiş ve istenmeyen posta için kullanılıyor olabilir; gönderimleri denetleyin. Sınır meşruysa yöneticiniz Posta sayfasından yükseltebilir."
    );
    let mut recipients: Vec<i64> = state
        .db
        .call(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM users WHERE role = 'admin' AND status = 'active'")?;
            stmt.query_map([], |row| row.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();
    if let Some(owner) = owner {
        if !recipients.contains(&owner) {
            recipients.push(owner);
        }
    }
    for user_id in recipients {
        crate::notify::send_event(&state, user_id, "mail_limit", &subject_line, &body).await;
    }
}

// ---- Policy protokolü ----

/// Postfix'e dönülecek eylem. Yalnızca ASCII (SMTP yanıtına girer).
fn action_for(state: &AppState, attributes: &HashMap<String, String>) -> String {
    const PASS: &str = "DUNNO";
    if attributes.get("request").map(String::as_str) != Some("smtpd_access_policy") {
        return PASS.into();
    }
    if attributes.get("protocol_state").is_some_and(|value| !value.eq_ignore_ascii_case("RCPT")) {
        return PASS.into();
    }
    let user = match attributes.get("sasl_username").map(|value| value.trim().to_ascii_lowercase()) {
        Some(user) if !user.is_empty() && user.len() <= 320 => user,
        // Kimliksiz bağlantılar bu sınırın konusu değil (submission zaten kimlik ister).
        _ => return PASS.into(),
    };
    let limits = current_limits(&state.db);
    let now_secs = unix_now();
    let verdict = {
        let mut guard = lock();
        guard.requests += 1;
        if guard.requests % 5000 == 0 {
            guard.usage.collect_garbage(now_secs);
            let hour = now_secs / 3600;
            guard.reported.retain(|_, reported| *reported >= hour);
        }
        decide(&mut guard.usage, &limits, &user, now_secs)
    };
    match verdict {
        Verdict::Allow => PASS.into(),
        Verdict::Deny { scope, subject, limit } => {
            let hour = now_secs / 3600;
            let first_this_hour = {
                let mut guard = lock();
                let key = format!("{}:{subject}", scope.as_str());
                let seen = guard.reported.get(&key) == Some(&hour);
                guard.reported.insert(key, hour);
                !seen
            };
            if first_this_hour {
                tokio::spawn(report_hit(state.clone(), scope, subject, limit));
            }
            let target = if scope == Scope::Mailbox { "this mailbox" } else { "this domain" };
            let target_tr = if scope == Scope::Mailbox { "Bu kutu" } else { "Bu alan adi" };
            format!("450 4.7.1 {target_tr} saatlik gonderim sinirina ({limit}) ulasti / Hourly sending limit ({limit}) reached for {target}, try again later")
        }
    }
}

async fn handle_connection(stream: UnixStream, state: AppState) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut attributes: HashMap<String, String> = HashMap::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = tokio::time::timeout(IDLE_TIMEOUT, (&mut reader).take(MAX_LINE).read_line(&mut line)).await;
        match read {
            Ok(Ok(count)) if count > 0 && line.ends_with('\n') => {}
            // Bağlantı kapandı, zaman aşımı, okuma hatası ya da aşırı uzun satır.
            _ => return,
        }
        let text = line.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            let reply = format!("action={}\n\n", action_for(&state, &attributes));
            attributes.clear();
            if write_half.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        if attributes.len() >= MAX_ATTRIBUTES {
            return;
        }
        if let Some((name, value)) = text.split_once('=') {
            attributes.insert(name.to_string(), value.to_string());
        }
    }
}

/// Postfix'in kuyruk klasörü (`private/` altındaki soketlere buradan göreli bağlanır).
async fn queue_directory() -> PathBuf {
    match exec::run("postconf", &["-h", "queue_directory"]).await {
        Ok(output) if output.success() && output.stdout.trim().starts_with('/') => PathBuf::from(output.stdout.trim()),
        _ => PathBuf::from(DEFAULT_QUEUE_DIR),
    }
}

/// Soketi açar ve bağlantıları kabul eder; soket kaybolursa ya da hata olursa döner.
async fn serve_once(state: &AppState) -> AppResult<()> {
    let private_dir = queue_directory().await.join("private");
    if !private_dir.is_dir() {
        return Err(AppError::internal("Postfix kuyruk klasörü bulunamadı"));
    }
    let postfix = sysuser::lookup("postfix").ok_or_else(|| AppError::internal("postfix kullanıcısı bulunamadı"))?;
    let path = private_dir.join(SOCKET_NAME);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::os::unix::fs::chown(&path, Some(postfix.uid), Some(postfix.gid))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!(socket = %path.display(), "giden posta sınırı servisi dinliyor");
    loop {
        match tokio::time::timeout(Duration::from_secs(60), listener.accept()).await {
            Ok(Ok((stream, _))) => {
                tokio::spawn(handle_connection(stream, state.clone()));
            }
            Ok(Err(err)) => return Err(err.into()),
            // Soket dosyası silindiyse (Postfix yeniden kurulduysa) yeniden açılır.
            Err(_) if !Path::new(&path).exists() => return Ok(()),
            Err(_) => {}
        }
    }
}

/// Panel açıldığında başlar: posta kuruluysa servisi çalıştırır ve Postfix'i yapılandırır.
pub async fn run(state: AppState) {
    let mut configured = false;
    loop {
        if super::is_installed() {
            if !configured {
                let _guard = state.sys_lock.lock().await;
                match configure_postfix().await {
                    Ok(changed) => {
                        configured = true;
                        if changed {
                            tracing::info!("Postfix giden posta sınırı servisine bağlandı");
                        }
                    }
                    Err(err) => tracing::warn!(error = %err, "Postfix giden posta sınırı için yapılandırılamadı"),
                }
            }
            if let Err(err) = serve_once(&state).await {
                tracing::warn!(error = %err, "giden posta sınırı servisi durdu");
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

// ---- Postfix yapılandırması ----

/// Kimlik doğrulamalı gönderim servislerini policy servisine bağlar. Tekrar çağrılabilir;
/// yapılandırma zaten yerindeyse `false` döner. Servisler henüz tanımlı değilse (posta
/// kurulumu sürüyor) hiçbir şey yapmaz.
pub async fn configure_postfix() -> AppResult<bool> {
    let wanted = format!("check_policy_service {POSTFIX_ENDPOINT},permit");
    let mut changed = false;
    for service in ["submission", "submissions"] {
        let defined = exec::run("postconf", &["-M", &format!("{service}/inet")]).await?;
        if !defined.success() || defined.stdout.trim().is_empty() {
            continue;
        }
        let current = exec::run("postconf", &["-P", &format!("{service}/inet/smtpd_recipient_restrictions")]).await?;
        if current.stdout.contains(SOCKET_NAME) {
            continue;
        }
        let mut options = vec!["-P".to_string()];
        options.extend(
            [
                format!("smtpd_recipient_restrictions={wanted}"),
                // Panel kapalıysa ya da yanıt vermezse posta durmaz.
                "smtpd_policy_service_default_action=DUNNO".to_string(),
                "smtpd_policy_service_timeout=5s".to_string(),
                "smtpd_policy_service_try_limit=1".to_string(),
            ]
            .into_iter()
            .map(|option| format!("{service}/inet/{option}")),
        );
        let args: Vec<&str> = options.iter().map(String::as_str).collect();
        exec::run_ok("postconf", &args).await?;
        changed = true;
    }
    if changed {
        exec::run_ok("postfix", &["check"]).await?;
        if crate::sys::services::is_active("postfix.service").await {
            exec::run_ok("postfix", &["reload"]).await?;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(default_limit: i64) -> Limits {
        Limits { default_limit, ..Limits::default() }
    }

    #[test]
    fn allows_up_to_limit_then_denies() {
        let mut usage = Usage::default();
        let limits = limits(3);
        for _ in 0..3 {
            assert_eq!(decide(&mut usage, &limits, "a@x.test", 1000), Verdict::Allow);
        }
        assert!(matches!(decide(&mut usage, &limits, "a@x.test", 1000), Verdict::Deny { scope: Scope::Mailbox, limit: 3, .. }));
        // Başka kutu etkilenmez.
        assert_eq!(decide(&mut usage, &limits, "b@x.test", 1000), Verdict::Allow);
    }

    #[test]
    fn window_slides() {
        let mut usage = Usage::default();
        let limits = limits(2);
        assert_eq!(decide(&mut usage, &limits, "a@x.test", 0), Verdict::Allow);
        assert_eq!(decide(&mut usage, &limits, "a@x.test", 10), Verdict::Allow);
        assert!(matches!(decide(&mut usage, &limits, "a@x.test", 20), Verdict::Deny { .. }));
        // Bir saat sonra pencere boşalır.
        assert_eq!(decide(&mut usage, &limits, "a@x.test", 3700), Verdict::Allow);
    }

    #[test]
    fn zero_means_unlimited_and_domain_limit_applies() {
        let mut usage = Usage::default();
        let unlimited = limits(0);
        for _ in 0..50 {
            assert_eq!(decide(&mut usage, &unlimited, "a@x.test", 5), Verdict::Allow);
        }
        // Varsayılan 1 ise alan adı sınırı 5; altı farklı kutu birer ileti gönderince alan adı dolar.
        let mut usage = Usage::default();
        let tight = limits(1);
        for index in 0..5 {
            assert_eq!(decide(&mut usage, &tight, &format!("k{index}@x.test"), 5), Verdict::Allow);
        }
        assert!(matches!(decide(&mut usage, &tight, "k9@x.test", 5), Verdict::Deny { scope: Scope::Domain, limit: 5, .. }));
    }

    #[test]
    fn mailbox_override_beats_default() {
        let mut usage = Usage::default();
        let mut custom = limits(1);
        custom.mailboxes.insert("a@x.test".into(), Some(0));
        for _ in 0..4 {
            assert_eq!(decide(&mut usage, &custom, "a@x.test", 5), Verdict::Allow);
        }
    }
}
