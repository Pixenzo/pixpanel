# Araştırma notları

10.11.2026 tarihinde HestiaCP, 1Panel, CloudPanel, CyberPanel ve aaPanel kaynaklarından çıkarılan, uygulamada işe yarayacak somut bilgiler. Bir inceleme ajanının raporundan damıtılmıştır; "doğrulanmadı" yazanlar uygulanırken ayrıca sınanmalıdır.

## nginx + PHP-FPM (HestiaCP şablonlarından)

- Gizli dosyalar: `location ~ /\.(?!well-known\/) { deny all; return 404; }` — `.git`, `.env`, `.htaccess` dahil hepsini kapatır.
- PHP: `location ~ [^/]\.php(/|$) { try_files $uri =404; include fastcgi_params; fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name; fastcgi_pass unix:...; }` — var olmayan dosya PHP'ye gitmez.
- Statik dosyalar: `expires max; fastcgi_hide_header "Set-Cookie";`
- WordPress: `location ~* /(?:uploads|files)/.*\.php$ { deny all; }`
- Hestia'da eksik olan, bizim ekleyeceğimiz: `composer.json/lock`, `*.sql`, `*.bak`, `*.log` gibi dosyalara engel.
- Genel: `server_tokens off`, `ssl_protocols TLSv1.2 TLSv1.3`, tanımsız alan adları için `default_server` (444/404).
- OCSP stapling kullanılmaz (Let's Encrypt 2025'te desteği bitirdi).

PHP-FPM havuzu (site başına):

```ini
[site-adi]
listen = /run/php/php8.3-fpm-ALANADI.sock
listen.owner = KULLANICI
listen.group = www-data
listen.mode = 0660
user = KULLANICI
group = KULLANICI
pm = ondemand
pm.max_children = 8
pm.max_requests = 4000
pm.process_idle_timeout = 10s
php_admin_value[upload_tmp_dir] = /home/KULLANICI/tmp
php_admin_value[session.save_path] = /home/KULLANICI/tmp
php_admin_value[open_basedir] = <site kökü>:/home/KULLANICI/tmp:/tmp:/usr/share
php_admin_value[sendmail_path] = /usr/sbin/sendmail -t -i -f admin@ALANADI
env[TMPDIR] = /home/KULLANICI/tmp
```

Dizin izinleri (Hestia): ev dizini başkalarına kapalı; `public_html` sahibi `kullanıcı:www-data`, mod 751; dosyalar 644, klasörler 755; `tmp` 700; nginx günlükleri `root:kullanıcı` 640.

## ACME

- HTTP-01 için dosya yazmaya gerek yok; nginx doğrudan yanıtlayabilir:
  `location ~ "^/\.well-known/acme-challenge/(.*)$" { default_type text/plain; return 200 "$1.<HESAP_PARMAK_IZI>"; }`
- Joker sertifika için DNS-01: yerel DNS'e `_acme-challenge` TXT yazılır.
- İstekten önce alan adının bu sunucuya çözüldüğü denetlenmeli.

## Debian 13 (trixie)

- Depolar (hepsi trixie'yi destekliyor): sury PHP (`https://packages.sury.org/php/ trixie main`, anahtar `debsuryorg-archive-keyring.deb`), nginx.org, MariaDB, PGDG (`trixie-pgdg`), Docker, NodeSource (`node_24.x nodistro`).
- Debian'ın kendi paketleri: PHP 8.4, MariaDB 11.8, PostgreSQL 17, nginx 1.26, PowerDNS 4.9, Redis 8, fail2ban 1.1.
- `/tmp` artık tmpfs → PHP geçici dosyaları kullanıcı ev dizininde tutulmalı.
- `/etc/sysctl.conf` okunmuyor → `/etc/sysctl.d/*.conf`.
- Dovecot 2.4'ün yapılandırma biçimi eski sürümlerle uyumsuz.
- Hestia SFTP için `KexAlgorithms +diffie-hellman-group-exchange-sha256` eklemek zorunda kalmış.

## PowerDNS 4.9

```bash
apt install --no-install-recommends pdns-server pdns-backend-sqlite3   # recommends açık olursa bind arka ucu da gelir
sqlite3 /var/lib/powerdns/pdns.sqlite3 < /usr/share/pdns-backend-sqlite3/schema/schema.sqlite3.sql
chown pdns:pdns /var/lib/powerdns/pdns.sqlite3
```

`/etc/powerdns/pdns.d/`: `launch+=gsqlite3`, `gsqlite3-database=...`, `gsqlite3-dnssec=on`, `gsqlite3-pragma-foreign-keys=yes`; API: `api=yes`, `api-key=...`, `webserver=yes`, `webserver-address=127.0.0.1`, `webserver-port=8081`.

API (`X-API-Key` başlığı, taban `/api/v1/servers/localhost`):

- Bölge oluştur: `POST /zones` `{"name":"example.org.","kind":"Native","nameservers":["ns1.example.org."]}`
- Kayıt yaz: `PATCH /zones/example.org.` `{"rrsets":[{"name":"www.example.org.","type":"A","ttl":3600,"changetype":"REPLACE","records":[{"content":"1.2.3.4","disabled":false}]}]}`
- Kayıt sil: aynı uç, `"changetype":"DELETE"`. Bölge sil: `DELETE /zones/example.org.`
- Adlar sonda nokta ile tam yazılır.

## Posta: Stalwart hakkında uyarı

- Yönetim API'si v0.16'da (Nisan 2026) REST'ten JMAP'a taşındı; v1.0 (Ekim 2026) veritabanı düzenini değiştirdi ve yerinde yükseltmeyi kırdı. 7 ayda iki kırıcı değişiklik.
- Varsayılan olarak 443 ve 8080'i dinler → nginx ile çakışır, dinleyiciler değiştirilmeli (nasıl yapılacağı doğrulanmadı).
- Yönetim: `POST /jmap/`, `x:Domain/set`, `x:Account/set`; kimlik `Authorization: Bearer API_...`. Listeler JSON dizisi değil nesne olarak yazılır.
- Kullanılacaksa: sürüm sabitlenmeli, erişim tek bir bağdaştırıcı katmanının arkasında durmalı.

## Diğer panellerde olup eklenmeye değer fikirler

- vhost düzenleyici: kaydederken sözdizimi denetimi ve hata hâlinde otomatik geri alma.
- Uygulama şablonları (WordPress, Laravel, statik, Node, Python) ve tek tıkla kurulum.
- Site başına: IP/bot engelleme, parola koruması, yönlendirmeler, hotlink engeli, HTTP/3, "yalnızca Cloudflare" kipi.
- Panel erişimini IP listesiyle daraltma, gizli giriş yolu.
- Kullanıcı paketleri ve kotalar (site, veritabanı, disk sayısı), cgroup ile kaynak sınırı.
- Uzak yedek hedefleri (restic / rclone), git ile dağıtım.
- Site başına bant genişliği ve istatistik.

## Diğer panellerin hatalarından dersler

- HestiaCP'nin kritik açıklarının ortak kökü: kullanıcı verisini `anahtar='değer'` olarak dosyaya yazıp sonra `eval` ile okumak ve her işlemi root kabuk betiğiyle yapmak.
- 1Panel: sıralama alanı, dosya yolu, güvenlik duvarı IP'si, konteyner adı gibi alanlardan SQL/komut/yol enjeksiyonu; `X-Forwarded-For` başlığına güvenen IP denetimi.
- CyberPanel (2024): kimlik doğrulamasız uç noktadan komut çalıştırma, binlerce sunucuda fidye yazılımı.
- CloudPanel: PHP-FPM'e TCP ile bağlanıyor (aynı makinedeki her süreç erişebilir); unix soketi + dosya izni daha sıkı.
