//! Uygulama portlarının korunması.
//!
//! Node.js ve Python siteleri yerel bir portu dinler; web sunucusu ziyaretçi isteklerini o porta
//! iletir. Linux'ta 1024 üstü bir portu her kullanıcı açabilir: uygulama durmuşken (çöktüğünde,
//! yeniden başlarken) sunucudaki başka bir hesap o portu önce açarsa ziyaretçilerin isteklerini
//! (çerezler, parolalar) kendi sürecine çeker.
//!
//! Koruma systemd'nin port bağlama kısıtıyla sağlanır: müşteri kodunun çalışabildiği her birimde
//! (SSH oturumları, zamanlı görevler, PHP havuzları, panelin başlattığı komutlar, hesap Redis'leri)
//! uygulama port aralığını bağlamak yasaktır; her uygulama birimi yalnızca kendi portunu açabilir.
//!
//! Kısıt çekirdeğin cgroup-BPF desteğine dayanır. Yetkisiz konteynerlerde (LXC) çekirdek buna
//! izin vermez; systemd ayarı sessizce yok sayar. Bu yüzden desteğin gerçekten çalıştığı açılışta
//! denenir ve çalışmıyorsa sağlık merkezinde bildirilir.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::sys::exec;

/// Uygulamalara verilen port aralığı (`node::PORT_BASE` + site numarası bu aralıkta kalır).
pub const APP_PORT_RANGE: &str = "tcp:20000-29999";
/// Desteği denemek için kullanılan, uygulamalara verilmeyen port.
const PROBE_PORT: u16 = 19999;

const UNKNOWN: u8 = 0;
const SUPPORTED: u8 = 1;
const UNSUPPORTED: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Kısıt bu sunucuda çalışıyor mu? Açılıştaki deneme bitmeden `None` döner.
pub fn supported() -> Option<bool> {
    match STATE.load(Ordering::Relaxed) {
        SUPPORTED => Some(true),
        UNSUPPORTED => Some(false),
        _ => None,
    }
}

/// Uygulama biriminin kendi dosyasına yazılan satırlar: yalnızca kendi portu serbesttir.
pub fn unit_lines(port: i64) -> String {
    format!("SocketBindAllow=tcp:{port}\nSocketBindDeny={APP_PORT_RANGE}\n")
}

/// Uygulama portu açması gerekmeyen, ama müşteri kodunun çalışabildiği birimlerin satırı.
pub fn deny_line() -> String {
    format!("SocketBindDeny={APP_PORT_RANGE}\n")
}

/// Kısıtın uygulanacağı ortak birimler: oturumlar, zamanlı görevler, panelin kendisi (hesap adına
/// çalıştırdığı komutlar onun altındadır) ve kurulu PHP havuz servisleri.
async fn shared_units() -> Vec<String> {
    let mut units = vec!["user.slice".to_string(), "cron.service".to_string(), "pixpanel.service".to_string()];
    if let Ok(output) = exec::run("systemctl", &["list-unit-files", "--no-legend", "--plain", "--type=service", "php*-fpm.service"]).await {
        for line in output.stdout.lines() {
            if let Some(name) = line.split_whitespace().next().filter(|name| name.starts_with("php") && name.ends_with("-fpm.service")) {
                units.push(name.to_string());
            }
        }
    }
    units
}

/// Birimde kural zaten tanımlı mı? `set-property` liste ayarlarına ekleme yapar; denetlenmezse
/// kural her açılışta bir kez daha yazılır.
async fn already_denied(unit: &str) -> bool {
    let range = APP_PORT_RANGE.trim_start_matches("tcp:");
    matches!(exec::run("systemctl", &["show", "--property=SocketBindDeny", "--value", "--", unit]).await, Ok(output) if output.stdout.contains(range))
}

/// Kısıtı ortak birimlere uygular (yineleme zararsızdır) ve desteğin çalıştığını dener.
/// Panel açılırken ve yeni bir PHP sürümü kurulduktan sonra çağrılır.
pub async fn apply() {
    let property = format!("SocketBindDeny={APP_PORT_RANGE}");
    for unit in shared_units().await {
        if already_denied(&unit).await {
            continue;
        }
        // `set-property` ayarı kalıcı yazar ve çalışan birime hemen uygular.
        match exec::run("systemctl", &["set-property", "--", &unit, &property]).await {
            Ok(output) if output.success() => {}
            Ok(output) => tracing::debug!(unit = %unit, "port kısıtı uygulanamadı: {}", output.stderr.trim()),
            Err(err) => tracing::debug!(unit = %unit, error = %err, "port kısıtı uygulanamadı"),
        }
    }
    // Kısıt gelmeden önce kurulmuş uygulama ve hesap Redis birimleri de (yeniden başlatılmadan)
    // aynı kurala bağlanır; yeni kurulanların kuralı zaten birim dosyasındadır.
    if let Ok(entries) = std::fs::read_dir("/etc/systemd/system") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("pixpanel-") || already_denied(&name).await {
                continue;
            }
            if let Some(id) = name.strip_prefix("pixpanel-node-").and_then(|rest| rest.strip_suffix(".service")).and_then(|id| id.parse::<i64>().ok()) {
                let allow = format!("SocketBindAllow=tcp:{}", crate::hosting::node::port(id));
                let _ = exec::run("systemctl", &["set-property", "--", &name, &allow, &property]).await;
            } else if name.starts_with("pixpanel-redis-") && name.ends_with(".service") {
                let _ = exec::run("systemctl", &["set-property", "--", &name, &property]).await;
            }
        }
    }
    let works = probe().await;
    STATE.store(if works { SUPPORTED } else { UNSUPPORTED }, Ordering::Relaxed);
    if works {
        tracing::info!("uygulama portları korunuyor");
    } else {
        tracing::warn!("bu ortamda port bağlama kısıtı desteklenmiyor; uygulama portları korunamıyor");
    }
}

/// Kısıt gerçekten işliyor mu? Geçici bir birimde, yasaklanmış bir portu açmayı dener.
async fn probe() -> bool {
    let Ok(binary) = std::env::current_exe() else { return false };
    let binary = binary.to_string_lossy().trim_end_matches(" (deleted)").to_string();
    let property = format!("SocketBindDeny=tcp:{PROBE_PORT}");
    let port = PROBE_PORT.to_string();
    let args = ["--wait", "--quiet", "--collect", "-p", &property, "--", &binary, "probe-bind", &port];
    // Yardımcı komut, bağlama reddedilirse 0 ile çıkar.
    matches!(exec::run("systemd-run", &args).await, Ok(output) if output.success())
}

/// `pixpanel probe-bind <port>`: portu açmayı dener. Açamazsa (kısıt işliyor) 0, açarsa 1 döner.
pub fn run_probe(port: u16) -> i32 {
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => 1,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => 0,
        // Port başka bir nedenle açılamadı (ör. kullanımda): destek kanıtlanmadı.
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_lines_allow_only_own_port() {
        assert_eq!(unit_lines(20007), "SocketBindAllow=tcp:20007\nSocketBindDeny=tcp:20000-29999\n");
        assert_eq!(deny_line(), "SocketBindDeny=tcp:20000-29999\n");
    }
}
