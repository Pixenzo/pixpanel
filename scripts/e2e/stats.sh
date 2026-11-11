#!/usr/bin/env bash
# Site istatistiklerinin sınaması: bilinen satırlar günlüğe yazılır, özetin aynı sayıları
# verdiği, satırların iki kez sayılmadığı ve günlük döndürmenin sayımı bozmadığı denetlenir.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash stats.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-stats
JC=/tmp/pxjar-stats-customer
DOMAIN=istatistik-e2e.test
LOG=/var/log/nginx/$DOMAIN.access.log
CUSTOMER=istmusteri
PASS='Istatistik-Parola-13579'
BROWSER='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36'
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
# hit <ip> <istek> <durum> <bayt> [tarayıcı]: günlüğe nginx'in yazacağı biçimde bir satır ekler.
hit() { printf '%s - - [%s] "%s" %s %s "-" "%s"\n' "$1" "$(LC_ALL=C date '+%d/%b/%Y:%H:%M:%S %z')" "$2" "$3" "$4" "${5:-$BROWSER}" >> "$LOG"; }
# totals <alanlar>: dönemdeki günlerin toplamı.
totals() { api GET "/sites/$S/stats?days=7" | jget "tuple(sum(x[k] for x in d['days']) for k in $1)"; }

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
OLD=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$DOMAIN'), '')")
[ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
OLDU=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$CUSTOMER'), '')")
[ -n "$OLDU" ] && api DELETE /users/$OLDU >/dev/null
rm -f $LOG $LOG.1

say "Hazırlık"
S=$(api POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"static\"}" | jget 'd["id"]')
# Sınama boyunca panelin dakikalık yoklamaları da bu günlüğe düşer; onlar sayılmamalıdır.
check "başlangıçta istatistik boş" "$(api GET "/sites/$S/stats" | jget '(d["days"], d["top_paths"], d["month_bytes"])')" "([], [], 0)"
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' "$B/sites/$S/stats")" 401

say "Sayım"
hit 203.0.113.7 'GET / HTTP/1.1' 200 1000
hit 203.0.113.7 'GET /hakkinda?kaynak=reklam HTTP/1.1' 200 2000
hit 203.0.113.7 'GET /stil.css HTTP/1.1' 200 300
hit 203.0.113.8 'GET / HTTP/1.1' 200 1000
hit 203.0.113.8 'POST /iletisim HTTP/1.1' 302 50
hit 203.0.113.9 'GET /yok HTTP/1.1' 404 150
hit 203.0.113.9 'GET /hata HTTP/1.1' 502 10
hit 66.249.66.1 'GET / HTTP/1.1' 200 1000 'Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)'
hit 127.0.0.1 'GET / HTTP/1.1' 200 75 'PixPanel-Health/1'
echo "bozuk bir satır" >> $LOG
check "istek, sayfa, ziyaretçi, bot" "$(totals "('requests','pages','visitors','bots')")" "(8, 3, 3, 1)"
check "aktarılan veri (durağan dosyalar dahil)" "$(totals "('bytes',)")" "(5510,)"
check "durum kodları" "$(totals "('s2xx','s3xx','s4xx','s5xx')")" "(5, 1, 1, 1)"
check "en çok görüntülenen sayfalar (sorgu dizgesi atılır, dosyalar sayılmaz)" "$(api GET "/sites/$S/stats" | jget '[(p["path"], p["hits"]) for p in d["top_paths"]]')" "[('/', 2), ('/hakkinda', 1)]"
check "aylık trafik" "$(api GET "/sites/$S/stats" | jget 'd["month_bytes"]')" 5510
check "yeniden sorgulamak satırları iki kez saymaz" "$(totals "('requests','pages','visitors')")" "(8, 3, 3)"

say "Artımlı okuma"
hit 203.0.113.7 'GET /urunler HTTP/1.1' 200 400
hit 203.0.113.50 'GET / HTTP/1.1' 200 1000
check "yalnızca yeni satırlar eklenir; aynı ziyaretçi yeniden sayılmaz" "$(totals "('requests','pages','visitors')")" "(10, 5, 4)"
# Yarım yazılmış satır bir sonraki okumaya bırakılır.
printf '203.0.113.60 - - [%s] "GET /yarim HTTP/1.1" 200 ' "$(LC_ALL=C date '+%d/%b/%Y:%H:%M:%S %z')" >> $LOG
check "yarım satır sayılmaz" "$(totals "('requests',)")" "(10,)"
printf '700 "-" "%s"\n' "$BROWSER" >> $LOG
check "satır tamamlanınca sayılır" "$(totals "('requests','bytes')")" "(11, 7610)"

say "Günlük döndürme"
hit 203.0.113.70 'GET /dondurmeden-once HTTP/1.1' 200 100
mv $LOG $LOG.1
: > $LOG; chown www-data:adm $LOG 2>/dev/null; nginx -s reopen
hit 203.0.113.71 'GET /dondurmeden-sonra HTTP/1.1' 200 200
check "eski dosyanın kalanı ve yeni dosya birlikte sayılır" "$(totals "('requests','pages','visitors','bytes')")" "(13, 8, 7, 7910)"
check "döndürmeden sonra da çift sayım yok" "$(totals "('requests',)")" "(13,)"
hit 203.0.113.72 'GET / HTTP/1.1' 200 90
check "yeni dosyadan okuma sürer" "$(totals "('requests','bytes')")" "(14, 8000)"

say "Gerçek istekler ve yetki"
curl -s -o /dev/null -A "$BROWSER" -H "Host: $DOMAIN" http://127.0.0.1/
curl -s -o /dev/null -A "$BROWSER" -H "Host: $DOMAIN" http://127.0.0.1/olmayan-sayfa
check "nginx'in yazdığı satırlar da sayılır" "$(totals "('requests','s4xx')")" "(16, 2)"
api POST /health/check >/dev/null
check "panelin kendi yoklaması ziyaret sayılmaz" "$(totals "('requests',)")" "(16,)"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
curl -sk -o /dev/null -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\"}"
check "başkasının sitesinin istatistiği görülemez" "$(status $JC GET "/sites/$S/stats")" 404
check "ziyaretçi adresleri veritabanında saklanmaz" "$(python3 -c 'import sqlite3; c=sqlite3.connect("/var/lib/pixpanel/pixpanel.db"); print(sum(1 for (sql,) in c.execute("SELECT sql FROM sqlite_master WHERE name LIKE \"site_stats%\"") if "ip" in sql.lower().replace("skip","")))')" 0

say "Temizlik"
check "site sil" "$(code DELETE "/sites/$S?delete_files=true")" 200
check "silinen sitenin istatistikleri de silinir" "$(python3 -c 'import sqlite3,sys; c=sqlite3.connect("/var/lib/pixpanel/pixpanel.db"); print(sum(c.execute("SELECT COUNT(*) FROM "+t+" WHERE site_id=?", (int(sys.argv[1]),)).fetchone()[0] for t in ("site_stats_daily","site_stats_visitors","site_stats_paths","site_log_state")))' "$S")" 0
U=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
api DELETE /users/$U >/dev/null
rm -f $LOG $LOG.1
echo "bitti"
