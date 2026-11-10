#!/usr/bin/env bash
# Canlı olay akışının (GET /api/v1/events) sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash events.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-events
JC=/tmp/pxjar-events-customer
CUSTOMER=olaymusteri
CUSTOMER_PASS='Olay-Parola-13579'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
capi() { local m=$1 p=$2; shift 2; curl -sk -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 300)]"; fi; }
lacks() { if grep -qF -- "$3" <<<"$2"; then echo "  HATA $1: [$3] görünmemeliydi"; else echo "  OK   $1"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
# listen <çerez dosyası> <saniye> <çıktı dosyası>: akışı arka planda dinler.
listen() { (timeout "$2" curl -skN -b "$1" "$B/events" > "$3" 2>/dev/null &); }

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Hazırlık"
OLD=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$CUSTOMER'), '')")
[ -n "$OLD" ] && api DELETE "/users/$OLD" >/dev/null
OLD=$(api GET /sites | jget 'next((s["id"] for s in d if s["domain"]=="olay-e2e.test"), "")')
[ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CUSTOMER_PASS\",\"role\":\"user\",\"email\":\"\"}")" 200
check "müşteri girişi" "$(curl -sk -o /dev/null -w '%{http_code}' -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CUSTOMER_PASS\"}")" 200

say "Erişim"
check "oturumsuz akış açılmaz" "$(curl -sk -o /dev/null -w '%{http_code}' -m 5 $B/events)" 401
check "akış olay akışı olarak sunulur" "$(curl -sk -o /dev/null -m 2 -w '%{content_type}' -b $J $B/events)" "text/event-stream"

say "Yönetici akışı"
listen $J 9 /tmp/px-events-admin
listen $JC 9 /tmp/px-events-customer
sleep 1.5
SITE=$(api POST /sites -d '{"domain":"olay-e2e.test","kind":"static"}' | jget 'd["id"]')
CSITE=$(capi POST /sites -d '{"domain":"olay-musteri.test","kind":"static"}' | jget 'd["id"]')
sleep 8
ADMIN=$(cat /tmp/px-events-admin)
CUST=$(cat /tmp/px-events-customer)
has "ilk olay sürümü bildirir" "$(head -1 /tmp/px-events-admin)" '"type":"hello"'
has "sunucu yükü akar" "$ADMIN" '"type":"metrics"'
check "yük örnekleri yaklaşık 2 saniyede bir" "$(grep -c '"type":"metrics"' /tmp/px-events-admin | awk '{print ($1 >= 3 && $1 <= 6) ? "evet" : $1}')" evet
has "site oluşturma bildirildi" "$ADMIN" '{"type":"changed","area":"site"}'
check "yönetici müşterinin değişikliğini de görür" "$(grep -c '"type":"changed","area":"site"' /tmp/px-events-admin)" 2

say "Müşteri akışı"
has "müşteri de karşılanır" "$(head -1 /tmp/px-events-customer)" '"type":"hello"'
lacks "müşteriye sunucu yükü gitmez" "$CUST" '"type":"metrics"'
check "müşteri yalnızca kendi değişikliğini görür" "$(grep -c '"type":"changed"' /tmp/px-events-customer)" 1
lacks "müşteri yöneticinin görevlerini görmez" "$CUST" '"type":"job"'

say "Görevler"
listen $J 8 /tmp/px-events-job
sleep 1
# Kısa süren gerçek bir görev: erişilemeyen bir depodan yayınlama hemen başarısız olur.
DEPLOY=$(api PUT /sites/$SITE/git -d '{"repo_url":"https://127.0.0.1:1/yok.git","branch":"main"}' >/dev/null; api POST /sites/$SITE/git/deploy -d '{}' | jget 'd.get("deployment", "")')
sleep 6
JOBS=$(cat /tmp/px-events-job)
has "görev başlangıcı bildirildi" "$JOBS" '"status":"running"'
has "görev bitişi bildirildi" "$JOBS" '"status":"failed"'
has "bitiş zamanı dolu" "$(grep '"status":"failed"' /tmp/px-events-job | head -1)" '"finished_at":1'

say "Temizlik"
check "site sil" "$(code DELETE "/sites/$SITE?delete_files=true")" 200
capi DELETE "/sites/$CSITE?delete_files=true" >/dev/null
U=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
check "müşteriyi sil" "$(code DELETE /users/$U)" 200
rm -f /tmp/px-events-*
echo "bitti"
