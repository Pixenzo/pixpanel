//! İstemci otomatik ayarı: Thunderbird/K-9 (autoconfig), Outlook (autodiscover, POX) ve Apple
//! (.mobileconfig).
//!
//! Üç genel uç (`/api/v1/mail/public/...`) oturum istemez ve yalnızca bu sunucuda tanımlı posta
//! alan adları için yanıt verir; bilinmeyen alan adına 404 döner. Adresin (kutunun) var olup
//! olmadığına bakılmaz: yanıt yalnızca alan adına ve sunucu adına bağlıdır, böylece kutu adresleri
//! sorgulanarak sayılamaz.
//!
//! İstekler nginx'ten gelir: her posta alan adı için `autoconfig.<alan>` ve `autodiscover.<alan>`
//! adlı sunucu blokları (tek bir üretilmiş dosyada) ve sitenin kendi yapılandırmasına dahil
//! edilen küçük bir parça (`/.well-known/autoconfig/...` ve `/autodiscover/...` yolları) bu uçlara
//! vekil olur.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::{now, Db};
use crate::error::{AppError, AppResult};
use crate::hosting::{atomic_write, domain as domain_rules, nginx, ssl};

/// Sitenin yapılandırmasına dahil edilen parçaların klasörü (`<site alan adı>.inc`).
pub const WELLKNOWN_DIR: &str = "/etc/nginx/pixpanel/mail-wellknown";
const SERVER_CONF: &str = "_mail-autoconfig.conf";
const IMAP_PORT: u16 = 993;
const SUBMISSION_PORT: u16 = 587;
const SMTPS_PORT: u16 = 465;
const MAX_BODY: usize = 16 * 1024;

// ---- Panelin adresi (nginx buradan vekil olur) ----

static UPSTREAM: OnceLock<String> = OnceLock::new();

/// Panelin dinlediği adresi nginx vekili için saklar (açılışta bir kez).
pub fn init(cfg: &crate::config::Config) {
    let host = match cfg.listen_addr {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let scheme = if cfg.tls { "https" } else { "http" };
    let _ = UPSTREAM.set(format!("{scheme}://{host}:{}", cfg.listen_port));
}

fn upstream() -> &'static str {
    UPSTREAM.get().map(String::as_str).unwrap_or("https://127.0.0.1:8443")
}

// ---- DNS önerileri ----

/// İstemci ayarı için önerilen DNS kaydı (`dns::Record` biçiminde içerik).
pub struct ClientRecord {
    pub kind: &'static str,
    pub name: String,
    /// CNAME için hedef (noktasız), SRV için `ağırlık port hedef`.
    pub content: String,
    pub priority: Option<u32>,
    pub purpose: &'static str,
}

pub fn client_dns_records(domain: &str, mail_host: &str) -> Vec<ClientRecord> {
    let record = |kind, name: String, content: String, priority, purpose| ClientRecord { kind, name, content, priority, purpose };
    vec![
        record("CNAME", format!("autoconfig.{domain}"), mail_host.to_string(), None, "Thunderbird ve K-9 için otomatik ayar (autoconfig)"),
        record("CNAME", format!("autodiscover.{domain}"), mail_host.to_string(), None, "Outlook için otomatik ayar (autodiscover)"),
        record("SRV", format!("_autodiscover._tcp.{domain}"), format!("1 443 autodiscover.{domain}"), Some(0), "Outlook için otomatik ayar bulma kaydı"),
        record("SRV", format!("_imaps._tcp.{domain}"), format!("1 {IMAP_PORT} {mail_host}"), Some(0), "IMAP sunucusunu bildirir (RFC 6186)"),
        record("SRV", format!("_submission._tcp.{domain}"), format!("1 {SUBMISSION_PORT} {mail_host}"), Some(0), "Giden posta sunucusunu bildirir (RFC 6186)"),
    ]
}

// ---- Adres doğrulama ----

fn email_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}@[A-Za-z0-9](?:[A-Za-z0-9.-]{0,250}[A-Za-z0-9])?$").expect("geçerli düzenli ifade"))
}

/// Dışarıdan gelen e-posta adresini sıkı biçimde doğrular; `(yerel kısım, alan adı)` döner.
/// Satır sonu, boşluk, tırnak, `<`, `&` gibi karakterler içeren değerler reddedilir.
pub fn parse_email(value: &str) -> AppResult<(String, String)> {
    let value = value.trim();
    if value.len() > 320 || !email_pattern().is_match(value) {
        return Err(AppError::bad("Geçersiz e-posta adresi"));
    }
    let (local, domain) = value.rsplit_once('@').ok_or_else(|| AppError::bad("Geçersiz e-posta adresi"))?;
    let domain = domain_rules::normalize(domain).map_err(|_| AppError::bad("Geçersiz e-posta adresi"))?;
    Ok((local.to_ascii_lowercase(), domain))
}

/// Outlook'un POX isteğinden `<EMailAddress>` değerini alır. İstek gerçek bir XML ayrıştırıcıdan
/// geçirilmez (varlık/DTD işleme yüzeyi açılmaz); yalnızca bu etiket aranır.
pub fn extract_autodiscover_email(body: &[u8]) -> AppResult<String> {
    if body.len() > MAX_BODY {
        return Err(AppError::bad("İstek çok büyük"));
    }
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"(?is)<(?:[A-Za-z0-9_]+:)?EMailAddress>\s*([^<]{1,400}?)\s*</(?:[A-Za-z0-9_]+:)?EMailAddress>").expect("geçerli düzenli ifade"));
    let text = String::from_utf8_lossy(body);
    pattern
        .captures(&text)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| AppError::bad("İstekte e-posta adresi yok"))
}

/// Alan adı bu sunucuda posta alan adı olarak tanımlı mı?
pub fn is_mail_domain(db: &Db, domain: &str) -> bool {
    db.call(|conn| conn.query_row("SELECT EXISTS(SELECT 1 FROM mail_domains WHERE domain = ?1)", params![domain], |row| row.get::<_, bool>(0))).unwrap_or(false)
}

/// `autoconfig.ornek.com` gibi bir ana makine adından posta alan adını çıkarır.
pub fn domain_from_host(host: &str) -> Option<String> {
    let host = host.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
    let base = host.strip_prefix("autoconfig.").or_else(|| host.strip_prefix("autodiscover.")).unwrap_or(&host);
    domain_rules::normalize(base).ok()
}

// ---- Yanıt gövdeleri ----

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Mozilla autoconfig (config-v1.1.xml). Yanıt alan adı ve sunucuya bağlıdır; adrese değil.
pub fn render_autoconfig(domain: &str, host: &str) -> String {
    let (domain, host) = (escape(domain), escape(host));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<clientConfig version=\"1.1\">\n\
  <emailProvider id=\"{domain}\">\n\
    <domain>{domain}</domain>\n\
    <displayName>{domain}</displayName>\n\
    <displayShortName>{domain}</displayShortName>\n\
    <incomingServer type=\"imap\">\n\
      <hostname>{host}</hostname>\n\
      <port>{IMAP_PORT}</port>\n\
      <socketType>SSL</socketType>\n\
      <authentication>password-cleartext</authentication>\n\
      <username>%EMAILADDRESS%</username>\n\
    </incomingServer>\n\
    <outgoingServer type=\"smtp\">\n\
      <hostname>{host}</hostname>\n\
      <port>{SUBMISSION_PORT}</port>\n\
      <socketType>STARTTLS</socketType>\n\
      <authentication>password-cleartext</authentication>\n\
      <username>%EMAILADDRESS%</username>\n\
    </outgoingServer>\n\
    <outgoingServer type=\"smtp\">\n\
      <hostname>{host}</hostname>\n\
      <port>{SMTPS_PORT}</port>\n\
      <socketType>SSL</socketType>\n\
      <authentication>password-cleartext</authentication>\n\
      <username>%EMAILADDRESS%</username>\n\
    </outgoingServer>\n\
  </emailProvider>\n\
</clientConfig>\n"
    )
}

/// Outlook POX autodiscover yanıtı. `email` önceden `parse_email` ile doğrulanmış olmalıdır.
pub fn render_autodiscover(email: &str, host: &str) -> String {
    let (email, host) = (escape(email), escape(host));
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<Autodiscover xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006\">\n\
  <Response xmlns=\"http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a\">\n\
    <Account>\n\
      <AccountType>email</AccountType>\n\
      <Action>settings</Action>\n\
      <Protocol>\n\
        <Type>IMAP</Type>\n\
        <Server>{host}</Server>\n\
        <Port>{IMAP_PORT}</Port>\n\
        <DomainRequired>off</DomainRequired>\n\
        <LoginName>{email}</LoginName>\n\
        <SPA>off</SPA>\n\
        <SSL>on</SSL>\n\
        <AuthRequired>on</AuthRequired>\n\
      </Protocol>\n\
      <Protocol>\n\
        <Type>SMTP</Type>\n\
        <Server>{host}</Server>\n\
        <Port>{SUBMISSION_PORT}</Port>\n\
        <DomainRequired>off</DomainRequired>\n\
        <LoginName>{email}</LoginName>\n\
        <SPA>off</SPA>\n\
        <Encryption>TLS</Encryption>\n\
        <AuthRequired>on</AuthRequired>\n\
        <UsePOPAuth>off</UsePOPAuth>\n\
        <SMTPLast>off</SMTPLast>\n\
      </Protocol>\n\
    </Account>\n\
  </Response>\n\
</Autodiscover>\n"
    )
}

fn profile_uuid(seed: &str) -> String {
    let digest = hex::encode(Sha256::digest(seed.as_bytes())).to_ascii_uppercase();
    format!("{}-{}-{}-{}-{}", &digest[0..8], &digest[8..12], &digest[12..16], &digest[16..20], &digest[20..32])
}

/// İmzasız Apple yapılandırma profili (iOS/macOS Mail). Parola içermez; cihaz sorar.
pub fn render_mobileconfig(email: &str, host: &str) -> String {
    let (email_xml, host_xml) = (escape(email), escape(host));
    let payload_uuid = profile_uuid(&format!("payload:{email}"));
    let profile_uuid = profile_uuid(&format!("profile:{email}"));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>PayloadContent</key>\n\
  <array>\n\
    <dict>\n\
      <key>EmailAccountDescription</key><string>{email_xml}</string>\n\
      <key>EmailAccountName</key><string>{email_xml}</string>\n\
      <key>EmailAccountType</key><string>EmailTypeIMAP</string>\n\
      <key>EmailAddress</key><string>{email_xml}</string>\n\
      <key>IncomingMailServerAuthentication</key><string>EmailAuthPassword</string>\n\
      <key>IncomingMailServerHostName</key><string>{host_xml}</string>\n\
      <key>IncomingMailServerPortNumber</key><integer>{IMAP_PORT}</integer>\n\
      <key>IncomingMailServerUseSSL</key><true/>\n\
      <key>IncomingMailServerUsername</key><string>{email_xml}</string>\n\
      <key>OutgoingMailServerAuthentication</key><string>EmailAuthPassword</string>\n\
      <key>OutgoingMailServerHostName</key><string>{host_xml}</string>\n\
      <key>OutgoingMailServerPortNumber</key><integer>{SUBMISSION_PORT}</integer>\n\
      <key>OutgoingMailServerUseSSL</key><true/>\n\
      <key>OutgoingMailServerUsername</key><string>{email_xml}</string>\n\
      <key>OutgoingPasswordSameAsIncomingPassword</key><true/>\n\
      <key>PayloadDescription</key><string>{email_xml} posta hesabi</string>\n\
      <key>PayloadDisplayName</key><string>{email_xml}</string>\n\
      <key>PayloadIdentifier</key><string>panel.pixpanel.mail.{payload_uuid}</string>\n\
      <key>PayloadType</key><string>com.apple.mail.managed</string>\n\
      <key>PayloadUUID</key><string>{payload_uuid}</string>\n\
      <key>PayloadVersion</key><integer>1</integer>\n\
    </dict>\n\
  </array>\n\
  <key>PayloadDescription</key><string>{email_xml} posta ayarlari</string>\n\
  <key>PayloadDisplayName</key><string>{email_xml}</string>\n\
  <key>PayloadIdentifier</key><string>panel.pixpanel.mail.profile.{profile_uuid}</string>\n\
  <key>PayloadRemovalDisallowed</key><false/>\n\
  <key>PayloadType</key><string>Configuration</string>\n\
  <key>PayloadUUID</key><string>{profile_uuid}</string>\n\
  <key>PayloadVersion</key><integer>1</integer>\n\
</dict>\n\
</plist>\n"
    )
}

// ---- nginx ----

/// Alan adı için kurulacak iki ad (`autoconfig.`, `autodiscover.`) ve durumları.
#[derive(Debug, Clone, Serialize)]
pub struct NameStatus {
    pub name: String,
    /// Aynı adla başka bir site var; çakışmamak için sunucu bloğu yazılmaz.
    pub conflict: bool,
    /// Ad HTTPS ile de sunuluyor (adı kapsayan geçerli bir sertifika var).
    pub https: bool,
}

fn site_names_taken(db: &Db) -> BTreeSet<String> {
    db.call(|conn| {
        let mut stmt = conn.prepare("SELECT domain FROM sites UNION SELECT name FROM site_aliases")?;
        stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<BTreeSet<_>>>()
    })
    .unwrap_or_default()
}

fn name_covers(pattern: &str, name: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => name.split_once('.').is_some_and(|(label, rest)| !label.is_empty() && rest == suffix),
        None => pattern == name,
    }
}

/// Adı kapsayan, süresi dolmamış ve kendinden imzalı olmayan bir sertifika arar: önce adın
/// kendi sertifikası, sonra posta alan adının sitesinin sertifikası (joker ya da çok adlı olabilir).
fn certificate_for(name: &str, domain: &str) -> Option<(PathBuf, PathBuf)> {
    // Alan adı "kendi adıyla bağlan"ı açtıysa onun sertifikası (SAN olarak bu adları da kapsar).
    if let Some(found) = super::tls::certificate_for(domain, name) {
        return Some(found);
    }
    for owner in [name, domain] {
        let (cert, key) = nginx::certificate_paths(owner);
        if !cert.exists() || !key.exists() {
            continue;
        }
        if let Some(info) = ssl::read_info(owner) {
            if !info.self_signed && info.not_after > now() && info.names.iter().any(|pattern| name_covers(pattern, name)) {
                return Some((cert, key));
            }
        }
    }
    None
}

fn plan_names(domain: &str, taken: &BTreeSet<String>) -> Vec<(NameStatus, Option<(PathBuf, PathBuf)>)> {
    ["autoconfig", "autodiscover"]
        .into_iter()
        .map(|prefix| {
            let name = format!("{prefix}.{domain}");
            let conflict = taken.contains(&name);
            let certificate = if conflict { None } else { certificate_for(&name, domain) };
            (NameStatus { name, conflict, https: certificate.is_some() }, certificate)
        })
        .collect()
}

/// Arayüz için: alan adının iki adının sunum durumu.
pub fn names_status(db: &Db, domain: &str) -> Vec<NameStatus> {
    let taken = site_names_taken(db);
    plan_names(domain, &taken).into_iter().map(|(status, _)| status).collect()
}

/// Vekil konumları. Hem özel adların sunucu bloklarında hem de sitenin dahil edilen parçasında kullanılır.
fn locations() -> String {
    let base = format!("{}/api/v1/mail/public", upstream());
    let proxy = |path: &str, target: &str| {
        format!(
            "    location = {path} {{\n\
             \x20       proxy_pass {base}/{target};\n\
             \x20       proxy_http_version 1.1;\n\
             \x20       proxy_set_header Host $host;\n\
             \x20       proxy_set_header Connection \"\";\n\
             \x20       proxy_read_timeout 15s;\n\
             \x20       client_max_body_size 16k;\n\
             \x20   }}\n"
        )
    };
    let mut out = String::new();
    out.push_str(&proxy("/mail/config-v1.1.xml", "autoconfig"));
    out.push_str(&proxy("/.well-known/autoconfig/mail/config-v1.1.xml", "autoconfig"));
    out.push_str(&proxy("/mail/mobileconfig", "mobileconfig"));
    out.push_str(&proxy("/autodiscover/autodiscover.xml", "autodiscover"));
    out.push_str(&proxy("/Autodiscover/Autodiscover.xml", "autodiscover"));
    out
}

/// `webmail.<alan>` sunucu blokları: HTTPS'te web postasını (Roundcube araç sunucusu, yalnızca
/// `/webmail/` yolu) sunar; ziyaretçinin gerçek adresi `X-Real-IP` ile iletilir. Düz HTTP'de yalnızca
/// sertifika doğrulaması yanıtlanır, gerisi HTTPS'e yönlendirilir (parola açık gitmez).
fn webmail_blocks(domain: &str, taken: &BTreeSet<String>) -> String {
    let name = super::tls::webmail_host(domain);
    if taken.contains(&name) {
        return String::new();
    }
    let certificate = super::tls::certificate_for(domain, &name);
    let mut out = String::new();
    let _ = writeln!(out, "server {{");
    nginx::listen_lines(&mut out, 80, "", None);
    let _ = writeln!(out, "    server_name {name};");
    out.push_str(nginx::ACME_LOCATION);
    let fallback = if certificate.is_some() { "return 301 https://$host$request_uri;" } else { "return 404;" };
    let _ = writeln!(out, "    location / {{\n        {fallback}\n    }}\n}}");
    if let Some((cert, key)) = certificate {
        let _ = writeln!(out, "server {{");
        nginx::listen_lines(&mut out, 443, " ssl", None);
        let _ = writeln!(out, "    server_name {name};");
        let _ = writeln!(out, "    ssl_certificate {};", cert.display());
        let _ = writeln!(out, "    ssl_certificate_key {};", key.display());
        let _ = writeln!(out, "    location = / {{\n        return 302 /webmail/;\n    }}");
        let proxy = [
            "    location /webmail/ {".to_string(),
            format!("        proxy_pass http://127.0.0.1:{};", crate::dbtools::PORT),
            "        proxy_http_version 1.1;".to_string(),
            "        proxy_set_header Host $host;".to_string(),
            "        proxy_set_header X-Forwarded-Host $host;".to_string(),
            "        proxy_set_header X-Real-IP $remote_addr;".to_string(),
            "        proxy_set_header Connection \"\";".to_string(),
            "        proxy_read_timeout 600s;".to_string(),
            "        proxy_send_timeout 600s;".to_string(),
            "    }".to_string(),
        ];
        let _ = writeln!(out, "{}", proxy.join("\n"));
        let _ = writeln!(out, "    location / {{\n        return 404;\n    }}\n}}");
    }
    out
}

fn server_blocks(domains: &[String], taken: &BTreeSet<String>, webmail: &BTreeSet<String>) -> String {
    let mut out = String::from("# PixPanel tarafından üretildi: posta istemcisi otomatik ayarı. Elle yapılan değişiklikler kaybolur.\n");
    let locations = locations();
    for domain in domains {
        for (status, certificate) in plan_names(domain, taken) {
            if status.conflict {
                continue;
            }
            let _ = writeln!(out, "server {{");
            nginx::listen_lines(&mut out, 80, "", None);
            let _ = writeln!(out, "    server_name {};", status.name);
            out.push_str(nginx::ACME_LOCATION);
            out.push_str(&locations);
            out.push_str("    location / {\n        return 404;\n    }\n}\n");
            if let Some((cert, key)) = certificate {
                let _ = writeln!(out, "server {{");
                nginx::listen_lines(&mut out, 443, " ssl", None);
                let _ = writeln!(out, "    server_name {};", status.name);
                let _ = writeln!(out, "    ssl_certificate {};", cert.display());
                let _ = writeln!(out, "    ssl_certificate_key {};", key.display());
                out.push_str(&locations);
                out.push_str("    location / {\n        return 404;\n    }\n}\n");
            }
        }
        if webmail.contains(domain) {
            out.push_str(&webmail_blocks(domain, taken));
        }
    }
    out
}

/// Posta alan adı → o adı sunan sitenin alan adı (alan adı sitenin kendisi ya da takma adı olabilir).
fn serving_sites(db: &Db, domains: &[String]) -> BTreeSet<String> {
    let mut sites = BTreeSet::new();
    for domain in domains {
        let found: Option<String> = db
            .call(|conn| {
                use rusqlite::OptionalExtension;
                conn.query_row(
                    "SELECT domain FROM sites WHERE domain = ?1
                     UNION SELECT s.domain FROM site_aliases a JOIN sites s ON s.id = a.site_id WHERE a.name = ?1 LIMIT 1",
                    params![domain],
                    |row| row.get(0),
                )
                .optional()
            })
            .unwrap_or(None);
        sites.extend(found);
    }
    sites
}

fn wellknown_path(site: &str) -> PathBuf {
    Path::new(WELLKNOWN_DIR).join(format!("{site}.inc"))
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// nginx yapılandırmasını posta alan adlarına göre yeniler (yalnızca içerik değiştiyse yazar).
/// `nginx -t` başarısız olursa yazılanlar geri alınır. Tekrar çağrılabilir.
pub async fn sync_nginx(db: &Db) -> AppResult<()> {
    if !nginx::is_installed() || !Path::new(nginx::SITES_DIR).is_dir() {
        return Ok(());
    }
    let domains: Vec<String> = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT domain FROM mail_domains ORDER BY domain")?;
        stmt.query_map([], |row| row.get(0))?.collect()
    })?;
    let taken = site_names_taken(db);
    let sites = serving_sites(db, &domains);
    // `webmail.<alan>` yalnızca web postası (Roundcube) kuruluysa sunulur.
    let webmail = if crate::dbtools::installed().roundcube { super::tls::webmail_domains(db) } else { BTreeSet::new() };

    // Hedef durum: yol → içerik (`None`: dosya olmamalı).
    let mut wanted: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    let server_path = Path::new(nginx::SITES_DIR).join(SERVER_CONF);
    wanted.insert(server_path, (!domains.is_empty()).then(|| server_blocks(&domains, &taken, &webmail)));
    let snippet = format!("# PixPanel tarafından üretildi: posta istemcisi otomatik ayarı.\n{}", locations());
    for site in &sites {
        wanted.insert(wellknown_path(site), Some(snippet.clone()));
    }
    // Artık gerekmeyen parçalar kaldırılır.
    if let Ok(entries) = std::fs::read_dir(WELLKNOWN_DIR) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "inc") && !wanted.contains_key(&path) {
                wanted.insert(path, None);
            }
        }
    }

    let changes: Vec<(&PathBuf, &Option<String>)> = wanted.iter().filter(|(path, content)| read_text(path) != **content).collect();
    if !changes.is_empty() {
        std::fs::create_dir_all(WELLKNOWN_DIR)?;
        let snapshots: Vec<nginx::Snapshot> = changes.iter().map(|(path, _)| nginx::Snapshot::take(path)).collect();
        let applied: AppResult<()> = (|| {
            for (path, content) in &changes {
                match content {
                    Some(text) => atomic_write(path, text.as_bytes(), 0o644)?,
                    None => {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
            Ok(())
        })();
        let outcome = match applied {
            Ok(()) => nginx::test().await,
            Err(err) => Err(err),
        };
        if let Err(err) = outcome {
            for snapshot in &snapshots {
                snapshot.restore();
            }
            return Err(err);
        }
        nginx::reload().await?;
    }

    // Parçayı henüz içermeyen (bu özellikten önce yazılmış) site yapılandırmaları yeniden üretilir.
    for site in &sites {
        let conf = Path::new(nginx::SITES_DIR).join(format!("{site}.conf"));
        let Some(text) = read_text(&conf) else { continue };
        if text.contains(WELLKNOWN_DIR) {
            continue;
        }
        let id: Option<i64> = db
            .call(|conn| {
                use rusqlite::OptionalExtension;
                conn.query_row("SELECT id FROM sites WHERE domain = ?1", params![site], |row| row.get(0)).optional()
            })
            .unwrap_or(None);
        let Some(id) = id else { continue };
        match crate::hosting::sites::load(db, id) {
            Ok(Some(record)) => {
                if let Err(err) = crate::hosting::sites::provision(&record).await {
                    tracing::warn!(site = %site, error = %err, "site yapılandırması otomatik ayar için yenilenemedi");
                }
            }
            _ => continue,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_email_accepts_normal_and_rejects_injection() {
        assert_eq!(parse_email("Ali@Ornek.com").unwrap(), ("ali".into(), "ornek.com".into()));
        for bad in ["a@b.com\r\nX: y", "a b@ornek.com", "a@ornek.com<", "\"a\"@ornek.com", "a@ornek.com&x=1", "../a@ornek.com", "a@", "@ornek.com", "a@ornek", "a@-ornek.com"] {
            assert!(parse_email(bad).is_err(), "{bad:?} reddedilmeliydi");
        }
    }

    #[test]
    fn extracts_pox_address() {
        let body = br#"<?xml version="1.0"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006"><Request><EMailAddress>ali@ornek.com</EMailAddress><AcceptableResponseSchema>x</AcceptableResponseSchema></Request></Autodiscover>"#;
        assert_eq!(extract_autodiscover_email(body).unwrap(), "ali@ornek.com");
        assert!(extract_autodiscover_email(b"<a/>").is_err());
    }

    #[test]
    fn webmail_block_serves_only_acme_until_certificate_exists() {
        let mut taken = BTreeSet::new();
        let block = webmail_blocks("ornek.test", &taken);
        assert!(block.contains("server_name webmail.ornek.test;"));
        assert!(block.contains("acme-challenge"));
        assert!(block.contains("return 404;"));
        // Sertifika yokken HTTPS bloğu ve web postası vekili yazılmaz.
        assert!(!block.contains("listen 443") && !block.contains("/webmail/"));
        // Aynı adla bir site varsa blok yazılmaz.
        taken.insert("webmail.ornek.test".to_string());
        assert!(webmail_blocks("ornek.test", &taken).is_empty());
    }

    #[test]
    fn responses_do_not_depend_on_mailbox() {
        assert_eq!(render_autoconfig("ornek.com", "mail.ornek.com"), render_autoconfig("ornek.com", "mail.ornek.com"));
        let xml = render_autodiscover("ali@ornek.com", "mail.ornek.com");
        assert!(xml.contains("<Port>993</Port>") && xml.contains("<Port>587</Port>"));
        assert!(render_mobileconfig("ali@ornek.com", "mail.ornek.com").contains("<integer>993</integer>"));
    }
}
