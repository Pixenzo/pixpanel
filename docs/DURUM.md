# Durum

Son güncelleme: 10.11.2026 (veritabanı araçları, web postası, Redis, Node.js, vekil kuralları, FTP, DNS ve Cloudflare eklendi)

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
- Posta: Postfix + Dovecot 2.4 + Rspamd. Alan adları (DKIM anahtarı, MX/SPF/DKIM/DMARC kayıt önerileri ve canlı DNS denetimi, hepsini yakala adresi), kutular (kota, kapatma, parola), takma adlar, yönlendirme (kopyalı/kopyasız), otomatik yanıt (tarih aralıklı), kural kurucuyla filtreler (Sieve betiğine çevrilir), posta kuyruğu, istenmeyen posta eşikleri ile beyaz/kara liste, Rspamd etkinlik görünümü.
  - Hesaplar panelin veritabanında; her değişiklikte `/etc/pixpanel/mail/` altındaki düz eşleme dosyaları (texthash + Dovecot passwd-file) baştan üretilir. Postalar `/var/vmail/<alan>/<kutu>` altında, `vmail` kullanıcısıyla.
  - Yönetici dışındakiler yalnızca kendi sitelerinin alan adına posta açabilir (başkasının adresine giden postayı ele geçirmeyi önler).
  - Kimliği doğrulanan kullanıcı yalnızca kendi adresinden (ve hedefi olduğu takma adlardan) gönderebilir.
- Veritabanı araçları: MariaDB için phpMyAdmin, PostgreSQL için Adminer. Veritabanı satırından tek tıkla açılır; panel o veritabanına özel, 4 saat sonra silinen geçici bir hesap (`pxt_…`) oluşturur ve kimlik bilgisini tek kullanımlık bir anahtar dosyasıyla araca geçirir. Araçlar yalnızca 127.0.0.1:8446'da dinler; dışarıya panelin `/tools/` yolu üzerinden, panel oturumu denetlenerek açılır.
- Web postası: Roundcube, panel adresinin `/webmail/` yolunda (kendi SQLite veritabanıyla). Kutu sahipleri panel hesabı olmadan girer.
- Redis yönetimi (yalnızca yönetici): durum, bellek sınırı ve ilke, parola, AOF, anahtar tarayıcı, boşaltma. Redis ile panelin kendi küçük RESP istemcisi konuşur.
- Node.js uygulamaları: sürümler nodejs.org'un resmî derlemelerinden (sağlama toplamı doğrulanarak) `/opt/pixpanel/node/<sürüm>` altına kurulur; site başına `pixpanel-node-<kimlik>.service`, başlatma komutu, ortam değişkenleri, bellek sınırı, npm komutları, uygulama günlüğü. Port = 20000 + site kimliği.
- Yol bazlı vekil kuralları: sitenin bir yolunu başka bir adrese (ör. Docker konteyneri) iletme; önek atma ve WebSocket seçenekleri.
- Alt alan adı ekleme: site kartından ya da çalışma alanından; listede üst sitenin altında gösterilir.
- FTP: pure-ftpd, site başına sanal hesaplar; hesap sitenin klasörüne ya da alt klasörüne hapsedilir, dosyalara sitenin kullanıcısıyla erişir. FTPS açık; sistem kullanıcıları FTP ile giremez.
- DNS: bölgeler bu sunucudaki PowerDNS'te ya da Cloudflare'de durabilir; kayıtlar sağlayıcının API'siyle okunur ve yazılır (A, AAAA, CNAME, MX, TXT, NS, SRV, CAA).
  - Site eklenince A kaydı, posta açılınca MX/SPF/DKIM/DMARC kayıtları kendiliğinden eklenir (farklı değerdeki var olan kayıtlara dokunulmaz).
  - Let's Encrypt için DNS doğrulaması (DNS-01) ve joker sertifika: `_acme-challenge` kaydı eklenir, iş bitince silinir.
  - Cloudflare: API anahtarıyla bağlanma, kayıtlarda vekil (turuncu bulut) seçeneği, SSL kipi, geliştirme kipi, önbellek temizleme; nginx'te gerçek ziyaretçi adresi (yalnızca Cloudflare aralıklarına güvenilir).
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
- Posta (`scripts/e2e/mail.sh`): kimlikli gönderim (587/465), IMAP, takma ad dağıtımı, dışarıdan teslim, açık aktarmanın ve başkası adına gönderimin reddi, filtreyle klasöre taşıma, otomatik yanıt, kopyasız yönlendirme, hepsini yakala, kutuyu kapatınca oturumun hemen reddedilmesi, parola değişimi; tırnak/satır sonu/yol kaçışı içeren kural ve adların reddedildiği ya da zararsız kaldığı.
- Veritabanı araçları (`scripts/e2e/dbtools.sh`): phpMyAdmin ve Adminer'in parola sormadan açıldığı; panel oturumu olmadan 401, anahtarın ikinci kullanımda 403 verdiği; geçici hesabın yalnızca o veritabanında yetkili olduğu ve süresi dolunca silindiği; araç sunucusunun dışarıdan erişilemediği.
- Web postası (`webmail.sh`): giriş sayfasının panel oturumu olmadan açıldığı, kutu adresi ve parolasıyla oturum açılıp gelen kutusunun göründüğü, yanlış parolanın reddedildiği.
- Redis (`dbtools.sh`): ayarların uygulanıp dosyaya yazıldığı, parola koyup kaldırma, anahtar tarayıcı; anahtar adına gömülen komutun çalışmadığı.
- Node.js ve vekil kuralları (`node.sh`): sürüm kurulumu, örnek uygulamanın yayına girmesi, ortam değişkenlerinin bozulmadan ulaşması, npm betiği, başlat/durdur, önekli iletim; satır sonu ve birim dosyasına sızma denemelerinin reddedildiği.
- FTP (`ftp.sh`): bileşenin panelden sıfırdan kurulumu, FTPS ile oturum, hapis, yüklenen dosyanın sahipliği ve web'den sunulması, alt klasör hesabı; sistem kullanıcısının giremediği; kök dışını gösteren sembolik bağın reddedildiği.
- DNS (`dns.sh`): yerel bölgede kayıtların gerçekten yayınlandığı (`dig` ile), otomatik site ve posta kayıtları, kayıt ekleme/güncelleme/silme, uzun DKIM kaydının parçalı yayını, joker sertifikanın DNS doğrulamasıyla Pebble'dan alındığı, gerçek ziyaretçi adresi ayarı. Cloudflare bölümü `mock_cloudflare.py` sahte API'sine karşı sınandı.
- Müşteri hesabının başka hesabın sitesini, veritabanını ve bileşen sayfasını göremediği.
- Dosya yöneticisinde `..`, gerçek sistem yolu ve `/etc/passwd` dosyasına verilen sembolik bağın "bulunamadı" döndüğü (hapis çalışıyor).
- Kullanıcının ev klasörünün üst düzeyini (`sites` dahil) değiştiremediği.
- Terminalde bağlantı kopunca kabuk sürecinin arkada kalmadığı; yabancı kaynaktan gelen WebSocket isteğinin reddedildiği.
- Cron komutuna satır sonu ekleyerek root adına satır yazma denemesinin reddedildiği.

## Test ortamı notları

- Uçtan uca sınamalar: `scripts/e2e/features.sh` (PHP genel ayarları, bildirimler, Git yayınlama, Docker). Test makinesine kopyalanıp `PX_USER=… PX_PASS=… bash features.sh [php git docker]` ile çalışır; `mocks.py` sahte Telegram (9911) ve SMTP (2525) sunucusudur, `/etc/pixpanel/config.toml` içindeki `telegram_api` ona bakar.
- `/etc/pixpanel/config.toml` içinde sınama adresleri tanımlı: `telegram_api` (9911) ve `cloudflare_api` (9912, `scripts/e2e/mock_cloudflare.py`); canlıda bu satırlar olmamalı.
- DNS sınaması Pebble'ı geçici olarak yerel DNS sunucusuna yöneltir (`-dnsserver 127.0.0.1:53`) ve iş bitince geri alır; `deneme.test` yerel bölgesi ve joker sertifikası yerinde duruyor.
- Posta sınaması `scripts/e2e/mail.sh` + `mailflow.py`; test makinesinde `deneme.test` için örnek kutular (bilgi, destek) duruyor. Makinenin gerçek bir alan adı ve açık 25. portu olmadığından dış sunucularla alışveriş (gerçek teslim, SPF/DKIM doğrulaması) denenmedi.
- Git sınaması için makinede `gitsrv` kullanıcısı ve `/home/gitsrv/demo.git` deposu var.

- Test makinesinde `/etc/pixpanel/config.toml` paneli Pebble'a yönlendirir (`acme_directory`, `acme_root_ca`); gerçek Let's Encrypt için bu dosya silinmelidir.
- Pebble `/opt/pebble` altında, `pebble-test.service` olarak çalışır; `/etc/hosts` içinde `*.test` adları 127.0.0.1'e yönlenir.

## Sıradaki (öncelik sırasıyla)

1. Güvenlik duvarı (nftables), fail2ban. FTP'nin pasif portları (30000–30099), posta ve DNS portları için kurallar buraya bağlanacak.
2. Yedekleme (siteler, veritabanları, posta; uzak hedefler).
3. Posta eksikleri: posta sunucusu adı için kendiliğinden Let's Encrypt, karantina (şu an şüpheli iletiler Junk klasörüne gider), kutu sahibinin kendi kurallarını ve parolasını web postasından yönetebilmesi, hesap başına kutu sınırı.
4. DNS eksikleri: DNSSEC, ikincil ad sunucusu (AXFR), bölge içe aktarma, Cloudflare'de SRV/CAA yazma, "yalnızca Cloudflare" kipi.
5. İki adımlı doğrulama, İngilizce çeviri, kurulum betiği, OpenAPI belgesi.

## Bilinen eksikler

- Cloudflare bütünleşmesi gerçek bir Cloudflare hesabıyla denenmedi; yalnızca API'nin belgelenen biçimini taklit eden sahte sunucuya karşı sınandı.
- Web postası, phpMyAdmin, Adminer ve PowerDNS paketleri test makinesine elle kuruldu; panelden sıfırdan kurulumları (Bileşenler sayfası) ayrıca denenmeli. FTP ve Node.js panelden kurularak sınandı.
- Redis'te hesaplar arası yalıtım yok; bu yüzden yalnızca yönetici yönetebiliyor. Hesap başına Redis (ACL ya da ayrı örnek) yok.
- Node.js sürümleri panelden güncellenemiyor (yeniden kurmak en son yamayı getirir); uygulamalar güncelleme sonrası elle yeniden başlatılmalı.
- Web postasında kullanıcı kendi parolasını ve filtrelerini değiştiremiyor (panelden yapılır).

- Posta bileşeninin sıfırdan kurulumu (Bileşenler sayfasından) denenmedi; test makinesinde paketler elle kuruldu, yapılandırma `POST /mail/setup` ile uygulandı.
- Docker, PHP, Ayarlar ve Posta sayfalarının arayüzü tarayıcıda açılıp gözle denetlendi ama her düğmesi tek tek tıklanarak sınanmadı; arka uçları `scripts/e2e` ile sınandı.
- Web kancası yalnızca adresteki gizli anahtarla korunuyor; GitHub imza başlığı (`X-Hub-Signature-256`) doğrulanmıyor.

- Panel sertifikası kendinden imzalı; panelin kendi alan adına Let's Encrypt alması SSL aşamasında gelecek.
- Birim testleri az (7 test: alan adı, sertifika, SQL kaçışlama, cron doğrulama) ve `scripts/test.sh` ile test makinesinde çalışıyor; kod Linux'a özgü olduğundan Windows'ta derlenmiyor.
- Uçtan uca doğrulama test makinesinde elle (betiklerle) yapılıyor; otomatik bir test takımı yok.
- Hosting hesapları terminal kullanamıyor (yalnızca yönetici); hesap başına kabuk/SFTP izni eklenecek.
- PostgreSQL'e uzaktan bağlantı ayarı (pg_hba) panelden yapılamıyor.
- Dosya yöneticisinde arşiv açarken boyut sınırı yok; disk kotası da henüz yok.

- Test makinesinde `allowed_origins` içinde geliştirme sunucusu tanımlı; canlıda boş olmalı.
