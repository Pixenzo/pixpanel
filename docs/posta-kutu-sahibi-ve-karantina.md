# Posta: kutu sahibinin kendi işleri ve karantina (17a, 17d)

Kod: `server/src/mail/owner.rs`, `server/src/mail/quarantine.rs`, `server/src/api/mail.rs`, `server/src/dbtools/` (Roundcube),
`web/src/pages/MailQuarantine.tsx`. Sınama: `scripts/e2e/mail-owner.sh` (+ `managesieve.py`).

## A. Kutu sahibinin kendi işleri

Kutu sahibinin panel hesabı yoktur; yalnızca adresi ve kutu parolası vardır.

### Parola değiştirme

- Uç: `POST /api/v1/mail/public/password` `{ address, current_password, new_password }` (oturumsuz; gövde en çok 4 KB).
- Geçerli parola, panelin kutu parolasını sakladığı Argon2 özetiyle doğrulanır. Yeni parola panelin parola kuralından geçer
  (`auth::validate_password`, en az 10 karakter) ve eskisiyle aynı olamaz. Değişiklik, panelin "kutu parolası değiştir" yolundaki gibi
  veritabanına yazılır, eşleme dosyaları yeniden üretilir (`mail::sync`), kutunun açık IMAP oturumları kapatılır.
- **Kaba kuvvet koruması:** panelin giriş sınırıyla aynı altyapı (`state.throttle`, 5 dakikada 8 başarısız deneme): hem adres başına
  hem istemci adresi başına; aşılınca 429. Sınır, geçerli parola gelse bile adres kilitliyken uygulanır (kilit yalnızca bu uç içindir,
  IMAP/SMTP girişi etkilenmez). Yanlış parola, var olmayan adres ve kapalı kutu **aynı yanıtı** (400 "Adres ya da geçerli parola hatalı")
  ve benzer süreyi verir (var olmayan adreste sahte özetle Argon2 yine koşar).
- **İstemci adresi:** gerçek bağlantı adresidir; `X-Forwarded-For` okunmaz. Tek istisna web postası: Roundcube PHP süreci panele
  sunucunun içinden bağlandığı için bağlantı adresi hep yereldir; bu yüzden sürücü ziyaretçinin adresini `X-PixPanel-Client-Ip` başlığıyla,
  web postası ile panelin paylaştığı gizli anahtarı `X-PixPanel-Webmail-Key` başlığıyla gönderir. Panel bildirilen adresi yalnızca
  bağlantı yerelse **ve** anahtar doğruysa kullanır (sabit zamanlı karşılaştırma). Anahtar `/var/lib/pixpanel/tools/roundcube/api_key`
  (yalnız root) ve `/etc/roundcube/config.inc.php` (root:web grubu, 0640) içindedir.
- **Günlük:** başarısız deneme `pixpanel::api::auth` hedefiyle, panelin giriş hapsinin (`pixpanel` süzgeci) okuduğu biçimde yazılır:
  `başarısız posta kutusu parola denemesi ip=<adres> username=<kutu adresi>`. Adres `ip=` alanından sonra gelir ve yalnızca
  katı biçimli adres olabilir (`=` içeremez); sahte adres başkasını engelletemez. Süzgeç değişmedi.
- Denetim günlüğü: `mail.mailbox.self_password` (kayıt, alan adının sahibi hesabın altına düşer; işlemi yapan olarak kutu adresi yazılır).

### Roundcube

- Yapılandırma `dbtools::ensure_runtime` ile (idempotent; panel açılışında ve web postası ilk bakışta) yazılır: `password` ve `managesieve`
  eklentileri (kuruluysa) etkinleştirilir. Eklenti ayarları eklentilerin kendi `config.inc.php` dosyalarına yazılır (Roundcube eklenti
  ayarını ana ayarın üstüne bindirir; Debian'da bu dosya `/etc/roundcube/plugins/<ad>/` altına sembolik bağdır, hedefi yazılır).
- Parola: yerleşik `httpapi` sürücüsü form kodlamalı gönderir, başlık ekleyemez ve sertifika doğrulamasını kapatamaz; bu yüzden küçük
  bir özel sürücü (`drivers/pixpanel.php`, kaynağı `dbtools/roundcube-password-driver.php`) yazıldı. Sürücü paket dosyaları arasında
  durur; dpkg bilmediği dosyaya dokunmaz, panel her `ensure_runtime`'da yeniden yazar.
- ManageSieve: Dovecot'un `managesieve-login` servisi **yalnızca 127.0.0.1:4190** dinler (daha önce her adresi dinliyordu), STARTTLS ister.
  Roundcube `tls://127.0.0.1:4190` ile bağlanır. Otomatik yanıt arayüzü (`managesieve_vacation = 1`) açıktır.

### Panel kuralları ile kutu sahibinin betiği çakışmaz

- Önceki düzen: panel betiği `~/sieve/pixpanel.sieve`, etkinleştirme `~/.dovecot.sieve` bağıydı; kutu sahibi ManageSieve ile ezebilirdi.
- Yeni düzen (Dovecot 2.4):
  - `sieve_script panel { type = before; path = /var/vmail/<alan>/<kutu>/sieve-before/ }` — panelin betiği (`pixpanel.sieve`) kutu
    sahibinin betiğinden **önce** çalışır ve ManageSieve'in gördüğü `~/sieve` dışındadır: kutu sahibi onu göremez, okuyamaz, silemez.
  - `sieve_script personal` (`~/sieve`, `~/.dovecot.sieve`) tamamen kutu sahibinindir; panel ona dokunmaz.
  - Sıra: genel istenmeyen posta betiği (Junk'a taşı + dur) → panelin betiği → kutu sahibinin betiği. Dovecot kuralı: sıradaki betik, bir
    önceki betik iletiyi "tutuyorsa" (keep) çalışır. Yani panelin bir filtresi iletiyi klasöre taşıyıp `stop` derse kutu sahibinin betiği
    o ileti için çalışmaz (panel kuralları önceliklidir); otomatik yanıt gibi iletiyi tutan eylemlerde ikisi de çalışır.
- **Geçiş:** panel açılışında (`tasks.rs` → `mail::upgrade_owner_features`) bir kez: yeni Dovecot yapılandırması yazılır ve sınanır
  (hata olursa eski dosya geri konur), Dovecot yeniden yüklenir, her kutunun panel betiği yeni yere yazılır, eski düzenin artıkları
  (`~/sieve/pixpanel.sieve`, yalnızca ona işaret eden `.dovecot.sieve` bağı) silinir ve en sonda `/etc/pixpanel/mail/sieve-layout`
  işareti yazılır (adım başarısız olursa işaret yazılmaz, sonraki açılışta yeniden denenir). `POST /mail/setup` da aynı geçişi uygular.
  Eski artıkların silinmesi yalnızca geçişte yapılır; sonradan kutu sahibinin açtığı "pixpanel" adlı kendi betiği silinmez.
- Kutu sahibi ManageSieve'e `redirect` yazarak ileti yönlendirebilir (panelde de yapılabilen bir şey); Dovecot'un `sieve_max_redirects`
  sınırı geçerlidir. Bu çıkış, panelin saatlik gönderim sınırını (kimlikli SMTP) aşar; ele geçirilmiş kutu riskine karşı ayrı bir
  sınır gerekirse sonraki iş.

## B. Karantina

- Kip (sunucu geneli, İstenmeyen posta sekmesi): **Reddet** (varsayılan, bugünkü davranış) / **Karantinaya al**.
  `SpamSettings.reject_action`; eski kayıtlar `reject` sayılır.
- Mekanizma (en az parçalı olan): Rspamd `quarantine_on_reject = true` (`/etc/rspamd/local.d/worker-proxy.inc` içinde panelin işaretli
  bloğu; dosyanın öbür satırlarına dokunulmaz) reddedilecek iletiler için milter "karantina" komutu gönderir, Postfix iletiyi kendi
  **bekletme (hold) kuyruğuna** alır. İletiler `/var/spool/postfix/hold` altında (yalnız postfix) durur; ayrı bir depo ya da
  `/var/vmail/.quarantine` yoktur. SMTP'de ileti kabul edilir (gönderene hata dönmez, geri saçılım olmaz).
  Ayar `rspamadm configtest` ile sınanır, hata olursa önceki dosyalar geri konur.
- Panel: `postqueue -j` (yalnızca `queue_name = hold`) ile listeler, `postcat -q -h -b` ile ilk 64 KB'ı gösterir (süreç yeterince
  okununca öldürülür), `postsuper -H` ile bırakır (normal teslimata girer, gelen kutusuna düşer; yeniden taranmaz),
  `postsuper -d <kimlik> hold` ile siler. Kuyruk kimliği `^[0-9A-Za-z]{6,20}$`.
- Liste: alıcı, gönderen, konu (RFC 2047 çözülür), puan, tarih, boyut. Puan iletinin `X-Spamd-Result` başlığından, yoksa Rspamd
  geçmişinden (gönderen + konu eşleşmesi, son ~200 ileti) alınır; bulunamazsa boş görünür.
- **Sahiplik kuralı (sade olan seçildi):** yönetici bütün karantinayı görür. Diğer hesaplar yalnızca **bütün alıcıları** kendi
  görebildikleri posta alan adlarında olan iletileri görür (liste, içerik, bırakma, silme, toplu silme hepsi aynı süzgeçten geçer);
  bir alıcısı bile başka hesabın ya da dış adresin olan ileti onlara hiç gösterilmez (404). İleti alıcı başına ayrıştırılmaz.
  Böyle iletiler yönetici tarafından yönetilir.
- Saklama: 14 günden eski iletiler `tasks.rs` içindeki 10 dakikalık döngüyle silinir (`quarantine::purge_expired`).
- Denetim günlüğü: `mail.quarantine.release`, `mail.quarantine.delete`, `mail.quarantine.bulk_delete`; ayar değişikliği
  `mail.settings` kaydının ayrıntısında.
