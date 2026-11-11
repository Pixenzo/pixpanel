#!/usr/bin/env bash
# Python uygulamalarının sınaması: sanal ortam, başlatma komutu, bağımlılık kurulumu, servis.
# Test makinesinde çalışır; bağımlılık sınaması PyPI'dan küçük bir paket indirir.
# Kullanım: PX_USER=... PX_PASS=... bash python.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-python
DOMAIN=python-uygulama.test
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
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
site() { curl -s -H "Host: $DOMAIN" "http://127.0.0.1$1"; }
# Uygulama yeniden başlarken kısa süre 502 dönebilir; beklenen yanıt gelene kadar denenir.
site_until() { local want=$1 path=$2 got=; for _ in $(seq 1 20); do got=$(site "$path"); case "$got" in *"$want"*) break ;; esac; sleep 0.5; done; echo "$got"; }
ROOT=/home/$PX_USER/sites/$DOMAIN

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
OLD=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$DOMAIN'), '')")
[ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null

say "Python kurulumu"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="python")')" != True ]; then
  check "Python bileşeni kurulumu" "$(wait_job "$(api POST /components/python/install | jget 'd["job_id"]')")" success
fi
VERSION=$(api GET /sites/options | jget 'd["python_versions"][0]')
has "seçeneklerde sürüm görünüyor" "$VERSION" "3."

say "Python sitesi"
S=$(api POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"python\"}" | jget 'd["id"]')
check "site ayarları" "$(api GET /sites/$S | jget '(d["kind"], d["node"]["version"], d["node"]["command"])')" "('python', '$VERSION', 'python app.py')"
has "örnek uygulama yayında" "$(site_until 'Python' /)" "Python $VERSION"
check "sanal ortam site klasöründe ve hesabın" "$(stat -c '%U' $ROOT/.venv/bin/python) $(readlink -f $ROOT/.venv/bin/python | grep -c "python$VERSION")" "$PX_USER 1"
check "servis sitenin kullanıcısıyla çalışıyor" "$(ps -o user= -p "$(systemctl show -p MainPID --value pixpanel-node-$S)" | tr -d ' ')" "$PX_USER"
check "uygulama sanal ortamın Python'uyla çalışıyor" "$(readlink /proc/$(systemctl show -p MainPID --value pixpanel-node-$S)/cwd)" "$ROOT"
check "yalnız yerel adresi dinliyor" "$(ss -ltnH "sport = :$((20000 + S))" | awk '{print $4}')" "127.0.0.1:$((20000 + S))"
check "durum ucu" "$(api GET /sites/$S/node | jget '(d["state"], d["port"])')" "('active', $((20000 + S)))"

say "Ayarlar ve ortam değişkenleri"
cat > $ROOT/app.py <<'XEOF'
import os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        try:
            import itsdangerous
            extra = "paket-var"
        except ImportError:
            extra = "paket-yok"
        body = f"{os.environ.get('SELAM')}|{sys.prefix}|{extra}|{self.path}".encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
HTTPServer((os.environ["HOST"], int(os.environ["PORT"])), H).serve_forever()
XEOF
chown $PX_USER:www-data $ROOT/app.py
cat > /tmp/px-python.json <<XEOF
{"node":{"version":"$VERSION","command":"python app.py","memory_mb":256,"env":[{"key":"SELAM","value":"a b\"c\$d"}]}}
XEOF
check "ayar kaydı" "$(code PATCH /sites/$S -d @/tmp/px-python.json)" 200
check "ortam değişkeni bozulmadan ulaştı; sanal ortam etkin" "$(site_until '|/yol' /yol)" "a b\"c\$d|$ROOT/.venv|paket-yok|/yol"
check "bellek sınırı uygulandı" "$(systemctl show -p MemoryMax --value pixpanel-node-$S)" 268435456
check "sanal ortam değişkeni elle tanımlanamaz" "$(code PATCH /sites/$S -d "{\"node\":{\"version\":\"$VERSION\",\"command\":\"python app.py\",\"env\":[{\"key\":\"VIRTUAL_ENV\",\"value\":\"/tmp\"}]}}")" 400
check "satır sonlu komut" "$(code PATCH /sites/$S -d "{\"node\":{\"version\":\"$VERSION\",\"command\":\"python app.py\\nUser=root\"}}")" 400
check "kurulu olmayan sürüm" "$(code PATCH /sites/$S -d '{"node":{"version":"2.7","command":"python app.py"}}')" 400

say "Bağımlılıklar"
check "requirements.txt yokken" "$(code POST /sites/$S/node/run -d '{"script":"install"}')" 400
echo "itsdangerous==2.2.0" > $ROOT/requirements.txt; chown $PX_USER:www-data $ROOT/requirements.txt
JOB=$(api POST /sites/$S/node/run -d '{"script":"install","restart":true}' | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "bağımlılık kurulumu" "$RESULT" success
[ "$RESULT" = success ] || api GET "/jobs/$JOB" | jget 'd["log"][-800:]' | sed 's/^/        /'
check "paket sanal ortama, hesabın kullanıcısıyla kuruldu" "$(stat -c %U $ROOT/.venv/lib/python$VERSION/site-packages/itsdangerous/__init__.py)" "$PX_USER"
check "sistemin Python'una karışmadı" "$(python3 -c 'import itsdangerous' 2>&1 | grep -c ModuleNotFoundError)" 1
has "uygulama yeniden başladı ve paketi görüyor" "$(site_until 'paket-var' /)" "paket-var"

say "Servis işlemleri ve günlük"
check "durdur" "$(api POST /sites/$S/node/stop | jget 'd["state"]')" inactive
check "durmuşken 502" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: $DOMAIN" http://127.0.0.1/)" 502
check "başlat" "$(api POST /sites/$S/node/start | jget 'd["state"]')" active
check "uygulama günlüğü kaynaklarda" "$(api GET /logs/sources | jget "len([s for s in d if s['id']=='site:$S:app'])")" 1
check "askıya alınca uygulama durur" "$(code PATCH /sites/$S -d '{"status":"suspended"}') $(systemctl is-active pixpanel-node-$S)" "200 inactive"
check "yeniden yayınlanınca başlar" "$(code PATCH /sites/$S -d '{"status":"active"}') $(systemctl is-active pixpanel-node-$S)" "200 active"

say "Tür değişimi ve temizlik"
check "durağan siteye çevirince servis kalkar" "$(code PATCH /sites/$S -d '{"kind":"static"}') $(test -e /etc/systemd/system/pixpanel-node-$S.service && echo var || echo yok)" "200 yok"
check "yeniden Python'a çevirince çalışır" "$(code PATCH /sites/$S -d '{"kind":"python"}') $(sleep 2; systemctl is-active pixpanel-node-$S)" "200 active"
check "siteyi sil" "$(code DELETE "/sites/$S?delete_files=true") $(test -e /etc/systemd/system/pixpanel-node-$S.service && echo var || echo yok) $(test -e $ROOT && echo var || echo yok)" "200 yok yok"
rm -f $J /tmp/px-python.json
echo "bitti"
