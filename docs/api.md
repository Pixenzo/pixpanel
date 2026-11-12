# HTTP API başvurusu

Panelin arayüzü ve `pixpanel` komut satırı aynı HTTP API'yi kullanır; arayüzde yapılan her işlem betikten de yapılabilir. Doğrulamalar, yetki denetimi ve denetim günlüğü aynıdır.

Bu belgedeki uç tabloları `server/src/api` altındaki yönlendiricilerden çıkarılmıştır ve `scripts/check-api-docs.py` ile koddan sapıp sapmadığı denetlenir (uç ekleyen ya da silen değişiklik bu belgeyi de güncellemelidir).

## Genel kurallar

- **Temel adres:** `https://SUNUCU:8443/api/v1`. Aşağıdaki tablolardaki yollar bu adrese eklenir.
- **Biçim:** istek ve yanıt gövdeleri JSON'dur (`Content-Type: application/json`). Zamanlar Unix saniyesidir. Liste uçlarında sayfalama yoktur; görev listesi son 200 kaydı döner.
- **Başarı:** `200` ve JSON gövde. Gövdesi olmayan işlemler `{"ok": true}` döner.
- **Dosya yüklemeleri** (`/files/upload`, `/databases/{id}/import`) gövde boyutu sınırı olmayan uçlardır; öteki istek gövdeleri için axum'un varsayılan 2 MB sınırı geçerlidir.

### Kimlik doğrulama

İki yol vardır.

**1. Oturum çerezi (arayüz).** `POST /auth/login` başarılı olunca `pixpanel_session` çerezini (HttpOnly, SameSite=Strict) kurar. Değişiklik yapan isteklerde (`POST`, `PUT`, `PATCH`, `DELETE`) çerezle birlikte **`X-PixPanel-CSRF`** başlığı da gönderilmelidir; değeri önemsizdir, varlığı yeter. Okuma isteklerinde gerekmez.

```sh
curl -sk -c jar.txt https://SUNUCU:8443/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"PAROLA"}'
curl -sk -b jar.txt https://SUNUCU:8443/api/v1/sites
curl -sk -b jar.txt -H 'X-PixPanel-CSRF: 1' -X POST https://SUNUCU:8443/api/v1/auth/logout
```

İki adımlı doğrulama açıksa ilk istek `401` ve `two_factor_required` koduyla döner; aynı isteği `"code": "123456"` (uygulama kodu ya da kurtarma kodu) ekleyerek yineleyin. Başarısız girişler adres başına sınırlanır (5 dakikada 8 hata sonrası `429`).

**2. API anahtarı (betikler).** Anahtar `Authorization: Bearer pix_...` başlığıyla gönderilir; çerez ve CSRF başlığı gerekmez.

```sh
curl -sk https://SUNUCU:8443/api/v1/sites -H "Authorization: Bearer $PIX_TOKEN"
```

Anahtarlar Hesabım sayfasından (ya da `POST /auth/tokens` ile, yalnızca oturumdan) üretilir; ham anahtar yalnızca üretildiği yanıtta görünür, sunucuda yalnızca özeti saklanır. Anahtarın kısıtları:

| Özellik | Açıklama |
| --- | --- |
| `read_only` | Yalnızca `GET`, `HEAD`, `OPTIONS` isteklerine izin verir; öbürleri `403`. |
| `allowed_ips` | Virgülle ayrılmış adres listesi; boşsa her adres. Başka adresten gelen istek `401`. Yalnızca gerçek bağlantı adresine bakılır (`X-Forwarded-For` dikkate alınmaz). |
| `expires_in_days` | Verilirse o kadar gün sonra anahtar geçersiz olur. |
| `scope` | Erişilebilen alanların listesi (aşağıda). Boş ya da verilmemişse hesabın bütün yetkileri. |

Anahtarla yapılamayan işlemler: yeni anahtar üretmek, iki adımlı doğrulama ayarlarını değiştirmek, başka hesap adına oturum açmak ya da iki adımlı doğrulamasını sıfırlamak, SSH anahtarı/parolası eklemek. Bunlar yalnızca tarayıcı oturumundan yapılır (anahtarla `403`).

### Anahtar kapsamı (alan bazında izin)

`POST /auth/tokens` gövdesindeki `scope` alanı, anahtarın çağırabileceği alanları sınırlar. Geçerli alan adları alt hesap yetki alanlarıyla aynıdır:

`sites`, `files`, `databases`, `mail`, `dns`, `ftp`, `ssl`, `cron`, `backups`, `logs`, `docker`

```sh
# yalnızca siteleri ve DNS'i yönetebilen, 90 gün geçerli anahtar (oturum çerezi gerekir)
curl -sk -b jar.txt -H 'X-PixPanel-CSRF: 1' -H 'Content-Type: application/json' \
  https://SUNUCU:8443/api/v1/auth/tokens \
  -d '{"name":"deploy","scope":["sites","dns"],"expires_in_days":90}'
```

Kurallar:

- Kapsam yetkiyi **asla genişletmez**: sonuç, hesabın (alt hesapsa alt hesabın) yetkisiyle kapsamın kesişimidir. Alt hesap, kendi yetkisinde olmayan bir alanı kapsama alamaz (`400`). Bilinmeyen alan adı da `400`.
- Kapsamlı anahtar yalnızca kapsamındaki alanların uçlarını çağırabilir; başka alanın ucu `403` verir.
- Kapsamlı anahtar **alan dışı** uçlara hiç girmez (`403`): kullanıcılar, sunucu/sistem, hizmetler, güvenlik duvarı, fail2ban, bileşenler, terminal, PHP yönetimi, paketler, marka, bildirimler, web kancaları, olay akışı, oturum/anahtar/parola uçları, SFTP/SSH erişimi, genel Redis ve yedek yönetimi (`/backup` altında yalnızca `/backup/mine` hesap yedekleri `backups` alanıyla açıktır). Güvenli varsayılan budur: tabloda olmayan her uç kapsamlı anahtara kapalıdır.
- Her kapsamlı anahtarın girebildiği alan dışı uçlar yalnızca şunlardır: `GET /auth/me` (kimliği ve `token_scope` alanını okur) ve görev uçları (`/jobs...`; yalnızca kapsamdaki alanlara ait görevler görünür, sistem görevleri görünmez).
- Alan–yol eşlemesi: `/sites/{id}/ssl...` `ssl`; `/sites/{id}/ftp...` `ftp`; `/sites...` (git, WordPress, Node dahil) `sites`; `/databases...` ve hesabın kendi Redis'i (`/redis/mine...`, `/redis/keys`, `/redis/key`, `/redis/command`, `/redis/flush`) `databases`; `/mail...` `mail`; `/dns...` `dns`; `/files...` `files`; `/cron...` `cron`; `/logs...` `logs`; `/docker...` `docker`; `/backup/mine...` `backups`.
- Anahtar listesi (`GET /auth/tokens`) her anahtarın `scope` alanını gösterir (`null`: kapsam yok).

### Hata biçimi

Hata durumunda HTTP durum kodu ve şu gövde döner; `message` kullanıcıya gösterilebilir Türkçe metindir:

```json
{ "error": { "code": "bad_request", "message": "Parola en az 10 karakter olmalı" } }
```

| HTTP | `code` | Anlamı |
| --- | --- | --- |
| 400 | `bad_request` | Geçersiz girdi (ad biçimi, eksik alan, geçersiz alan adı vb.). |
| 401 | `unauthorized` | Oturum/anahtar yok, süresi dolmuş, adres izinli değil ya da hesap askıda. |
| 401 | `two_factor_required` | Girişte doğrulama kodu istenir. |
| 403 | `forbidden` | Yetki yok: rol, alt hesap alanı, anahtar kapsamı ya da salt okunur anahtar. Oturum çerezli değişiklik isteğinde CSRF başlığı eksikse de budur. |
| 404 | `not_found` | Kayıt yok **ya da** başka hesaba ait (varlığı sızdırılmaz). |
| 409 | `conflict` | Aynı kayıt zaten var / kota ya da çakışma. |
| 429 | `too_many_requests` | Çok fazla başarısız giriş. |
| 500 | `internal` | Sunucu hatası. |

### Görev (job) modeli

Uzun süren işlemler (SSL alma, yedek, bileşen kurulumu, içe aktarma vb.) isteği hemen `{"job_id": 42}` ile yanıtlar ve işi arka planda yürütür. İlerleme iki yoldan izlenir:

- `GET /jobs/42` yoklama: `{"job": {...}, "log": "..."}`. `job.status` `running`, `success` ya da `failed` olur; `job.kind` işin türüdür (`ssl.letsencrypt`, `backup`, `database.import` gibi).
- `GET /jobs/42/stream` canlı günlük (Server-Sent Events). Önce o ana kadar birikmiş günlük satırları, sonra yenileri gelir; iş bitince akış kapanır. Her olay `data:` satırında bir JSON'dur: `{"type":"log","line":"..."}` ve en sonda `{"type":"done","status":"success"}`.

Yönetici bütün görevleri, diğer hesaplar yalnızca kendi başlattıklarını görür (başkasının görevi `404`).

### Roller ve "Yetki" sütunu

Roller: `admin` (yönetici), `reseller` (bayi), `user` (müşteri), `member` (alt hesap). Bayi kendi müşterilerini, müşteri kendi alt hesaplarını yönetir; alt hesaplar üst hesabın kaynaklarında yalnızca kendilerine verilen alanlarda çalışır. Kaynaklar her zaman kaynağın sahibi hesaba göre süzülür; başkasının kaynağı `404` döner.

Tablolardaki "Yetki" sütunu:

| Değer | Anlamı |
| --- | --- |
| Genel | Oturum istemez. |
| Giriş | Giriş yapmış her hesap. |
| Oturum | Yalnızca tarayıcı oturumu; API anahtarıyla `403`. |
| `alan` | O yetki alanı gerekir (alt hesap için alan yetkisi; kapsamlı anahtar için kapsam). Örn. `sites`. |
| Yönetici | Yalnızca `admin` rolü. Kapsamlı bir anahtar bu uçlara, ucun ait olduğu alan kapsamdaysa girebilir (örn. `docker` kapsamlı yönetici anahtarı `/docker`'a); alan dışı uçlara hiçbir zaman. |
| Yönetici/Bayi | `admin` ve `reseller`. |
| Sahip/yönetici | Hesabın kendisi ya da onu yöneten hesap (yönetici; bayi kendi müşterisini). |

Komut satırı: `pixpanel api METODA /yol [--data ...] [--wait]` bu API'yi geçici bir yerel anahtarla çağırır (bkz. `docs/komut-satiri.md`).

---

## Kimlik ve hesap (`/auth`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| POST | `/auth/login` | Giriş: `{username, password, code?}`; oturum çerezini kurar. | Genel |
| POST | `/auth/logout` | Oturumu kapatır, çerezi siler. | Giriş |
| POST | `/auth/impersonate/stop` | Başka hesap adına açılmış oturumu kapatıp asıl hesaba döner. | Oturum |
| GET | `/auth/me` | Kimlik, rol, yetki alanları, dil; anahtarla çağrılıyorsa `token_scope`. | Giriş |
| POST | `/auth/password` | Parola değiştirir (`current_password`, `new_password`); diğer oturumlar kapanır. | Giriş |
| GET | `/auth/2fa` | İki adımlı doğrulama durumu ve kalan kurtarma kodu sayısı. | Giriş |
| POST | `/auth/2fa/setup` | Gizli anahtar ve QR üretir (`password`). | Oturum |
| POST | `/auth/2fa/enable` | Uygulamadaki kodla doğrulamayı açar; kurtarma kodlarını verir. | Oturum |
| POST | `/auth/2fa/disable` | Doğrulamayı kapatır (`password`). | Oturum |
| POST | `/auth/2fa/recovery-codes` | Kurtarma kodlarını yeniler (`password`). | Oturum |
| GET | `/auth/sessions` | Açık oturumlar. | Giriş |
| DELETE | `/auth/sessions/{id}` | Bir oturumu kapatır. | Giriş |
| GET | `/auth/tokens` | API anahtarları (ham anahtar yok; `scope` dahil). | Giriş |
| POST | `/auth/tokens` | Anahtar üretir: `{name, read_only?, allowed_ips?, expires_in_days?, scope?}`; yanıt `{id, token}`. | Oturum |
| DELETE | `/auth/tokens/{id}` | Anahtarı siler. | Giriş |

## Siteler (`/sites`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/sites` | Siteler. | `sites` |
| POST | `/sites` | Site ekler: `{domain, kind, php_version?, proxy_target?, node_version?, aliases?, user_id?}`. | `sites` |
| GET | `/sites/options` | Site eklerken seçilebilecekler (kurulu PHP/Node sürümleri, hesaplar vb.). | `sites` |
| GET | `/sites/summary` | Sitelerin özet görünümü. | `sites` |
| GET | `/sites/{id}` | Site ayrıntısı. | `sites` |
| PATCH | `/sites/{id}` | Site ayarlarını günceller (PHP sürümü, şablon, kurallar, takma adlar vb.). | `sites` |
| DELETE | `/sites/{id}` | Siteyi kaldırır (`?delete_files=true` ile dosyalarıyla). | `sites` |
| POST | `/sites/{id}/clone` | Siteyi kopyalar ya da hazırlık ortamı açar (görev). | `sites` |
| POST | `/sites/{id}/cache/purge` | Site önbelleğini temizler. | `sites` |
| POST | `/sites/{id}/migrate/probe` | Başka sunucudan taşıma için bağlantıyı yoklar. | `sites` |
| POST | `/sites/{id}/migrate` | Siteyi başka sunucudan çeker (görev). | `sites` |
| GET | `/sites/{id}/uptime` | Erişilebilirlik izleme kayıtları. | `sites` |
| GET | `/sites/{id}/stats` | Ziyaret ve trafik istatistikleri (`?days=`). | `sites` |
| GET | `/sites/{id}/node` | Node.js/Python uygulamasının durumu. | `sites` |
| POST | `/sites/{id}/node/run` | Uygulama komutunu bir kez çalıştırır (görev). | `sites` |
| POST | `/sites/{id}/node/{action}` | Uygulamayı yönetir (başlat, durdur, yeniden başlat vb.). | `sites` |
| GET | `/sites/{id}/git` | Git dağıtım ayarı ve son dağıtımlar. | `sites` |
| PUT | `/sites/{id}/git` | Git dağıtımını kaydeder. | `sites` |
| DELETE | `/sites/{id}/git` | Git dağıtımını kaldırır. | `sites` |
| POST | `/sites/{id}/git/deploy` | Şimdi dağıtır (görev). | `sites` |
| POST | `/sites/{id}/git/webhook-token` | Dağıtım web kancası anahtarını yeniler. | `sites` |
| GET | `/sites/{id}/git/deployments/{deployment}` | Bir dağıtımın günlüğü. | `sites` |
| GET | `/sites/{id}/wordpress` | WordPress durumu. | `sites` |
| POST | `/sites/{id}/wordpress/install` | WordPress kurar (görev). | `sites` |
| POST | `/sites/{id}/wordpress/update` | WordPress çekirdeğini günceller (görev). | `sites` |
| POST | `/sites/{id}/wordpress/login` | Tek tıkla yönetici girişi bağlantısı üretir. | `sites` |
| PUT | `/sites/{id}/wordpress/settings` | WordPress ayarlarını kaydeder. | `sites` |
| GET | `/sites/{id}/wordpress/staging` | Hazırlık ortamı durumu. | `sites` |
| POST | `/sites/{id}/wordpress/staging` | Hazırlık ortamı oluşturur (görev). | `sites` |
| POST | `/sites/{id}/wordpress/staging/sync` | Hazırlık ortamını canlıya/canlıdan eşler (görev). | `sites` |
| POST | `/sites/{id}/wordpress/{kind}/{slug}` | Eklenti ya da temayı yönetir (`kind`: `plugins` / `themes`). | `sites` |

### SSL (`/sites/{id}/ssl`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/sites/{id}/ssl` | Sertifika durumu. | `ssl` |
| DELETE | `/sites/{id}/ssl` | SSL'i kapatır. | `ssl` |
| POST | `/sites/{id}/ssl/self-signed` | Kendi imzalı sertifika üretir. | `ssl` |
| POST | `/sites/{id}/ssl/custom` | Elle sertifika yükler. | `ssl` |
| POST | `/sites/{id}/ssl/letsencrypt` | Let's Encrypt sertifikası alır: `{include_aliases?, dns?, wildcard?}`; yanıt `{job_id}`. | `ssl` |

### FTP (`/sites/{id}/ftp`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/sites/{id}/ftp` | Sitenin FTP hesapları. | `ftp` |
| POST | `/sites/{id}/ftp` | FTP hesabı açar. | `ftp` |
| PATCH | `/sites/{id}/ftp/{account}` | FTP hesabını günceller (parola, klasör, kota). | `ftp` |
| DELETE | `/sites/{id}/ftp/{account}` | FTP hesabını siler. | `ftp` |

### Git web kancası

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| POST | `/hooks/deploy/{token}` | Git sağlayıcısının çağırdığı dağıtım kancası; kimlik yerine yol içindeki anahtar (ve imza) doğrulanır. | Genel |

## Dosyalar (`/files`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/files/roots` | Erişilebilen kök klasörler. | `files` |
| POST | `/files/op` | Dosya işlemi: `{scope, request}`; `request` listele, oku, yaz, klasör aç, yeniden adlandır, taşı, kopyala, sil, izin değiştir, sıkıştır, çıkar işlemlerinden biridir. İşlem hesabın Linux kullanıcısı olarak yapılır. | `files` |
| GET | `/files/download` | Dosya indirir. | `files` |
| POST | `/files/upload` | Dosya yükler (multipart). | `files` |

## Veritabanları (`/databases`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/databases` | Motorlar, veritabanları, kullanıcılar ve boyutlar. | `databases` |
| POST | `/databases` | Veritabanı oluşturur: `{engine, name, user?: {name, password, host?}}`. | `databases` |
| GET | `/databases/slow-log` | Yavaş sorgu günlüğü ayarı. | Yönetici |
| PUT | `/databases/slow-log` | Yavaş sorgu günlüğü ayarını kaydeder. | Yönetici |
| POST | `/databases/users` | Veritabanı kullanıcısı oluşturur. | `databases` |
| PATCH | `/databases/users/{id}` | Kullanıcının parolasını/adresini günceller. | `databases` |
| DELETE | `/databases/users/{id}` | Kullanıcıyı siler. | `databases` |
| DELETE | `/databases/{id}` | Veritabanını siler. | `databases` |
| GET | `/databases/{id}/export` | Dışa aktarır (SQL dökümü indirir). | `databases` |
| POST | `/databases/{id}/import` | SQL dosyası içe aktarır (görev). | `databases` |
| POST | `/databases/{id}/copy` | Veritabanını kopyalar (görev). | `databases` |
| POST | `/databases/{id}/maintenance` | Onar/optimize et (görev). | `databases` |
| POST | `/databases/{id}/open` | phpMyAdmin/Adminer için bağlantı adresi üretir (`{url}`). | `databases` |
| PUT | `/databases/{id}/grants/{db_user_id}` | Kullanıcıya yetki verir (`full`, `readwrite`, `readonly`). | `databases` |
| DELETE | `/databases/{id}/grants/{db_user_id}` | Yetkiyi kaldırır. | `databases` |

### Redis (`/redis`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/redis` | Sunucu geneli Redis özeti. | Yönetici |
| POST | `/redis/config` | Sunucu geneli Redis ayarları. | Yönetici |
| GET | `/redis/accounts` | Hesapların Redis örnekleri. | Yönetici |
| PUT | `/redis/accounts/{user_id}` | Hesabın Redis bellek sınırını ayarlar. | Yönetici |
| DELETE | `/redis/accounts/{user_id}` | Hesabın Redis örneğini kapatır. | Yönetici |
| GET | `/redis/mine` | Hesabın kendi Redis örneği. | `databases` |
| POST | `/redis/mine/enable` | Kendi Redis örneğini açar. | `databases` |
| POST | `/redis/mine/disable` | Kendi Redis örneğini kapatır. | `databases` |
| POST | `/redis/mine/password` | Parolayı yeniler. | `databases` |
| GET | `/redis/keys` | Anahtarları listeler. | `databases` |
| GET | `/redis/key` | Bir anahtarın değerini okur. | `databases` |
| POST | `/redis/key` | Anahtarı oluşturur/düzenler. | `databases` |
| DELETE | `/redis/key` | Anahtarı siler. | `databases` |
| POST | `/redis/command` | Redis komutu çalıştırır. | `databases` |
| POST | `/redis/flush` | Veritabanını boşaltır. | `databases` |

## E-posta (`/mail`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/mail` | Posta alan adları, kutular ve takma adlar. | `mail` |
| POST | `/mail/domains` | Posta alan adı ekler. | `mail` |
| GET | `/mail/domains/{id}` | Alan adı ayrıntısı. | `mail` |
| PATCH | `/mail/domains/{id}` | Alan adı ayarlarını günceller. | `mail` |
| DELETE | `/mail/domains/{id}` | Alan adını ve kutularını siler. | `mail` |
| GET | `/mail/domains/{id}/dns` | Gerekli DNS kayıtlarını (MX, SPF, DKIM, DMARC) denetler. | `mail` |
| POST | `/mail/domains/{id}/dns/apply` | Eksik DNS kayıtlarını panelin DNS'ine yazar. | `mail` |
| GET | `/mail/domains/{id}/client-settings` | Posta istemcisi ayarları. | `mail` |
| POST | `/mail/domains/{id}/mailboxes` | Posta kutusu açar: `{local, password, quota_mb?}`. | `mail` |
| POST | `/mail/domains/{id}/aliases` | Takma ad ekler. | `mail` |
| PATCH | `/mail/mailboxes/{id}` | Kutuyu günceller (parola, kota, yönlendirme, otomatik yanıt, süzgeçler). | `mail` |
| DELETE | `/mail/mailboxes/{id}` | Kutuyu siler. | `mail` |
| POST | `/mail/mailboxes/{id}/import` | Başka IMAP sunucusundan posta aktarır (görev). | `mail` |
| PATCH | `/mail/aliases/{id}` | Takma adı günceller. | `mail` |
| DELETE | `/mail/aliases/{id}` | Takma adı siler. | `mail` |
| GET | `/mail/limit-hits` | Gönderim sınırına takılan kayıtlar. | `mail` |
| GET | `/mail/quarantine` | Karantinadaki iletiler. | `mail` |
| POST | `/mail/quarantine/bulk-delete` | Karantinadan toplu siler. | `mail` |
| GET | `/mail/quarantine/{id}` | Karantinadaki iletiyi gösterir. | `mail` |
| DELETE | `/mail/quarantine/{id}` | Karantinadaki iletiyi siler. | `mail` |
| POST | `/mail/quarantine/{id}/release` | İletiyi karantinadan çıkarıp teslim eder. | `mail` |
| GET | `/mail/activity` | Posta sunucusu etkinlik günlüğü. | Yönetici |
| GET | `/mail/queue` | Postfix kuyruğu. | Yönetici |
| POST | `/mail/queue/flush` | Kuyruğu yeniden dener. | Yönetici |
| DELETE | `/mail/queue/{id}` | Kuyruktaki iletiyi siler. | Yönetici |
| GET | `/mail/settings` | Sunucu geneli posta ve spam ayarları. | Yönetici |
| PUT | `/mail/settings` | Posta ve spam ayarlarını kaydeder. | Yönetici |
| GET | `/mail/limits` | Gönderim sınırı ayarları. | Yönetici |
| PUT | `/mail/limits` | Gönderim sınırı ayarlarını kaydeder. | Yönetici |
| POST | `/mail/setup` | Posta sunucusunu kurar/yeniden yapılandırır. | Yönetici |
| POST | `/mail/public/password` | Kutu sahibinin parola değiştirmesi (adres + geçerli parola ile doğrulanır; Roundcube eklentisi kullanır). | Genel |
| GET | `/mail/public/autoconfig` | Thunderbird otomatik ayar belgesi. | Genel |
| GET | `/mail/public/mobileconfig` | Apple posta profili. | Genel |
| POST | `/mail/public/autodiscover` | Outlook otomatik ayar yanıtı. | Genel |

## DNS (`/dns`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/dns` | DNS bölgeleri ve sağlayıcı durumu. | `dns` |
| POST | `/dns/zones` | Bölge ekler: `{domain, provider}` (`local` ya da `cloudflare`). | `dns` |
| GET | `/dns/zones/{id}` | Bölge ve kayıtları. | `dns` |
| DELETE | `/dns/zones/{id}` | Bölgeyi siler. | `dns` |
| POST | `/dns/zones/{id}/records` | Kayıt ekler: `{record: {type, name, content, ttl, priority?}}`; yanıt `{records}`. | `dns` |
| PUT | `/dns/zones/{id}/records` | Kaydı günceller: `{old, new}`. | `dns` |
| POST | `/dns/zones/{id}/records/delete` | Kaydı siler: `{record}`. | `dns` |
| POST | `/dns/zones/{id}/sync` | Bölgeyi sağlayıcıyla eşler. | `dns` |
| POST | `/dns/zones/{id}/dnssec` | DNSSEC'i açar/kapatır. | `dns` |
| GET | `/dns/zones/{id}/export` | Bölgeyi BIND bölge dosyası olarak dışa aktarır (`{filename, content}`). | `dns` |
| POST | `/dns/zones/{id}/import` | BIND biçiminde içe aktarır. | `dns` |
| POST | `/dns/zones/{id}/cloudflare` | Bölgenin Cloudflare ayarları (vekil, önbellek). | `dns` |
| POST | `/dns/zones/{id}/purge` | Cloudflare önbelleğini temizler. | `dns` |
| GET | `/dns/cloudflare` | Hesabın Cloudflare anahtarı durumu. | `dns` |
| PUT | `/dns/cloudflare` | Cloudflare anahtarını kaydeder. | `dns` |
| DELETE | `/dns/cloudflare` | Cloudflare anahtarını siler. | `dns` |
| GET | `/dns/settings` | Yerel DNS sunucusu ayarları. | Yönetici |
| PUT | `/dns/settings` | Yerel DNS sunucusu ayarlarını kaydeder. | Yönetici |
| POST | `/dns/setup` | Yerel DNS sunucusunu kurar. | Yönetici |
| GET | `/dns/cluster` | İkincil ad sunucusu (küme) durumu. | Yönetici |
| PUT | `/dns/cluster` | Küme ayarlarını kaydeder. | Yönetici |
| POST | `/dns/cluster/sync` | Bölgeleri ikincil sunucuya eşler. | Yönetici |
| PUT | `/dns/replica` | Bu sunucuyu bir birincil sunucunun ikincisi yapar (birincil çağırır). | Yönetici |
| DELETE | `/dns/replica/{primary}` | İkincillikten ayırır. | Yönetici |

## Yedekler

### Hesabın kendi yedekleri (`/backup/mine`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/backup/mine` | Hesabın yedekleri ve alınabilecek kaynaklar. | `backups` |
| POST | `/backup/mine/run` | Elle yedek alır: `{site_ids?, database_ids?, mail_domain_ids?}`; yanıt `{job_id}`. | `backups` |
| POST | `/backup/mine/restore` | Yedekten geri yükler (görev). | `backups` |
| DELETE | `/backup/mine/snapshots/{target}/{snapshot}` | Yedeği siler. | `backups` |
| GET | `/backup/mine/snapshots/{target}/{snapshot}/files` | Yedekteki dosyaları listeler. | `backups` |

### Sunucu yedekleri (`/backup`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/backup` | Yedek yerleri, planlar ve son çalışmalar. | Yönetici |
| POST | `/backup/run` | Elle yedek alır: `{target_id, scope}` (görev). | Yönetici |
| POST | `/backup/restore` | Yedekten geri yükler (görev). | Yönetici |
| POST | `/backup/targets` | Yedek yeri ekler (yerel, SFTP, S3 vb.). | Yönetici |
| PUT | `/backup/targets/{id}` | Yedek yerini günceller. | Yönetici |
| DELETE | `/backup/targets/{id}` | Yedek yerini siler. | Yönetici |
| POST | `/backup/targets/{id}/check` | Yedek yerine bağlantıyı sınar. | Yönetici |
| GET | `/backup/targets/{id}/password` | Yedek deposunun parolasını gösterir. | Yönetici |
| GET | `/backup/targets/{id}/snapshots` | Yedekleri listeler. | Yönetici |
| DELETE | `/backup/targets/{id}/snapshots/{snapshot}` | Yedeği siler. | Yönetici |
| GET | `/backup/targets/{id}/snapshots/{snapshot}/files` | Yedekteki dosyaları listeler. | Yönetici |
| POST | `/backup/plans` | Zamanlanmış yedek planı ekler. | Yönetici |
| PUT | `/backup/plans/{id}` | Planı günceller. | Yönetici |
| DELETE | `/backup/plans/{id}` | Planı siler. | Yönetici |
| POST | `/backup/plans/{id}/run` | Planı şimdi çalıştırır (görev). | Yönetici |

## Zamanlanmış görevler (`/cron`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/cron` | Zamanlanmış görevler. | `cron` |
| POST | `/cron` | Görev ekler. | `cron` |
| PATCH | `/cron/{id}` | Görevi günceller (zaman, komut, açık/kapalı). | `cron` |
| DELETE | `/cron/{id}` | Görevi siler. | `cron` |
| POST | `/cron/{id}/run` | Görevi şimdi çalıştırır (görev). | `cron` |

## Günlükler (`/logs`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/logs/sources` | Okunabilir günlük kaynakları. | `logs` |
| GET | `/logs/read` | Bir kaynaktan son satırları okur. | `logs` |

## Docker (`/docker`)

Docker uçlarının hepsi yönetici içindir; `docker` alanı kapsamlı bir yönetici anahtarı da bunları çağırabilir.

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/docker` | Docker durumu. | Yönetici |
| GET | `/docker/containers` | Konteynerler. | Yönetici |
| POST | `/docker/containers` | Konteyner oluşturur (görev). | Yönetici |
| GET | `/docker/containers/stats` | Konteyner kaynak kullanımı. | Yönetici |
| GET | `/docker/containers/{id}` | Konteyner ayrıntısı. | Yönetici |
| DELETE | `/docker/containers/{id}` | Konteyneri siler. | Yönetici |
| GET | `/docker/containers/{id}/logs` | Konteyner günlüğü. | Yönetici |
| POST | `/docker/containers/{id}/{action}` | Konteyneri yönetir (`start`, `stop`, `restart`, `pause`, `unpause`, `kill`). | Yönetici |
| GET | `/docker/images` | İmajlar. | Yönetici |
| POST | `/docker/images/pull` | İmaj çeker (görev). | Yönetici |
| POST | `/docker/images/prune` | Kullanılmayan imajları temizler. | Yönetici |
| DELETE | `/docker/images/{id}` | İmajı siler. | Yönetici |
| GET | `/docker/volumes` | Birimler. | Yönetici |
| POST | `/docker/volumes` | Birim oluşturur. | Yönetici |
| DELETE | `/docker/volumes/{name}` | Birimi siler. | Yönetici |
| GET | `/docker/networks` | Ağlar. | Yönetici |
| POST | `/docker/networks` | Ağ oluşturur. | Yönetici |
| DELETE | `/docker/networks/{name}` | Ağı siler. | Yönetici |
| GET | `/docker/stacks` | Compose yığınları. | Yönetici |
| POST | `/docker/stacks` | Yığın oluşturur. | Yönetici |
| GET | `/docker/stacks/{name}` | Yığının Compose dosyası. | Yönetici |
| PUT | `/docker/stacks/{name}` | Yığını kaydeder. | Yönetici |
| DELETE | `/docker/stacks/{name}` | Yığını siler. | Yönetici |
| POST | `/docker/stacks/{name}/{action}` | Yığını yönetir (`up`, `down` vb.; görev). | Yönetici |

## Görevler, olaylar

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/jobs` | Son 200 görev (yönetici hepsini, diğerleri kendilerininkini). | Giriş |
| GET | `/jobs/{id}` | Görev ve tam günlüğü. | Giriş |
| GET | `/jobs/{id}/stream` | Canlı günlük (SSE). | Giriş |
| GET | `/events` | Canlı olay akışı (SSE): sunucu yükü, görev durumları, kaynak değişiklikleri. En çok 15 dakika sürer. | Giriş |

## Kullanıcılar ve erişim (`/users`)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/users` | Hesaplar (yönetici hepsini, bayi/müşteri kendi açtıklarını). Alt hesap `403`. | Giriş |
| POST | `/users` | Hesap açar (yönetici: admin/bayi/müşteri; bayi: müşteri; müşteri: alt hesap). | Giriş |
| PATCH | `/users/{id}` | Hesabı günceller (e-posta, paket, askıya alma, alt hesap yetkileri). | Sahip/yönetici |
| DELETE | `/users/{id}` | Hesabı siler. | Sahip/yönetici |
| GET | `/users/{id}/usage` | Hesabın kaynak kullanımı. | Sahip/yönetici |
| POST | `/users/{id}/2fa/reset` | Hesabın iki adımlı doğrulamasını sıfırlar. | Oturum |
| POST | `/users/{id}/impersonate` | Hesap adına oturum açar. | Oturum |
| GET | `/users/{id}/access` | SFTP/SSH erişim durumu. | Sahip/yönetici |
| PUT | `/users/{id}/access` | Erişim düzeyini ayarlar (kapalı, SFTP, kabuk). | Sahip/yönetici |
| PUT | `/users/{id}/access/password` | SFTP parolasını ayarlar. | Oturum |
| POST | `/users/{id}/access/keys` | Açık SSH anahtarı ekler. | Oturum |
| DELETE | `/users/{id}/access/keys/{key}` | SSH anahtarını siler. | Sahip/yönetici |

## Paketler, marka, bildirimler, web kancaları

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/packages` | Hosting paketleri. | Yönetici/Bayi |
| POST | `/packages` | Paket oluşturur. | Yönetici/Bayi |
| PATCH | `/packages/{id}` | Paketi günceller. | Yönetici/Bayi |
| DELETE | `/packages/{id}` | Paketi siler. | Yönetici/Bayi |
| GET | `/packages/usage` | Hesabın kendi kota kullanımı. | Giriş |
| GET | `/branding` | Panel markası (ad, logo); oturumsuz da okunur, bayinin markası alan adına göre seçilir. | Genel |
| PUT | `/branding` | Sunucu geneli markayı kaydeder. | Yönetici |
| GET | `/branding/resellers/{id}` | Bayinin markası. | Yönetici/Bayi |
| PUT | `/branding/resellers/{id}` | Bayinin markasını kaydeder. | Yönetici/Bayi |
| DELETE | `/branding/resellers/{id}` | Bayinin markasını siler. | Yönetici/Bayi |
| GET | `/notifications` | Hesabın bildirim kanalları ve olay tercihleri. | Giriş |
| PUT | `/notifications` | Bildirim tercihlerini kaydeder. | Giriş |
| POST | `/notifications/test` | Deneme bildirimi gönderir. | Giriş |
| GET | `/notifications/smtp` | Bildirim e-postası (SMTP) ayarı. | Yönetici |
| PUT | `/notifications/smtp` | SMTP ayarını kaydeder. | Yönetici |
| POST | `/notifications/smtp/test` | SMTP ayarını sınar. | Yönetici |
| GET | `/webhooks` | Hesabın web kancaları ve seçilebilir olaylar. Alt hesap `403`. | Giriş |
| POST | `/webhooks` | Web kancası ekler. | Giriş |
| PUT | `/webhooks/{id}` | Web kancasını günceller. | Giriş |
| DELETE | `/webhooks/{id}` | Web kancasını siler. | Giriş |
| POST | `/webhooks/{id}/secret` | İmza gizli anahtarını yeniler. | Giriş |
| POST | `/webhooks/{id}/test` | Deneme olayı gönderir. | Giriş |
| GET | `/webhooks/{id}/deliveries` | Son gönderimler. | Giriş |

## Sunucu ve sistem (yalnızca yönetici)

| Yöntem | Yol | Açıklama | Yetki |
| --- | --- | --- | --- |
| GET | `/system/overview` | CPU, bellek, disk, ağ özeti. | Yönetici |
| GET | `/system/metrics` | Son metrik örnekleri. | Yönetici |
| GET | `/system/metrics/archive` | Uzun dönem metrik arşivi. | Yönetici |
| GET | `/system/processes` | Süreçler. | Yönetici |
| POST | `/system/processes/{pid}/signal` | Sürece sinyal gönderir. | Yönetici |
| GET | `/system/services` | systemd hizmetleri. | Yönetici |
| POST | `/system/services/{name}/{action}` | Hizmeti başlatır/durdurur/yeniden başlatır. | Yönetici |
| GET | `/system/audit` | Denetim günlüğü. | Yönetici |
| GET | `/system/updates` | Bekleyen paket güncellemeleri. | Yönetici |
| POST | `/system/updates/refresh` | Güncelleme listesini yeniler. | Yönetici |
| POST | `/system/updates/install` | Güncellemeleri kurar (görev). | Yönetici |
| POST | `/system/reboot` | Sunucuyu yeniden başlatır. | Yönetici |
| POST | `/system/reboot/cancel` | Zamanlanmış yeniden başlatmayı iptal eder. | Yönetici |
| GET | `/system/update` | Panel sürümü ve yeni sürüm bilgisi. | Yönetici |
| POST | `/system/update/check` | Yeni panel sürümü var mı diye bakar. | Yönetici |
| POST | `/system/update/apply` | Paneli günceller (görev). | Yönetici |
| GET | `/system/ips` | Sunucu IP adresleri ve atamaları. | Yönetici |
| POST | `/system/ips` | IP adresi ekler. | Yönetici |
| PUT | `/system/ips/{address}` | IP adresinin açıklamasını/atamasını günceller. | Yönetici |
| DELETE | `/system/ips/{address}` | IP adresini kaldırır. | Yönetici |
| GET | `/system/certificate` | Panelin kendi sertifikası. | Yönetici |
| POST | `/system/certificate/letsencrypt` | Panele Let's Encrypt sertifikası alır (görev). | Yönetici |
| POST | `/system/certificate/custom` | Panele elle sertifika yükler. | Yönetici |
| POST | `/system/certificate/selfsigned` | Panele kendi imzalı sertifika üretir. | Yönetici |
| GET | `/health` | Sağlık denetimi bulguları. | Yönetici |
| POST | `/health/check` | Denetimi şimdi çalıştırır. | Yönetici |
| POST | `/health/fix` | Bir bulguyu otomatik düzeltir. | Yönetici |
| PUT | `/health/settings` | Sağlık denetimi ayarlarını kaydeder. | Yönetici |
| GET | `/components` | Kurulabilir bileşenler ve durumları. | Yönetici |
| POST | `/components/{id}/install` | Bileşeni kurar (görev). | Yönetici |
| POST | `/components/{id}/remove` | Bileşeni kaldırır (ayarlar kalır; görev). | Yönetici |
| DELETE | `/components/{id}` | Bileşeni ve verisini tamamen siler. | Yönetici |
| GET | `/php` | Kurulu PHP sürümleri. | Yönetici |
| PUT | `/php/{version}/settings` | Sürümün php.ini ayarlarını kaydeder. | Yönetici |
| GET | `/php/{version}/extensions` | Sürümün eklentileri. | Yönetici |
| POST | `/php/{version}/extensions/{name}` | Eklentiyi açar/kapatır. | Yönetici |
| POST | `/php/{version}/extensions/{name}/install` | Eklentiyi kurar (görev). | Yönetici |
| GET | `/firewall` | Güvenlik duvarı kural seti. | Yönetici |
| PUT | `/firewall` | Kural setini uygular (60 saniye içinde onaylanmazsa geri alınır). | Yönetici |
| POST | `/firewall/confirm` | Uygulanan kural setini onaylar. | Yönetici |
| POST | `/firewall/revert` | Önceki kural setine döner. | Yönetici |
| GET | `/fail2ban` | fail2ban durumu, engelli adresler. | Yönetici |
| PUT | `/fail2ban` | Hapis ayarlarını ve beyaz listeyi kaydeder. | Yönetici |
| POST | `/fail2ban/ban` | Adresi engeller. | Yönetici |
| POST | `/fail2ban/unban` | Adresin engelini kaldırır. | Yönetici |
| GET | `/terminal/ws` | Tarayıcı terminali (WebSocket). | Yönetici |

---

## Örnekler

Aşağıdaki örneklerde `PIX` değişkeni API anahtarını, `H` ortak başlıkları tutar:

```sh
BASE=https://SUNUCU:8443/api/v1
PIX=pix_xxxxxxxxxxxxxxxxxxxxxxxx
H=(-sk -H "Authorization: Bearer $PIX" -H 'Content-Type: application/json')
```

### Site listesi

```sh
curl "${H[@]}" $BASE/sites
```

```json
[
  { "id": 12, "owner": "musteri", "domain": "ornek.com", "aliases": ["www.ornek.com"],
    "kind": "php", "php_version": "8.3", "ssl_mode": "letsencrypt", "status": "active",
    "created_at": 1789200000 }
]
```

(Yanıtta sitenin başka alanları da vardır; burada kısaltıldı.)

### Site ekleme

`kind`: `php`, `static`, `proxy`, `node`, `python`. PHP sitesinde `php_version`, vekil sitede `proxy_target`, Node sitesinde `node_version` zorunludur.

```sh
curl "${H[@]}" -X POST $BASE/sites \
  -d '{"domain":"ornek.com","kind":"php","php_version":"8.3","aliases":["www.ornek.com"]}'
```

Başarıda oluşan site nesnesi döner (`id`, `domain`, `owner`, `kind`, ...). Alan adı geçersizse `400`, kota dolduysa `409` gelir.

### Veritabanı oluşturma

`engine`: `mariadb` ya da `postgresql`. Yönetici olmayan hesaplarda ad `HESAPADI_` önekiyle başlamalıdır. `user` verilirse veritabanıyla birlikte tam yetkili bir kullanıcı da açılır.

```sh
curl "${H[@]}" -X POST $BASE/databases \
  -d '{"engine":"mariadb","name":"musteri_blog","user":{"name":"musteri_blog","password":"UzunBirParola123"}}'
```

```json
{ "id": 7 }
```

### Posta kutusu açma

Önce posta alan adının kimliğini `GET /mail` ile bulun.

```sh
curl "${H[@]}" -X POST $BASE/mail/domains/3/mailboxes \
  -d '{"local":"info","password":"UzunBirParola123","quota_mb":2048}'
```

```json
{ "id": 21, "domain_id": 3, "local": "info", "address": "info@ornek.com",
  "quota_mb": 2048, "enabled": true, "used_bytes": null, "created_at": 1789200000 }
```

(Yanıtta yönlendirme, otomatik yanıt ve süzgeç alanları da yer alır.)

### DNS kaydı ekleme

Bölge kimliğini `GET /dns` ile bulun. Yanıt, bölgenin güncel kayıt listesidir.

```sh
curl "${H[@]}" -X POST $BASE/dns/zones/5/records \
  -d '{"record":{"type":"A","name":"blog.ornek.com","content":"203.0.113.10","ttl":300}}'
```

```json
{ "records": [ { "id": "", "type": "A", "name": "blog.ornek.com", "content": "203.0.113.10", "ttl": 300, "priority": null } ] }
```

MX için `priority`, TXT için tırnaksız metin `content` olarak verilir.

### Yedek alma (hesabın kendi yedeği)

```sh
curl "${H[@]}" -X POST $BASE/backup/mine/run \
  -d '{"site_ids":[12],"database_ids":[7]}'
```

```json
{ "job_id": 42 }
```

Yönetici, sunucu yedeği için `POST /backup/run` ile `{"target_id": 1, "scope": {"all_sites": true, "all_databases": true}}` gönderir.

### SSL alma (Let's Encrypt)

```sh
curl "${H[@]}" -X POST $BASE/sites/12/ssl/letsencrypt -d '{"include_aliases":true}'
```

```json
{ "job_id": 43 }
```

### Görev izleme

Yoklama:

```sh
curl "${H[@]}" $BASE/jobs/43
```

```json
{ "job": { "id": 43, "kind": "ssl.letsencrypt", "title": "ornek.com için SSL", "status": "running",
           "user_id": 3, "created_at": 1789200100, "finished_at": null },
  "log": "..." }
```

`status` `success` ya da `failed` olunca iş bitmiştir.

Canlı günlük:

```sh
curl "${H[@]}" -N $BASE/jobs/43/stream
# data: {"type":"log","line":"$ certbot ..."}
# data: {"type":"done","status":"success"}
```

### Kapsamlı anahtar üretme ve kullanma

```sh
# oturumla (çerez + CSRF başlığı)
curl -sk -b jar.txt -H 'X-PixPanel-CSRF: 1' -H 'Content-Type: application/json' \
  -X POST $BASE/auth/tokens -d '{"name":"site-botu","scope":["sites","ssl"],"read_only":false}'
# {"id": 5, "token": "pix_..."}

curl "${H[@]}" $BASE/auth/me          # "token_scope": ["sites","ssl"]
curl "${H[@]}" $BASE/sites            # 200
curl "${H[@]}" $BASE/databases        # 403 (kapsam dışı)
curl "${H[@]}" $BASE/users            # 403 (alan dışı yönetim ucu)
```

### Kimlik ve hata

```sh
curl "${H[@]}" $BASE/auth/me
```

```json
{ "id": 1, "username": "admin", "email": "admin@ornek.com", "role": "admin", "parent_id": null,
  "permissions": [], "language": "tr", "via_token": true, "impersonated_by": null }
```

Geçersiz alan adı:

```json
{ "error": { "code": "bad_request", "message": "Geçersiz alan adı" } }
```
