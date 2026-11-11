#!/usr/bin/env bash
# Sunucu yükünün uzun dönem geçmişinin sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash metrics.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-metrics
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
sql() { python3 -c 'import sqlite3,sys; c=sqlite3.connect("/var/lib/pixpanel/pixpanel.db", timeout=20); print(c.execute(sys.argv[1]).fetchone()[0]); c.commit()' "$1"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Erişim ve aralıklar"
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' "$B/system/metrics/archive?range=day")" 401
check "geçersiz aralık" "$(code GET '/system/metrics/archive?range=decade')" 400
check "aralıksız istek" "$(code GET /system/metrics/archive)" 400
for range in day week month year; do
  check "$range: nokta sayısı sınırlı, zaman sıralı" "$(api GET "/system/metrics/archive?range=$range" | jget '(len(d) <= 400, [p["ts"] for p in d] == sorted(p["ts"] for p in d))')" "(True, True)"
done

say "Kayıt"
# Panel dakikada bir kayıt yazar; en çok 70 saniye beklenir.
BEFORE=$(sql "SELECT COUNT(*) FROM metrics_minutes WHERE ts >= strftime('%s','now') - 180")
for _ in $(seq 1 14); do
  [ "$(sql "SELECT COUNT(*) FROM metrics_minutes WHERE ts >= strftime('%s','now') - 180")" -gt "$BEFORE" ] && break
  sleep 5
done
check "dakikalık kayıt yazılıyor" "$(sql "SELECT COUNT(*) > 0 FROM metrics_minutes WHERE ts >= strftime('%s','now') - 180")" 1
check "kayıt makul değerler taşır" "$(sql "SELECT cpu >= 0 AND cpu <= 100 AND mem_total > 0 AND mem_used > 0 AND mem_used <= mem_total FROM metrics_minutes ORDER BY ts DESC LIMIT 1")" 1
check "içinde bulunulan saatin özeti de güncel" "$(sql "SELECT COUNT(*) FROM metrics_hours WHERE ts = strftime('%s','now') / 3600 * 3600")" 1
LAST=$(api GET '/system/metrics/archive?range=day' | jget '(lambda p: (0 <= p["cpu"] <= 100, 0 < p["mem"] <= 100, p["net_rx"] >= 0))(d[-1])')
check "günlük görünümün son noktası" "$LAST" "(True, True, True)"
echo "bitti"
