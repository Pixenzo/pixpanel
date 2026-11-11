//! Sunucunun IP adresleri: var olanların bulunması, yenisinin eklenmesi, hesaba ayrılması
//! ve sitelerin belirli bir adrese bağlanması.
//!
//! Adresin kaynağı işletim sistemidir (`ip -j addr`); veritabanındaki satır yalnızca panelin
//! o adres hakkında bildiklerini tutar (etiket, kime ayrıldığı, dışarıdan görünen adresi ve
//! adresi panelin ekleyip eklemediği). Panelin eklediği adresler açılışta yeniden kurulur:
//! ağ ayarı dağıtıma ve sanallaştırmaya göre değiştiği için panel onların dosyalarına dokunmaz.

use std::net::IpAddr;
use std::path::Path;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::sys::exec;

const BOOT_UNIT: &str = "pixpanel-addresses.service";
const BOOT_UNIT_PATH: &str = "/etc/systemd/system/pixpanel-addresses.service";

/// İşletim sisteminde tanımlı bir adres.
#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    pub address: IpAddr,
    pub prefix: u8,
    pub interface: String,
}

/// Panelin bir adres hakkında tuttuğu bilgiyle birleştirilmiş görünüm.
#[derive(Debug, Serialize)]
pub struct Address {
    pub address: String,
    pub prefix: u8,
    pub interface: String,
    pub family: &'static str,
    /// Adres şu an sunucuda tanımlı mı? Panelin eklediği adres açılışta kurulamadıysa `false` olur.
    pub present: bool,
    /// Adresi panel ekledi; kaldırılabilir ve açılışta yeniden kurulur.
    pub managed: bool,
    pub label: String,
    /// Sunucu bir yönlendiricinin arkasındaysa adresin internetten görünen karşılığı (DNS'e bu yazılır).
    pub public_address: String,
    /// Adres bir hesaba ayrıldıysa o hesap; değilse yalnızca yönetici kullanabilir.
    pub user_id: Option<i64>,
    pub owner: Option<String>,
    pub site_count: i64,
}

/// Konteyner köprüleri ve sanal uçlar barındırma için kullanılmaz.
fn hosting_interface(name: &str) -> bool {
    !(name == "lo" || name.starts_with("docker") || name.starts_with("br-") || name.starts_with("veth") || name.starts_with("virbr"))
}

/// `ip -j addr show` çıktısından barındırmada kullanılabilecek adresleri çıkarır.
pub fn parse_ip_json(raw: &str) -> Vec<Detected> {
    let Ok(serde_json::Value::Array(links)) = serde_json::from_str(raw) else { return Vec::new() };
    let mut found = Vec::new();
    for link in &links {
        let interface = link["ifname"].as_str().unwrap_or("");
        if !hosting_interface(interface) {
            continue;
        }
        for info in link["addr_info"].as_array().into_iter().flatten() {
            // Yalnızca genel kapsamlı adresler: bağlantıya özel (fe80::) ve geri döngü adresleri dışarıda kalır.
            if info["scope"].as_str() != Some("global") {
                continue;
            }
            let Some(address) = info["local"].as_str().and_then(|text| text.parse::<IpAddr>().ok()) else { continue };
            // Geçici IPv6 adresleri kendiliğinden değişir; siteye bağlanamaz.
            if info["temporary"].as_bool() == Some(true) {
                continue;
            }
            let prefix = info["prefixlen"].as_u64().unwrap_or(0) as u8;
            found.push(Detected { address, prefix, interface: interface.to_string() });
        }
    }
    found
}

pub async fn detect() -> Vec<Detected> {
    match exec::run("ip", &["-j", "addr", "show"]).await {
        Ok(output) if output.success() => parse_ip_json(&output.stdout),
        _ => Vec::new(),
    }
}

fn family(address: &IpAddr) -> &'static str {
    if address.is_ipv4() { "ipv4" } else { "ipv6" }
}

/// Adres metnini doğrular ve tek biçime indirger (IPv6'nın kısa yazımı).
pub fn normalize(text: &str) -> AppResult<IpAddr> {
    let address: IpAddr = text.trim().parse().map_err(|_| AppError::bad("Geçerli bir IP adresi girin"))?;
    let special = match address {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80,
    };
    if special {
        return Err(AppError::bad("Bu adres barındırma için kullanılamaz"));
    }
    Ok(address)
}

struct Row {
    address: String,
    prefix: u8,
    interface: String,
    managed: bool,
    label: String,
    public_address: String,
    user_id: Option<i64>,
    owner: Option<String>,
}

fn rows(db: &Db) -> AppResult<Vec<Row>> {
    db.call(|conn| {
        let mut stmt = conn.prepare(
            "SELECT a.address, a.prefix, a.interface, a.managed, a.label, a.public_address, a.user_id, u.username
             FROM server_ips a LEFT JOIN users u ON u.id = a.user_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Row {
                address: row.get(0)?,
                prefix: row.get(1)?,
                interface: row.get(2)?,
                managed: row.get(3)?,
                label: row.get(4)?,
                public_address: row.get(5)?,
                user_id: row.get(6)?,
                owner: row.get(7)?,
            })
        })?;
        rows.collect()
    })
}

fn site_counts(db: &Db) -> AppResult<std::collections::HashMap<String, i64>> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT ip, count(*) FROM sites WHERE ip != '' GROUP BY ip")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        rows.collect()
    })
}

/// Sunucudaki adresler ile panelin kayıtlarının birleşimi; önce IPv4, sonra adres sırasıyla.
pub async fn list(db: &Db) -> AppResult<Vec<Address>> {
    let detected = detect().await;
    let mut known = rows(db)?;
    let counts = site_counts(db)?;
    let mut out = Vec::new();
    for found in &detected {
        let text = found.address.to_string();
        let row = known.iter().position(|row| row.address == text).map(|index| known.swap_remove(index));
        out.push(Address {
            site_count: counts.get(&text).copied().unwrap_or(0),
            address: text,
            prefix: found.prefix,
            interface: found.interface.clone(),
            family: family(&found.address),
            present: true,
            managed: row.as_ref().is_some_and(|row| row.managed),
            label: row.as_ref().map(|row| row.label.clone()).unwrap_or_default(),
            public_address: row.as_ref().map(|row| row.public_address.clone()).unwrap_or_default(),
            user_id: row.as_ref().and_then(|row| row.user_id),
            owner: row.and_then(|row| row.owner),
        });
    }
    // Kaydı olup sunucuda bulunmayanlar: panelin eklediği ama şu an kurulu olmayan adresler.
    for row in known.into_iter().filter(|row| row.managed) {
        let Ok(address) = row.address.parse::<IpAddr>() else { continue };
        out.push(Address {
            site_count: counts.get(&row.address).copied().unwrap_or(0),
            family: family(&address),
            address: row.address,
            prefix: row.prefix,
            interface: row.interface,
            present: false,
            managed: true,
            label: row.label,
            public_address: row.public_address,
            user_id: row.user_id,
            owner: row.owner,
        });
    }
    out.sort_by_key(|item| (item.family == "ipv6", item.address.parse::<IpAddr>().ok()));
    Ok(out)
}

/// Sunucuya yeni adres ekler ve açılışta yeniden kurulacak şekilde kaydeder.
pub async fn add(db: &Db, address: &str, prefix: u8, interface: Option<&str>, label: &str) -> AppResult<()> {
    let address = normalize(address)?;
    let max_prefix = if address.is_ipv4() { 32 } else { 128 };
    if prefix == 0 || prefix > max_prefix {
        return Err(AppError::bad(format!("Ağ öneki 1 ile {max_prefix} arasında olmalı")));
    }
    let label = validate_label(label)?;
    let detected = detect().await;
    if detected.iter().any(|found| found.address == address) {
        return Err(AppError::conflict("Bu adres sunucuda zaten tanımlı"));
    }
    // Arayüz yalnızca sunucuda gerçekten var olanlardan seçilebilir; verilmezse ilk adresin arayüzü.
    let interface = match interface.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => detected.iter().find(|found| found.interface == name).map(|found| found.interface.clone()).ok_or_else(|| AppError::bad("Böyle bir ağ arayüzü yok"))?,
        None => detected.first().map(|found| found.interface.clone()).ok_or_else(|| AppError::bad("Sunucuda kullanılabilir ağ arayüzü bulunamadı"))?,
    };
    let text = address.to_string();
    let cidr = format!("{text}/{prefix}");
    let output = exec::run("ip", &["addr", "add", &cidr, "dev", &interface]).await?;
    if !output.success() {
        return Err(AppError::bad(format!("Adres eklenemedi: {}", output.stderr.trim())));
    }
    db.call(|conn| {
        conn.execute(
            "INSERT INTO server_ips (address, prefix, interface, managed, label, created_at) VALUES (?1, ?2, ?3, 1, ?4, ?5)
             ON CONFLICT(address) DO UPDATE SET prefix = ?2, interface = ?3, managed = 1, label = ?4",
            params![text, prefix, interface, label, now()],
        )
    })?;
    install_boot_unit().await;
    Ok(())
}

fn validate_label(label: &str) -> AppResult<String> {
    let label = label.trim();
    if label.chars().count() > 60 || label.chars().any(char::is_control) {
        return Err(AppError::bad("Etiket en çok 60 karakter olabilir"));
    }
    Ok(label.to_string())
}

/// Adresin etiketini, dış adresini ve ayrıldığı hesabı kaydeder. Adres sunucuda tanımlı olmalıdır.
pub async fn describe(db: &Db, address: &str, label: &str, public_address: &str, user_id: Option<i64>) -> AppResult<()> {
    let address = normalize(address)?;
    let label = validate_label(label)?;
    let public_address = match public_address.trim() {
        "" => String::new(),
        text => {
            let public = normalize(text)?;
            if public.is_ipv4() != address.is_ipv4() {
                return Err(AppError::bad("Dış adres, adresle aynı türden (IPv4 ya da IPv6) olmalı"));
            }
            public.to_string()
        }
    };
    let text = address.to_string();
    let present = detect().await.into_iter().find(|found| found.address == address);
    let known: bool = db.call(|conn| conn.query_row("SELECT 1 FROM server_ips WHERE address = ?1", params![text], |_| Ok(true)).optional())?.unwrap_or(false);
    if present.is_none() && !known {
        return Err(AppError::NotFound);
    }
    if let Some(id) = user_id {
        let role: Option<String> = db.call(|conn| conn.query_row("SELECT role FROM users WHERE id = ?1", params![id], |row| row.get(0)).optional())?;
        match role.as_deref() {
            Some("member") => return Err(AppError::bad("Adres alt hesaba ayrılamaz; hesabın sahibine ayırın")),
            Some(_) => {}
            None => return Err(AppError::bad("Böyle bir hesap yok")),
        }
    }
    let (prefix, interface) = present.map(|found| (found.prefix, found.interface)).unwrap_or_default();
    db.call(|conn| {
        conn.execute(
            "INSERT INTO server_ips (address, prefix, interface, managed, label, public_address, user_id, created_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7)
             ON CONFLICT(address) DO UPDATE SET label = ?4, public_address = ?5, user_id = ?6",
            params![text, prefix, interface, label, public_address, user_id, now()],
        )
    })?;
    Ok(())
}

/// Panelin eklediği adresi sunucudan kaldırır. Adrese bağlı site varsa kaldırılmaz.
pub async fn remove(db: &Db, address: &str) -> AppResult<()> {
    let text = normalize(address)?.to_string();
    let row: Option<(u8, String, bool)> =
        db.call(|conn| conn.query_row("SELECT prefix, interface, managed FROM server_ips WHERE address = ?1", params![text], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional())?;
    let (prefix, interface, managed) = row.ok_or(AppError::NotFound)?;
    if !managed {
        return Err(AppError::bad("Bu adresi panel eklemedi; sunucunun ağ ayarından kaldırılmalıdır"));
    }
    let sites: i64 = db.call(|conn| conn.query_row("SELECT count(*) FROM sites WHERE ip = ?1", params![text], |row| row.get(0)))?;
    if sites > 0 {
        return Err(AppError::conflict(format!("Bu adrese bağlı {sites} site var; önce onları başka adrese alın")));
    }
    if detect().await.iter().any(|found| found.address.to_string() == text) {
        let cidr = format!("{text}/{prefix}");
        let output = exec::run("ip", &["addr", "del", &cidr, "dev", &interface]).await?;
        if !output.success() {
            return Err(AppError::internal(format!("Adres kaldırılamadı: {}", output.stderr.trim())));
        }
    }
    db.call(|conn| conn.execute("DELETE FROM server_ips WHERE address = ?1", params![text]))?;
    Ok(())
}

/// Panelin eklediği adreslerden eksik olanları kurar; kurulanların sayısını döner.
/// Açılışta web sunucusundan önce ve panel başlarken çağrılır.
pub async fn apply_managed(db: &Db) -> AppResult<usize> {
    let managed: Vec<(String, u8, String)> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT address, prefix, interface FROM server_ips WHERE managed = 1")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    })?;
    if managed.is_empty() {
        return Ok(0);
    }
    let detected = detect().await;
    let mut added = 0;
    for (address, prefix, interface) in managed {
        if detected.iter().any(|found| found.address.to_string() == address) {
            continue;
        }
        let cidr = format!("{address}/{prefix}");
        match exec::run("ip", &["addr", "add", &cidr, "dev", &interface]).await {
            Ok(output) if output.success() => added += 1,
            Ok(output) => tracing::warn!(address = %address, "adres kurulamadı: {}", output.stderr.trim()),
            Err(err) => tracing::warn!(address = %address, error = %err, "adres kurulamadı"),
        }
    }
    Ok(added)
}

/// Açılışta adresleri web sunucusundan önce kuran birim. Yazılamazsa panel yine de
/// başlarken adresleri kurar; yalnızca web sunucusunun ilk açılışı o siteler için gecikir.
async fn install_boot_unit() {
    if Path::new(BOOT_UNIT_PATH).exists() {
        return;
    }
    let Ok(binary) = std::env::current_exe() else { return };
    // Panel güncellenirken ikili dosya yerinde değiştirilir; o durumda çekirdek yolun sonuna
    // " (deleted)" yazar. Birime o hâliyle yazılırsa açılışta çalışmaz.
    let binary = std::path::PathBuf::from(binary.to_string_lossy().trim_end_matches(" (deleted)"));
    let unit = format!(
        "[Unit]\nDescription=PixPanel ek IP adresleri\nAfter=network.target\nBefore=nginx.service\n\n\
         [Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart={} apply-addresses\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        binary.display()
    );
    if std::fs::write(BOOT_UNIT_PATH, unit).is_err() {
        return;
    }
    let _ = exec::run("systemctl", &["daemon-reload"]).await;
    let _ = exec::run("systemctl", &["enable", BOOT_UNIT]).await;
}

/// Hesabın sitelerinde seçebileceği adresler. Yönetici sunucudaki her adresi, diğerleri
/// yalnızca sitenin sahibine ayrılmış olanları seçebilir.
pub async fn selectable(db: &Db, is_admin: bool, owner_id: i64) -> AppResult<Vec<Address>> {
    Ok(list(db).await?.into_iter().filter(|item| item.present && (is_admin || item.user_id == Some(owner_id))).collect())
}

/// Siteye yazılacak adresi doğrular; boş metin "bütün adresler" demektir.
pub async fn validate_for_site(db: &Db, is_admin: bool, owner_id: i64, requested: &str) -> AppResult<String> {
    if requested.trim().is_empty() {
        return Ok(String::new());
    }
    let text = normalize(requested)?.to_string();
    if selectable(db, is_admin, owner_id).await?.iter().any(|item| item.address == text) {
        Ok(text)
    } else {
        Err(AppError::bad("Bu adres bu site için kullanılamıyor"))
    }
}

/// Sitenin web sunucusunda bağlanacağı adres: seçilen adres sunucuda duruyorsa o,
/// yoksa (adres kaldırılmışsa) bütün adresler. Böylece eksik adres siteyi yayından düşürmez.
pub async fn bind_for(site_ip: &str) -> Option<IpAddr> {
    let address: IpAddr = site_ip.parse().ok()?;
    detect().await.iter().any(|found| found.address == address).then_some(address)
}

/// Sitenin DNS kayıtlarına yazılacak adres: siteye özel adres varsa onun dış karşılığı
/// (tanımlıysa) ya da kendisi; yoksa `None` (çağıran sunucunun genel adresini kullanır).
pub fn dns_address(db: &Db, site_ip: &str) -> Option<IpAddr> {
    let address: IpAddr = site_ip.parse().ok()?;
    let public: Option<String> = db.call(|conn| conn.query_row("SELECT public_address FROM server_ips WHERE address = ?1", params![site_ip], |row| row.get(0)).optional()).ok().flatten();
    Some(public.and_then(|text| text.parse().ok()).unwrap_or(address))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8,"scope":"host"}]},
      {"ifname":"eth0","addr_info":[
        {"family":"inet","local":"192.168.1.25","prefixlen":24,"scope":"global"},
        {"family":"inet","local":"192.168.1.26","prefixlen":32,"scope":"global","secondary":true},
        {"family":"inet6","local":"2001:db8::10","prefixlen":64,"scope":"global"},
        {"family":"inet6","local":"2001:db8::dead","prefixlen":64,"scope":"global","temporary":true},
        {"family":"inet6","local":"fe80::1","prefixlen":64,"scope":"link"}]},
      {"ifname":"docker0","addr_info":[{"family":"inet","local":"172.17.0.1","prefixlen":16,"scope":"global"}]},
      {"ifname":"br-1a2b","addr_info":[{"family":"inet","local":"172.18.0.1","prefixlen":16,"scope":"global"}]}
    ]"#;

    #[test]
    fn parses_only_hosting_addresses() {
        let found: Vec<String> = parse_ip_json(SAMPLE).iter().map(|item| format!("{}/{} {}", item.address, item.prefix, item.interface)).collect();
        assert_eq!(found, ["192.168.1.25/24 eth0", "192.168.1.26/32 eth0", "2001:db8::10/64 eth0"]);
    }

    #[test]
    fn broken_output_yields_nothing() {
        assert!(parse_ip_json("").is_empty());
        assert!(parse_ip_json("{}").is_empty());
    }

    #[test]
    fn normalize_rejects_special_addresses() {
        for bad in ["127.0.0.1", "0.0.0.0", "224.0.0.1", "169.254.1.1", "::1", "fe80::1", "ff02::1", "1.2.3", "1.2.3.4/24", "ornek.com", "1.2.3.4; id"] {
            assert!(normalize(bad).is_err(), "{bad}");
        }
        assert_eq!(normalize(" 10.0.0.5 ").unwrap().to_string(), "10.0.0.5");
        assert_eq!(normalize("2001:0db8:0000::0010").unwrap().to_string(), "2001:db8::10");
    }
}
