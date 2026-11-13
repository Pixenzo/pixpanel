#!/usr/bin/env bash
# 13.11.2026 güvenlik incelemesinde bulunan açıkların sömürü denemeleri: her denetim açığı
# gerçekten kullanmaya çalışır ve engellendiğini doğrular. Test makinesinde, root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash security.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
P=https://127.0.0.1:8443
J=/tmp/pxjar-sec
JC=/tmp/pxjar-sec-customer
JM=/tmp/pxjar-sec-member
CUSTOMER=guvenlikmusteri
MEMBER=guvenlikalt
PASS='Guvenlik-Sinama-97531'
PYDOMAIN=guvenlik-py.test
PROXYDOMAIN=guvenlik-vekil.test
VICTIM=guvenlik-kurban.test
DB=guvenlikdb
MARK=/tmp/px-sec-root-marker
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
capi() { local m=$1 p=$2; shift 2; curl -sk -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
ccode() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 300)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$PASS\"}"; }
cleanup() {
  for d in $PYDOMAIN $PROXYDOMAIN $VICTIM; do
    local s; s=$(site_id $d); [ -n "$s" ] && api DELETE "/sites/$s?delete_files=true" >/dev/null
  done
  local z; z=$(api GET /dns | jget "next((x['id'] for x in (d.get('zones', d) if isinstance(d, dict) else d) if x['domain']=='$VICTIM'), '')"); [ -n "$z" ] && api DELETE "/dns/zones/$z" >/dev/null
  local db; db=$(api GET /databases | jget "next((x['id'] for x in d.get('databases', d) if x['name']=='$DB'), '')"); [ -n "$db" ] && api DELETE "/databases/$db" >/dev/null
  local u; u=$(user_id $CUSTOMER); [ -n "$u" ] && api DELETE "/users/$u" >/dev/null
  rm -f $MARK $MARK-sql $MARK-pg
}

rm -f $J $JC $JM
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
check "müşteri girişi" "$(login $JC $CUSTOMER)" 200

say "Kullanıcının yazabildiği PATH ile root olarak kod çalıştırma"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="python")')" = True ]; then
  PS=$(api POST /sites -d "{\"domain\":\"$PYDOMAIN\",\"kind\":\"python\"}" | jget 'd["id"]')
  ROOT=/home/$PX_USER/sites/$PYDOMAIN
  for _ in $(seq 1 60); do [ -x "$ROOT/.venv/bin/pip" ] && break; sleep 1; done
  # Müşterinin kendi klasörüne koyabileceği sahte programlar: root olarak çalışırlarsa işaret bırakırlar.
  for fake in setpriv systemd-run; do
    printf '#!/bin/sh\nid -u > %s\nexec /usr/bin/%s "$@"\n' "$MARK" "$fake" > "$ROOT/.venv/bin/$fake"
    chmod 755 "$ROOT/.venv/bin/$fake"; chown "$PX_USER:" "$ROOT/.venv/bin/$fake"
  done
  echo "" > "$ROOT/requirements.txt"; chown "$PX_USER:" "$ROOT/requirements.txt"
  check "bağımlılık kurulumu çalışır" "$(wait_job "$(api POST /sites/$PS/node/run -d '{"script":"install"}' | jget 'd["job_id"]')")" success
  check "sitenin klasöründeki sahte program root olarak çalışmadı" "$(cat $MARK 2>/dev/null || echo calismadi)" calismadi
  rm -f "$ROOT/.venv/bin/setpriv" "$ROOT/.venv/bin/systemd-run" $MARK
else
  echo "  (Python bileşeni kurulu değil; bu bölüm atlandı)"
fi

say "Web postası yolundan veritabanı araçlarına oturumsuz erişim"
check "/webmail/../tools/ engellenir" "$(curl -sk --path-as-is -o /dev/null -w '%{http_code}' "$P/webmail/../tools/phpmyadmin/index.php")" 404
check "kodlanmış nokta (%2e%2e) engellenir" "$(curl -sk --path-as-is -o /dev/null -w '%{http_code}' "$P/webmail/%2e%2e/tools/phpmyadmin/index.php")" 404
check "kodlanmış bölü (%2f) engellenir" "$(curl -sk --path-as-is -o /dev/null -w '%{http_code}' "$P/webmail/..%2ftools/phpmyadmin/index.php")" 404
check "oturumsuz /tools/ yine kapalı" "$(curl -sk -o /dev/null -w '%{http_code}' "$P/tools/phpmyadmin/index.php")" 401
WM=$(curl -sk -o /dev/null -w '%{http_code}' "$P/webmail/")
check "web postası açılmaya devam eder" "$([ "$WM" = 200 ] || [ "$WM" = 404 ] && echo ok || echo "$WM")" ok
check "web postasına sınırı aşan gövde gönderilemez" "$(curl -sk --http1.1 -o /dev/null -w '%{http_code}' -m 10 -X POST -H 'content-length: 99999999999' --data-binary 'x' "$P/webmail/" 2>/dev/null)" 413
# Boyutu bildirilmeyen gövde akarken kesilir: 70 MB gönderilirken bağlantı yarıda kalır ya da hata döner.
BIG=$(head -c 70000000 /dev/zero | curl -sk -o /dev/null -w '%{http_code}' -m 30 -X POST -H 'transfer-encoding: chunked' --data-binary @- "$P/webmail/" 2>/dev/null)
check "boyutu bildirilmeyen büyük gövde de kesilir" "$([ "$BIG" = 200 ] && echo gecti || echo kesildi)" kesildi

say "Müşterinin vekil sitesiyle iç servislere ulaşma"
proxy() { ccode POST /sites -d "{\"domain\":\"$PROXYDOMAIN\",\"kind\":\"proxy\",\"proxy_target\":\"$1\"}"; }
check "127.0.0.1'e çözülen ad (nip.io) reddedilir" "$(proxy 'http://127.0.0.1.nip.io:8443')" 400
check "localhost alt adı reddedilir" "$(proxy 'http://panel.localhost:8443')" 400
check "IPv6'ya gömülü 127.0.0.1 reddedilir" "$(proxy 'http://[::ffff:127.0.0.1]:11334')" 400
check "iç ağ adresi reddedilir" "$(proxy 'http://10.20.30.40:8080')" 400
check "metaveri adresi reddedilir" "$(proxy 'http://169.254.169.254/')" 400
check "çözülemeyen ad reddedilir" "$(proxy 'http://bu-ad-yok.invalid:8080')" 400
check "yerel uygulama portu kabul edilir" "$(proxy 'http://127.0.0.1:3000')" 200
S=$(site_id $PROXYDOMAIN); [ -n "$S" ] && api DELETE "/sites/$S?delete_files=true" >/dev/null

say "Başka hesabın alan adını sahiplenme"
check "yönetici bölge açar" "$(code POST /dns/zones -d "{\"domain\":\"$VICTIM\",\"provider\":\"local\"}")" 200
check "müşteri o alan adının kendisine site açamaz" "$(ccode POST /sites -d "{\"domain\":\"$VICTIM\",\"kind\":\"static\"}")" 400
check "müşteri alt adına da açamaz" "$(ccode POST /sites -d "{\"domain\":\"www.$VICTIM\",\"kind\":\"static\"}")" 400
check "sahibi açabilir" "$(code POST /sites -d "{\"domain\":\"$VICTIM\",\"kind\":\"static\"}")" 200

say "Taşıma aracıyla iç ağ taraması"
CS=$(capi POST /sites -d "{\"domain\":\"$PROXYDOMAIN\",\"kind\":\"static\"}" | jget 'd["id"]')
probe() { capi POST "/sites/$CS/migrate/probe" -d "{\"host\":\"$1\",\"username\":\"root\",\"password\":\"x\",\"path\":\"/var/www\"}" | jget 'd["error"]["message"]'; }
has "özel ağ adresi (192.168) reddedilir" "$(probe 192.168.1.1)" "Bu adrese bağlanılamaz"
has "özel ağ adresi (10/8) reddedilir" "$(probe 10.0.0.1)" "Bu adrese bağlanılamaz"
has "yerel adres reddedilir" "$(probe 127.0.0.1)" "Bu adrese bağlanılamaz"
api DELETE "/sites/$CS?delete_files=true" >/dev/null

say "SQL dökümüyle sunucuda komut çalıştırma"
DID=$(api POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DB\"}" | jget 'd.get("id") or d["database"]["id"]')
W=$(mktemp -d)
printf 'CREATE TABLE t (a int);\n\\! touch %s\nsystem touch %s\nINSERT INTO t VALUES (1);\n' "$MARK-sql" "$MARK-sql" > $W/kotu.sql
curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -F "file=@$W/kotu.sql" "$B/databases/$DID/import" | jget 'd["job_id"]' > $W/job
wait_job "$(cat $W/job)" >/dev/null
check "MariaDB dökümündeki kabuk komutu çalışmadı" "$([ -e $MARK-sql ] && echo calisti || echo calismadi)" calismadi
has "görev günlüğü engellendiğini söyler" "$(api GET "/jobs/$(cat $W/job)" | jget 'd["log"]')" "sandbox"
api DELETE "/databases/$DID" >/dev/null
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="postgresql")')" = True ]; then
  PID=$(api POST /databases -d "{\"engine\":\"postgresql\",\"name\":\"$DB\"}" | jget 'd.get("id") or d["database"]["id"]')
  printf 'CREATE TABLE t (a int);\n\\! touch %s\n\\copy (select 1) to program '"'"'touch %s'"'"'\nINSERT INTO t VALUES (1);\n' "$MARK-pg" "$MARK-pg" > $W/kotu-pg.sql
  JOB=$(curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -F "file=@$W/kotu-pg.sql" "$B/databases/$PID/import" | jget 'd["job_id"]')
  wait_job "$JOB" >/dev/null
  check "PostgreSQL dökümündeki kabuk komutu çalışmadı" "$([ -e $MARK-pg ] && echo calisti || echo calismadi)" calismadi
  check "dökümün geri kalanı yüklendi" "$(su postgres -c "psql -tA -d $DB -c 'select count(*) from t'" 2>/dev/null)" 1
  api DELETE "/databases/$PID" >/dev/null
fi
rm -rf $W

say "Giriş günlüğüne satır sokup başka adresi engelletme"
FAKE=203.0.113.77
curl -sk -o /dev/null -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login \
  -d "{\"username\":\"x\\nWARN pixpanel::api::auth: sahte ip=$FAKE username=y\",\"password\":\"z\"}"
sleep 1
check "kullanıcı adındaki satır sonu günlüğe sahte satır üretmedi" "$(journalctl -u pixpanel --since '-1min' -o cat | grep -c "ip=$FAKE")" 0

say "API anahtarı (salt okunur dahil) yöneticinin oturum gerektiren işlerini yapamaz"
TOKEN=$(api POST /auth/tokens -d '{"name":"guvenlik-sinama","read_only":true}' | jget 'd["token"]')
TOKEN_ID=$(api GET /auth/tokens | jget "next(t['id'] for t in d if t['name']=='guvenlik-sinama')")
ws() { curl -sk --http1.1 -o /dev/null -w '%{http_code}' -m 5 -H "authorization: Bearer $1" -H 'connection: Upgrade' -H 'upgrade: websocket' -H 'sec-websocket-version: 13' -H 'sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==' "$B/terminal/ws?scope=root"; }
check "salt okunur anahtarla root kabuğu açılmaz" "$(ws "$TOKEN")" 403
RW=$(api POST /auth/tokens -d '{"name":"guvenlik-sinama-yazar"}' | jget 'd["token"]')
RW_ID=$(api GET /auth/tokens | jget "next(t['id'] for t in d if t['name']=='guvenlik-sinama-yazar')")
check "yazma yetkili anahtarla da root kabuğu açılmaz" "$(ws "$RW")" 403
check "anahtarla yönetici hesabı açılamaz" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $RW" -H 'content-type: application/json' -X POST $B/users -d "{\"username\":\"anahtaryonetici\",\"password\":\"$PASS\",\"role\":\"admin\"}")" 403
check "anahtarla müşterinin parolası değiştirilemez" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $RW" -H 'content-type: application/json' -X PATCH $B/users/$(user_id $CUSTOMER) -d "{\"password\":\"Baska-Parola-12345\"}")" 403
api DELETE "/auth/tokens/$TOKEN_ID" >/dev/null; api DELETE "/auth/tokens/$RW_ID" >/dev/null
check "oturumdan kendi parolası bu uçla değiştirilemez (eski parola sorulmaz)" "$(ccode PATCH /users/$(capi GET /auth/me | jget 'd.get("id") or d["user"]["id"]') -d '{"password":"Baska-Parola-12345"}')" 403

say "Parolası sıfırlanan hesabın anahtarları iptal edilir"
CT=$(capi POST /auth/tokens -d '{"name":"musteri-anahtari"}' | jget 'd["token"]')
check "müşterinin anahtarı çalışıyor" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $CT" $B/sites)" 200
check "yönetici müşterinin parolasını sıfırlar" "$(code PATCH /users/$(user_id $CUSTOMER) -d "{\"password\":\"$PASS-2\"}")" 200
check "eski anahtar artık geçersiz" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $CT" $B/sites)" 401
code PATCH /users/$(user_id $CUSTOMER) -d "{\"password\":\"$PASS\"}" >/dev/null
login $JC $CUSTOMER >/dev/null

say "Sahibi askıya alınan alt hesap"
check "alt hesap açılır" "$(ccode POST /users -d "{\"username\":\"$MEMBER\",\"password\":\"$PASS\",\"role\":\"member\",\"permissions\":[\"files\"]}")" 200
check "alt hesap girer" "$(login $JM $MEMBER)" 200
MT=$(curl -sk -b $JM -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/tokens -d '{"name":"alt-anahtar"}' | jget 'd["token"]')
check "müşteri askıya alınır" "$(code PATCH /users/$(user_id $CUSTOMER) -d '{"status":"suspended"}')" 200
check "alt hesap giriş yapamaz" "$(login $JM $MEMBER)" 400
check "alt hesabın anahtarı çalışmaz" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $MT" $B/auth/me)" 401
check "müşteri yeniden etkinleşir" "$(code PATCH /users/$(user_id $CUSTOMER) -d '{"status":"active"}')" 200
check "alt hesap yeniden girer" "$(login $JM $MEMBER)" 200
login $JC $CUSTOMER >/dev/null

say "Deneme bildirimiyle posta yağdırma"
for _ in $(seq 1 8); do ccode POST /notifications/test -d '{"channel":"email"}' >/dev/null; done
check "dokuzuncu deneme sınırlanır" "$(ccode POST /notifications/test -d '{"channel":"email"}')" 429

say "Temizlik"
cleanup
rm -f $J $JC $JM
echo "  OK   bitti"
