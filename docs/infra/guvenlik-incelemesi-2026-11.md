# Güvenlik incelemesi (13.11.2026)

İlk açık sürümden sonra bütün sunucu kodu beş alanda okundu: kimlik ve oturum, komut çalıştırma ve girdi doğrulama, dosya ve arşiv işlemleri, ağ ve posta, 324 HTTP ucunun yetki denetimi. İnceleme kodu okuyarak yapıldı (beş ayrı okuyucu); her bulgu sonra kodda doğrulandı, düzeltildi ve `scripts/e2e/security.sh` ile test makinesinde sömürülmeye çalışılarak sınandı (47 denetim).

## Düzeltilenler

Önem sırasıyla. "Kim" sütunu açığı kullanabilecek en düşük yetkili hesaptır.

| Açık | Kim | Etki | Düzeltme |
| --- | --- | --- | --- |
| Python sitesinin `.venv/bin` klasörüne `setpriv` ya da `systemd-run` adında dosya koyup bağımlılık kurulumunu ya da Git yayınını başlatmak | Python sitesi olan müşteri | **Root olarak kod çalıştırma.** Komutun `PATH` değeri kullanıcının klasörünü içeriyordu; yetkiyi düşüren program yalın adla çağrıldığı için panel onu, henüz root iken, o klasörde buluyordu. | Yetki düşüren programlar her yerde tam yolla çağrılır (`sys::runas::SETPRIV`, `SYSTEMD_RUN`). |
| `/webmail/../tools/phpmyadmin/` yolu | Oturumsuz herkes | Veritabanı araçlarının (phpMyAdmin, Adminer) oturum öncesi yüzeyine panel oturumu olmadan ulaşma. Giriş yine tek kullanımlık anahtar istiyordu. | Vekil, iletilen adresi sadeleştirdikten sonra beklenen önekte kaldığını denetler; kodlanmış nokta ve bölü reddedilir. |
| Salt okunur yönetici API anahtarıyla `GET /terminal/ws?scope=root` | Ele geçirilmiş yönetici anahtarı | Root kabuğu. WebSocket bağlantısı GET olduğu için salt okunur kapıdan geçiyordu. | Terminal yalnızca tarayıcı oturumundan açılır. Anahtarla yönetici hesabı açmak ve başkasının parolasını değiştirmek de kapatıldı. |
| Vekil sitesinin hedefi olarak `127.0.0.1.nip.io:8443`, A kaydı 127.0.0.1 olan bir alan adı ya da `[::ffff:127.0.0.1]` | Müşteri | Panelin, posta süzgecinin, Redis'in yerel portlarını kendi alan adından internete açma. | Hedef alan adı çözülür, her adres denetlenir; gömülü IPv4 biçimleri ve özel ağlar web kancalarıyla aynı kuralla reddedilir. |
| Başka hesabın DNS bölgesi ya da posta alanı olan (henüz sitesi olmayan) alan adına site açmak | Müşteri | O alan adını yayınlama ve sertifika alma; asıl sahibinin site açmasını engelleme. | Adın kendisi de (yalnızca alt adları değil) başka hesabınsa site açılamaz. |
| SQL dökümüne `\! komut` ya da `\copy … to program` yazıp yüklemek | Veritabanı olan müşteri | Sunucuda `nobody` olarak komut çalıştırma, herkesin okuyabildiği dosyaları görev günlüğünden okuma. | MariaDB istemcisi `--sandbox` ile, psql rastgele anahtarlı `\restrict` kipinde çalışır. |
| Kullanıcı adına satır sonu koyarak giriş denemek | Oturumsuz herkes | fail2ban'in okuduğu günlüğe sahte satır sokup istenen bir adresi (yöneticinin, müşterinin) panelden engelletme. | Geçerli bir kullanıcı adında bulunamayacak karakter içeren ad günlüğe yazılmaz. |
| Kendi hesabıyla araya giriş yapıp sayacı sıfırlamak; IPv6 /64 bloğundan her denemede yeni adres | Herhangi bir hesabı olan biri / oturumsuz | Giriş sınırının aşılması: sınırsız parola ve iki adımlı doğrulama kodu denemesi. Eşzamanlı Argon2 hesaplarıyla bellek tüketme. | Başarılı giriş sayacı sıfırlamaz; IPv6'da /64 başına sayılır; aynı anda en çok 4 parola doğrulaması koşar. |
| Sahibi askıya alınmış müşterinin alt hesabıyla giriş | Alt hesap | Askıdaki hesabın dosyalarına, veritabanlarına, zamanlı görevlerine erişmeyi sürdürme. | Sahibi askıdaki alt hesap giremez; oturumu ve API anahtarı da çalışmaz. |
| Taşıma aracında karşı sunucu olarak `192.168.x.x`, `10.x.x.x` | Müşteri | İç ağdaki makineleri tarama, SSH parolası deneme. | Özel ağ adreslerine bağlanılmaz (web kancalarıyla aynı kural). |
| Paketi olan bayinin müşteriyi paketsiz açması ya da paketini kaldırması | Bayi | Kendi paketindeki tavanları fiilen kaldırma (paketsiz hesap sınırsızdır). | Paketi olan bayinin müşterileri paketli olmak zorundadır. |
| Uygulama sitesinde bellek sınırını 0 yapmak | Müşteri | Port koruması açıkken uygulama servisi hesabın işlemci, bellek, süreç sınırının dışında kalıyordu; sunucuyu tüketme. | Paket sınırı uygulama birimine doğrudan yazılır; site ayarı paket sınırını aşamaz. |
| Web postasına çok büyük gövde göndermek | Oturumsuz herkes | Diski ve araç havuzunu doldurma. | Web postasına 64 MB, oturumlu araçlara 1 GB sınır; boyutu bildirilmeyen gövde akarken kesilir. |
| İki adımlı doğrulama kodunu ya da kurtarma kodunu aynı anda iki girişte kullanmak | Kodu ele geçiren | Tek kullanımlık kodun iki kez kullanılması. | Kod, koşullu güncellemeyle yalnızca bir kez tüketilir. |
| Ele geçirilmiş oturumla iki adımlı doğrulamayı kapatmak için parolayı denemek | Ele geçirilmiş oturum | Parolayı tahminle bulma. | Oturum içi parola sorgusu hesap başına sınırlıdır. |
| Parolası ya da iki adımlı doğrulaması sıfırlanan hesabın API anahtarları | Saldırganın açtığı anahtar | Hesap kurtarıldıktan sonra da erişimin sürmesi. | Sıfırlamada hesabın anahtarları da silinir. |
| Kendi parolasını `PATCH /users/{kendi}` ile değiştirmek | Ele geçirilmiş oturum | Mevcut parolayı bilmeden parola değiştirme. | Kendi parolası yalnızca mevcut parolayı soran `/auth/password` ucundan değişir. |
| Deneme bildirimi ucunu art arda çağırmak | Herhangi bir hesap | Panelin SMTP sunucusundan istenen adrese posta yağdırma. | Hesap başına beş dakikada sekiz deneme. |

## Temiz çıkanlar

- **Başkasının kaynağına erişim (IDOR):** 324 uçta site, veritabanı, veritabanı kullanıcısı, posta kutusu, takma ad, DNS bölgesi, zamanlı görev, FTP hesabı, yedek, çöp kutusu, web kancası ve görev günlüğü için başka hesabın kaydı `404` döner. Belgede "Yönetici" yazan bütün uçlar `require_admin` çağırır.
- **Kabuk kullanımı:** kullanıcı girdisi hiçbir yerde kabuk dizesine girmez.
- **Yapılandırma üretimi:** nginx, PHP-FPM, systemd, cron, sieve, Postfix, Rspamd, DNS ve nftables girdileri beyaz listeden ya da kaçışlamadan geçer.
- **SQL:** MariaDB ve PostgreSQL ad ve parolaları kaçışlanır ya da istemci değişkeniyle verilir.
- **Kullanıcı dosyaları:** dosya yöneticisi, arşiv açma, yedek geri yükleme, Git yayını ve WordPress işleri kullanıcının kimliğiyle ve `chroot` içinde çalışır; bağ ve aygıt dosyaları atlanır.
- **Güncelleme:** yalnızca imzalı ve daha yeni sürüm kabul edilir.
- **Web kancaları:** her denemede adres yeniden çözülür ve sabitlenir; yönlendirme izlenmez.
- **Oturum:** çerezler `HttpOnly`, `SameSite=Strict`, TLS'de `Secure`. `X-Forwarded-For` hiçbir yerde kullanılmaz.

## Açık kalanlar

Düşük önemli ya da tasarım kararı gerektirenler; ayrıca ele alınmalı.

- **Aynı kökende üçüncü taraf uygulamalar:** Roundcube, phpMyAdmin ve Adminer panelle aynı kökende çalışır ve içerik güvenlik politikası (CSP) yoktur. Bu uygulamalardan birinde bir XSS (örneğin gelen bir HTML e-postada), aynı tarayıcıda panel oturumu açık bir yöneticinin yetkisiyle API'yi çağırabilir. **Çözüldü (13.11.2026):** araçlar panelle aynı adreste ama ayrı kökende, kendi portunda (`tools_port`, varsayılan 8444) çalışır; alan adı gerekmez. Panel `/tools/` ve `/webmail/` isteklerini oraya yönlendirir (307), araçların panel oturum çerezini ezen `Set-Cookie` yanıtları süzülür. Sınama: `security.sh` "ayrı kökende" bölümü.
- **Yerel araç sunucusu:** `127.0.0.1:8446` aynı makinedeki müşteri koduna açıktır. Panel oturum denetimi atlanır ve `X-Real-IP` sahtelenebilir; Roundcube'un adres başına giriş sınırı bu yolla aşılabilir.
- **Bayi ve askıya alma:** bayi, yöneticinin askıya aldığı müşterisini yeniden etkinleştirebilir.
- **Olay akışı:** yalnızca dosya yetkisi olan alt hesap da sahibinin bütün görev başlıklarını görür.
- **Silinen bayinin müşterileri:** bayi silinince müşterilerin Linux tarafındaki artıkları (zamanlı görev dosyası, SSH anahtarı) temizlenmeyebilir (doğrulanmadı).
- **Diğerleri:**
  - Dosya indirmede FIFO dosyası yardımcı süreci asılı bırakır.
  - Git ve SFTP yedek bağlantıları ilk bağlantıda sunucu anahtarını sorgusuz kabul eder.
  - `install.sh` eski bir imzalı sürüme düşürmeyi engellemez.
  - Liste gönderim kısıtı 25. porttan sahte gönderenle aşılabilir.
  - Zamanlı görevlerde "şimdi çalıştır" hesabın kaynak sınırının dışında koşar.
