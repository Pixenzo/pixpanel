//! Alan adı doğrulama ve normalleştirme.

use crate::error::{AppError, AppResult};

/// Alan adını küçük harfe çevirir, Türkçe gibi ASCII dışı karakterleri
/// punycode'a dönüştürür ve biçimini doğrular.
///
/// Dönen değer dosya adı ve nginx yapılandırmasında güvenle kullanılabilir:
/// yalnızca `a-z`, `0-9`, `-` ve `.` içerir.
pub fn normalize(input: &str) -> AppResult<String> {
    let trimmed = input.trim().trim_end_matches('.').to_lowercase();
    let invalid = || AppError::bad(format!("Geçersiz alan adı: {}", input.trim()));
    if trimmed.is_empty() || trimmed.contains('*') {
        return Err(invalid());
    }
    let ascii = idna::domain_to_ascii(&trimmed).map_err(|_| invalid())?;

    let labels: Vec<&str> = ascii.split('.').collect();
    let label_ok = |label: &&str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let tld_ok = labels.last().is_some_and(|tld| tld.bytes().any(|b| b.is_ascii_lowercase()));
    if ascii.len() > 253 || labels.len() < 2 || !labels.iter().all(label_ok) || !tld_ok {
        return Err(invalid());
    }
    Ok(ascii)
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn accepts_and_normalizes() {
        assert_eq!(normalize(" Example.COM. ").unwrap(), "example.com");
        assert_eq!(normalize("blog.example.com").unwrap(), "blog.example.com");
        let idn = normalize("çiçek.com.tr").unwrap();
        assert!(idn.starts_with("xn--") && idn.ends_with(".com.tr") && idn.is_ascii());
    }

    #[test]
    fn rejects_dangerous_or_malformed() {
        for bad in [
            "", "localhost", "a..b.com", "-a.com", "a-.com", "*.example.com", "exa mple.com",
            "example.com;rm", "example.com/../etc", "../etc/passwd", "a.b\nserver{}", "1.2.3.4",
            "exa$mple.com", "example.com{",
        ] {
            assert!(normalize(bad).is_err(), "kabul edilmemeliydi: {bad:?}");
        }
    }
}
