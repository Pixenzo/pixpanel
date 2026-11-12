#!/usr/bin/env bash
# Bileşenlerin panelden sıfırdan kurulmasının temiz makinede koşan sınaması;
# components.sh tarafından çağrılır. /root/rel altında sürüm dosyalarını, /root/e2e
# altında öteki sınama betiklerini bekler. Makinede panel KURULU OLMAMALIDIR.
#
# Önce paneli kurar, sonra her bileşeni panelin API'siyle kurup çalıştığını dener,
# en sonda özellik sınamalarını (posta, web postası, veritabanı araçları...) bu
# yeni kurulmuş makinede koşturur.
set -uo pipefail
# pct exec yolu dar verir; panelin komut satırı /usr/local/bin altındadır.
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
REL=/root/rel
E2E=/root/e2e
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-components
ADMIN=admin
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | tail -c 300)]"; fi; }
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 900); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
field() { api GET /components | jget "next(c['$2'] for c in d if c['id']=='$1')"; }
wanted() { [ -z "${PX_ONLY:-}" ] && return 0; case " $PX_ONLY nginx " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# install_component <kimlik> <çalışan servis beklenir mi: yes|no>
install_component() {
  local id=$1 job result started=$SECONDS
  wanted "$id" || return 1
  job=$(api POST "/components/$id/install" | jget 'd["job_id"]')
  result=$(wait_job "$job")
  check "$id kurulumu ($((SECONDS - started)) sn)" "$result" success
  if [ "$result" != success ]; then
    api GET "/jobs/$job" | jget 'd["log"][-1500:]' | sed 's/^/        /'
    return 1
  fi
  check "$id kurulu görünüyor" "$(field "$id" installed)" True
  [ "$2" = yes ] && check "$id servisi çalışıyor" "$(field "$id" running)" True
  check "$id ikinci kez kurulamaz" "$(code POST "/components/$id/install")" 409
  return 0
}

. /etc/os-release
echo "Makine: $PRETTY_NAME"
[ -e /usr/local/bin/pixpanel ] && { echo "  HATA bu makinede panel zaten kurulu; sınama temiz makine ister"; exit 1; }

say "Panel kurulumu"
STARTED=$SECONDS
OUT=$(bash $REL/install.sh --from-dir $REL --admin $ADMIN 2>&1); RC=$?
check "kurulum ($((SECONDS - STARTED)) sn)" "$RC" 0
[ "$RC" = 0 ] || { echo "$OUT" | tail -15; exit 1; }
PASSWORD=$(sed -n 's/.*Parola *: *//p' <<<"$OUT" | head -1)
# Sınama betiklerinin kendi gereksinimi (JSON ayrıştırma, sahte sunucular); panelin değil.
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3 >/dev/null 2>&1
# Telegram ve Cloudflare çağrıları sınama boyunca yerel sahte sunuculara gider.
printf 'telegram_api = "http://127.0.0.1:9911"
cloudflare_api = "http://127.0.0.1:9912"
' >> /etc/pixpanel/config.toml
systemctl restart pixpanel
for _ in $(seq 1 20); do curl -sk -o /dev/null https://127.0.0.1:8443/ && break; sleep 1; done
check "giriş" "$(code POST /auth/login -d "{\"username\":\"$ADMIN\",\"password\":\"$PASSWORD\"}")" 200
# Sunucu kalıbı Postfix gibi tek tek paketlerle gelebilir; bu, bileşeni kurulu göstermemeli.
check "başlangıçta hiçbir bileşen kurulu değil" "$(api GET /components | jget '[c["id"] for c in d if c["installed"]]')" "[]"
check "olmayan bileşen" "$(code POST /components/yok/install)" 404

say "Web"
install_component nginx yes
# Öteki betikler 1 numaralı sitenin deneme.test (PHP), 2 numaralının statik.test olmasını bekler.
if install_component php-8.3 yes; then
  check "PHP sitesi 1 numarayla açıldı" "$(api POST /sites -d '{"domain":"deneme.test","kind":"php","php_version":"8.3"}' | jget 'd["id"]')" 1
  check "kendinden imzalı sertifika" "$(code POST /sites/1/ssl/self-signed)" 200
  ROOT_DIR=/home/$ADMIN/sites/deneme.test/public
  printf '<?php echo PHP_MAJOR_VERSION, ".", PHP_MINOR_VERSION, "|", get_current_user(), "|", extension_loaded("mysqli") ? "mysqli" : "", "|", extension_loaded("gd") ? "gd" : "";' > $ROOT_DIR/surum.php
  chown $ADMIN:www-data $ROOT_DIR/surum.php
  check "PHP sitesi çalışıyor, sitenin kullanıcısıyla" "$(curl -s -H 'Host: deneme.test' http://127.0.0.1/surum.php)" "8.3|$ADMIN|mysqli|gd"
  check "komut satırı PHP" "$(php8.3 -r 'echo PHP_MAJOR_VERSION;')" 8
  grep -q "deneme.test" /etc/hosts || echo "127.0.0.1 deneme.test statik.test" >> /etc/hosts
  check "HTTPS üzerinden de yayında" "$(curl -sk https://deneme.test/surum.php | cut -c1-3)" "8.3"
fi
if wanted nginx; then
  api POST /sites -d '{"domain":"statik.test","kind":"static"}' >/dev/null
  check "statik site açıldı" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: statik.test' http://127.0.0.1/)" 200
  check "tanınmayan alan adı siteye düşmez" "$(curl -s -H 'Host: baska.test' http://127.0.0.1/ | grep -c statik.test)" 0
  check "sertifika doğrulama klasörü web sunucusuna açık" "$(mkdir -p /var/lib/pixpanel/acme/.well-known/acme-challenge && echo tamam > /var/lib/pixpanel/acme/.well-known/acme-challenge/deneme && curl -s -H 'Host: statik.test' http://127.0.0.1/.well-known/acme-challenge/deneme; rm -f /var/lib/pixpanel/acme/.well-known/acme-challenge/deneme)" tamam
fi
if install_component node-22 no; then
  has "Node.js sürümü" "$(field node-22 version)" "22."
fi

say "Veritabanları"
if install_component mariadb yes; then
  check "MariaDB veritabanı ve kullanıcı" "$(code POST /databases -d '{"engine":"mariadb","name":"denemedb","user":{"name":"denemeu","password":"Vt-Parola-13579"}}')" 200
  check "kullanıcı kendi veritabanına bağlanıyor" "$(mariadb -u denemeu -pVt-Parola-13579 -N -e 'SELECT DATABASE()' denemedb 2>&1)" denemedb
  check "başka veritabanına erişemiyor" "$(mariadb -u denemeu -pVt-Parola-13579 -N -e 'SELECT COUNT(*) FROM mysql.user' 2>&1 | grep -c denied)" 1
fi
if install_component postgresql yes; then
  check "PostgreSQL veritabanı ve kullanıcı" "$(code POST /databases -d '{"engine":"postgresql","name":"denemepg","user":{"name":"denemepgu","password":"Vt-Parola-24680"}}')" 200
  check "kullanıcı kendi veritabanına bağlanıyor" "$(PGPASSWORD=Vt-Parola-24680 psql -h 127.0.0.1 -U denemepgu -tAc 'SELECT current_database()' denemepg 2>/dev/null)" denemepg
fi
if install_component redis yes; then
  check "Redis yanıt veriyor" "$(api GET /redis | jget 'd["running"]')" True
  check "Redis dışarıya kapalı" "$(ss -ltnH 'sport = :6379' | awk '{print $4}' | grep -vc '^127\.0\.0\.1:\|^\[::1\]:')" 0
fi
install_component phpmyadmin no
install_component adminer no

say "Sunucu hizmetleri"
install_component fail2ban yes
install_component dns yes
install_component restic no
install_component ftp yes
install_component docker yes && has "Docker konteyner çalıştırabiliyor" "$(docker run --rm hello-world 2>&1)" "Hello from Docker"

say "Posta"
install_component mail yes && {
  check "Dovecot ve Rspamd çalışıyor" "$(systemctl is-active dovecot rspamd | tr '\n' ' ')" "active active "
  check "SMTP dinliyor" "$(ss -ltnH 'sport = :25' | wc -l | sed 's/[1-9][0-9]*/var/')" var
}
install_component roundcube no && check "web postası yayında" "$(curl -sk -o /dev/null -w '%{http_code}' https://127.0.0.1:8443/webmail/)" 200

# Özellik sınamaları bu yeni kurulmuş makinede koşar; bir bileşen eksik yapılandırılmışsa
# burada ortaya çıkar. Her betiğin yalnızca hata satırları ve sayıları gösterilir.
say "Özellik sınamaları"
# Betiklerin geliştirme makinesinde hazır bulduğu kayıtlar: alt alan adlı bir site ve
# sitesiyle aynı adı taşıyan posta alan adı (DNS bölgesi açılınca kayıtları beklenir).
api POST /sites -d '{"domain":"uygulama.deneme.test","kind":"static"}' >/dev/null
wanted mail && api POST /mail/domains -d '{"domain":"deneme.test"}' >/dev/null
systemd-run --quiet --unit pxmocks python3 $E2E/mocks.py 2>/dev/null
suite() {
  local name=$1 need output
  shift
  for need in "$@"; do wanted "$need" || return 0; done
  output=$(cd $E2E && PX_USER=$ADMIN PX_PASS="$PASSWORD" bash $E2E/$name.sh 2>&1)
  grep "HATA" <<<"$output" | cut -c1-260 | sed "s/HATA/HATA [$name]/"
  echo "  ---- $name: $(grep -c '  OK ' <<<"$output") geçti, $(grep -c 'HATA' <<<"$output") hata"
}
suite mail mail
suite webmail mail roundcube
suite dbtools mariadb postgresql phpmyadmin adminer php-8.3
suite node node-22
suite ftp ftp
suite dns dns mail
suite dns-depth dns
suite redis redis
suite backup restic mariadb
suite features php-8.3 docker redis
# Panel sertifikası sınaması ACME sınama sunucusu (Pebble) ister; o yalnızca geliştirme makinesinde var.
[ -f /etc/systemd/system/pebble-test.service ] && suite panelcert
suite twofactor
suite events
suite webhooks
suite reseller-brand
suite sys-updates
suite health ftp
suite wordpress php-8.3 mariadb restic
suite packages mail ftp dns mariadb
suite limits php-8.3 python redis
suite backup-mine restic mariadb
suite access
suite stats
suite metrics
suite backup-ftp restic ftp
suite templates php-8.3
suite cli
suite api-scopes
suite files-extract
suite ips dns
suite redis-account redis
suite impersonate
suite rules php-8.3
suite site-perf php-8.3
suite http3
suite clone php-8.3 mariadb
suite branding
suite python python
suite python-uv python
suite dbimport mariadb
suite db-depth mariadb postgresql
suite db-remote mariadb postgresql
suite tenancy dns mail mariadb
suite portguard node-22
suite dkim mail
suite mail-depth mail
suite mail-import mail
suite mail-owner mail roundcube
suite migrate php-8.3 mariadb
suite staging php-8.3 mariadb
systemctl stop pxmocks 2>/dev/null

say "Kaynak kullanımı"
df -h / | tail -1 | awk '{print "  disk: " $3 " / " $2}'
free -m | awk '/^Mem/ {print "  bellek: " $3 " MB / " $2 " MB"}'
check "başarısız servis yok" "$(systemctl --failed --no-legend | wc -l)" 0
systemctl --failed --no-legend | sed 's/^/        /'
echo "bitti"
