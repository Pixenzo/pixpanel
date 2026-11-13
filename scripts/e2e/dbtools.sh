#!/usr/bin/env bash
# Veritabanı araçları (phpMyAdmin, Adminer) ve Redis yönetiminin sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash dbtools.sh
set -uo pipefail
P=https://127.0.0.1:8443
# Araçlar panelden ayrı kökende, kendi portunda yayınlanır.
T=https://127.0.0.1:8444
B=$P/api/v1
J=/tmp/pxjar-tools
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(printf '%s' "$2" | tr -d '\n' | head -c 240)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
OV=$(api GET /databases)
echo "$OV" | jget '"araçlar:", d["tools"]'
MY=$(echo "$OV" | jget 'next(x["id"] for x in d["databases"] if x["engine"]=="mariadb")')
MYNAME=$(echo "$OV" | jget 'next(x["name"] for x in d["databases"] if x["engine"]=="mariadb")')
PG=$(echo "$OV" | jget 'next(x["id"] for x in d["databases"] if x["engine"]=="postgresql")')
PGNAME=$(echo "$OV" | jget 'next(x["name"] for x in d["databases"] if x["engine"]=="postgresql")')

say "phpMyAdmin ($MYNAME)"
URL=$(api POST /databases/$MY/open | jget 'd["url"]')
has "adres üretildi" "$URL" "/tools/phpmyadmin/pixpanel-signon.php?token="
check "panel oturumu olmadan erişim" "$(curl -sk -o /dev/null -w '%{http_code}' "$T$URL")" 401
check "araç sunucusu dışarıya kapalı" "$(curl -s -o /dev/null -m 3 -w '%{http_code}' "http://$(hostname -I | awk '{print $1}'):8446/tools/phpmyadmin/")" 000
PAGE=$(curl -skL -b $J -c $J "$T$URL")
has "phpMyAdmin açıldı" "$PAGE" "phpMyAdmin"
has "veritabanı seçili" "$PAGE" "$MYNAME"
check "anahtar ikinci kez kullanılamaz" "$(curl -sk -o /dev/null -w '%{http_code}' -b $J "$T$URL")" 403
check "iç klasörler kapalı" "$(curl -sk -o /dev/null -w '%{http_code}' -b $J "$T/tools/phpmyadmin/libraries/classes/Config.php")" 404
TEMP=$(mariadb -N -e "SELECT user FROM mysql.user WHERE user LIKE 'pxt\_%'" | head -1)
has "geçici hesap oluştu" "$TEMP" "pxt_"
has "geçici hesap yalnızca bu veritabanında yetkili" "$(mariadb -N -e "SHOW GRANTS FOR '$TEMP'@'localhost'" | grep -v USAGE)" "\`$MYNAME\`"
check "başka veritabanına yetkisi yok" "$(mariadb -N -e "SHOW GRANTS FOR '$TEMP'@'localhost'" | grep -c 'ON \*\.\* TO.*ALL')" 0

say "Adminer ($PGNAME)"
URL=$(api POST /databases/$PG/open | jget 'd["url"]')
has "adres üretildi" "$URL" "/tools/adminer/?token="
PAGE=$(curl -skL -b $J -c $J "$T$URL")
has "Adminer açıldı" "$PAGE" "Adminer"
has "veritabanına bağlandı" "$PAGE" "$PGNAME"
if printf '%s' "$PAGE" | grep -qiE 'name="auth\[password\]"|Invalid credentials|Unable to connect'; then echo "  HATA giriş formu ya da bağlantı hatası göründü"; else echo "  OK   giriş formu gösterilmedi"; fi
check "anahtar ikinci kez kullanılamaz" "$(curl -sk -o /dev/null -w '%{http_code}' "$T$URL" -b /tmp/none-$$ -H "cookie: $(grep pixpanel_session $J | awk '{print $6"="$7}')")" 403

say "Geçici hesapların temizliği"
sqlite3 /var/lib/pixpanel/pixpanel.db "UPDATE db_tool_sessions SET expires_at = 1" 2>/dev/null || python3 -c "
import sqlite3; c=sqlite3.connect('/var/lib/pixpanel/pixpanel.db'); c.execute('UPDATE db_tool_sessions SET expires_at = 1'); c.commit()"
systemctl restart pixpanel; sleep 4
check "MariaDB geçici hesapları silindi" "$(mariadb -N -e "SELECT COUNT(*) FROM mysql.user WHERE user LIKE 'pxt\_%'")" 0
check "PostgreSQL geçici rolleri silindi" "$(runuser -u postgres -- psql -Atc "SELECT COUNT(*) FROM pg_roles WHERE rolname LIKE 'pxt\_%'")" 0
rm -f $J; api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Redis"
api GET /redis | jget '(d["version"], d["running"], d["config"])'
redis-cli -n 3 flushdb >/dev/null
redis-cli -n 3 set "oturum:1" "merhaba dünya" >/dev/null
redis-cli -n 3 rpush "kuyruk:işler" a b c >/dev/null
redis-cli -n 3 hset "kullanıcı:7" ad Ali yas 30 >/dev/null
redis-cli -n 3 setex "geçici" 500 x >/dev/null
api GET "/redis/keys?db=3&pattern=*" | jget 'sorted((k["key"], k["type"], k["ttl"] > 0) for k in d["keys"])'
api GET "/redis/keys?db=3&pattern=kuyruk:*" | jget '[k["key"] for k in d["keys"]]'
api GET "/redis/key?db=3&key=oturum:1" | jget '(d["type"], d["content"]["value"]["text"])'
api GET "/redis/key?db=3&key=$(python3 -c 'import urllib.parse; print(urllib.parse.quote("kuyruk:işler"))')" | jget '(d["type"], d["length"], d["content"]["items"])'
api GET "/redis/key?db=3&key=$(python3 -c 'import urllib.parse; print(urllib.parse.quote("kullanıcı:7"))')" | jget '(d["type"], d["content"]["fields"])'
check "olmayan anahtar" "$(code GET '/redis/key?db=3&key=yok')" 404
check "anahtar sil" "$(api DELETE '/redis/key?db=3&key=oturum:1' | jget 'd["removed"]')" 1
check "komut enjeksiyonu anahtar adı olarak kalır" "$(code GET "/redis/key?db=3&key=$(python3 -c 'import urllib.parse; print(urllib.parse.quote("x\r\nFLUSHALL\r\n"))')")" 404
check "enjeksiyon veriyi silmedi" "$(redis-cli -n 3 dbsize)" 3
check "geçersiz ilke" "$(code POST /redis/config -d '{"maxmemory_mb":64,"policy":"sil-hepsini","appendonly":false}')" 400
check "ayar kaydı" "$(code POST /redis/config -d '{"maxmemory_mb":128,"policy":"allkeys-lru","appendonly":false}')" 200
check "sınır uygulandı" "$(redis-cli config get maxmemory | tail -1)" 134217728
check "dosyaya yazıldı" "$(grep -c '^maxmemory 128mb\|^maxmemory 134217728' /etc/redis/redis.conf)" 1
check "kısa parola" "$(code POST /redis/config -d '{"maxmemory_mb":128,"policy":"allkeys-lru","appendonly":false,"password":"kisa"}')" 400
check "parola koy" "$(code POST /redis/config -d '{"maxmemory_mb":128,"policy":"allkeys-lru","appendonly":false,"password":"Redis-Parola_1234567"}')" 200
has "parolasız erişim reddedilir" "$(redis-cli ping 2>&1)" "NOAUTH"
api GET /redis | jget '"parolayla panel erişimi:", d["running"], d["config"]["password_set"]'
check "parolayı kaldır" "$(code POST /redis/config -d '{"maxmemory_mb":0,"policy":"noeviction","appendonly":false,"password":""}')" 200
check "parolasız erişim geri geldi" "$(redis-cli ping 2>&1)" "PONG"
check "veritabanını boşalt" "$(code POST /redis/flush -d '{"db":3}')" 200
check "boşaldı" "$(redis-cli -n 3 dbsize)" 0
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/redis)" 401
