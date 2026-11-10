# Felaket kurtarma: panelin kendisini yedekten geri getirmek

Site, veritabanı ve posta yedekleri panelin **Yedekler** sayfasından geri yüklenir. Bu belge, sunucu tümüyle kaybedildiğinde panelin kendi verisini (hesaplar, site ayarları, sertifikalar, posta ve DNS kayıtları) boş bir sunucuya geri getirmek içindir.

> **Durum (10.11.2026):** 1–4. adımlar (panel yedeğini açıp dosyaları yerine koymak) elle denendi. 5. adım, yani sitelerin sistem tarafını (Linux kullanıcıları, nginx, PHP havuzları) topluca yeniden kuran komut henüz yok; bu yüzden tam sunucu kurtarma uçtan uca sınanmadı.

## Elinizde olması gerekenler

- Yedeklerin durduğu yere erişim (SFTP sunucusu, S3 kovası ya da diskin kendisi).
- **Depo parolası.** Panelde *Yedekler → Yedek yerleri → Parolayı göster* ile görülür. Sunucu gittikten sonra panelden okunamaz; önceden panelin dışında bir yere kaydedilmiş olmalıdır. Parola olmadan yedekler açılamaz.
- Planlarda "Panel ayarları" işaretli olmalı; panel yedeği yalnızca o zaman alınır.

## Adımlar

1. Yeni sunucuya PixPanel'i ve restic'i kurun (`apt install restic`).

2. Depoyu tanıtın. Parolayı komut geçmişine yazmamak için dosyadan okutun:

   ```sh
   install -m 600 /dev/null /root/depo-parolasi   # içine parolayı yazın
   export RESTIC_PASSWORD_FILE=/root/depo-parolasi

   # Yerel klasör ya da bağlanmış disk:
   export RESTIC_REPOSITORY=/mnt/yedek/pixpanel
   # SFTP:
   export RESTIC_REPOSITORY=sftp://kullanici@sunucu:22//tam/yol
   # S3:
   export RESTIC_REPOSITORY=s3:https://s3.ornek.com/kova/onek
   export AWS_ACCESS_KEY_ID=…  AWS_SECRET_ACCESS_KEY=…
   ```

3. Panel yedeklerini listeleyin ve en yenisini geçici bir klasöre açın:

   ```sh
   restic snapshots --tag kind:panel
   restic restore latest --tag kind:panel --target /root/panel-geri
   ```

4. Paneli durdurup dosyaları yerine koyun:

   ```sh
   systemctl stop pixpanel
   rm -f /var/lib/pixpanel/pixpanel.db-wal /var/lib/pixpanel/pixpanel.db-shm
   install -m 600 /root/panel-geri/var/lib/pixpanel/backup/panel/pixpanel.db /var/lib/pixpanel/pixpanel.db
   cp -a /root/panel-geri/etc/pixpanel/. /etc/pixpanel/
   systemctl start pixpanel
   ```

   Bu noktada eski hesaplarla panele girilebilir; yedek yerleri ve planlar da geri gelmiştir.

5. Panelde *Bileşenler* sayfasından eski sunucuda kurulu olan bileşenleri (nginx, PHP sürümleri, veritabanları, posta…) kurun. Ardından *Yedekler* sayfasından siteleri, veritabanlarını ve postaları geri yükleyin.

   **Bilinen eksik:** panel veritabanında site kayıtları durur ama sistem tarafları (Linux kullanıcısı, nginx yapılandırması, PHP havuzu) yeni sunucuda yoktur. Şimdilik her sitenin ayarlarını panelden bir kez kaydetmek sistemi yeniden kurar; bunu topluca yapan komut yol haritasında.

6. `/root/depo-parolasi` ve `/root/panel-geri` klasörünü silin.

## Yedekte neler var

Her yedek deposundaki bir "anlık görüntü"dür ve etiketlerle ayırt edilir:

| Etiket | İçerik |
|---|---|
| `kind:site`, `name:<alan adı>` | Site klasörü, sertifikası ve ayarları (`site.json`) |
| `kind:db`, `name:<ad>`, `engine:<mariadb\|postgresql>` | SQL dökümü (`/<motor>-<ad>.sql`) |
| `kind:mail`, `name:<alan adı>` | `/var/vmail/<alan adı>` |
| `kind:panel` | Panel veritabanının kopyası ve `/etc/pixpanel` |
| `plan:<numara>` ya da `manual` | Hangi planın aldığı ya da elle alındığı |

Panel olmadan tek bir veritabanını geri almak:

```sh
restic dump latest --tag kind:db,name:blog /mariadb-blog.sql | mariadb blog
```
