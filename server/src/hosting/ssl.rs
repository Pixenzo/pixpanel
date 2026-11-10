//! Site sertifikaları: diskteki dosyalar, sertifika bilgisi, kendinden
//! imzalı üretim ve yüklenen sertifikaların doğrulanması.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::Serialize;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::pem::Pem;

use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::hosting::nginx::certificate_paths;

pub const MODES: &[&str] = &["none", "selfsigned", "custom", "letsencrypt"];
const SELF_SIGNED_DAYS: i64 = 365;

#[derive(Debug, Clone, Serialize)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    /// Sertifikanın geçerli olduğu alan adları.
    pub names: Vec<String>,
    pub not_before: i64,
    pub not_after: i64,
    pub self_signed: bool,
}

/// PEM metnindeki ilk sertifikanın bilgilerini çıkarır.
pub fn parse_certificate(pem: &[u8]) -> AppResult<CertInfo> {
    let invalid = || AppError::bad("Sertifika okunamadı; PEM biçiminde (-----BEGIN CERTIFICATE-----) olmalı");
    let first = Pem::iter_from_buffer(pem)
        .filter_map(Result::ok)
        .find(|block| block.label == "CERTIFICATE")
        .ok_or_else(invalid)?;
    let cert = first.parse_x509().map_err(|_| invalid())?;

    let mut names: Vec<String> = Vec::new();
    for extension in cert.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = extension.parsed_extension() {
            for name in &san.general_names {
                if let GeneralName::DNSName(dns) = name {
                    names.push(dns.to_lowercase());
                }
            }
        }
    }
    // Çok eski sertifikalarda alan adı yalnızca CN alanında bulunur.
    if names.is_empty() {
        names.extend(cert.subject().iter_common_name().filter_map(|cn| cn.as_str().ok()).map(str::to_lowercase));
    }
    Ok(CertInfo {
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        names,
        not_before: cert.validity().not_before.timestamp(),
        not_after: cert.validity().not_after.timestamp(),
        self_signed: cert.subject() == cert.issuer(),
    })
}

/// Sitenin diskteki sertifikasının bilgileri; sertifika yoksa `None`.
pub fn read_info(domain: &str) -> Option<CertInfo> {
    let (cert, _) = certificate_paths(domain);
    parse_certificate(&std::fs::read(cert).ok()?).ok()
}

/// Sertifikadaki bir ad (joker olabilir) verilen alan adını kapsıyor mu?
fn name_covers(pattern: &str, domain: &str) -> bool {
    match pattern.strip_prefix("*.") {
        // Joker yalnızca tek bir etiketi karşılar: *.ornek.com → a.ornek.com
        Some(suffix) => domain.split_once('.').is_some_and(|(label, rest)| !label.is_empty() && rest == suffix),
        None => pattern == domain,
    }
}

/// Yüklenen sertifika ve anahtarı kullanılmadan önce denetler.
/// Anahtarın sertifikayla eşleştiğini ayrıca `nginx -t` doğrular.
pub fn validate_custom(domain: &str, certificate: &str, private_key: &str) -> AppResult<CertInfo> {
    let info = parse_certificate(certificate.as_bytes())?;
    if info.not_after <= now() {
        return Err(AppError::bad("Sertifikanın süresi dolmuş"));
    }
    if !info.names.iter().any(|name| name_covers(name, domain)) {
        return Err(AppError::bad(format!(
            "Sertifika {domain} alan adını kapsamıyor (kapsadıkları: {})",
            if info.names.is_empty() { "yok".to_string() } else { info.names.join(", ") }
        )));
    }
    let has_key = Pem::iter_from_buffer(private_key.as_bytes())
        .filter_map(Result::ok)
        .any(|block| block.label.ends_with("PRIVATE KEY"));
    if !has_key {
        return Err(AppError::bad("Özel anahtar okunamadı; PEM biçiminde (-----BEGIN PRIVATE KEY-----) olmalı"));
    }
    if private_key.contains("ENCRYPTED") {
        return Err(AppError::bad("Parola korumalı özel anahtar kullanılamaz; önce parolasını kaldırın"));
    }
    Ok(info)
}

/// Verilen adlar için bir yıl geçerli, kendinden imzalı sertifika üretir.
/// Dönen değer (sertifika, özel anahtar) PEM metinleridir.
pub fn generate_self_signed(names: &[String]) -> AppResult<(String, String)> {
    let failed = |err: rcgen::Error| AppError::internal(format!("sertifika üretilemedi: {err}"));
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(failed)?;
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, names[0].clone());
    params.distinguished_name = subject;
    let start = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    params.not_before = start;
    params.not_after = start + time::Duration::days(SELF_SIGNED_DAYS);
    let key = rcgen::KeyPair::generate().map_err(failed)?;
    let cert = params.self_signed(&key).map_err(failed)?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// Diskteki sertifika dosyalarının anlık görüntüsü; kurulum başarısız
/// olursa eski sertifikaya dönmek için kullanılır.
pub struct Installed {
    domain: String,
    previous: Option<(Vec<u8>, Vec<u8>)>,
}

impl Installed {
    pub fn rollback(&self) {
        let result = match &self.previous {
            Some((cert, key)) => write_files(&self.domain, cert, key),
            None => remove(&self.domain),
        };
        if let Err(err) = result {
            tracing::error!(error = %err, domain = %self.domain, "sertifika geri alınamadı");
        }
    }
}

fn write_files(domain: &str, certificate: &[u8], private_key: &[u8]) -> AppResult<()> {
    let (cert_path, key_path) = certificate_paths(domain);
    let dir = cert_path.parent().expect("sertifika klasörü");
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    atomic_write(&key_path, private_key, 0o600)?;
    atomic_write(&cert_path, certificate, 0o644)
}

/// Sertifikayı sitenin klasörüne yazar; geri almak için tutamak döner.
pub fn install(domain: &str, certificate: &str, private_key: &str) -> AppResult<Installed> {
    let (cert_path, key_path) = certificate_paths(domain);
    let previous = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(cert), Ok(key)) => Some((cert, key)),
        _ => None,
    };
    write_files(domain, certificate.as_bytes(), private_key.as_bytes())?;
    Ok(Installed { domain: domain.to_string(), previous })
}

pub fn remove(domain: &str) -> AppResult<()> {
    let (cert_path, _) = certificate_paths(domain);
    let dir: &Path = cert_path.parent().expect("sertifika klasörü");
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_covers_single_label_only() {
        assert!(name_covers("ornek.com", "ornek.com"));
        assert!(name_covers("*.ornek.com", "blog.ornek.com"));
        assert!(!name_covers("*.ornek.com", "ornek.com"));
        assert!(!name_covers("*.ornek.com", "a.b.ornek.com"));
        assert!(!name_covers("ornek.com", "kotu-ornek.com"));
    }

    #[test]
    fn self_signed_roundtrip() {
        let names = vec!["ornek.test".to_string(), "www.ornek.test".to_string()];
        let (cert, key) = generate_self_signed(&names).unwrap();
        let info = validate_custom("www.ornek.test", &cert, &key).unwrap();
        assert!(info.self_signed);
        assert_eq!(info.names, names);
        assert!(validate_custom("baska.test", &cert, &key).is_err());
    }
}
