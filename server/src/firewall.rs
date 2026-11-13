//! Güvenlik duvarı: nftables üzerinde panelin kendi tablosu (`inet pixpanel`).
//!
//! Panel yalnızca kendi tablosuna dokunur; Docker'ın ve yöneticinin elle
//! yazdığı kurallar olduğu gibi kalır. Tabloda tek bir giriş zinciri vardır:
//! izin verilmeyen her bağlantı düşürülür. Konteynerlere yönlenen trafik giriş
//! zincirinden geçmediğinden Docker'ın dışarıya açtığı portlar bu kurallardan
//! etkilenmez.
//!
//! Kilitlenmeye karşı iki önlem vardır: yeni kurallar isteği yapanın panele
//! erişimini kesiyorsa hiç uygulanmaz; uygulanan kurallar da belirli bir süre
//! içinde onaylanmazsa kendiliğinden eski hâline döner.

use std::net::IpAddr;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, ftp};
use crate::state::AppState;
use crate::sys::exec;

const SETTING_KEY: &str = "firewall";
/// Onaylanmış kuralların yazıldığı dosya; açılışta bu dosya yüklenir.
const RULES_FILE: &str = "/etc/pixpanel/firewall.nft";
const UNIT_FILE: &str = "/etc/systemd/system/pixpanel-firewall.service";
const UNIT_NAME: &str = "pixpanel-firewall.service";
/// Uygulanan kuralların onaylanması için tanınan süre.
pub const CONFIRM_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub label: String,
    /// `tcp`, `udp` ya da `both`.
    pub protocol: String,
    /// Port ya da aralıklar: `8080`, `3000-3010`, `80,443`.
    pub ports: String,
    /// Boşsa her adres; yoksa tek adres ya da ağ (`203.0.113.0/24`).
    #[serde(default)]
    pub source: String,
    /// `accept` ya da `drop`.
    pub action: String,
}

/// Hazır kural kümesinin (SSH, web, posta...) durumu.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresetState {
    pub key: String,
    pub enabled: bool,
    /// Boşsa her adresten erişilir.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub presets: Vec<PresetState>,
    /// Hazır kümelerden önce, sırayla değerlendirilen özel kurallar.
    pub rules: Vec<Rule>,
    /// Bütün portlara erişimi engellenen adresler ve ağlar.
    pub blocked: Vec<String>,
}

/// Kurulu bir bileşenin dışarıdan erişilmesi gereken portları.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub tcp: Vec<String>,
    pub udp: Vec<String>,
    /// Kapatılırsa yönetici sunucuya ya da panele erişemeyebilir.
    pub critical: bool,
}

fn ssh_ports() -> Vec<String> {
    let output = std::process::Command::new("sshd").arg("-T").output();
    let ports: Vec<String> = output
        .ok()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.strip_prefix("port "))
                .filter(|port| port.bytes().all(|b| b.is_ascii_digit()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if ports.is_empty() { vec!["22".into()] } else { ports }
}

/// Sunucuda kurulu bileşenlere göre hazır kural kümeleri.
pub fn presets(cfg: &crate::config::Config) -> Vec<Preset> {
    let list = |ports: &[&str]| ports.iter().map(|port| port.to_string()).collect::<Vec<_>>();
    let mut presets = vec![
        Preset { key: "ssh", label: "SSH", description: "Sunucuya uzaktan komut satırı erişimi.", tcp: ssh_ports(), udp: Vec::new(), critical: true },
        Preset {
            key: "panel",
            label: "PixPanel",
            description: "Bu panelin kendisi ve hazır araçları (phpMyAdmin, Adminer, web postası).",
            tcp: vec![cfg.listen_port.to_string(), cfg.tools_port.to_string()],
            udp: Vec::new(),
            critical: true,
        },
    ];
    if exec::binary_exists("nginx") {
        presets.push(Preset { key: "web", label: "Web siteleri", description: "HTTP ve HTTPS.", tcp: list(&["80", "443"]), udp: list(&["443"]), critical: false });
    }
    if crate::mail::is_installed() {
        presets.push(Preset {
            key: "mail",
            label: "Posta",
            description: "SMTP, gönderim, IMAP ve filtre yönetimi.",
            tcp: list(&["25", "465", "587", "143", "993", "4190"]),
            udp: Vec::new(),
            critical: false,
        });
    }
    if crate::dns::local::is_installed() {
        presets.push(Preset { key: "dns", label: "DNS sunucusu", description: "Alan adı sorguları.", tcp: list(&["53"]), udp: list(&["53"]), critical: false });
    }
    if ftp::is_installed() {
        presets.push(Preset {
            key: "ftp",
            label: "FTP",
            description: "Denetim bağlantısı ve pasif veri portları.",
            tcp: vec!["21".into(), format!("{}-{}", ftp::PASSIVE_PORTS.0, ftp::PASSIVE_PORTS.1)],
            udp: Vec::new(),
            critical: false,
        });
    }
    presets
}

// ---- Doğrulama ----

/// Adres ya da ağ; nftables'a yazılacak biçimi ve IPv6 olup olmadığını döner.
fn parse_source(value: &str) -> AppResult<(String, bool)> {
    let source = value.trim();
    let invalid = || AppError::bad(format!("Geçersiz adres ya da ağ: {source} (örnek: 203.0.113.5 ya da 203.0.113.0/24)"));
    let (address, prefix) = match source.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix.parse::<u8>().map_err(|_| invalid())?)),
        None => (source, None),
    };
    let ip: IpAddr = address.parse().map_err(|_| invalid())?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if prefix.is_some_and(|bits| bits > max) {
        return Err(invalid());
    }
    Ok((match prefix {
        Some(bits) => format!("{ip}/{bits}"),
        None => ip.to_string(),
    }, ip.is_ipv6()))
}

/// `80,443,3000-3010` biçimindeki port listesini doğrular ve nftables kümesine çevirir.
fn parse_ports(value: &str) -> AppResult<Vec<(u16, u16)>> {
    let invalid = || AppError::bad(format!("Geçersiz port: {value} (örnek: 8080 ya da 3000-3010 ya da 80,443)"));
    let mut ranges = Vec::new();
    for part in value.split(',').map(str::trim).filter(|part| !part.is_empty()) {
        let (from, to) = part.split_once('-').unwrap_or((part, part));
        let (from, to): (u16, u16) = (from.trim().parse().map_err(|_| invalid())?, to.trim().parse().map_err(|_| invalid())?);
        if from == 0 || from > to {
            return Err(invalid());
        }
        ranges.push((from, to));
    }
    if ranges.is_empty() || ranges.len() > 30 {
        return Err(invalid());
    }
    Ok(ranges)
}

fn port_set(ranges: &[(u16, u16)]) -> String {
    let items: Vec<String> = ranges.iter().map(|(from, to)| if from == to { from.to_string() } else { format!("{from}-{to}") }).collect();
    format!("{{ {} }}", items.join(", "))
}

impl Settings {
    /// Ayarları doğrular ve bilinen hazır kümelerle eşitler: yeni kurulan bir
    /// bileşenin kümesi açık olarak eklenir, kaldırılanınki atılır.
    pub fn normalized(mut self, available: &[Preset]) -> AppResult<Self> {
        self.presets = available
            .iter()
            .map(|preset| {
                self.presets.iter().find(|state| state.key == preset.key).cloned().unwrap_or(PresetState { key: preset.key.to_string(), enabled: true, source: String::new() })
            })
            .collect();
        for state in &mut self.presets {
            state.source = if state.source.trim().is_empty() { String::new() } else { parse_source(&state.source)?.0 };
        }
        if self.rules.len() > 200 {
            return Err(AppError::bad("En fazla 200 özel kural tanımlanabilir"));
        }
        for rule in &mut self.rules {
            rule.label = rule.label.trim().chars().filter(|c| !c.is_control()).take(80).collect();
            if !matches!(rule.protocol.as_str(), "tcp" | "udp" | "both") {
                return Err(AppError::bad("Geçersiz protokol"));
            }
            if !matches!(rule.action.as_str(), "accept" | "drop") {
                return Err(AppError::bad("Geçersiz kural türü"));
            }
            parse_ports(&rule.ports)?;
            rule.source = if rule.source.trim().is_empty() { String::new() } else { parse_source(&rule.source)?.0 };
        }
        if self.blocked.len() > 5000 {
            return Err(AppError::bad("Engelli adres listesi 5000 kaydı aşamaz"));
        }
        let mut blocked = Vec::new();
        for entry in self.blocked.iter().filter(|entry| !entry.trim().is_empty()) {
            let (source, _) = parse_source(entry)?;
            if !blocked.contains(&source) {
                blocked.push(source);
            }
        }
        self.blocked = blocked;
        Ok(self)
    }
}

pub fn load(db: &Db, cfg: &crate::config::Config) -> Settings {
    let saved: Settings = db.setting(SETTING_KEY).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
    // Kayıtlı ayarlar daha önce doğrulanmıştır; yalnızca hazır kümeler güncellenir.
    saved.clone().normalized(&presets(cfg)).unwrap_or(saved)
}

fn save(db: &Db, settings: &Settings) -> AppResult<()> {
    db.set_setting(SETTING_KEY, &serde_json::to_string(settings).map_err(|err| AppError::internal(err.to_string()))?)
}

// ---- Kural metni ----

fn source_match(source: &str) -> String {
    if source.is_empty() {
        return String::new();
    }
    let family = if source.contains(':') { "ip6" } else { "ip" };
    format!("{family} saddr {source} ")
}

/// nftables'a yüklenecek kural dosyası. Girdiler doğrulanmış olmalıdır.
pub fn render(settings: &Settings, available: &[Preset]) -> String {
    let mut out = String::from(
        "# PixPanel tarafından üretildi. Elle yapılan değişiklikler kaybolur.\n\
         table inet pixpanel\n\
         delete table inet pixpanel\n",
    );
    if !settings.enabled {
        return out;
    }
    let (blocked4, blocked6): (Vec<&String>, Vec<&String>) = settings.blocked.iter().partition(|entry| !entry.contains(':'));
    let elements = |entries: &[&String]| {
        if entries.is_empty() { String::new() } else { format!("        elements = {{ {} }}\n", entries.iter().map(|entry| entry.as_str()).collect::<Vec<_>>().join(", ")) }
    };
    out.push_str("table inet pixpanel {\n");
    out.push_str(&format!("    set blocked4 {{\n        type ipv4_addr\n        flags interval\n{}    }}\n", elements(&blocked4)));
    out.push_str(&format!("    set blocked6 {{\n        type ipv6_addr\n        flags interval\n{}    }}\n", elements(&blocked6)));
    // fail2ban'in engellediği adresler bu kümelere eklenir.
    out.push_str("    set banned4 {\n        type ipv4_addr\n        flags timeout\n    }\n");
    out.push_str("    set banned6 {\n        type ipv6_addr\n        flags timeout\n    }\n");
    out.push_str(
        "    chain input {\n\
         \x20       type filter hook input priority filter; policy drop;\n\
         \x20       ct state established,related accept\n\
         \x20       ct state invalid drop\n\
         \x20       iif \"lo\" accept\n\
         \x20       meta l4proto { icmp, ipv6-icmp } accept\n\
         \x20       ip saddr @blocked4 drop\n\
         \x20       ip6 saddr @blocked6 drop\n\
         \x20       ip saddr @banned4 drop\n\
         \x20       ip6 saddr @banned6 drop\n",
    );
    for rule in &settings.rules {
        let Ok(ranges) = parse_ports(&rule.ports) else { continue };
        let protocols: &[&str] = match rule.protocol.as_str() {
            "tcp" => &["tcp"],
            "udp" => &["udp"],
            _ => &["tcp", "udp"],
        };
        for protocol in protocols {
            out.push_str(&format!("        {}{protocol} dport {} {}\n", source_match(&rule.source), port_set(&ranges), rule.action));
        }
    }
    for preset in available {
        let Some(state) = settings.presets.iter().find(|state| state.key == preset.key && state.enabled) else { continue };
        for (protocol, ports) in [("tcp", &preset.tcp), ("udp", &preset.udp)] {
            if !ports.is_empty() {
                out.push_str(&format!("        {}{protocol} dport {{ {} }} accept\n", source_match(&state.source), ports.join(", ")));
            }
        }
    }
    out.push_str("    }\n}\n");
    out
}

// ---- Kilitlenme denetimi ----

fn source_covers(source: &str, ip: IpAddr) -> bool {
    if source.is_empty() {
        return true;
    }
    let (address, prefix) = source.split_once('/').unwrap_or((source, ""));
    let Ok(network) = address.parse::<IpAddr>() else { return false };
    match (network, ip) {
        (IpAddr::V4(network), IpAddr::V4(ip)) => {
            let bits: u32 = prefix.parse().unwrap_or(32);
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(network) & mask == u32::from(ip) & mask
        }
        (IpAddr::V6(network), IpAddr::V6(ip)) => {
            let bits: u32 = prefix.parse().unwrap_or(128);
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(network) & mask == u128::from(ip) & mask
        }
        _ => false,
    }
}

/// Verilen adresten gelen TCP bağlantısı bu ayarlarla kabul edilir mi?
/// Kuralların değerlendirilme sırası `render` ile aynıdır.
pub fn allows(settings: &Settings, available: &[Preset], ip: IpAddr, port: u16) -> bool {
    if !settings.enabled || ip.is_loopback() {
        return true;
    }
    if settings.blocked.iter().any(|entry| source_covers(entry, ip)) {
        return false;
    }
    for rule in &settings.rules {
        let matches_port = parse_ports(&rule.ports).is_ok_and(|ranges| ranges.iter().any(|(from, to)| (*from..=*to).contains(&port)));
        if rule.protocol != "udp" && matches_port && source_covers(&rule.source, ip) {
            return rule.action == "accept";
        }
    }
    available.iter().any(|preset| {
        settings.presets.iter().any(|state| state.key == preset.key && state.enabled && source_covers(&state.source, ip))
            && preset.tcp.iter().any(|ports| parse_ports(ports).is_ok_and(|ranges| ranges.iter().any(|(from, to)| (*from..=*to).contains(&port))))
    })
}

// ---- Uygulama ve onay ----

struct Pending {
    settings: Settings,
    deadline: Instant,
    /// Aynı anda tek bir bekleyen değişiklik olur; sayaç eski zamanlayıcıları geçersiz kılar.
    generation: u64,
}

static PENDING: LazyLock<Mutex<Option<Pending>>> = LazyLock::new(Mutex::default);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pending() -> std::sync::MutexGuard<'static, Option<Pending>> {
    PENDING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn is_available() -> bool {
    exec::binary_exists("nft")
}

/// Kural metnini önce sınar, sonra tek adımda yükler.
async fn load_ruleset(text: &str) -> AppResult<()> {
    let path = Path::new("/etc/pixpanel/firewall.candidate.nft");
    atomic_write(path, text.as_bytes(), 0o600)?;
    let path_str = path.to_string_lossy().into_owned();
    let check = exec::run("nft", &["-c", "-f", &path_str]).await?;
    if !check.success() {
        let _ = std::fs::remove_file(path);
        return Err(AppError::bad(format!("Kurallar geçersiz: {}", check.stderr.trim())));
    }
    let applied = exec::run("nft", &["-f", &path_str]).await?;
    let _ = std::fs::remove_file(path);
    if !applied.success() {
        return Err(AppError::internal(format!("Kurallar uygulanamadı: {}", applied.stderr.trim())));
    }
    Ok(())
}

/// Onaylanmış kuralları diske yazar ve açılışta yüklenmelerini sağlar.
async fn persist(settings: &Settings, text: &str) -> AppResult<()> {
    atomic_write(Path::new(RULES_FILE), text.as_bytes(), 0o600)?;
    let unit = format!(
        "# PixPanel tarafından üretildi.\n\
         [Unit]\n\
         Description=PixPanel güvenlik duvarı kuralları\n\
         Wants=network-pre.target\n\
         Before=network-pre.target\n\n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart=/usr/sbin/nft -f {RULES_FILE}\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );
    if std::fs::read_to_string(UNIT_FILE).ok().as_deref() != Some(unit.as_str()) {
        atomic_write(Path::new(UNIT_FILE), unit.as_bytes(), 0o644)?;
        exec::run_ok("systemctl", &["daemon-reload"]).await?;
    }
    let action = if settings.enabled { "enable" } else { "disable" };
    exec::run_ok("systemctl", &[action, "--quiet", "--", UNIT_NAME]).await?;
    Ok(())
}

/// Yeni ayarları uygular; `CONFIRM_SECS` içinde `confirm` çağrılmazsa eski ayarlara dönülür.
pub async fn apply_pending(state: &AppState, settings: Settings) -> AppResult<()> {
    let available = presets(&state.cfg);
    load_ruleset(&render(&settings, &available)).await?;
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    *pending() = Some(Pending { settings, deadline: Instant::now() + Duration::from_secs(CONFIRM_SECS), generation });

    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(CONFIRM_SECS)).await;
        let expired = pending().as_ref().is_some_and(|waiting| waiting.generation == generation);
        if expired {
            tracing::warn!("güvenlik duvarı değişikliği onaylanmadı; önceki kurallara dönülüyor");
            if let Err(err) = revert(&state).await {
                tracing::error!(error = %err, "güvenlik duvarı önceki kurallara döndürülemedi");
            }
        }
    });
    Ok(())
}

/// Bekleyen değişikliği kalıcı yapar.
pub async fn confirm(state: &AppState) -> AppResult<()> {
    let settings = pending().take().map(|waiting| waiting.settings).ok_or_else(|| AppError::bad("Onay bekleyen bir değişiklik yok (süre dolmuş olabilir)"))?;
    save(&state.db, &settings)?;
    persist(&settings, &render(&settings, &presets(&state.cfg))).await
}

/// Bekleyen değişikliği atar ve kayıtlı kuralları yeniden yükler.
pub async fn revert(state: &AppState) -> AppResult<()> {
    pending().take();
    let settings = load(&state.db, &state.cfg);
    load_ruleset(&render(&settings, &presets(&state.cfg))).await
}

/// Onay bekleyen ayarlar ve kalan süre (saniye).
pub fn pending_state() -> Option<(Settings, u64)> {
    pending().as_ref().map(|waiting| (waiting.settings.clone(), waiting.deadline.saturating_duration_since(Instant::now()).as_secs()))
}

/// Kayıtlı kuralları yeniden üretip yükler. Panel açılışında ve bir bileşen
/// kurulduktan sonra (yeni portların açılması için) çağrılır; onay gerektirmez.
pub async fn reapply(state: &AppState) -> AppResult<()> {
    if !is_available() || pending().is_some() {
        return Ok(());
    }
    let settings = load(&state.db, &state.cfg);
    if !settings.enabled {
        return Ok(());
    }
    let text = render(&settings, &presets(&state.cfg));
    load_ruleset(&text).await?;
    save(&state.db, &settings)?;
    persist(&settings, &text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available() -> Vec<Preset> {
        vec![
            Preset { key: "ssh", label: "SSH", description: "", tcp: vec!["22".into()], udp: vec![], critical: true },
            Preset { key: "panel", label: "Panel", description: "", tcp: vec!["8443".into()], udp: vec![], critical: true },
        ]
    }

    fn rule(ports: &str, source: &str, action: &str) -> Rule {
        Rule { label: String::new(), protocol: "tcp".into(), ports: ports.into(), source: source.into(), action: action.into() }
    }

    #[test]
    fn rejects_injection_in_rule_fields() {
        for (ports, source) in [("80; flush ruleset", ""), ("80 accept\n", ""), ("0", ""), ("90-80", ""), ("80", "1.2.3.4; drop"), ("80", "1.2.3.4/40"), ("80", "ornek.com")] {
            let settings = Settings { enabled: true, rules: vec![rule(ports, source, "accept")], ..Settings::default() };
            assert!(settings.normalized(&available()).is_err(), "{ports} / {source}");
        }
        let bad_block = Settings { enabled: true, blocked: vec!["1.2.3.4 }".into()], ..Settings::default() };
        assert!(bad_block.normalized(&available()).is_err());
    }

    #[test]
    fn lockout_check_follows_rule_order() {
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        let base = Settings { enabled: true, ..Settings::default() }.normalized(&available()).unwrap();
        assert!(allows(&base, &available(), ip, 8443));
        assert!(!allows(&base, &available(), ip, 3306));

        let mut restricted = base.clone();
        restricted.presets.iter_mut().find(|state| state.key == "panel").unwrap().source = "198.51.100.0/24".into();
        assert!(!allows(&restricted, &available(), ip, 8443));
        assert!(allows(&restricted, &available(), "198.51.100.7".parse().unwrap(), 8443));

        let mut blocked = base.clone();
        blocked.blocked = vec!["203.0.113.0/24".into()];
        assert!(!allows(&blocked, &available(), ip, 8443));

        let mut dropped = base.clone();
        dropped.rules = vec![rule("8000-9000", "", "drop")];
        assert!(!allows(&dropped, &available(), ip, 8443));

        let disabled = Settings::default();
        assert!(allows(&disabled, &available(), ip, 3306));
    }

    #[test]
    fn rendered_ruleset_shape() {
        let settings = Settings {
            enabled: true,
            rules: vec![Rule { label: "x".into(), protocol: "both".into(), ports: "3000-3010,8080".into(), source: "2001:db8::/32".into(), action: "accept".into() }],
            blocked: vec!["198.51.100.9".into(), "2001:db8:1::1".into()],
            ..Settings::default()
        }
        .normalized(&available())
        .unwrap();
        let text = render(&settings, &available());
        assert!(text.contains("ip6 saddr 2001:db8::/32 tcp dport { 3000-3010, 8080 } accept"), "{text}");
        assert!(text.contains("ip6 saddr 2001:db8::/32 udp dport { 3000-3010, 8080 } accept"));
        assert!(text.contains("elements = { 198.51.100.9 }") && text.contains("elements = { 2001:db8:1::1 }"));
        assert!(text.contains("tcp dport { 22 } accept") && text.contains("policy drop"));
        assert_eq!(render(&Settings::default(), &available()).lines().count(), 3);
    }
}
