//! İki adımlı doğrulama: zamana dayalı tek kullanımlık kodlar (TOTP, RFC 6238).
//!
//! Google Authenticator, Aegis, 1Password gibi uygulamalarla uyumlu olması için
//! yaygın değerler kullanılır: SHA-1, 6 hane, 30 saniyelik adım.

use hmac::{Hmac, Mac};
use sha1::Sha1;

const STEP_SECS: i64 = 30;
const DIGITS: u32 = 6;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Yeni bir gizli anahtar üretir (160 bit, base32).
pub fn generate_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    encode_base32(&bytes)
}

fn encode_base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn decode_base32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| *c != b'=' && *c != b' ') {
        let value = BASE32.iter().position(|b| *b == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// Verilen zaman adımı için kodu hesaplar.
fn code_at(secret: &[u8], step: i64) -> Option<String> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).ok()?;
    mac.update(&(step as u64).to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = u32::from_be_bytes([digest[offset] & 0x7f, digest[offset + 1], digest[offset + 2], digest[offset + 3]]);
    Some(format!("{:0width$}", binary % 10u32.pow(DIGITS), width = DIGITS as usize))
}

/// Kodu doğrular; saat kayması için bir önceki ve bir sonraki adım da kabul edilir.
///
/// Geçerliyse kodun ait olduğu zaman adımını döner. Çağıran, aynı kodun ikinci
/// kez kullanılmasını önlemek için bu adımı saklamalı ve `after_step` olarak geri vermelidir.
pub fn verify(secret: &str, code: &str, now: i64, after_step: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let key = decode_base32(secret)?;
    let current = now / STEP_SECS;
    (current - 1..=current + 1).filter(|step| *step > after_step).find(|step| {
        // Karşılaştırma sabit sürede yapılır.
        code_at(&key, *step).is_some_and(|expected| expected.bytes().zip(code.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0)
    })
}

/// Doğrulama uygulamalarının QR koddan okuduğu adres.
pub fn provisioning_uri(secret: &str, account: &str, issuer: &str) -> String {
    let encode = |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>().replace('+', "%20");
    format!("otpauth://totp/{}:{}?secret={secret}&issuer={}&algorithm=SHA1&digits=6&period=30", encode(issuer), encode(account), encode(issuer))
}

/// Kurtarma kodu: okunaklı, karışabilen karakterler olmadan (`xxxxx-xxxxx`).
pub fn generate_recovery_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::rngs::OsRng;
    let mut part = || (0..5).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect::<String>();
    format!("{}-{}", part(), part())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Ek B'deki sınama değerleri (SHA-1, anahtar "12345678901234567890"; son 6 hane).
    #[test]
    fn matches_rfc_test_vectors() {
        let secret = encode_base32(b"12345678901234567890");
        for (time, expected) in [(59, "287082"), (1111111109, "081804"), (1234567890, "005924"), (2000000000, "279037")] {
            assert_eq!(code_at(&decode_base32(&secret).unwrap(), time / STEP_SECS).unwrap(), expected);
            assert!(verify(&secret, expected, time, 0).is_some());
        }
    }

    #[test]
    fn rejects_wrong_replayed_and_malformed_codes() {
        let secret = encode_base32(b"12345678901234567890");
        let step = verify(&secret, "287082", 59, 0).unwrap();
        // Aynı kod ikinci kez kabul edilmez.
        assert!(verify(&secret, "287082", 59, step).is_none());
        assert!(verify(&secret, "000000", 59, 0).is_none());
        assert!(verify(&secret, "28708", 59, 0).is_none());
        assert!(verify(&secret, "28708a", 59, 0).is_none());
        // İki adımdan eski kod kabul edilmez.
        assert!(verify(&secret, "287082", 59 + 90, 0).is_none());
    }

    #[test]
    fn base32_roundtrip() {
        let secret = generate_secret();
        assert_eq!(secret.len(), 32);
        assert_eq!(encode_base32(&decode_base32(&secret).unwrap()), secret);
    }
}
