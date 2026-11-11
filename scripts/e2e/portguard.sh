#!/usr/bin/env bash
# Uygulama portlarının korunması: bir hesap, başka hesabın Node.js/Python uygulamasının portunu
# açamaz (uygulama durmuşken bile). Koruma çekirdek desteği ister; yetkisiz konteynerde çalışmaz.
# Betik iki durumu da sınar: destek varsa kısıtın işlediğini, yoksa panelin bunu bildirdiğini.
# Panelin kurulu olduğu makinede çalışır. Kullanım: PX_USER=... PX_PASS=... bash portguard.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-pg
JA=/tmp/pxjar-pg-a
A=portbir
O=portiki
PASS='Port-Sinama-Parolasi-1'
DOMAIN=port-uygulama.test
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 200)]" ;; esac; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
# bind_as <kullanıcı> <port>: kullanıcının oturum diliminde (SSH ile girmiş gibi) portu açmayı dener.
bind_as() {
  systemd-run --wait --quiet --collect --pipe --slice=user.slice -p User="$1" python3 -c "
import socket, sys
s = socket.socket()
try:
    s.bind(('127.0.0.1', int(sys.argv[1]))); print('acildi')
except OSError as e:
    print('reddedildi' if e.errno == 1 else 'hata: %s' % e)
" "$2" 2>&1 | tail -1
}

rm -f $J $JA
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
id=$(site_id $DOMAIN); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null
for name in $A $O; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done

say "Hazırlık"
check "iki hesap" "$(code POST /users -d "{\"username\":\"$A\",\"password\":\"$PASS\",\"role\":\"user\"}") $(code POST /users -d "{\"username\":\"$O\",\"password\":\"$PASS\",\"role\":\"user\"}")" "200 200"
curl -sk -o /dev/null -c $JA -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$A\",\"password\":\"$PASS\"}"
NODE=$(api GET /sites/options | jget 'd["node_versions"][0]')
S=$(call $JA POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"node\",\"node_version\":\"$NODE\"}" | jget 'd["id"]')
PORT=$((20000 + S))
# İkinci hesabın Linux kullanıcısı ilk sitesiyle oluşur.
call $J POST /sites -d "{\"domain\":\"port-komsu.test\",\"kind\":\"static\",\"user_id\":$(user_id $O)}" >/dev/null
for _ in $(seq 1 20); do curl -s -H "Host: $DOMAIN" http://127.0.0.1/ | grep -q Node && break; sleep 0.5; done
has "uygulama kendi portunu açar ve yayındadır" "$(curl -s -H "Host: $DOMAIN" http://127.0.0.1/)" "Node.js"
has "birim dosyasında yalnız kendi portu serbest" "$(grep SocketBind /etc/systemd/system/pixpanel-node-$S.service | tr '\n' ' ')" "SocketBindAllow=tcp:$PORT SocketBindDeny=tcp:20000-29999"

SUPPORTED=$(journalctl -u pixpanel --no-pager 2>/dev/null | grep -c "uygulama portları korunuyor")
if [ "$SUPPORTED" -gt 0 ]; then
  say "Kısıt işliyor"
  check "ortak birimlere kural yazıldı" "$(systemctl show -p SocketBindDeny --value user.slice | grep -c 20000) $(systemctl show -p SocketBindDeny --value cron.service | grep -c 20000) $(systemctl show -p SocketBindDeny --value pixpanel.service | grep -c 20000)" "1 1 1"
  check "PHP havuz servisine kural yazıldı" "$(for u in $(systemctl list-unit-files --no-legend 'php*-fpm.service' | awk '{print $1}'); do systemctl show -p SocketBindDeny --value $u | grep -c 20000; done | sort -u)" 1
  check "uygulama çalışırken komşu portu açamaz" "$(bind_as $O $PORT)" reddedildi
  call $JA POST /sites/$S/node/stop >/dev/null
  check "uygulama durmuşken de komşu portu açamaz" "$(bind_as $O $PORT)" reddedildi
  check "hesabın kendisi de oturumundan açamaz (yalnız uygulama birimi açar)" "$(bind_as $A $PORT)" reddedildi
  check "komşu başka sitelere ayrılmış portları da açamaz" "$(bind_as $O 20001) $(bind_as $O 29999)" "reddedildi reddedildi"
  check "aralık dışındaki portlar serbest" "$(bind_as $O 30123)" acildi
  check "uygulama yeniden başlayınca portunu geri alır" "$(call $JA POST /sites/$S/node/start | jget 'd["state"]') $(for _ in $(seq 1 20); do curl -s -H "Host: $DOMAIN" http://127.0.0.1/ | grep -q Node && { echo yayında; break; }; sleep 0.5; done)" "active yayında"
  # Uygulamanın kendisi başka sitenin portunu açmaya çalışırsa:
  cat > /home/$A/sites/$DOMAIN/server.js <<XEOF
const http = require("node:http");
const other = http.createServer(() => {});
other.on("error", (e) => main(e.code));
other.listen($((PORT + 1)), "127.0.0.1", () => main("ACILDI"));
function main(result) { http.createServer((req, res) => res.end("komsu-port:" + result)).listen(process.env.PORT, process.env.HOST); }
XEOF
  chown $A:www-data /home/$A/sites/$DOMAIN/server.js
  call $JA POST /sites/$S/node/restart >/dev/null
  OUT=; for _ in $(seq 1 20); do OUT=$(curl -s -H "Host: $DOMAIN" http://127.0.0.1/); case "$OUT" in komsu-port:*) break ;; esac; sleep 0.5; done
  check "uygulama başka sitenin portunu açamaz" "$OUT" "komsu-port:EPERM"
  check "sağlık merkezinde uyarı yok" "$(api POST /health/check | jget "len([f for f in d['report']['findings'] if f['key'].startswith('security.app_ports')])")" 0
else
  say "Kısıt bu ortamda desteklenmiyor"
  has "panel durumu günlüğe yazdı" "$(journalctl -u pixpanel --no-pager | grep 'port bağlama kısıtı' | tail -1)" "desteklenmiyor"
  check "sağlık merkezi yöneticiyi uyarır" "$(api POST /health/check | jget "[f['severity'] for f in d['report']['findings'] if f['key'].startswith('security.app_ports')]")" "['info']"
  check "uygulama yine de çalışır" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: $DOMAIN" http://127.0.0.1/)" 200
fi

say "Temizlik"
for name in $DOMAIN port-komsu.test; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
check "hesapları sil" "$(code DELETE /users/$(user_id $A)) $(code DELETE /users/$(user_id $O))" "200 200"
rm -f $J $JA
echo "bitti"
