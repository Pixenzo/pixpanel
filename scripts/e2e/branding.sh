#!/usr/bin/env bash
# Marka ayarlarının sınaması: ad, logo ve giriş sayfası metni; okuma herkese açık, yazma yöneticiye.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash branding.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-brand
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
PNG='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=='
body() { python3 -c 'import json,sys; print(json.dumps({"name":sys.argv[1],"logo":sys.argv[2],"login_message":sys.argv[3]}))' "$@"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
BEFORE=$(api GET /branding)

say "Okuma ve yazma"
check "oturumsuz okunur (giriş sayfası için)" "$(curl -sk -o /dev/null -w '%{http_code}' $B/branding)" 200
check "oturumsuz yazılamaz" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -X PUT $B/branding -d "$(body X '' '')")" 401
check "kaydet" "$(api PUT /branding -d "$(body ' Örnek Hosting ' "$PNG" 'Destek: destek@ornek.com')" | jget '(d["name"], d["logo"][:22], d["login_message"])')" "('Örnek Hosting', 'data:image/png;base64,', 'Destek: destek@ornek.com')"
check "oturumsuz okuyan aynı markayı görür" "$(curl -sk $B/branding | jget 'd["name"]')" "Örnek Hosting"
check "denetim günlüğüne yazıldı" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='branding.update'")" 1

say "Geçersiz değerler"
check "uzun ad" "$(code PUT /branding -d "$(body "$(printf 'a%.0s' $(seq 1 41))" '' '')")" 400
check "uzun metin" "$(code PUT /branding -d "$(body Ad '' "$(printf 'a%.0s' $(seq 1 301))")")" 400
check "görsel olmayan veri adresi" "$(code PUT /branding -d "$(body Ad 'data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==' '')")" 400
check "dış adres logo olamaz" "$(code PUT /branding -d "$(body Ad 'https://ornek.com/logo.png' '')")" 400
check "bozuk base64" "$(code PUT /branding -d "$(body Ad 'data:image/png;base64,***' '')")" 400
check "çok büyük logo" "$(python3 -c 'import json; print(json.dumps({"name":"Ad","logo":"data:image/png;base64,"+"A"*210000,"login_message":""}))' > /tmp/brand-big.json; code PUT /branding -d @/tmp/brand-big.json; rm -f /tmp/brand-big.json)" 400
check "reddedilen istek öncekini bozmaz" "$(api GET /branding | jget 'd["name"]')" "Örnek Hosting"

say "Yetki"
ACCOUNT=markamusteri
OLD=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$ACCOUNT'), '')"); [ -n "$OLD" ] && api DELETE "/users/$OLD" >/dev/null
api POST /users -d "{\"username\":\"$ACCOUNT\",\"password\":\"Marka-Sinama-Parola-1\",\"role\":\"reseller\"}" >/dev/null
curl -sk -o /dev/null -c /tmp/pxjar-brand-c -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$ACCOUNT\",\"password\":\"Marka-Sinama-Parola-1\"}"
check "yönetici olmayan değiştiremez" "$(curl -sk -o /dev/null -w '%{http_code}' -b /tmp/pxjar-brand-c -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X PUT $B/branding -d "$(body Baska '' '')")" 403
api DELETE "/users/$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$ACCOUNT')")" >/dev/null

say "Temizlik"
check "önceki markaya dön" "$(code PUT /branding -d "$BEFORE")" 200
check "boş marka panelin kendi adına döner" "$(api PUT /branding -d "$(body '' '' '')" >/dev/null; api GET /branding | jget '(d["name"], d["logo"], d["login_message"])'; api PUT /branding -d "$BEFORE" >/dev/null)" "('', '', '')"
rm -f $J /tmp/pxjar-brand-c
echo "bitti"
