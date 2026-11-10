# Durum

Son güncelleme: 10.11.2026 (PixPanel adı, yeni arayüz, PHP genel ayarları, Git yayınlama, Docker)

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
| Posta | Postfix + Dovecot + Rspamd, web postası için hazır bir istemci | mailcow aynı yazılımları ~15 konteyner ve ~6 GB bellekle paketliyor, 80/443'ü ve hesapları kendi yönetmek istiyor; Stalwart'ın API'si 7 ayda iki kez kırıldı |
| Ad | PixPanel (Pixenzo ürünü) | Kullanıcı kararı (10.11.2026); eski çalışma adı Capsule'dü, yerel klasör adı hâlâ `capsule` |
| Git ile yayınlama | Site klasörü deponun çalışma kopyası; `fetch` + `checkout -f`, ardından hesabın betiği | İzlenmeyen dosyalar (yüklemeler, `.env`) yerinde kalır; her şey site sahibinin kullanıcısıyla yürür |
| Docker | `docker` komutu üzerinden, yalnızca yönetici | Docker erişimi root yetkisidir; soket API'si yerine CLI daha az kod ve daha az kırılganlık |
| SSL | Panel içinde ACME istemcisi (HTTP-01 ve joker için DNS-01), özel yükleme, kendinden imzalı | |
| Yetki | Panel root çalışır; her sistem işlemi tipli `sys` katmanından geçer | Kabuk enjeksiyonu sınıfını kökten kapatır |

## Tamamlanan

- Çekirdek: yapılandırma, SQLite + göçler, hata türü, denetim günlüğü.
- Kimlik: Argon2 parola, çerez oturumu (CSRF başlığı şartı), API anahtarları (salt okunur, IP kısıtı, süre), giriş denemesi sınırı.
- Kullanıcılar: yönetici / bayi / müşteri / alt hesap hiyerarşisi, alt hesaplara alan bazlı yetki.
- Görev sistemi: arka plan görevleri, canlı çıktı akışı (SSE).
- Sunucu: canlı ölçümler (işlemci, bellek, ağ, disk), süreç listesi ve sonlandırma, systemd servis yönetimi.
- Bileşen kurucu: nginx, PHP 7.4–8.5 (sury), MariaDB, PostgreSQL, Redis, Docker panelden kurulur.
- Siteler: PHP / statik / ters vekil; takma adlar, yayın klasörü, askıya alma, silme; yöneticiye özel nginx yönergeleri.
  - Her hesap kendi Linux kullanıcısıyla çalışır; site klasörleri diğer hesaplara kapalıdır.
  - Site başına PHP sürümü ve ayarları (bellek, yükleme, süre, open_basedir, exec, ek yönergeler); sürüm geçişi kesintisiz.
  - nginx ve PHP-FPM yapılandırması uygulanmadan önce sınanır, hata hâlinde geri alınır.
- SSL: Let's Encrypt (HTTP-01), özel sertifika yükleme, kendinden imzalı; HTTPS zorlama, HSTS; 30 gün kala otomatik yenileme.
- Veritabanları: MariaDB ve PostgreSQL; veritabanı, kullanıcı, üç düzeyli yetki (tam / okuma-yazma / salt okunur), dışa ve içe aktarma (.sql, .sql.gz).
- Dosya yöneticisi: gezinme, yükleme (ilerleme çubuğu, sürükle-bırak), indirme (klasör tar.gz olarak), kes/kopyala/yapıştır, yeniden adlandırma, izinler, zip ve tar.gz sıkıştırma/açma, sözdizimi renklendirmeli düzenleyici.
  - İşlemler hesabın klasörüne `chroot` yapıp hesabın kimliğine düşen ayrı bir süreçte yürür (`server/src/fsop.rs`).
- Web terminali: root ya da seçilen hesap adına kabuk (yalnızca yönetici).
- Zamanlanmış görevler: hesap başına cron, hazır zamanlamalar, "şimdi çalıştır".
- Günlükler: site erişim/hata günlükleri ve servis günlükleri; arama ve canlı izleme.
- PHP genel ayarları: sürüm başına varsayılanlar (`/etc/pixpanel/php/<sürüm>.json` + `conf.d/99-pixpanel.ini`); siteler tek tek değerleri devralır ya da ezer; eklenti açma/kapama ve kurma.
- Git ile yayınlama: depo + dal + yayın betiği, site başına SSH yayın anahtarı, web kancası (`/api/v1/hooks/deploy/<anahtar>`), yayın geçmişi ve günlüğü, eski sürüme dönüş; yayın sürerken gelen istekler tek bir sıradaki yayına indirgenir.
- Bildirimler: SMTP (yönetici tanımlar) ve Telegram (hesap kendi botunu tanımlar); yayın başarılı/başarısız ve sertifika yenileme hatası olayları.
- Docker: konteyner (oluştur, başlat/durdur, günlük, ayrıntı, içinde terminal), imaj, birim, ağ ve compose yığınları (`/opt/pixpanel/stacks/<ad>/compose.yaml`); yayınlanan portu tek tıkla ters vekil sitesine bağlama.
- Arayüz düzeni (Plesk "power user" görünümünden esinle): ana sayfa siteler; her site, araçlarını ve o anki durumlarını gösteren bir kart. Site çalışma alanında sol bölüm menüsü. Ctrl+K ile site/araç/sayfa araması. Koyu yan menü.
- Arayüz: giriş, genel bakış, siteler ve site ayrıntısı (genel, PHP, vekil, SSL, gelişmiş), veritabanları, dosyalar, zamanlı görevler, günlükler, bileşenler, servisler, süreçler, terminal, görevler, kullanıcılar, denetim günlüğü, hesabım; açık ve koyu tema.

## Test makinesinde doğrulananlar

- PHP sitenin kendi kullanıcısıyla çalıştığı; başka hesabın dosyalarını ve `/etc/passwd` dosyasını okuyamadığı.
- `.git`, `.env`, `*.sql` gibi dosyaların dışarıya kapalı olduğu; tanımsız alan adına bağlantının kapatıldığı.
- Sürekli istek altında 6 PHP sürüm geçişinde 400 isteğin 400'ünün başarılı olduğu.
- Alan adı, PHP yönergesi, vekil hedefi, yayın klasörü ve parola alanlarına enjeksiyon denemelerinin reddedildiği.
- Bozuk nginx yönergesi ve anahtarı uyuşmayan sertifikanın uygulanmadığı, sitenin çalışmaya devam ettiği.
- Let's Encrypt akışının Pebble (resmî sınama sunucusu) ile uçtan uca çalıştığı; doğrulanamayan alan adında sitenin etkilenmediği.
- Tırnak, ters eğik çizgi ve kabuk karakterleri içeren veritabanı parolasının iki motorda da çalıştığı.
- Genel PHP ayarının siteye devralındığı, sitenin kendi değeriyle ezebildiği; geçersiz saat dilimi ve satır sonlu yönergenin reddedildiği.
- Git: yayın anahtarıyla SSH üzerinden çekme, betiğin site kullanıcısıyla çalışması, web kancasında başka dalın atlanması ve art arda gelen çağrıların sıraya alınması, eski sürüme dönüş, hatalı betikte yayının başarısız sayılıp e-posta ve Telegram bildirimi gitmesi; `ext::`, `--upload-pack`, `file://`, parolalı adres ve seçenek gibi dal adlarının reddedildiği.
- Docker: konteyner ve compose yığını oluşturma/kaldırma; `--privileged` gibi seçenek enjeksiyonu ve birim kaçışı denemelerinin reddedildiği; ortam değişkeni değerlerinin görev günlüğüne yazılmadığı.
- Müşteri hesabının başka hesabın sitesini, veritabanını ve bileşen sayfasını göremediği.
- Dosya yöneticisinde `..`, gerçek sistem yolu ve `/etc/passwd` dosyasına verilen sembolik bağın "bulunamadı" döndüğü (hapis çalışıyor).
- Kullanıcının ev klasörünün üst düzeyini (`sites` dahil) değiştiremediği.
- Terminalde bağlantı kopunca kabuk sürecinin arkada kalmadığı; yabancı kaynaktan gelen WebSocket isteğinin reddedildiği.
- Cron komutuna satır sonu ekleyerek root adına satır yazma denemesinin reddedildiği.

## Test ortamı notları

- Uçtan uca sınamalar: `scripts/e2e/features.sh` (PHP genel ayarları, bildirimler, Git yayınlama, Docker). Test makinesine kopyalanıp `PX_USER=… PX_PASS=… bash features.sh [php git docker]` ile çalışır; `mocks.py` sahte Telegram (9911) ve SMTP (2525) sunucusudur, `/etc/pixpanel/config.toml` içindeki `telegram_api` ona bakar.
- Git sınaması için makinede `gitsrv` kullanıcısı ve `/home/gitsrv/demo.git` deposu var.

- Test makinesinde `/etc/pixpanel/config.toml` paneli Pebble'a yönlendirir (`acme_directory`, `acme_root_ca`); gerçek Let's Encrypt için bu dosya silinmelidir.
- Pebble `/opt/pebble` altında, `pebble-test.service` olarak çalışır; `/etc/hosts` içinde `*.test` adları 127.0.0.1'e yönlenir.

## Sıradaki (öncelik sırasıyla)

1. Posta: Postfix + Dovecot + Rspamd; kutular, kota, takma adlar, yönlendirme, otomatik yanıt, Sieve filtreleri, DKIM/SPF/DMARC, karantina, web postası.
2. Güvenlik duvarı (nftables), fail2ban.
3. DNS (PowerDNS), FTP (pure-ftpd).
4. Node.js uygulamaları, phpMyAdmin.
5. Yedekleme.
6. İki adımlı doğrulama, İngilizce çeviri, kurulum betiği, OpenAPI belgesi.

## Bilinen eksikler

- Panel sertifikası kendinden imzalı; panelin kendi alan adına Let's Encrypt alması SSL aşamasında gelecek.
- Birim testleri az (7 test: alan adı, sertifika, SQL kaçışlama, cron doğrulama) ve `scripts/test.sh` ile test makinesinde çalışıyor; kod Linux'a özgü olduğundan Windows'ta derlenmiyor.
- Uçtan uca doğrulama test makinesinde elle (betiklerle) yapılıyor; otomatik bir test takımı yok.
- Hosting hesapları terminal kullanamıyor (yalnızca yönetici); hesap başına kabuk/SFTP izni eklenecek.
- PostgreSQL'e uzaktan bağlantı ayarı (pg_hba) panelden yapılamıyor.
- Dosya yöneticisinde arşiv açarken boyut sınırı yok; disk kotası da henüz yok.

- Test makinesinde `allowed_origins` içinde geliştirme sunucusu tanımlı; canlıda boş olmalı.
