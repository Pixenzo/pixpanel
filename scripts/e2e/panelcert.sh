#!/usr/bin/env bash
# Panelin kendi sertifikasının sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash panelcert.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-cert
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 240)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 120); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
# Verilen porttaki servisin sunduğu sertifikanın seri numarası ve vereni.
serial() { echo | timeout 8 openssl s_client -connect "127.0.0.1:$1" -servername panel.test ${2:+-starttls $2} 2>/dev/null | openssl x509 -noout -serial -issuer 2>/dev/null | tr '\n' ' '; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
grep -q "panel.test" /etc/hosts || echo "127.0.0.1 panel.test" >> /etc/hosts

say "Başlangıç"
api POST /system/certificate/selfsigned -d '{"hostname":""}' >/dev/null
api GET /system/certificate | jget '(d["mode"], d["certificate"]["self_signed"], d["nginx_installed"])'
PID=$(systemctl show -p MainPID --value pixpanel)
BEFORE=$(serial 8443)

say "Let's Encrypt (Pebble)"
check "geçersiz alan adı" "$(code POST /system/certificate/letsencrypt -d '{"hostname":"../x","dns":false}')" 400
check "bölgesi olmayan adda DNS doğrulaması" "$(code POST /system/certificate/letsencrypt -d '{"hostname":"panel.test","dns":true}')" 400
JOB=$(api POST /system/certificate/letsencrypt -d '{"hostname":"panel.test","dns":false}' | jget 'd["job_id"]')
check "sertifika alındı" "$(wait_job "$JOB")" success
AFTER=$(serial 8443)
has "panel yeni sertifikayı sunuyor" "$AFTER" "Pebble"
check "sertifika değişti" "$([ "$BEFORE" != "$AFTER" ] && echo evet)" evet
check "panel yeniden başlamadı" "$(systemctl show -p MainPID --value pixpanel)" "$PID"
check "oturum sürüyor" "$(code GET /auth/me)" 200
api GET /system/certificate | jget '(d["mode"], d["hostname"], d["certificate"]["names"])'
SER=$(awk '{print $1}' <<<"$AFTER")
has "IMAP aynı sertifikayı sunuyor" "$(serial 993)" "$SER"
has "SMTP (587) aynı sertifikayı sunuyor" "$(serial 587 smtp)" "$SER"
has "FTP aynı sertifikayı sunuyor" "$(serial 21 ftp)" "$SER"

say "Yüklenen sertifika"
openssl req -x509 -newkey rsa:2048 -nodes -keyout /tmp/px-a.key -out /tmp/px-a.crt -days 5 -subj "/CN=panel.test" -addext "subjectAltName=DNS:panel.test" 2>/dev/null
openssl req -x509 -newkey rsa:2048 -nodes -keyout /tmp/px-b.key -out /tmp/px-b.crt -days 5 -subj "/CN=baska.test" -addext "subjectAltName=DNS:baska.test" 2>/dev/null
body() { python3 -c 'import json,sys; print(json.dumps({"hostname":"panel.test","certificate":open(sys.argv[1]).read(),"private_key":open(sys.argv[2]).read()}))' "$1" "$2"; }
check "anahtarı uyuşmayan sertifika" "$(code POST /system/certificate/custom -d "$(body /tmp/px-a.crt /tmp/px-b.key)")" 400
check "alan adını kapsamayan sertifika" "$(code POST /system/certificate/custom -d "$(body /tmp/px-b.crt /tmp/px-b.key)")" 400
check "reddedilenler sertifikayı değiştirmedi" "$(serial 8443)" "$AFTER"
check "geçerli sertifika" "$(code POST /system/certificate/custom -d "$(body /tmp/px-a.crt /tmp/px-a.key)")" 200
has "panel yüklenen sertifikayı sunuyor" "$(serial 8443)" "CN=panel.test"
check "kip" "$(api GET /system/certificate | jget 'd["mode"]')" custom
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/system/certificate)" 401

say "Son durum"
# Sonraki denemeler için Let's Encrypt (Pebble) sertifikası bırakılır.
check "yeniden al" "$(wait_job "$(api POST /system/certificate/letsencrypt -d '{"hostname":"panel.test","dns":false}' | jget 'd["job_id"]')")" success
rm -f /tmp/px-a.* /tmp/px-b.*
echo "bitti"
