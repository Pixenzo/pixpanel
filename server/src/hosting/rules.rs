//! Site kuralları: yönlendirmeler, parola korumalı klasörler, adres engelleme, bakım kipi,
//! WWW tercihi, görsel bağlantı (hotlink) koruması ve özel hata sayfaları.
//!
//! Bunlar webcilerin sürekli elle yazdığı web sunucusu ayarlarıdır. Sitenin sahibi serbest
//! metin yazmaz: her kural doğrulanmış alanlardan oluşur ve yapılandırma bu alanlardan
//! üretilir. Yapılandırmaya giren her değer burada beyaz listeden geçer.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::sys::exec;

const HTPASSWD_DIR: &str = "/etc/nginx/pixpanel/htpasswd";
pub const PAGES_DIR: &str = "/var/lib/pixpanel/pages";
pub const MAINTENANCE_PAGE: &str = "maintenance.html";
const MAX_REDIRECTS: usize = 200;
const MAX_PROTECTED: usize = 20;
const MAX_USERS: usize = 30;
/// Bir sitedeki bütün korumalı klasörlerin toplam kullanıcı sayısı. Her yeni parola ayrı bir
/// özet hesabıdır; sınırsız olsaydı tek istek paneli uzun süre meşgul edebilirdi.
const MAX_TOTAL_USERS: usize = 60;
const MAX_ADDRESSES: usize = 200;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SiteRules {
    /// Asıl ad tercihi: `www` (çıplak ad www'ye yönlenir), `apex` (www çıplak ada yönlenir) ya da boş.
    pub www: String,
    pub redirects: Vec<Redirect>,
    pub protected: Vec<Protected>,
    /// Siteye erişemeyecek adresler (IP ya da ağ).
    pub blocked: Vec<String>,
    /// Doluysa siteye yalnızca bu adresler erişir.
    pub allowed: Vec<String>,
    pub maintenance: bool,
    /// Bakım kipindeyken siteyi görmeye devam eden adresler.
    pub maintenance_allowed: Vec<String>,
    /// Görsellerin başka sitelerden doğrudan gösterilmesi engellensin mi?
    pub hotlink_protection: bool,
    pub error_pages: ErrorPages,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Redirect {
    pub from: String,
    pub to: String,
    /// 301 (kalıcı) ya da 302 (geçici).
    pub code: u16,
    /// Yalnızca tam bu adres mi, yoksa altındaki her adres de mi?
    pub exact: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Protected {
    pub path: String,
    /// Tarayıcının parola penceresinde gösterdiği ad.
    pub label: String,
    pub users: Vec<ProtectedUser>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProtectedUser {
    pub name: String,
    /// Parolanın özeti (SHA-512 crypt). Arayüze gönderilmez.
    pub hash: String,
    /// Yalnızca istekte gelir: verilirse parola değişir, verilmezse kayıtlı özet korunur.
    #[serde(skip_serializing)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ErrorPages {
    pub not_found: String,
    pub forbidden: String,
    pub server_error: String,
}

impl SiteRules {
    /// Arayüze gönderilen görünüm: parola özetleri çıkarılır.
    pub fn public_view(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        if let Some(list) = value.get_mut("protected").and_then(|item| item.as_array_mut()) {
            for area in list {
                if let Some(users) = area.get_mut("users").and_then(|item| item.as_array_mut()) {
                    for user in users {
                        if let Some(object) = user.as_object_mut() {
                            object.remove("hash");
                        }
                    }
                }
            }
        }
        value
    }
}

/// `Site.rules` alanını arayüze parola özetleri olmadan yazar.
pub fn serialize_public<S: serde::Serializer>(rules: &SiteRules, serializer: S) -> Result<S::Ok, S::Error> {
    rules.public_view().serialize(serializer)
}

// ---- Doğrulama ----

/// Site içi yol: `/` ile başlar, yalnızca harf, rakam ve `/ _ - .` içerir.
fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 200
        && !path.contains("..")
        && !path.contains("//")
        && path.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
}

/// Yönlendirme hedefi: site içi yol (sorgu dizgesi olabilir) ya da tam http(s) adresi.
/// Yapılandırmayı bozabilecek karakterler (boşluk, tırnak, `;`, `$`, süslü ayraç…) kabul edilmez.
fn normalize_target(target: &str) -> AppResult<String> {
    let target = target.trim();
    let safe = |text: &str| text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/_-.~:?&=%#+,@!".contains(&byte));
    if target.is_empty() || target.len() > 500 || !safe(target) {
        return Err(AppError::bad(format!("Geçersiz hedef adres: {target}")));
    }
    if target.starts_with('/') && !target.starts_with("//") {
        return Ok(target.to_string());
    }
    let url = url::Url::parse(target).map_err(|_| AppError::bad(format!("Hedef, / ile başlayan bir yol ya da http(s) adresi olmalı: {target}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::bad(format!("Hedef, / ile başlayan bir yol ya da http(s) adresi olmalı: {target}")));
    }
    Ok(target.to_string())
}

/// IP adresi ya da ağ (`203.0.113.0/24`); tek biçime indirgenir.
fn normalize_address(text: &str) -> AppResult<String> {
    let text = text.trim();
    let invalid = || AppError::bad(format!("Geçersiz adres: {text} (örnek: 203.0.113.5 ya da 203.0.113.0/24)"));
    match text.split_once('/') {
        Some((address, prefix)) => {
            let address: IpAddr = address.parse().map_err(|_| invalid())?;
            let prefix: u8 = prefix.parse().map_err(|_| invalid())?;
            if prefix > if address.is_ipv4() { 32 } else { 128 } {
                return Err(invalid());
            }
            Ok(format!("{address}/{prefix}"))
        }
        None => Ok(text.parse::<IpAddr>().map_err(|_| invalid())?.to_string()),
    }
}

fn normalize_addresses(list: &[String]) -> AppResult<Vec<String>> {
    if list.len() > MAX_ADDRESSES {
        return Err(AppError::bad(format!("En çok {MAX_ADDRESSES} adres girilebilir")));
    }
    let mut out = Vec::new();
    for item in list.iter().filter(|item| !item.trim().is_empty()) {
        let address = normalize_address(item)?;
        if !out.contains(&address) {
            out.push(address);
        }
    }
    Ok(out)
}

fn valid_user_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'@'))
}

fn normalize_error_page(path: &str, label: &str) -> AppResult<String> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(String::new());
    }
    if !valid_path(path) || path.ends_with('/') || path.to_ascii_lowercase().ends_with(".php") {
        return Err(AppError::bad(format!("{label} sayfası, sitenin içindeki bir dosyanın yolu olmalı (örnek: /404.html)")));
    }
    Ok(path.to_string())
}

/// SHA-512 crypt özeti üretir (nginx'in ve sistemin tanıdığı biçim). Parola komut satırına değil,
/// standart girdiye yazılır; süreç listesinde görünmez.
async fn hash_password(password: &str) -> AppResult<String> {
    let mut child = exec::base_command("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| AppError::internal("Parola özeti üretilemedi: openssl bulunamadı"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(password.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    let output = child.wait_with_output().await?;
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || !hash.starts_with("$6$") {
        return Err(AppError::internal("Parola özeti üretilemedi"));
    }
    Ok(hash)
}

/// İstekle gelen kuralları doğrular ve saklanacak hâle getirir. `previous`, parolası
/// değiştirilmeyen kullanıcıların kayıtlı özetlerini korumak içindir.
pub async fn normalize(input: SiteRules, previous: &SiteRules, names: &[String]) -> AppResult<SiteRules> {
    let www = match input.www.as_str() {
        "" => String::new(),
        "www" | "apex" => {
            // Tercih ancak sitede iki ad da varsa anlamlıdır.
            let has_pair = names.iter().any(|name| names.contains(&format!("www.{name}")));
            if !has_pair {
                return Err(AppError::bad("WWW tercihi için sitede hem alan adı hem de www'li hâli bulunmalı (takma ad olarak ekleyin)"));
            }
            input.www.clone()
        }
        _ => return Err(AppError::bad("Geçersiz WWW tercihi")),
    };

    if input.redirects.len() > MAX_REDIRECTS {
        return Err(AppError::bad(format!("Bir siteye en çok {MAX_REDIRECTS} yönlendirme eklenebilir")));
    }
    let mut redirects: Vec<Redirect> = Vec::new();
    for item in &input.redirects {
        let from = item.from.trim();
        if !valid_path(from) || from.starts_with("/.well-known") {
            return Err(AppError::bad(format!("Geçersiz kaynak adres: {from} (örnek: /eski-sayfa ; harf, rakam, / _ - . kullanılabilir)")));
        }
        if from == "/" && !item.exact {
            return Err(AppError::bad("Sitenin tamamını yönlendirmek için site türünü ya da alan adı ayarını kullanın; / yalnızca tam eşleşmeyle yönlendirilebilir"));
        }
        if redirects.iter().any(|other| other.from == from) {
            return Err(AppError::bad(format!("{from} için birden fazla yönlendirme var")));
        }
        let to = normalize_target(&item.to)?;
        if to == from {
            return Err(AppError::bad(format!("{from} kendisine yönlendirilemez")));
        }
        let code = match item.code {
            0 | 301 => 301,
            302 => 302,
            _ => return Err(AppError::bad("Yönlendirme türü 301 (kalıcı) ya da 302 (geçici) olmalı")),
        };
        redirects.push(Redirect { from: from.to_string(), to, code, exact: item.exact });
    }
    // Daha özgül (uzun) kaynaklar önce yazılır.
    redirects.sort_by(|a, b| b.from.len().cmp(&a.from.len()).then(a.from.cmp(&b.from)));

    if input.protected.len() > MAX_PROTECTED {
        return Err(AppError::bad(format!("Bir sitede en çok {MAX_PROTECTED} korumalı klasör olabilir")));
    }
    if input.protected.iter().map(|area| area.users.len()).sum::<usize>() > MAX_TOTAL_USERS {
        return Err(AppError::bad(format!("Bir sitede korumalı klasörlerin toplam kullanıcı sayısı {MAX_TOTAL_USERS} olabilir")));
    }
    let mut protected: Vec<Protected> = Vec::new();
    for item in &input.protected {
        let path = match item.path.trim() {
            "/" => "/".to_string(),
            path => path.trim_end_matches('/').to_string(),
        };
        if !valid_path(&path) || path.starts_with("/.well-known") {
            return Err(AppError::bad(format!("Geçersiz klasör yolu: {path} (örnek: /yonetim)")));
        }
        if protected.iter().any(|other| other.path == path) {
            return Err(AppError::bad(format!("{path} iki kez korunmuş")));
        }
        let label = item.label.trim();
        let label_ok = label.chars().count() <= 60 && label.chars().all(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '_' | '-' | '.'));
        if !label_ok || label.eq_ignore_ascii_case("off") {
            return Err(AppError::bad("Koruma adı en çok 60 karakter olabilir; harf, rakam, boşluk ve _ - . kullanılabilir"));
        }
        if item.users.is_empty() || item.users.len() > MAX_USERS {
            return Err(AppError::bad(format!("{path} için 1 ile {MAX_USERS} arasında kullanıcı tanımlayın")));
        }
        let old = previous.protected.iter().find(|other| other.path == path);
        let mut users: Vec<ProtectedUser> = Vec::new();
        for user in &item.users {
            let name = user.name.trim();
            if !valid_user_name(name) {
                return Err(AppError::bad(format!("Geçersiz kullanıcı adı: {name} (harf, rakam ve _ - . @ kullanılabilir)")));
            }
            if users.iter().any(|other| other.name == name) {
                return Err(AppError::bad(format!("{path} için {name} iki kez girilmiş")));
            }
            let hash = match user.password.as_deref().filter(|password| !password.is_empty()) {
                Some(password) => {
                    if password.len() < 8 || password.len() > 128 {
                        return Err(AppError::bad("Klasör parolası 8 ile 128 karakter arasında olmalı"));
                    }
                    hash_password(password).await?
                }
                None => old
                    .and_then(|area| area.users.iter().find(|other| other.name == name))
                    .map(|other| other.hash.clone())
                    .ok_or_else(|| AppError::bad(format!("{name} için parola girin")))?,
            };
            users.push(ProtectedUser { name: name.to_string(), hash, password: None });
        }
        protected.push(Protected { path, label: if label.is_empty() { "Korumalı alan".to_string() } else { label.to_string() }, users });
    }
    // Uzun yol önce: iç içe korumada en özgül olan geçerli olur.
    protected.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.path.cmp(&b.path)));

    Ok(SiteRules {
        www,
        redirects,
        protected,
        blocked: normalize_addresses(&input.blocked)?,
        allowed: normalize_addresses(&input.allowed)?,
        maintenance: input.maintenance,
        maintenance_allowed: normalize_addresses(&input.maintenance_allowed)?,
        hotlink_protection: input.hotlink_protection,
        error_pages: ErrorPages {
            not_found: normalize_error_page(&input.error_pages.not_found, "404")?,
            forbidden: normalize_error_page(&input.error_pages.forbidden, "403")?,
            server_error: normalize_error_page(&input.error_pages.server_error, "5xx")?,
        },
    })
}

// ---- Yapılandırma üretimi ----

fn htpasswd_path(site_id: i64, index: usize) -> PathBuf {
    PathBuf::from(format!("{HTPASSWD_DIR}/{site_id}-{index}"))
}

/// Yolu düzenli ifadeye çevirir: yol yalnızca güvenli karakterlerden oluşur, kaçışlanan tek şey noktadır.
///
/// İfade, yolun önünde bir betik adı bulunmasına da izin verir: PHP uygulamalarında
/// `/index.php/yonetim` isteği uygulamanın `/yonetim` sayfasını açar; yalnızca yolun başına
/// bakılsaydı koruma bu yazımla atlatılırdı. Eşleme büyük/küçük harf ayırmaz (bkz. `render_preamble`).
fn path_regex(path: &str) -> String {
    if path == "/" {
        return "^/".to_string();
    }
    format!("^(?:/.*\\.php)?{}(/|$)", path.replace('.', "\\."))
}

/// Sitenin yapılandırma dosyasının başına (http düzeyine) yazılan eşlemeler.
pub fn render_preamble(site_id: i64, rules: &SiteRules) -> String {
    let mut out = String::new();
    if !rules.protected.is_empty() {
        // Parola koruması yola göre değişken üzerinden açılır; böylece sitenin türünden
        // (PHP, durağan, vekil) bağımsız çalışır ve içerik kurallarını ezmez.
        let _ = writeln!(out, "map $uri $pix_auth_{site_id} {{\n    default off;");
        for area in &rules.protected {
            let _ = writeln!(out, "    ~*{} \"{}\";", path_regex(&area.path), area.label);
        }
        let _ = writeln!(out, "}}\nmap $uri $pix_authfile_{site_id} {{\n    default /dev/null;");
        for (index, area) in rules.protected.iter().enumerate() {
            let _ = writeln!(out, "    ~*{} {};", path_regex(&area.path), htpasswd_path(site_id, index).display());
        }
        out.push_str("}\n");
    }
    if rules.maintenance {
        // 1: bakım sayfası gösterilir. İzinli adresler ve sertifika doğrulaması dışarıda tutulur.
        let _ = writeln!(out, "geo $pix_maint_ip_{site_id} {{\n    default 1;");
        for address in &rules.maintenance_allowed {
            let _ = writeln!(out, "    {address} 0;");
        }
        let _ = writeln!(out, "}}\nmap $uri $pix_maint_uri_{site_id} {{\n    default 1;\n    ~^/\\.well-known/acme-challenge/ 0;\n}}");
    }
    out
}

/// Sunucu bloğunun içine, içerik kurallarından önce yazılan yönergeler.
/// `names`: sitenin adları (WWW tercihi için); `has_docroot`: site dosya sunuyor mu?
pub fn render_server(site_id: i64, rules: &SiteRules, domain: &str, has_docroot: bool) -> String {
    let mut out = String::new();
    match rules.www.as_str() {
        "www" => {
            let _ = writeln!(out, "    if ($host = '{domain}') {{\n        return 301 $scheme://www.{domain}$request_uri;\n    }}");
        }
        "apex" => {
            let _ = writeln!(out, "    if ($host = 'www.{domain}') {{\n        return 301 $scheme://{domain}$request_uri;\n    }}");
        }
        _ => {}
    }
    if rules.maintenance {
        let _ = writeln!(out, "    set $pix_maint \"$pix_maint_ip_{site_id}$pix_maint_uri_{site_id}\";");
        out.push_str("    if ($pix_maint = \"11\") {\n        return 503;\n    }\n");
        // Adlandırılmış konuma geçişte sunucu düzeyindeki koşul yeniden çalışmaz; aksi hâlde
        // bakım sayfasının kendisi de 503'e takılıp döngüye girerdi.
        let _ = writeln!(
            out,
            "    error_page 503 @pix_maintenance;\n    location @pix_maintenance {{\n        root {PAGES_DIR};\n        auth_basic off;\n        add_header Retry-After 600 always;\n        rewrite ^ /{MAINTENANCE_PAGE} break;\n    }}"
        );
    }
    for address in &rules.blocked {
        let _ = writeln!(out, "    deny {address};");
    }
    if !rules.allowed.is_empty() {
        for address in &rules.allowed {
            let _ = writeln!(out, "    allow {address};");
        }
        out.push_str("    deny all;\n");
    }
    if !rules.protected.is_empty() {
        let _ = writeln!(out, "    auth_basic $pix_auth_{site_id};\n    auth_basic_user_file $pix_authfile_{site_id};");
    }
    if has_docroot {
        let pages = &rules.error_pages;
        if !pages.not_found.is_empty() {
            let _ = writeln!(out, "    error_page 404 {};", pages.not_found);
        }
        if !pages.forbidden.is_empty() {
            let _ = writeln!(out, "    error_page 403 {};", pages.forbidden);
        }
        // Bakım kipi 503'ü kendi sayfasına bağlar; özel sayfa onu ezmemeli.
        if !pages.server_error.is_empty() {
            let codes = if rules.maintenance { "500 502 504" } else { "500 502 503 504" };
            let _ = writeln!(out, "    error_page {codes} {};", pages.server_error);
        }
    }
    for item in &rules.redirects {
        if item.exact {
            let _ = writeln!(out, "    location = {} {{\n        return {} {};\n    }}", item.from, item.code, item.to);
        } else {
            // Altındaki adresler de taşınır: /eski/x → /yeni/x. Konum tam olarak bu yolu ve altını
            // eşler; "/eski" kuralı "/eskimis" adresini yakalamaz (önek konumu yakalardı ve o
            // adresler içerik kurallarının dışında kalırdı).
            let base = item.from.trim_end_matches('/');
            let target = item.to.trim_end_matches('/');
            // Yakalanan yol `return` ile ham yazılmaz; `rewrite` hedefi kendisi kaçışlar (çözülmüş
            // yoldaki satır başı gibi karakterler yanıt başlığına olduğu gibi geçmez).
            let pattern = base.replace('.', "\\.");
            let flag = if item.code == 301 { "permanent" } else { "redirect" };
            let _ = writeln!(out, "    location ~ ^{pattern}(/|$) {{\n        rewrite ^{pattern}(/.*)?$ {target}$1 {flag};\n    }}");
        }
    }
    out
}

/// Görsellerin başka sitelerden gösterilmesini engelleyen kural (dosya sunan sitelerde).
pub fn render_hotlink(rules: &SiteRules) -> &'static str {
    if rules.hotlink_protection {
        "    location ~* \\.(?:jpe?g|png|gif|webp|avif|svg)$ {\n        valid_referers none blocked server_names;\n        if ($invalid_referer) {\n            return 403;\n        }\n        expires 30d;\n    }\n"
    } else {
        ""
    }
}

/// Parola dosyalarını yazar; artık kullanılmayanları siler. Yapılandırmadan önce çağrılır.
pub fn write_password_files(site_id: i64, rules: &SiteRules) -> AppResult<()> {
    if !rules.protected.is_empty() {
        std::fs::create_dir_all(HTPASSWD_DIR)?;
    }
    for (index, area) in rules.protected.iter().enumerate() {
        let content: String = area.users.iter().map(|user| format!("{}:{}\n", user.name, user.hash)).collect();
        let path = htpasswd_path(site_id, index);
        // Dosyayı nginx'in işçi süreçleri okur; başkası okuyamaz.
        atomic_write(&path, content.as_bytes(), 0o640)?;
        if let Some(group) = crate::hosting::sysuser::lookup("www-data") {
            std::os::unix::fs::chown(&path, Some(0), Some(group.gid))?;
        }
    }
    remove_password_files(site_id, rules.protected.len());
    Ok(())
}

/// `keep` ve sonrasındaki sıra numaralı parola dosyalarını siler (site silinirken `keep` = 0).
pub fn remove_password_files(site_id: i64, keep: usize) {
    let prefix = format!("{site_id}-");
    let Ok(entries) = std::fs::read_dir(HTPASSWD_DIR) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.strip_prefix(&prefix).and_then(|rest| rest.parse::<usize>().ok()).is_some_and(|index| index >= keep) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Bakım sayfasını hazırlar (yoksa yazar).
pub fn ensure_pages() -> AppResult<()> {
    let path = Path::new(PAGES_DIR).join(MAINTENANCE_PAGE);
    if !path.exists() {
        std::fs::create_dir_all(PAGES_DIR)?;
        atomic_write(&path, include_str!("templates/maintenance.html").as_bytes(), 0o644)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["ornek.com".to_string(), "www.ornek.com".to_string()]
    }

    async fn check(rules: SiteRules) -> AppResult<SiteRules> {
        normalize(rules, &SiteRules::default(), &names()).await
    }

    #[tokio::test]
    async fn redirects_are_validated_and_sorted() {
        let rules = SiteRules {
            redirects: vec![
                Redirect { from: "/a".into(), to: "/yeni".into(), code: 0, exact: true },
                Redirect { from: "/eski/blog".into(), to: "https://blog.ornek.com/".into(), code: 302, exact: false },
            ],
            ..SiteRules::default()
        };
        let out = check(rules).await.unwrap();
        assert_eq!(out.redirects[0].from, "/eski/blog");
        assert_eq!(out.redirects[1].code, 301);
        for (from, to) in [
            ("eski", "/yeni"),
            ("/eski yol", "/yeni"),
            ("/a;b", "/yeni"),
            ("/a", "/yeni; return 200"),
            ("/a", "javascript:alert(1)"),
            ("/a", "//baska.com"),
            ("/a", "/b\"}"),
            ("/a", "/$host"),
            ("/a", "/a"),
            ("/.well-known/x", "/y"),
            ("/../etc", "/y"),
        ] {
            let rules = SiteRules { redirects: vec![Redirect { from: from.into(), to: to.into(), code: 301, exact: true }], ..SiteRules::default() };
            assert!(check(rules).await.is_err(), "{from} -> {to}");
        }
        let whole = SiteRules { redirects: vec![Redirect { from: "/".into(), to: "/x".into(), code: 301, exact: false }], ..SiteRules::default() };
        assert!(check(whole).await.is_err());
    }

    #[tokio::test]
    async fn addresses_are_normalized() {
        let rules = SiteRules { blocked: vec![" 203.0.113.5 ".into(), "2001:0db8::/32".into(), "".into(), "203.0.113.5".into()], ..SiteRules::default() };
        assert_eq!(check(rules).await.unwrap().blocked, ["203.0.113.5", "2001:db8::/32"]);
        for bad in ["ornek.com", "203.0.113.5/33", "1.2.3.4; allow all", "all"] {
            let rules = SiteRules { allowed: vec![bad.into()], ..SiteRules::default() };
            assert!(check(rules).await.is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn www_needs_both_names() {
        let rules = SiteRules { www: "www".into(), ..SiteRules::default() };
        assert!(normalize(rules.clone(), &SiteRules::default(), &["ornek.com".to_string()]).await.is_err());
        assert!(check(rules).await.is_ok());
        assert!(check(SiteRules { www: "x".into(), ..SiteRules::default() }).await.is_err());
    }

    #[tokio::test]
    async fn protected_areas_keep_existing_hashes() {
        let previous = SiteRules {
            protected: vec![Protected { path: "/yonetim".into(), label: "Yönetim".into(), users: vec![ProtectedUser { name: "ali".into(), hash: "$6$eski".into(), password: None }] }],
            ..SiteRules::default()
        };
        let input = SiteRules {
            protected: vec![Protected { path: "/yonetim/".into(), label: "Yönetim".into(), users: vec![ProtectedUser { name: "ali".into(), hash: "$6$sahte".into(), password: None }] }],
            ..SiteRules::default()
        };
        let out = normalize(input, &previous, &names()).await.unwrap();
        // İstekle gelen özet yok sayılır; kayıtlı olan korunur.
        assert_eq!(out.protected[0].users[0].hash, "$6$eski");
        assert_eq!(out.protected[0].path, "/yonetim");
        let new_user = SiteRules {
            protected: vec![Protected { path: "/yonetim".into(), label: "".into(), users: vec![ProtectedUser { name: "veli".into(), hash: "$6$sahte".into(), password: None }] }],
            ..SiteRules::default()
        };
        assert!(normalize(new_user, &previous, &names()).await.is_err());
        for (path, label, name) in [("/a b", "x", "ali"), ("/yonetim", "a\"; }", "ali"), ("/yonetim", "x", "a:b"), ("/yonetim", "x", "")] {
            let input = SiteRules {
                protected: vec![Protected { path: path.into(), label: label.into(), users: vec![ProtectedUser { name: name.into(), hash: String::new(), password: Some("parola-12345".into()) }] }],
                ..SiteRules::default()
            };
            assert!(normalize(input, &previous, &names()).await.is_err(), "{path} {label} {name}");
        }
    }

    #[test]
    fn public_view_hides_hashes() {
        let rules = SiteRules {
            protected: vec![Protected { path: "/x".into(), label: "X".into(), users: vec![ProtectedUser { name: "ali".into(), hash: "$6$gizli".into(), password: None }] }],
            ..SiteRules::default()
        };
        let text = rules.public_view().to_string();
        assert!(!text.contains("gizli"));
        assert!(text.contains("ali"));
        // Saklanan biçimde özet durur.
        assert!(serde_json::to_string(&rules).unwrap().contains("gizli"));
    }

    #[test]
    fn rendering() {
        let rules = SiteRules {
            www: "apex".into(),
            redirects: vec![
                Redirect { from: "/eski".into(), to: "/yeni".into(), code: 301, exact: true },
                Redirect { from: "/blog.v1".into(), to: "https://blog.ornek.com".into(), code: 302, exact: false },
            ],
            protected: vec![Protected { path: "/yonetim".into(), label: "Yönetim".into(), users: Vec::new() }],
            blocked: vec!["203.0.113.5".into()],
            allowed: vec!["198.51.100.0/24".into()],
            maintenance: true,
            maintenance_allowed: vec!["192.0.2.1".into()],
            hotlink_protection: true,
            error_pages: ErrorPages { not_found: "/404.html".into(), forbidden: String::new(), server_error: "/hata.html".into() },
        };
        let preamble = render_preamble(7, &rules);
        assert!(preamble.contains("map $uri $pix_auth_7 {"));
        assert!(preamble.contains("~*^(?:/.*\\.php)?/yonetim(/|$) \"Yönetim\";"));
        assert!(preamble.contains("~*^(?:/.*\\.php)?/yonetim(/|$) /etc/nginx/pixpanel/htpasswd/7-0;"));
        assert!(preamble.contains("geo $pix_maint_ip_7 {\n    default 1;\n    192.0.2.1 0;"));
        let server = render_server(7, &rules, "ornek.com", true);
        assert!(server.contains("if ($host = 'www.ornek.com') {\n        return 301 $scheme://ornek.com$request_uri;"));
        assert!(server.contains("deny 203.0.113.5;\n    allow 198.51.100.0/24;\n    deny all;"));
        assert!(server.contains("location = /eski {\n        return 301 /yeni;"));
        assert!(server.contains("location ~ ^/blog\\.v1(/|$) {\n        rewrite ^/blog\\.v1(/.*)?$ https://blog.ornek.com$1 redirect;"));
        assert!(server.contains("error_page 503 @pix_maintenance;"));
        assert!(server.contains("error_page 404 /404.html;"));
        assert!(server.contains("error_page 500 502 504 /hata.html;"));
        assert!(server.contains("auth_basic $pix_auth_7;"));
        assert!(render_hotlink(&rules).contains("valid_referers none blocked server_names;"));
        // Dosya sunmayan sitede hata sayfası yazılmaz.
        assert!(!render_server(7, &rules, "ornek.com", false).contains("error_page 404"));
        assert_eq!(render_preamble(7, &SiteRules::default()), "");
        assert_eq!(render_server(7, &SiteRules::default(), "ornek.com", true), "");
    }
}
