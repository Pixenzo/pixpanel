# Durum

Son güncelleme: 10.11.2026

## Kararlar

| Konu | Karar | Neden |
|---|---|---|
| Arka uç | Rust, axum, rusqlite (SQLite) | Katı derleyici, tek statik ikili dosya, düşük bellek |
| Arayüz | React 19, Vite, Tailwind 4, TanStack Query | İkili dosyaya gömülür, ayrı kurulum gerekmez |
| Derleme | Yerelde `cargo zigbuild`, hedef `x86_64-unknown-linux-musl` | Test makinesi 2 çekirdek; statik ikili her Debian/Ubuntu'da çalışır |
| Web sunucusu | nginx + site başına ayrı kullanıcıyla PHP-FPM havuzu (unix soketi) | Siteler arası yalıtım |
| PHP sürümleri | sury deposu (trixie'de 5.6–8.5 mevcut) | Site başına sürüm seçimi |
| DNS | PowerDNS + gsqlite3, HTTP API | Temiz API, DNSSEC |
| FTP | pure-ftpd, sanal kullanıcılar | |
| Posta | Henüz karar verilmedi: Stalwart ya da Postfix + Dovecot + Rspamd | Stalwart'ın API'si 7 ayda iki kez kırıldı (bkz. araştırma notları) |
| SSL | Panel içinde ACME istemcisi (HTTP-01 ve joker için DNS-01), özel yükleme, kendinden imzalı | |
| Yetki | Panel root çalışır; her sistem işlemi tipli `sys` katmanından geçer | Kabuk enjeksiyonu sınıfını kökten kapatır |

## Tamamlanan

- Çekirdek: yapılandırma, SQLite + göçler, hata türü, denetim günlüğü.
- Kimlik: Argon2 parola, çerez oturumu (CSRF başlığı şartı), API anahtarları (salt okunur, IP kısıtı, süre), giriş denemesi sınırı.
- Kullanıcılar: yönetici / bayi / müşteri / alt hesap hiyerarşisi, alt hesaplara alan bazlı yetki.
- Görev sistemi: arka plan görevleri, canlı çıktı akışı (SSE).
- Sunucu: canlı ölçümler (işlemci, bellek, ağ, disk), süreç listesi ve sonlandırma, systemd servis yönetimi.
- Arayüz: giriş, genel bakış, servisler, süreçler, görevler, kullanıcılar, denetim günlüğü, hesabım; açık ve koyu tema.
- Test makinesinde uçtan uca denendi (API + tarayıcı).

## Sıradaki (öncelik sırasıyla)

1. Paket/bileşen kurucu (nginx, PHP sürümleri, MariaDB, PostgreSQL, Redis, Docker… panelden kurulum).
2. Siteler: alan adı, alt alan adı, takma ad; nginx vhost üretimi; site başına PHP sürümü ve ayarları; ters vekil; Node.js uygulamaları.
3. SSL: Let's Encrypt, özel sertifika, kendinden imzalı.
4. Dosya yöneticisi ve web terminali.
5. Veritabanları: MariaDB + PostgreSQL (veritabanı, kullanıcı, yetki, içe/dışa aktarma, phpMyAdmin).
6. DNS, FTP, zamanlanmış görevler.
7. Güvenlik duvarı (nftables), fail2ban, günlük tarayıcı.
8. Docker yönetimi.
9. Yedekleme.
10. Posta.
11. İki adımlı doğrulama, İngilizce çeviri, kurulum betiği, OpenAPI belgesi.

## Bilinen eksikler

- Panel sertifikası kendinden imzalı; panelin kendi alan adına Let's Encrypt alması SSL aşamasında gelecek.
- Otomatik test yok; doğrulama şimdilik test makinesinde elle yapılıyor.
