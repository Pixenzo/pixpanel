#!/usr/bin/env bash
# HTTP/3 (QUIC) sınaması: sunucu geneli ayar açılınca sertifikalı site UDP 443'ten yanıt verir.
# nginx kurulu makinede çalışır. Kullanım: PX_USER=... PX_PASS=... bash http3.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-h3
JC=/tmp/pxjar-h3-c
DOM=http3-sinama.test
PLAIN=http3-duz.test
C=h3musteri
PASS='Http3-Sinama-Parolasi-1'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
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
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
# h3 <alan adı> <yol>: yalnız HTTP/3 ile istek; "<durum> <sürüm>" yazar (bağlanamazsa "000 0").
h3() { curl -sk --http3-only --max-time 8 -o /dev/null -w '%{http_code} %{http_version}' --resolve "$1:443:127.0.0.1" "https://$1$2" 2>/dev/null || true; }
cleanup() {
  for name in $DOM $PLAIN; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
  id=$(user_id $C); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
}

rm -f $J $JC
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/system/http3)" 401
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
INITIAL=$(api GET /system/http3 | jget 'd["enabled"]')
cleanup
if [ "$(api GET /system/http3 | jget 'd["supported"]')" != True ]; then
  echo "  ATLANDI nginx HTTP/3 desteğiyle derlenmemiş"
  echo "bitti"; exit 0
fi
if ! curl -V | grep -q nghttp3 && ! curl -V | grep -qi 'HTTP3'; then
  echo "  ATLANDI bu makinedeki curl HTTP/3 konuşamıyor"
  echo "bitti"; exit 0
fi

say "Hazırlık"
api PUT /system/http3 -d '{"enabled":false}' >/dev/null
S=$(api POST /sites -d "{\"domain\":\"$DOM\",\"kind\":\"static\"}" | jget 'd["id"]')
P=$(api POST /sites -d "{\"domain\":\"$PLAIN\",\"kind\":\"static\"}" | jget 'd["id"]')
echo "http3 sayfasi" > /home/$PX_USER/sites/$DOM/public/index.html
check "kendinden imzalı sertifika" "$(code POST /sites/$S/ssl/self-signed)" 200
check "site HTTPS ile yayında" "$(curl -sk -o /dev/null -w '%{http_code}' --resolve "$DOM:443:127.0.0.1" "https://$DOM/")" 200
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$C\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$C\",\"password\":\"$PASS\"}"
check "müşteri ayarı göremez" "$(curl -sk -o /dev/null -w '%{http_code}' -b $JC -H 'x-pixpanel-csrf: 1' $B/system/http3)" 403
check "müşteri ayarı değiştiremez" "$(curl -sk -o /dev/null -w '%{http_code}' -b $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X PUT $B/system/http3 -d '{"enabled":true}')" 403

say "Kapalıyken"
check "HTTP/3 ile bağlanılamaz" "$(h3 $DOM / | cut -d' ' -f1)" 000
check "Alt-Svc başlığı yok" "$(curl -skI --resolve "$DOM:443:127.0.0.1" "https://$DOM/" | grep -ci '^alt-svc')" 0

say "Açınca"
check "ayarı aç" "$(api PUT /system/http3 -d '{"enabled":true}' | jget '(d["enabled"], d.get("failed"))')" "(True, [])"
check "nginx yapılandırması geçerli" "$(nginx -t >/dev/null 2>&1 && echo ok)" ok
check "nginx UDP 443 dinliyor" "$(ss -lunp | grep -c ':443 .*nginx')" "$(ss -lunp | grep -c ':443 .*nginx' | awk '{print ($1 > 0) ? $1 : "dinlemiyor"}')"
sleep 1
check "site HTTP/3 ile yanıt veriyor" "$(h3 $DOM /)" "200 3"
has "içerik HTTP/3 ile geliyor" "$(curl -sk --http3-only --max-time 8 --resolve "$DOM:443:127.0.0.1" "https://$DOM/")" "http3 sayfasi"
has "Alt-Svc başlığı duyuruluyor" "$(curl -skI --resolve "$DOM:443:127.0.0.1" "https://$DOM/" | tr -d '\r')" 'alt-svc: h3=":443"'
check "HTTP/2 çalışmaya devam ediyor" "$(curl -sk -o /dev/null -w '%{http_code} %{http_version}' --resolve "$DOM:443:127.0.0.1" "https://$DOM/")" "200 2"
check "tanımsız alan adı HTTP/3 ile de yanıt almaz" "$(h3 tanimsiz-h3.test / | cut -d' ' -f1)" 000
check "sertifikasız site etkilenmedi" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: $PLAIN" http://127.0.0.1/)" 200
check "ayar okunuyor" "$(api GET /system/http3 | jget 'd["enabled"]')" True
check "denetim günlüğüne yazıldı" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='system.http3'")" 1
check "panel yeniden başlayınca ayar yerinde" "$(systemctl restart pixpanel; sleep 3; curl -sk -o /dev/null -c $J -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}"; api GET /system/http3 | jget 'd["enabled"]')" True
check "yeni açılan sertifikalı site de HTTP/3 alır" "$(code POST /sites/$P/ssl/self-signed; sleep 1; echo " $(h3 $PLAIN / | cut -d' ' -f2)")" "200 3"

say "Kapatınca"
check "ayarı kapat" "$(api PUT /system/http3 -d '{"enabled":false}' | jget 'd["enabled"]')" False
sleep 1
check "HTTP/3 ile bağlanılamaz" "$(h3 $DOM / | cut -d' ' -f1)" 000
check "site HTTPS ile yayında" "$(curl -sk -o /dev/null -w '%{http_code}' --resolve "$DOM:443:127.0.0.1" "https://$DOM/")" 200
check "nginx yapılandırması geçerli" "$(nginx -t >/dev/null 2>&1 && echo ok)" ok

say "Temizlik"
cleanup
[ "$INITIAL" = True ] && api PUT /system/http3 -d '{"enabled":true}' >/dev/null
check "siteler silindi" "$(site_id $DOM)$(site_id $PLAIN)" ""
rm -f $J $JC
echo "bitti"
