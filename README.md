# PixPanel

Kendi sunucunuzda çalışan web barındırma paneli. Siteleri, veritabanlarını, postayı, DNS'i ve yedekleri tek yerden yönetir; sertifika yenileme, güncelleme, yedekleme ve sağlık denetimi gibi tekrarlayan işleri kendisi yapar.

Bir [Pixenzo](https://github.com/Pixenzo) ürünüdür. Tek bir statik ikili dosya olarak kurulur; arayüz ikilinin içine gömülüdür.

## Neler yapar

- **Siteler:** nginx, site başına ayrı kullanıcıyla PHP-FPM (PHP 7.4–8.5), Node.js ve Python uygulamaları, statik siteler; Git ile yayınlama, hazırlık ortamı, site kopyalama.
- **SSL:** Let's Encrypt (HTTP-01 ve joker sertifika için DNS-01), kendi sertifikanızı yükleme, otomatik yenileme, HTTP/3.
- **Veritabanları:** MariaDB, PostgreSQL, Redis; phpMyAdmin ve Adminer; içe aktarma, uzaktan erişim (TLS zorunlu), silinenler için çöp kutusu.
- **Posta:** Postfix, Dovecot, Rspamd, Roundcube; DKIM, karantina, posta listeleri, isteğe bağlı virüs taraması.
- **DNS:** PowerDNS ile yerel bölgeler, DNSSEC, ikincil sunucu; Cloudflare bağlantısı.
- **Yedekleme:** restic ile yerel, SFTP, S3 ve FTP hedefleri; zamanlanmış planlar, tek tıkla geri yükleme.
- **WordPress:** kurulum, güncelleme, güvenlik sıkılaştırma, gece bakımı, alt klasördeki kurulumlar.
- **Hesaplar:** yönetici, bayi, müşteri ve alt hesap; paketler ve kotalar, trafik sınırı, bayi markası, iki adımlı doğrulama, kapsamlı API anahtarları.
- **Sunucu:** canlı ölçümler, servis yönetimi, güvenlik duvarı ve fail2ban, sistem güncellemeleri, sağlık merkezi ve haftalık özet.
- **Taşıma:** başka sunucudan SSH ile site taşıma; cPanel, Plesk ve HestiaCP yedek dosyalarından içe aktarma.

## Gereksinimler

Debian 13 ya da Ubuntu 26.04, x86_64, root erişimi. Temiz bir sunucu önerilir.

## Kurulum

```
curl -fsSL https://github.com/Pixenzo/pixpanel/releases/latest/download/install.sh | bash
```

Betik sürüm dosyalarının imzasını doğrular, paneli kurar ve ilk yönetici parolasını bir kez gösterir. Panel `https://<sunucu>:8443` adresinde açılır. Aynı komut yeniden çalıştırılırsa paneli son sürüme yükseltir.

Henüz yayınlanmış bir sürüm yoktur; o zamana kadar panel kaynaktan derlenip `install.sh --from-dir <klasör>` ile kurulur (bkz. [`docs/infra/surum-yayinlama.md`](docs/infra/surum-yayinlama.md)).

## Kaynaktan derleme

Arka uç Rust (axum, SQLite), arayüz React 19 + Vite + Tailwind 4.

```
cd web && pnpm install && pnpm build
cd ../server && cargo build --release
```

Sürüm paketi için: `scripts/release.sh <sürüm>`.

## Belgeler

- [`docs/DURUM.md`](docs/DURUM.md): neyin bittiği, neyin sınandığı, bilinen sınırlar
- [`docs/YOL-HARITASI.md`](docs/YOL-HARITASI.md): yol haritası
- [`docs/api.md`](docs/api.md): API uçları
- [`docs/komut-satiri.md`](docs/komut-satiri.md): `pixpanel` komut satırı
- [`docs/infra/`](docs/infra): sürüm yayınlama ve felaket kurtarma

## Lisans

Lisans henüz belirlenmedi; belirlenene kadar tüm hakları Pixenzo'ya aittir.
