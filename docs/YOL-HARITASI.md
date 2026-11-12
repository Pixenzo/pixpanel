# Yol haritası

Son güncelleme: 10.11.2026

Plesk'te olup PixPanel'de olmayanların, en kritikten en sıradana doğru sıralanmış iş paketleri.
Her paket tek başına bir alt ajana verilebilecek biçimde yazıldı. Tamamlananlar `docs/DURUM.md` içinde.

## Nasıl okunur

- **Öncelik:** P0 = bunlar olmadan gerçek sunucuya konmaz. P1 = müşteriye açmak için gerekli. P2 = Plesk'le aradaki fark. P3 = ürünleşme.
- **Büyüklük:** K (tek oturum), O (birkaç oturum), B (parçalara bölünmeli).
- **Devir:** paketin alt ajana ne kadar uygun olduğu.
  - *Arayüz* işleri bu projede alt ajanla sorunsuz yürüdü (Docker, Posta, DNS, Redis sayfaları öyle yazıldı).
  - *Sistem katmanı* işleri (nftables, sshd, yedekleme) test makinesinde deneme-yanılma ister; arka ucu ana oturum yazar ya da ajan yazar, ana oturum dağıtıp sınar.

## Alt ajanlara devir kuralları

Bu kurallar her paketin brifine eklenir.

1. **Önce oku:** `CLAUDE.md` (dil ve güvenlik kuralları), bu dosyadaki paket, pakette "örnek al" denen modül.
2. **Dil:** kod İngilizce; yorum, belge ve kullanıcıya görünen metin Türkçe; arayüzde her metin `t("…")` içinde.
3. **Güvenlik (pazarlık yok):** komutlar yalnızca `sys::exec` ile, argüman dizisi olarak; kullanıcı girdisi beyaz listeyle doğrulanır; yapılandırma dosyası yazıldıktan sonra sınanır, hata olursa eski hâli geri konur; kullanıcının klasöründe iş `sys::runas` ile yapılır; her uç `AuthUser` alır, sahiplik `owner_id()` ile denetlenir; değişiklikler `auth::audit` ile kaydedilir.
4. **Ortak dosyalara dokunma:** `db.rs` (göçler), `api/mod.rs`, `main.rs`, `components.rs`, `web/src/main.tsx`, `web/src/layout/navigation.ts`, `web/src/lib/siteTools.ts`. Ajan yeni dosyalarını yazar ve bu dosyalara eklenmesi gereken satırları raporunda verir; birleştirmeyi ana oturum yapar. (Aynı anda çalışan ajanların çakışmasını bu önler.)
5. **Test makinesi tektir:** ajan `scripts/deploy-dev.sh` çalıştırmaz. Derleme denetimi için `cargo zigbuild --target x86_64-unknown-linux-musl` ve `pnpm typecheck` yeter. Dağıtım ve uçtan uca sınama ana oturumda, sırayla yapılır.
6. **Her paket bir `scripts/e2e/<ad>.sh` ile gelir:** mutlu yol, yetki denetimi (oturumsuz 401, başka hesabın kaynağı 404) ve enjeksiyon denemeleri (satır sonu, tırnak, `../`, seçenek gibi görünen ad). Örnek: `scripts/e2e/ftp.sh`.
7. **Git Bash tuzağı:** Türkçe karakter ya da kesme işareti içeren metni heredoc ile yazma; dosyayı Write aracıyla yaz.
8. **Rapor:** ne yapıldı, ne denenmedi, ortak dosyalara eklenecek satırlar.

---

## P0 — Canlıya çıkmadan gerekli

### 1. Güvenlik duvarı (nftables) — TAMAMLANDI (10.11.2026)

- **Amaç:** sunucuya yalnızca gereken portlardan erişilsin; kurallar panelden yönetilsin.
- **Kapsam:**
  - `inet pixpanel` tablosu; panel yalnızca kendi tablosuna dokunur (Docker'ın ve kullanıcının kurallarını bozmaz).
  - Hazır kural kümeleri, kurulu bileşenlere göre: SSH, panel (8443), web (80/443), posta (25, 465, 587, 143, 993, 4190), DNS (53 tcp/udp), FTP (21 ve 30000–30099).
  - Özel kurallar: port ya da aralık, tcp/udp, kaynak adres ya da ağ, izin/engel; adres engelleme listesi.
  - **Kilitlenmeye karşı:** yeni kural seti uygulanır, 60 saniye içinde panelden onaylanmazsa eski sete dönülür. SSH ve panel portu için "kendini dışarıda bırakma" denetimi.
  - Bileşen kurulunca ilgili küme kendiliğinden açılır (FTP kurulunca pasif portlar gibi).
- **Kapsam dışı:** IPv6'ya özel kurallar (aynı küme `inet` ailesiyle ikisini de kapsar), hız sınırlama.
- **Örnek al:** `hosting/nginx.rs` (yaz, sına, geri al deseni).
- **Kabul:** dışarıdan kapalı porta bağlantı reddedilir; onaylanmayan kural seti 60 saniyede geri alınır; Docker konteynerinin yayınladığı port çalışmaya devam eder; LXC içinde çalışır.
- **Risk:** yetkisiz LXC'de nftables davranışı; önce elle doğrulanmalı.

### 2. fail2ban — TAMAMLANDI (10.11.2026)

- **Amaç:** parola deneme saldırıları kendiliğinden engellensin.
- **Kapsam:** hapisler: sshd, panel girişi, dovecot, postfix-sasl, pure-ftpd, roundcube, phpMyAdmin. Panel başarısız girişleri fail2ban'in okuyabileceği sabit biçimde günlüğe yazar. Arayüz: engelli adresler, engeli kaldırma, beyaz liste, hapis başına deneme sayısı ve süre.
- **Bağımlılık:** 1 (engeller `pixpanel` tablosundaki bir kümeye yazılır).
- **Kabul:** N yanlış parola denemesinden sonra adres engellenir (SSH, panel, IMAP, FTP için ayrı ayrı); panelden kaldırılır; beyaz listedeki adres engellenmez.

### 3. Giriş güvenliği — TAMAMLANDI (10.11.2026; panele adres kısıtı güvenlik duvarındaki panel kümesiyle yapılıyor, parola ilkesi ve oturumları toplu kapatma kaldı)

- **Kapsam:**
  - İki adımlı doğrulama (TOTP; `users.totp_secret` sütunu hazır), kurtarma kodları, yöneticinin başkasının iki adımını sıfırlaması.
  - Panel erişimini adres listesiyle daraltma (kilitlenmeye karşı komut satırından kapatma: `pixpanel` alt komutu).
  - Parola ilkesi, oturumları toplu kapatma, giriş bildirimi (yeni adresten giriş).
- **Kabul:** kod olmadan giriş reddedilir; kurtarma kodu bir kez çalışır; API anahtarları iki adımdan etkilenmez; adres listesi dışından 403.

### 4. Panelin ve postanın sertifikası — TAMAMLANDI (10.11.2026)

- **Kapsam:** panelin kendi alan adı için Let's Encrypt (HTTP-01 nginx üzerinden ya da DNS-01); sertifika değişince panel yeniden başlamadan yeni sertifikayı kullanır; posta servisleri (Postfix, Dovecot) ve FTP aynı sertifikayı alır ve yeniden yüklenir; kendiliğinden yenileme.
- **Örnek al:** `api/ssl.rs`, `hosting/acme.rs`, `mail::tls_paths`.
- **Kabul:** Pebble ile panel sertifikası alınır, tarayıcı yeniden başlatma olmadan yeni sertifikayı görür; IMAP ve SMTP aynı sertifikayı sunar.

### 5. Yedekleme ve geri yükleme — TAMAMLANDI (10.11.2026; kalanlar: S3'ün gerçek depoyla sınanması, PostgreSQL/posta geri yükleme sınaması, tam sunucu kurtarma komutu, müşteri hesaplarına açma)

- **Karar (önerilen):** depolama için **restic** (yinelemesiz, şifreli, artımlı; yerel klasör, SFTP, S3 uyumlu hedefler). Panel restic'i çağırır, kendi biçimini uydurmaz.
- **Parçalar:**
  - 5a. Hedefler (yerel, SFTP, S3) ve depo anahtarı yönetimi; bağlantı sınaması.
  - 5b. Ne yedeklenir: site dosyaları, veritabanı dökümleri (MariaDB, PostgreSQL), posta kutuları, panelin veritabanı ve `/etc/pixpanel`. Hesap ya da site bazında seçim.
  - 5c. Zamanlama ve saklama ilkesi (günlük/haftalık/aylık kaç kopya); başarısızlıkta bildirim (`notify` olayı).
  - 5d. Geri yükleme: tam site, yalnızca dosyalar, yalnızca veritabanı, tek dosya; başka bir alan adına geri yükleme (klon).
  - 5e. Felaket kurtarma: boş sunucuya panelin kendisini geri yükleme (belge + komut).
  - 5f. Arayüz: hedefler, planlar, yedek listesi, geri yükleme sihirbazı, canlı günlük.
- **Kabul:** site silinir, yedekten geri gelir (dosya sahipliği, veritabanı içeriği, posta dahil); bozuk hedefte görev başarısız olur ve bildirim gider; saklama ilkesi eski kopyaları siler.
- **Tasarım (10.11.2026, uygulandı; döküm dosya adı `/<motor>-<ad>.sql` oldu):**
  - İlk sürüm yalnız yönetici; restic ≥ 0.17 (Debian 13'te 0.18; Ubuntu 24.04'teki 0.16 yetmez). Bileşen kataloğuna `restic` eklenir.
  - Modüller: `backup/mod.rs` (türler, doğrulama, zamanlama), `backup/restic.rs` (restic çağrıları; parola ve anahtarlar yalnız ortam değişkeniyle), `backup/run.rs` (yedek ve geri yükleme görevleri), `api/backup.rs`; göç 12: `backup_targets`, `backup_plans`.
  - Hedef başına tek depo ve panelin ürettiği depo parolası (var olan depoyu bağlamak için parola verilebilir). SFTP için hedefe özel ed25519 anahtarı (`/etc/pixpanel/backup/target-<id>.key`), S3 için `AWS_*` ortam değişkenleri.
  - Her birim ayrı anlık görüntü, etiketlerle: `kind:site|db|mail|panel`, `name:…`, `engine:…`, `owner:…`, `plan:<id>` ya da `manual`. Site: site klasörü + sertifika klasörü + `site.json` bildirimi. Veritabanı: `--stdin-from-command` ile döküm akışı (`/databases/<motor>/<ad>.sql`). Posta: `/var/vmail/<alan>`. Panel: `VACUUM INTO` kopyası + `/etc/pixpanel`.
  - Saklama: `restic forget --tag plan:<id> --group-by tags --keep-daily/weekly/monthly --prune`. Elle alınan yedekler kendiliğinden silinmez.
  - Zamanlama: günlük/haftalık/aylık + saat; `tasks.rs` içinde dakikalık döngü, `next_run_at` sütunu. Başarısızlıkta `backup_failed` bildirimi.
  - Geri yükleme: site yerinde (tümü ya da seçilen yollar, isteğe bağlı fazlalıkları silme), başka siteye (klon), silinmiş siteyi bildirimden yeniden kurma; veritabanı aynı ya da başka veritabanına (PostgreSQL dökümü `--clean --if-exists` ile); posta yerinde. Posta alanını yeniden oluşturma ve panelin kendisini geri yükleme ilk sürümde belge + komut.
  - Uçlar: `GET /backup`, hedefler (`POST/PUT/DELETE`, `/check`, `/password`, `/snapshots`, `/snapshots/{id}/files`), planlar (`POST/PUT/DELETE`, `/run`), `POST /backup/run`, `POST /backup/restore`.

### 6. Kurulum betiği ve güncelleme — TAMAMLANDI (10.11.2026; kalanlar: ilk sürümün yayınlanması ve depo kapalıyken dağıtım kararı, arm64)

- **Kapsam:** `install/install.sh` (tek komut; Debian 13 ve Ubuntu LTS; sürüm ikilisini indirir, sağlama toplamını doğrular, servis birimini kurar, ilk yöneticiyi oluşturur, nginx'i kurar); sürüm derlemesi (`--release`, x86_64 ve arm64); panelin kendini güncellemesi (yeni ikiliyi indir, doğrula, değiştir, başlamazsa eskisine dön); kaldırma betiği.
- **Bağımlılık:** sürümlerin nerede yayınlanacağı kararı (aşağıda "Kararlar").
- **Kabul:** temiz Debian 13 ve Ubuntu makinede tek komutla çalışan panel; bilerek bozulmuş güncellemede eski sürüme dönüş.

### 7. Sınama altyapısı — TAMAMLANDI (10.11.2026; kalan: Cloudflare'in gerçek hesapla bir kez elle doğrulanması, temiz makinede Let's Encrypt)

- **Neden P0:** bileşenlerin sıfırdan kurulumu bugün yalnızca FTP ve Node.js için sınandı; her değişiklikte hepsini elle koşturmak sürdürülemez.
- **Kapsam:** `scripts/e2e/run-all.sh` (bütün betikler, özet tablo); Proxmox'ta test konteynerinin anlık görüntüsünü alıp geri dönen betik (temiz makinede bileşen kurulumlarını sınamak için); Ubuntu konteyneri; posta, phpMyAdmin, Adminer, Roundcube, PowerDNS bileşenlerinin panelden sıfırdan kurulum sınaması; Cloudflare'in gerçek hesapla bir kez elle doğrulanması.

---

## Ö — Öne alınanlar (kullanıcı isteği, 10.11.2026)

Kullanıcının sözü: "profesyonel şık ve premium duran, profesyonel bir web yönetim paneli istiyorum. Webcilerin normalde sürekli yapması gereken şeyleri otomatik yapıp halledecek bir sistem olmalı" ve "kaliteli, mobil uyumlu, anlaşılır, kullanılabilirliği yüksek, realtime özellikleri olan bir panel". Bu iki iş P1'in önüne alındı.

### Ö1. Arayüzün elden geçirilmesi — TEMELİ TAMAMLANDI (11.11.2026; kalan: sayfaların tek tek inceltilmesi, giriş sayfası, müşteriye başkasının yaptığı değişikliklerin de akması)

- **Canlı olay akışı:** tek bir SSE bağlantısı (`GET /api/v1/events`). Sunucu yükü (2 sn), görevlerin başlaması/bitmesi ve "şu kaynak değişti" bildirimleri akar; arayüz ilgili sorguları kendiliğinden yeniler. Başka sekmede ya da başka hesabın yaptığı değişiklik ekrana yansır, biten görev her sayfada bildirim verir, üst çubukta bağlantı göstergesi durur. Sayfalardaki sabit aralıklı yoklamalar kaldırılır ya da yedek olarak seyreltilir. Olaylar hesabın görebildiği kaynaklarla sınırlanır.
- **Telefon düzeni:** tablolar dar ekranda kart satırlarına döner; altta sabit gezinme çubuğu; pencereler alttan açılan sayfa olur; dokunma hedefleri en az 40 px; yatay kaydırma kalmaz.
- **Görsel dil:** var olan kimlik (IBM Plex, koyu yan gezinme, petrol mavisi vurgu) korunur ve inceltilir: tutarlı boşluk ve köşe ölçeği, sayfa başlıkları, boş durumlar, yükleme iskeletleri, klavye odağı.
- **Kabul:** her sayfa 390 px genişlikte yatay kaydırma olmadan kullanılabilir; iki sekmeden biri bir kaynağı değiştirince öteki 2 sn içinde güncellenir; bağlantı kopunca arayüz bunu gösterir ve kendiliğinden yeniden bağlanır.

### Ö2. Kendi kendine bakım ("Sağlık merkezi") — İLK SÜRÜM TAMAMLANDI (11.11.2026; kalan: haftalık özet, riskli işlemden önce yedek, yeni siteye varsayılan yedek planı, kara liste denetimi, müşteriye kendi sitesinin durumu)

Panelin ayırt edici özelliği: yöneticinin düzenli olarak elle yaptığı denetimleri panel sürekli yapar, güvenli olanları kendisi düzeltir, kalanını tek tıkla düzeltilecek biçimde sunar.

- **Denetim motoru:** kurallar düzenli koşar, her bulgu bir önem derecesi, açıklama ve varsa "Düzelt" eylemi taşır. İlk kurallar:
  - sertifikası olmayan ya da süresi yaklaşan site; DNS'i bu sunucuyu göstermeyen alan adı; sertifika alınabilecek durumda olup alınmamış site (DNS doğruysa kendiliğinden Let's Encrypt)
  - son 24 saatte yedeği olmayan site/veritabanı; hiç yedek planı olmayan sunucu; başarısız son yedek
  - çökmüş ya da durmuş servis (kendiliğinden yeniden başlatma, üst üste çökerse bildirim)
  - disk, bellek ve inode doluluğu; büyüyen günlükler (kendiliğinden döndürme); eski yedek/önbellek temizliği önerisi
  - bekleyen güvenlik güncellemeleri (işletim sistemi, panel); desteği bitmiş PHP sürümünde çalışan site
  - yanıt vermeyen site (dakikalık HTTP denetimi, 5xx oranı), yavaşlayan site
  - posta: kuyrukta bekleyen iletiler, eksik SPF/DKIM/DMARC, kara listeye girmiş sunucu adresi
  - güvenlik: parolayla root SSH girişi açık, güvenlik duvarı kapalı, iki adımlı doğrulaması olmayan yönetici
- **Kendiliğinden yapılanlar (her biri ayrı ayrı açılıp kapatılabilir):** sertifika alma/yenileme, servis yeniden başlatma, günlük döndürme, güvenlik güncellemeleri (unattended-upgrades), riskli işlemden önce yedek (PHP sürümü değişimi, güncelleme, geri yükleme), yeni siteye varsayılan yedek planı.
- **Raporlama:** her kendiliğinden işlem denetim günlüğüne ve bildirim kanalına düşer; haftalık özet e-postası (ne denetlendi, ne düzeltildi, ne bekliyor).
- **Arayüz:** giriş sayfası sunucunun genel sağlık durumunu ve bekleyen bulguları gösterir; her site kartında o sitenin durumu.
- **Kabul:** servis elle öldürülünce bir dakika içinde ayağa kalkar ve bildirim gelir; DNS'i doğru yeni sitede sertifika kendiliğinden alınır; bulgular düzeltilince listeden kendiliğinden düşer.

Ardından gelecek otomasyonlar P2'deki WordPress araçlarıyla birleşir (tek tıkla kurulum, çekirdek/eklenti güncellemelerinin yedek alıp sağlık denetimiyle yapılması, hazırlık kopyası).

---

### Ö3. VestaCP'de olup bizde olmayanlar (kullanıcı isteği, 11.11.2026)

Kullanıcının sözü: "Birden çok ip, komut satırı, dns kümesi, uzun dönem grafikler, site şablonları, ssh sftp erişimi, müşteri kendi yedeği, paketler ve kotalar, trafik ve ziyaretçi istatistikleri, çoklu dil, ftp ye yedek özelliklerinin hepsini ekleyelim". Yürütme sırası:

1. Paketler ve kotalar (aşağıdaki 8. madde)
2. Müşterinin kendi yedeği: hesap kendi sitelerinin, veritabanlarının ve postasının yedeğini görür, alır, geri yükler
3. SSH/SFTP erişimi (9. madde)
4. Trafik ve ziyaretçi istatistikleri (10. madde)
5. Uzun dönem sunucu grafikleri: gün, hafta, ay, yıl
6. FTP'ye yedek: düz FTP/FTPS yedek hedefi
7. Site şablonları: uygulamaya göre hazır nginx ayarı
8. Komut satırı: panelin işlemlerini betikten yapabilmek — **tamamlandı**
9. Birden çok IP: sunucuya IP ekleme, siteye özel IP — **tamamlandı**
10. DNS kümesi: ikincil ad sunucusuna eşitleme (18. madde) — **tamamlandı**
11. Çoklu dil: İngilizce çeviri (alt ajana verildi)

---

## P1 — Müşteriye açmak için gerekli

### 8. Paketler ve kotalar — TAMAMLANDI (11.11.2026; kalan: trafik sınırı, izinli PHP sürümleri/özellikler, bayinin toplam kullanımı, veritabanı boyutlarının diske sayılması)

- **Kapsam:** hizmet paketi (site, alt alan adı, veritabanı, posta kutusu, FTP hesabı, DNS bölgesi sayısı; disk ve aylık trafik sınırı; izinli PHP/Node.js sürümleri; izinli özellikler); hesaba paket atama; sınır aşımında oluşturmayı reddetme; kullanım görünümü (hesap ve bayi için).
- **Disk kotası kararı:** dosya sistemi kotası LXC'de güvenilir değil; panel `du` ile düzenli ölçer, sınır aşılınca uyarır ve yüklemeyi/yeni kaynak açmayı engeller ("yumuşak kota"). Gerçek kota ayrı bir iş.
- **Kabul:** sınırdaki hesap yeni site açamaz; bayi kendi müşterilerine kendi sınırını aşan paket veremez.

### 8a. Hesap başına Redis — TAMAMLANDI (11.11.2026; kalan: bellek sınırının pakete bağlanması)

- **Karar (10.11.2026):** ACL/önek yerine hesap başına ayrı Redis örneği. Site başına değil hesap başına; siteler veritabanı numarasıyla ayrılır.
- **Kapsam:** isteğe bağlı açılır ("Redis'i aç"); `pixpanel-redis-<hesap>.service` hesabın kendi kullanıcısıyla çalışır (Node.js birimleriyle aynı düzen); varsayılan bağlantı hesaba özel port + panelin ürettiği uzun parola (`127.0.0.1:<port>`; uygulamaların çoğu adres+port beklediği için; parola zorunlu çünkü yerel port bütün hesaplara açık, müşteri parola seçemez, yenileyebilir). Yanında unix soketi (`/run/pixpanel-redis/<hesap>/redis.sock`, dosya izniyle yalnız o hesap, parolasız). **Uzaktan erişim yok (kullanıcı kararı, 10.11.2026):** örnek yalnız `127.0.0.1`'i dinler, panelde bunu değiştiren ayar bulunmaz, müşteri istese de açamaz; dışarıdan erişim yalnız SSH tüneliyle (paket 9'daki hesap SSH erişimine bağlı). Yalnız yerel adresi dinlediği için güvenlik duvarında port açılsa bile ulaşılamaz. Bellek sınırı paketten gelir, dolunca en eski önbellek atılır.
- **Arayüz:** hazır bağlantı bilgisi (Laravel `.env`, WordPress `wp-config.php`, soket yolu), kullanım çubuğu; mevcut tarayıcı ve konsol hesabın kendi örneğine bağlanır. Ortak Redis yöneticiye kalır.
- **Kabul:** bir hesap diğerinin anahtarlarını göremez; bellek sınırını dolduran hesap başkasını etkilemez; kapatınca süreç ve veri kalkar.

### 9. Hesaplara SFTP ve SSH — TAMAMLANDI (11.11.2026; kalan: panelde hesap terminali)

- **Kapsam:** hesap başına SFTP (hapisli; ev klasörü düzeni buna hazır) ve isteğe bağlı kabuk erişimi; açık anahtar yönetimi; `sshd_config.d` altında panelin kendi dosyası; panelde hesap terminali (bugün yalnızca yönetici).
- **Kabul:** hesap SFTP ile yalnızca kendi klasörünü görür; kabuk izni olmayan hesap komut çalıştıramaz; bozuk sshd yapılandırması uygulanmaz (`sshd -t`).

### 10. İstatistik — TAMAMLANDI (11.11.2026; kalan: trafik sınırının uygulanması, yönlendiren/ülke/tarayıcı dökümü, veritabanı ve posta disk kullanımı)

- **Kapsam:** nginx erişim günlüklerinden site başına günlük özet (istek, tekil ziyaretçi, bant genişliği, durum kodları, en çok istenen yollar, botlar); disk kullanımı (site, veritabanı, posta); aylık trafik (paket sınırı için). Özetler SQLite'ta tutulur, ham günlük saklanmaz.
- **Kabul:** bilinen sayıda istek atılır, özet aynı sayıyı gösterir; günlük döndürme (logrotate) sayımı bozmaz.

### 11. İşletim araçları — O (parçalar ayrı ayrı devredilebilir)

- 11a. Müşteri olarak giriş (yönetici ve bayi için; denetim günlüğüne yazılır, kolayca geri dönülür). — **tamamlandı (11.11.2026)**
- 11b. Bildirim olayları: disk doluyor, servis durdu, sertifika bitiyor, yedek başarısız, hesap kotayı aştı, yeni adresten giriş. — **tamamlandı (11.11.2026; sunucu olayları yöneticiye `health_alert` ile, site ve kota olayları hesabın kendisine)**
- 11c. Sistem güncellemeleri: bekleyen paketler, güvenlik güncellemelerinin kendiliğinden kurulması, yeniden başlatma gerekli uyarısı.
- 11d. Servis izleme: duran servisi yeniden başlatma, geçmişe dönük ölçümler (bugün yalnızca son dakikalar).
- 11e. Marka: logo, ad, giriş sayfası metni (bayi bazında). — **sunucu genelinde tamamlandı (11.11.2026); bayi bazında ayrım kaldı**

---

## P2 — Plesk'le aradaki fark

### 12. WordPress araçları — İLK SÜRÜM TAMAMLANDI (11.11.2026: kurulum, parolasız giriş, güncelleme, eklenti/tema yönetimi, bakım kipi; 12.11.2026: hazırlık ortamı ve güncellemede kendiliğinden geri dönüş; kalan: alt klasördeki kurulumları bulma, sıkılaştırma denetimi)

- **Kapsam:** wp-cli ile tek tıkla kurulum (veritabanını da açar), var olan kurulumları bulma, çekirdek/eklenti/tema güncelleme (toplu ve kendiliğinden), parolasız yönetici girişi, klonlama ve hazırlık ortamı (alt alan adına kopya, geri aktarma), sıkılaştırma (dosya izinleri, `xmlrpc`, dosya düzenleyici, `wp-config` koruması), bakım kipi.
- **Örnek al:** `deploy.rs` (kullanıcı adına komut, canlı günlük).
- **Kabul:** kurulan site açılır ve giriş yapılır; klon bağımsız çalışır; komutlar sitenin kullanıcısıyla yürür.

### 13. Site ayarları paketi — İLK SÜRÜM TAMAMLANDI (11.11.2026: yönlendirme, parola korumalı klasör, adres kısıtlaması, bakım kipi, WWW tercihi, hotlink, hata sayfaları; 12.11.2026: tarayıcı önbelleği, FastCGI/vekil önbelleği, yalnızca Cloudflare kipi; kalan: HTTP/3)

Hepsi nginx şablonuna eklenir; her biri küçük ve bağımsızdır:
yönlendirmeler (yol ve alan adı), parola korumalı klasör, adres engelleme ve izin listesi, hotlink koruması, özel hata sayfaları, bakım kipi, sıkıştırma ve önbellek başlıkları, FastCGI/vekil önbelleği, HTTP/3, "yalnızca Cloudflare" kipi, WWW'li/WWW'siz tercih.
- **Kabul:** her ayar için `curl` ile davranış sınaması; bozuk girdide yapılandırma geri alınır.

### 14. Site kopyalama ve şablonlar — O (devre uygun)

- Siteyi başka alan adına kopyalama (dosyalar, veritabanı, ayarlar); hazır şablonlar (Laravel, statik, Node.js başlangıçları); küçük bir uygulama kataloğu.
- **Bağımlılık:** 5'in geri yükleme kodu yeniden kullanılır.

### 15. Taşıma aracı — B, sistem katmanı

- SSH ile başka sunucudan çekme (rsync + döküm); Plesk, cPanel ve HestiaCP yedeklerinden içe aktarma; posta kutularını IMAP ile taşıma (`imapsync`).
- **Durum (12.11.2026):** SSH ile çekme yapıldı (dosyalar + WordPress veritabanı; ayrıntı DURUM.md). WordPress dışı veritabanları da eklendi (bilgiler elle verilir). Kalanlar: yedek dosyalarından içe aktarma, posta kutuları.
- **Not:** kaynak biçimlerinin her biri ayrı bir araştırma işidir.

### 16. Python uygulamaları — İLK SÜRÜM TAMAMLANDI (11.11.2026; kalan: `uv` ile çoklu sürüm)

- Node.js modülünün eşleniği: sürüm (pyenv yerine `uv` ile yönetilen sürümler), sanal ortam, gunicorn/uvicorn, systemd servisi.
- **Örnek al:** `hosting/node.rs`, `api/sites.rs` içindeki Node.js uçları, `web/src/pages/SiteNode.tsx`.

### 17. Posta derinliği — B (parçalar devredilebilir)

- 17a. Kutu sahibinin kendi işleri: web postasından parola değiştirme, otomatik yanıt ve filtreler (panel kurallarıyla çakışmadan).
- 17b. İstemci otomatik ayarı (autoconfig, autodiscover). — **tamamlandı (12.11.2026)**
- 17c. Giden posta sınırı (kutu ve alan adı başına saatlik sınır; ele geçirilmiş hesaba karşı). — **tamamlandı (12.11.2026)**
- 17d. Karantina: şüpheli iletileri tutma, panelden bırakma ya da silme.
- 17e. Antivirüs (ClamAV; yaklaşık 1 GB bellek ister, isteğe bağlı bileşen).
- 17f. Posta listeleri (düşük öncelik).
- 17g. Alan adı başına sertifika (SNI) ve `webmail.<alan>` adresi.

### 18. DNS derinliği — TAMAMLANDI (12.11.2026; kalan: bölge şablonları)

- DNSSEC (anahtar üretimi, DS kaydının gösterimi), ikincil ad sunucusu (AXFR, bildirim), bölge şablonları, BIND bölge dosyası içe/dışa aktarma, Cloudflare'de SRV ve CAA yazma.

### 19. Veritabanı derinliği — İLK SÜRÜM TAMAMLANDI (12.11.2026: boyutlar, kopyalama, denetim/onarım, yavaş sorgu günlüğü; kalan: PostgreSQL uzaktan erişim, zamanlı döküm, uzak sunucu)

- PostgreSQL uzaktan erişim (`pg_hba`), veritabanı kopyalama, denetim ve onarım, zamanlı döküm (5 ile birlikte), yavaş sorgu günlüğü görünümü, uzak veritabanı sunucusu (düşük öncelik).

---

## P3 — Ürünleşme

| # | İş | Büyüklük | Devir |
|---|---|---|---|
| 20 | İngilizce çeviri (`t()` anahtarlarının çıkarılması, sözlük, dil seçimi; arka uç hata iletileri dahil) | O | Uygun |
| 21 | API belgesi (OpenAPI), API anahtarı kapsamları (alan bazında izin) | O | Uygun |
| 22 | Komut satırı aracı (`pixpanel site add …`; API'nin ince sarmalayıcısı) | O | Uygun |
| 23 | Olay kancaları (site açıldı, yayın bitti gibi olaylarda webhook) — **tamamlandı (12.11.2026)** | K | Uygun |
| 24 | Çoklu IP ve IPv6 (siteye adres atama, DNS'te AAAA) | O | Sistem katmanı |
| 25 | Kaynak sınırı (hesap başına işlemci ve bellek; systemd dilimleri) | O | Sistem katmanı |
| 26 | Eklenti sistemi | B | Tasarım önce |
| 27 | Mobil görünüm ve erişilebilirlik denetimi | O | Uygun |

---

## Teknik borç (araya sıkıştırılacak)

- Bileşen kaldırma (bugün yalnızca kurma var) ve sürüm güncelleme (Node.js dahil).
- ~~Git web kancasında imza doğrulaması (`X-Hub-Signature-256`).~~ Tamamlandı (12.11.2026).
- Arşiv açmada boyut sınırı (zip bombası).
- Redis için hesap başına yalıtım (ACL) ya da kullanımı yöneticiye bırakma kararı.
- Birim testleri: bugün 14; doğrulama işlevlerinin hepsi kapsanmalı.
- Arayüz paketinin bölünmesi (ilk yükleme boyutu), sayfa bazında tembel yükleme.
- `hosting/ssl.rs` içindeki kullanılmayan `MODES` sabiti.
- Yerel klasörün adı hâlâ `capsule`.

---

## Kararlar

10.11.2026'da kullanıcıyla netleşti:

1. **Hedef kitle:** panel müşteriye de açılacak ama acelesi yok. Önce P0 ve kendi kullanım için gerekenler; P1'in müşteriye dönük kısmı (paketler, kotalar, marka) sonraya kalır.
2. **Apache ve `.htaccess`:** eklenmeyecek. Yaygın uygulamalar için hazır nginx kuralları ve yönlendirme arayüzü (13) yeterli.
3. **Sürümler:** GitHub sürümleri üzerinden yayınlanacak (6. paket). Deponun açık mı kapalı mı olacağı ve push için kullanılacak kimlik, ilk push'tan önce ayrıca sorulacak.
4. **Antivirüs:** ClamAV isteğe bağlı bileşen olacak.
5. **İş bölümü:** önemli işleri (güvenlik, yedekleme, kurulum, sistem katmanı) ana oturum yazar; önemsiz ve arayüz ağırlıklı işler Sonnet alt ajanlarına verilir.

---

## Önerilen yürütme sırası

Aynı anda en fazla bir sistem katmanı işi (test makinesi tek) ve yanında bir-iki arayüz/arka uç işi yürür.

| Dalga | Sistem katmanı (ana oturum) | Yanında alt ajanla |
|---|---|---|
| 1 | 7 (sınama altyapısı), 1 (güvenlik duvarı) | 3 (giriş güvenliği) |
| 2 | 2 (fail2ban), 4 (sertifikalar) | 13'ten küçük parçalar |
| 3 | 5 (yedekleme: 5a–5e) | 5f (yedekleme arayüzü), 11a–11b |
| 4 | 6 (kurulum ve güncelleme) | 8 (paketler ve kotalar) |
| 5 | 9 (SFTP/SSH), 10 (istatistik arka ucu) | 10 (arayüz), 12 (WordPress) |
| 6 | 15 (taşıma) | 14, 16, 17a–17c, 18, 19 |
| 7 | 24, 25 | 20–23, 27 |
