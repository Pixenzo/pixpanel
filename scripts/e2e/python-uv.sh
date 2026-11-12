#!/usr/bin/env bash
# uv ile yönetilen Python sürümlerinin sınaması: sürüm kurma/kaldırma, site başına sürüm seçimi,
# sanal ortamın yeni yorumlayıcıyla yeniden kurulması, pyproject.toml ile bağımlılık kurulumu,
# bozulmuş uv ikilisinin yeniden indirilmesi.
# Test makinesinde root olarak çalışır; internet ister (GitHub: uv ve python-build-standalone, PyPI).
# Kullanım: PX_USER=... PX_PASS=... bash python-uv.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-pyuv
JC=/tmp/pxjar-pyuv-customer
DOMAIN=python-uv.test
CUSTOMER=pyuvmusteri
PASS='Pyuv-Parola-13579'
UV=/usr/local/lib/pixpanel/uv
UV_SHA=f56ca7e29e708e20ed6b96f9a8cb89ac45129239c6166481cc3ae3cbab286a0f
UV_VERSION=0.13.6
BODY=/tmp/px-python-uv.json
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
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
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
wait_job() { for _ in $(seq 1 600); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
job_tail() { api GET "/jobs/$1" | jget 'd["log"][-900:]' | sed 's/^/        /'; }
# Bileşeni kurar ve sonucu denetler; başarısızsa günlüğün sonunu gösterir.
install_python() {
  local job result
  job=$(api POST "/components/python-$1/install" | jget 'd["job_id"]')
  result=$(wait_job "$job")
  check "Python $1 kurulumu" "$result" success
  [ "$result" = success ] || job_tail "$job"
}
site() { curl -s -H "Host: $DOMAIN" "http://127.0.0.1$1"; }
# Uygulama yeniden başlarken kısa süre 502 dönebilir; beklenen yanıt gelene kadar denenir.
site_until() { local want=$1 path=$2 got=; for _ in $(seq 1 30); do got=$(site "$path"); case "$got" in *"$want"*) break ;; esac; sleep 0.5; done; echo "$got"; }
# Sitenin ayarlarını okuyup verilen Python parçasıyla (d: düğüm ayarları sözlüğü) değiştirir ve PATCH eder; HTTP kodunu yazar.
patch_node() {
  api GET /sites/$S | python3 -c "
import sys,json
d=json.load(sys.stdin)['node']
$1
print(json.dumps({'node':d}))" > $BODY
  code PATCH /sites/$S -d @$BODY
}
# Komutu sitenin Linux kullanıcısıyla çalıştırır.
as_user() { setpriv --reuid="$(id -u $PX_USER)" --regid="$(id -g $PX_USER)" --init-groups "$@"; }
ROOT=/home/$PX_USER/sites/$DOMAIN
S=

cleanup() {
  local id
  for old in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['domain']=='$DOMAIN')"); do api DELETE "/sites/$old?delete_files=true" >/dev/null; done
  id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$CUSTOMER'), '')")
  [ -n "$id" ] && api DELETE /users/$id >/dev/null
  # Önceki koşudan kalan uv Python sürümleri (kullanan site kalmadığı için kaldırılabilir).
  for installed in $(api GET /components | jget "' '.join(c['id'] for c in d if c['id'].startswith('python-') and c['installed'])"); do api DELETE /components/$installed >/dev/null; done
}

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Ön koşullar"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="python")')" != True ]; then
  check "Python bileşeni kurulumu" "$(wait_job "$(api POST /components/python/install | jget 'd["job_id"]')")" success
fi
SYS=$(api GET /sites/options | jget 'd["python_versions"][0]')
has "sistem Python'u görünüyor" "$SYS" "3."
# Sistemin Python'undan farklı bir sürüm seçilir (sınama, yorumlayıcının gerçekten değiştiğini ayırt edebilsin).
VER=3.12; [ "$SYS" = 3.12 ] && VER=3.13
OTHER=3.11; [ "$SYS" = 3.11 ] && OTHER=3.10
echo "  sistem Python: $SYS, sınanan sürüm: $VER, ikinci sürüm: $OTHER"
check "bileşen listesinde uv Python sürümleri" "$(api GET /components | jget "sorted(c['id'] for c in d if c['id'].startswith('python-'))")" "['python-3.10', 'python-3.11', 'python-3.12', 'python-3.13', 'python-3.14']"
check "kurulu uv sürümü yokken seçenekler boş" "$(api GET /sites/options | jget 'len(d["python_runtimes"])')" 0

say "Geçersiz sürüm dizeleri ve yetki"
# Kimliklerdeki özel karakterler yüzde kodlanır (; boşluk /); sunucu yine de beyaz listeyle reddeder.
for bad in '3.12;%20id' '..%2Fx' '2.7' '' '3.99' '3.12.1' '3.12%0Aid'; do
  check "kurulum, geçersiz sürüm [$bad]" "$(code POST "/components/python-$bad/install")" 400
  check "kaldırma, geçersiz sürüm [$bad]" "$(code DELETE "/components/python-$bad")" 400
done
check "oturumsuz kurulum" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'x-pixpanel-csrf: 1' -X POST $B/components/python-$VER/install)" 401
check "müşteri hesabı açılır" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
check "müşteri girişi" "$(login $JC $CUSTOMER "$PASS")" 200
check "yönetici olmayan kuramaz" "$(status $JC POST /components/python-$VER/install)" 403
check "yönetici olmayan kaldıramaz" "$(status $JC DELETE /components/python-$VER)" 403
check "kurulu olmayan sürüm kaldırılamaz" "$(code DELETE /components/python-$VER)" 400

say "Python $VER kurulumu (uv)"
install_python $VER
has "kararlı yoldan çalışıyor" "$(/opt/pixpanel/python/$VER/bin/python3 --version 2>&1)" "Python $VER"
check "ikinci kurulum reddedilir" "$(code POST /components/python-$VER/install)" 409
has "bileşen sürümü tam numarayla görünür" "$(api GET /components | jget "next(c['version'] for c in d if c['id']=='python-$VER')")" "$VER."
check "seçeneklerde kurulu sürüm" "$(api GET /sites/options | jget 'd["python_runtimes"]')" "['$VER']"
check "uv ikilisi gömülü özetle aynı" "$(sha256sum $UV | cut -d' ' -f1)" "$UV_SHA"
has "uv sürümü" "$($UV --version 2>&1)" "$UV_VERSION"
check "kararlı yol sembolik bağ ve uv deposunu gösteriyor" "$(readlink -f /opt/pixpanel/python/$VER | grep -c '^/opt/pixpanel/python-store/cpython-')" 1
check "dosyalar root'a ait" "$(find /opt/pixpanel/python-store ! -user root | wc -l)" 0
check "kimse yazamaz (grup/diğer yazma izni yok)" "$(find /opt/pixpanel/python-store ! -type l -perm /022 | wc -l)" 0
check "herkes çalıştırabilir" "$(setpriv --reuid=65534 --regid=65534 --clear-groups /opt/pixpanel/python/$VER/bin/python3 -c 'print(42)' 2>&1)" 42
check "ayrıcalıksız kullanıcı yazamaz" "$(setpriv --reuid=65534 --regid=65534 --clear-groups touch /opt/pixpanel/python/$VER/izinsiz 2>&1 | grep -c -i 'denied\|read-only')" 1
check "uv kullanıcı bin klasörüne yürütülebilir bırakmadı" "$(ls /root/.local/bin/python$VER 2>/dev/null | wc -l)" 0

say "Python sitesi (sistem Python'u, bugünkü davranış)"
S=$(api POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"python\"}" | jget 'd["id"]')
check "varsayılan: sürüm sistem, python alanı boş" "$(api GET /sites/$S | jget '(d["node"]["version"], d["node"]["python"])')" "('$SYS', '')"
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
        body = f"surum={sys.version_info.major}.{sys.version_info.minor}|{sys.prefix}|{extra}".encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
HTTPServer((os.environ["HOST"], int(os.environ["PORT"])), H).serve_forever()
XEOF
chown $PX_USER:www-data $ROOT/app.py
check "ayar kaydı (yalnız kendi kodu)" "$(patch_node "d['memory_mb']=256")" 200
check "sistem Python'uyla yayında" "$(site_until "surum=$SYS" /)" "surum=$SYS|$ROOT/.venv|paket-yok"
echo "itsdangerous==2.2.0" > $ROOT/requirements.txt; chown $PX_USER:www-data $ROOT/requirements.txt
check "durum ucu bağımlılık dosyasını bildirir" "$(api GET /sites/$S/node | jget 'd["dependencies"]')" requirements.txt
JOB=$(api POST /sites/$S/node/run -d '{"script":"install","restart":true}' | jget 'd["job_id"]')
check "requirements.txt ile kurulum (sistem sürümü)" "$(wait_job "$JOB")" success
check "paket sistem sürümlü ortamda görünüyor" "$(site_until paket-var /)" "surum=$SYS|$ROOT/.venv|paket-var"

say "Site Python $VER'e alınır"
check "kurulu olmayan sürüm reddedilir" "$(patch_node "d['python']='$OTHER'")" 400
check "geçersiz sürüm dizesi [3.12; id]" "$(patch_node "d['python']='3.12; id'")" 400
check "geçersiz sürüm dizesi [../x]" "$(patch_node "d['python']='../x'")" 400
check "geçersiz sürüm dizesi [2.7]" "$(patch_node "d['python']='2.7'")" 400
check "reddedilen değişiklik siteyi bozmadı" "$(site_until paket-var /)" "surum=$SYS|$ROOT/.venv|paket-var"
check "sürüm değişikliği kaydı" "$(patch_node "d['python']='$VER'")" 200
check "ayarlarda sürüm ve python alanı" "$(api GET /sites/$S | jget '(d["node"]["version"], d["node"]["python"], d["node"]["memory_mb"], d["node"]["command"])')" "('$VER', '$VER', 256, 'python app.py')"
has "sanal ortam yeni yorumlayıcıyla" "$($ROOT/.venv/bin/python --version 2>&1)" "Python $VER"
check "yorumlayıcı uv deposundan" "$(readlink -f $ROOT/.venv/bin/python | grep -c '^/opt/pixpanel/python-store/cpython-')" 1
check "sanal ortam sitenin kullanıcısına ait" "$(stat -c %U $ROOT/.venv) $(find $ROOT/.venv ! -user $PX_USER | wc -l)" "$PX_USER 0"
check "yorumlayıcı kaydı" "$(cat /etc/pixpanel/node/$S.python)" "$VER"
check "uygulama yeniden yayında ve yeni sürümü görüyor (eski paketler sıfırlandı)" "$(site_until "surum=$VER" /)" "surum=$VER|$ROOT/.venv|paket-yok"
check "servis sitenin kullanıcısıyla çalışıyor" "$(ps -o user= -p "$(systemctl show -p MainPID --value pixpanel-node-$S)" | tr -d ' ')" "$PX_USER"
check "yalnız yerel adresi dinliyor" "$(ss -ltnH "sport = :$((20000 + S))" | awk '{print $4}')" "127.0.0.1:$((20000 + S))"
check "bellek sınırı korundu" "$(systemctl show -p MemoryMax --value pixpanel-node-$S)" 268435456
has "servis birimi sanal ortamı kullanıyor" "$(cat /etc/systemd/system/pixpanel-node-$S.service)" "Environment=PATH=$ROOT/.venv/bin:"
check "durum ucu çalışıyor" "$(api GET /sites/$S/node | jget '(d["state"], d["port"])')" "('active', $((20000 + S)))"

say "Bağımlılıklar: requirements.txt, pyproject.toml, ikisi de yok"
JOB=$(api POST /sites/$S/node/run -d '{"script":"install","restart":true}' | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "requirements.txt ile kurulum (yeni sürüm)" "$RESULT" success
[ "$RESULT" = success ] || job_tail "$JOB"
check "paket yeni sürümün ortamına, sitenin kullanıcısıyla kuruldu" "$(stat -c %U $ROOT/.venv/lib/python$VER/site-packages/itsdangerous/__init__.py)" "$PX_USER"
check "uygulama paketi görüyor" "$(site_until paket-var /)" "surum=$VER|$ROOT/.venv|paket-var"
as_user $ROOT/.venv/bin/pip uninstall -y --disable-pip-version-check itsdangerous >/dev/null 2>&1
rm -f $ROOT/requirements.txt
cat > $ROOT/pyproject.toml <<'XEOF'
[build-system]
requires = ["setuptools>=61"]
build-backend = "setuptools.build_meta"

[project]
name = "pxdemo"
version = "0.1"
dependencies = ["itsdangerous==2.2.0"]

[tool.setuptools]
py-modules = []
XEOF
chown $PX_USER:www-data $ROOT/pyproject.toml
check "durum ucu pyproject.toml'u bildirir" "$(api GET /sites/$S/node | jget 'd["dependencies"]')" pyproject.toml
JOB=$(api POST /sites/$S/node/run -d '{"script":"install","restart":true}' | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "pyproject.toml ile kurulum" "$RESULT" success
[ "$RESULT" = success ] || job_tail "$JOB"
has "görev günlüğü pip install ." "$(api GET /jobs/$JOB | jget 'd["log"]')" "pip install ."
check "proje ve bağımlılığı kuruldu" "$(ls -d $ROOT/.venv/lib/python$VER/site-packages/pxdemo-0.1.dist-info $ROOT/.venv/lib/python$VER/site-packages/itsdangerous 2>/dev/null | wc -l)" 2
check "uygulama yeniden başladı ve paketi görüyor" "$(site_until paket-var /)" "surum=$VER|$ROOT/.venv|paket-var"
rm -f $ROOT/pyproject.toml
check "bağımlılık dosyası yokken" "$(code POST /sites/$S/node/run -d '{"script":"install"}')" 400
check "durum ucunda bağımlılık dosyası yok" "$(api GET /sites/$S/node | jget 'd["dependencies"]')" None

say "Kullanılan sürüm kaldırılamaz; site sisteme dönünce kaldırılabilir"
check "kullanılan sürüm kaldırılamaz" "$(code DELETE /components/python-$VER)" 409
has "hata iletisi siteyi söyler" "$(api DELETE /components/python-$VER)" "$DOMAIN"
check "reddedilince sürüm yerinde" "$(/opt/pixpanel/python/$VER/bin/python3 -c 'print(1)' 2>&1)" 1
check "siteyi sisteme döndür" "$(patch_node "d['python']='system'")" 200
check "ayarlarda sürüm sistemin sürümü, python alanı boş" "$(api GET /sites/$S | jget '(d["node"]["version"], d["node"]["python"], d["node"]["command"])')" "('$SYS', '', 'python app.py')"
check "sanal ortam sistem yorumlayıcısına döndü" "$(readlink -f $ROOT/.venv/bin/python | grep -c '^/opt/pixpanel/')" 0
has "sanal ortam sistemin sürümünde" "$($ROOT/.venv/bin/python --version 2>&1)" "Python $SYS"
check "uygulama sistem sürümüyle yayında (paketler sıfırlandı)" "$(site_until "surum=$SYS" /)" "surum=$SYS|$ROOT/.venv|paket-yok"
check "yorumlayıcı kaydı sisteme döndü" "$(cat /etc/pixpanel/node/$S.python)" system
check "sürüm artık kaldırılabilir" "$(code DELETE /components/python-$VER)" 200
check "kararlı yol silindi" "$(test -e /opt/pixpanel/python/$VER && echo var || echo yok)" yok
check "uv deposundan da silindi" "$(ls /opt/pixpanel/python-store 2>/dev/null | grep -c "^cpython-$VER")" 0
check "bileşen artık kurulu görünmüyor" "$(api GET /components | jget "next(c['installed'] for c in d if c['id']=='python-$VER')")" False
check "kaldırılan sürüm siteye atanamaz" "$(patch_node "d['python']='$VER'")" 400

say "Bozulmuş uv ikilisi çalıştırılmaz, yeniden indirilir"
rm -f /tmp/px-uv-ran
cp $UV /tmp/px-uv-yedek
printf '#!/bin/sh\ntouch /tmp/px-uv-ran\nexit 0\n' > $UV
chmod 755 $UV
check "ikili bozuldu (özet farklı)" "$([ "$(sha256sum $UV | cut -d' ' -f1)" != "$UV_SHA" ] && echo evet)" evet
install_python $OTHER
check "bozulmuş ikili çalıştırılmadı" "$(test -e /tmp/px-uv-ran && echo calisti || echo calismadi)" calismadi
check "uv yeniden indirildi, özet doğru" "$(sha256sum $UV | cut -d' ' -f1)" "$UV_SHA"
has "ikinci sürüm kuruldu" "$(/opt/pixpanel/python/$OTHER/bin/python3 --version 2>&1)" "Python $OTHER"
check "kullanılmayan sürüm kaldırılır" "$(code DELETE /components/python-$OTHER)" 200
rm -f /tmp/px-uv-ran /tmp/px-uv-yedek

say "Temizlik"
check "siteyi sil" "$(code DELETE "/sites/$S?delete_files=true") $(test -e /etc/pixpanel/node/$S.python && echo var || echo yok)" "200 yok"
cleanup
rm -f $J $JC $BODY
echo "bitti"
