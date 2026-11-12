//! Kutu sahibinin kendi işleri: panel hesabı olmadan, yalnızca adresi ve kutu parolasıyla yaptıkları.
//!
//! Şimdilik tek iş var: parola değiştirme (`POST /api/v1/mail/public/password`). Uç oturumsuzdur;
//! bu yüzden en dar yüzeyle yazılır (gövde sınırı, sıkı adres biçimi, adres ve istemci başına
//! deneme sınırı, yanlış parola ile var olmayan adrese aynı yanıt). Roundcube'un `password`
//! eklentisi bu uca bağlanır (`dbtools/roundcube-password-driver.php`).

use std::net::{IpAddr, SocketAddr};

use crate::auth::{AuthUser, Role};

/// İstek gövdesi için üst sınır (bayt).
pub const MAX_BODY: usize = 4096;
/// Parola alanlarının en uzun hâli (Argon2 girdisini sınırlar).
pub const MAX_PASSWORD_LEN: usize = 256;
/// Web postasının panele kendini tanıttığı başlık (ortak anahtar).
pub const KEY_HEADER: &str = "x-pixpanel-webmail-key";
/// Web postasının ziyaretçinin gerçek adresini bildirdiği başlık (yalnızca anahtar doğruysa geçerli).
pub const CLIENT_HEADER: &str = "x-pixpanel-client-ip";

/// `yerel@alan` adresini sıkı biçimde ayrıştırır; geçersizse `None`.
/// Yerel kısım: küçük harf, rakam ve `. _ - +` (ilk karakter harf/rakam); alan: harf, rakam, tire, nokta.
pub fn parse_address(value: &str) -> Option<(String, String)> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() > 254 {
        return None;
    }
    let (local, domain) = value.split_once('@')?;
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && local.as_bytes()[0].is_ascii_alphanumeric()
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        && !local.ends_with('.')
        && !local.contains("..");
    let domain_ok = domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-') && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    (local_ok && domain_ok).then(|| (local.to_string(), domain.to_string()))
}

/// Sabit zamanlı karşılaştırma (anahtarın ilk farklı baytından süre sızmasın).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Hız sınırı için kullanılacak istemci adresi.
///
/// Gerçek bağlantı adresi esastır; `X-Forwarded-For` gibi başlıklara güvenilmez. Tek istisna:
/// bağlantı panelin kendi makinesinden geliyorsa (web postası sunucunun içinden panele bağlanır,
/// bu yüzden bağlantı adresi hep yereldir) ve istek web postasıyla paylaşılan gizli anahtarı
/// taşıyorsa, anahtarla birlikte gelen ziyaretçi adresi kullanılır. Anahtarı yalnızca web postası
/// bilir (Roundcube yapılandırması root ve web grubuna okunur).
pub fn client_ip(peer: SocketAddr, listen: IpAddr, key: Option<&str>, reported: Option<&str>, expected_key: Option<&str>) -> String {
    let real = peer.ip().to_canonical();
    let local = real.is_loopback() || real == listen.to_canonical();
    let trusted = local
        && match (key, expected_key) {
            (Some(given), Some(expected)) => !expected.is_empty() && constant_time_eq(given.as_bytes(), expected.as_bytes()),
            _ => false,
        };
    if trusted {
        if let Some(ip) = reported.and_then(|value| value.trim().parse::<IpAddr>().ok()) {
            return ip.to_canonical().to_string();
        }
    }
    real.to_string()
}

/// Denetim günlüğü için kutu sahibini temsil eden kullanıcı: kayıt, alan adının sahibi hesabın
/// adına düşer; işlemi yapan olarak kutunun adresi görünür.
pub fn actor(domain_owner_id: i64, address: &str, ip: &str) -> AuthUser {
    AuthUser {
        id: domain_owner_id,
        username: address.to_string(),
        email: String::new(),
        role: Role::User,
        parent_id: None,
        permissions: Vec::new(),
        language: "tr".into(),
        ip: ip.to_string(),
        via_token: false,
        session_id: None,
        impersonator: None,
        impersonator_id: None,
        token_scope: None,
        scope_area: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_parsing_is_strict() {
        assert_eq!(parse_address(" Ali@Posta.Test "), Some(("ali".into(), "posta.test".into())));
        assert_eq!(parse_address("a.b_c-d+e@x-y.example.com"), Some(("a.b_c-d+e".into(), "x-y.example.com".into())));
        for bad in [
            "", "ali", "@posta.test", "ali@", "ali@posta", "a b@posta.test", "ali@@posta.test", "ali@posta..test", "ali@-posta.test",
            ".ali@posta.test", "ali.@posta.test", "al..i@posta.test", "ali@posta.test;rm", "ali\"@posta.test", "ali@posta.test,x@y.z", "ali@po sta.test",
            "çay@posta.test", "ali@pöst.test",
        ] {
            assert_eq!(parse_address(bad), None, "{bad:?}");
        }
        assert_eq!(parse_address(&format!("{}@posta.test", "a".repeat(65))), None);
    }

    #[test]
    fn client_ip_ignores_untrusted_reports() {
        let local: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let remote: SocketAddr = "203.0.113.9:50000".parse().unwrap();
        let listen: IpAddr = "0.0.0.0".parse().unwrap();
        // Anahtar doğru ve bağlantı yerel: bildirilen adres kullanılır.
        assert_eq!(client_ip(local, listen, Some("gizli"), Some("198.51.100.7"), Some("gizli")), "198.51.100.7");
        // Anahtar yanlış, eksik ya da beklenen anahtar tanımsız: gerçek bağlantı adresi.
        assert_eq!(client_ip(local, listen, Some("yanlis"), Some("198.51.100.7"), Some("gizli")), "127.0.0.1");
        assert_eq!(client_ip(local, listen, None, Some("198.51.100.7"), Some("gizli")), "127.0.0.1");
        assert_eq!(client_ip(local, listen, Some(""), Some("198.51.100.7"), Some("")), "127.0.0.1");
        assert_eq!(client_ip(local, listen, Some("gizli"), Some("198.51.100.7"), None), "127.0.0.1");
        // Dışarıdan gelen bağlantı doğru anahtarı bilse bile bildirilen adres sayılmaz.
        assert_eq!(client_ip(remote, listen, Some("gizli"), Some("198.51.100.7"), Some("gizli")), "203.0.113.9");
        // Bildirilen adres geçersizse gerçek adres.
        assert_eq!(client_ip(local, listen, Some("gizli"), Some("1.2.3.4, 5.6.7.8"), Some("gizli")), "127.0.0.1");
        // IPv4'e eşlenmiş IPv6 adresi kanonik hâle çevrilir.
        let mapped: SocketAddr = "[::ffff:203.0.113.9]:1".parse().unwrap();
        assert_eq!(client_ip(mapped, listen, None, None, None), "203.0.113.9");
    }

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
