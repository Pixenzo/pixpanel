#!/usr/bin/env bash
# Node.js uygulamaları ve yol bazlı vekil kurallarının sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash node.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-node
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
wait_job() { for _ in $(seq 1 240); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
site() { curl -s -H "Host: $1" "http://127.0.0.1$2"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Node.js kurulumu"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="node-22")')" != True ]; then
  check "Node.js 22 kurulumu" "$(wait_job "$(api POST /components/node-22/install | jget 'd["job_id"]')")" success
fi
api GET /components | jget '[(c["name"], c["version"]) for c in d if c["category"]=="node" and c["installed"]]'
check "seçeneklerde görünüyor" "$(api GET /sites/options | jget '"22" in d["node_versions"]')" True

say "Node.js sitesi"
OLD=$(api GET /sites | jget 'next((s["id"] for s in d if s["domain"]=="uygulama.test"), "")')
[ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
check "sürümsüz oluşturma" "$(code POST /sites -d '{"domain":"uygulama.test","kind":"node"}')" 400
check "kurulu olmayan sürüm" "$(code POST /sites -d '{"domain":"uygulama.test","kind":"node","node_version":"7"}')" 400
check "yol kaçışlı sürüm" "$(code POST /sites -d '{"domain":"uygulama.test","kind":"node","node_version":"../x"}')" 400
S=$(api POST /sites -d '{"domain":"uygulama.test","kind":"node","node_version":"22"}' | jget 'd["id"]')
sleep 2
has "örnek uygulama yayında" "$(site uygulama.test /)" "Node.js v22"
api GET /sites/$S/node | jget '(d["state"], d["port"], d["pid"] > 0)'
check "servis sitenin kullanıcısıyla çalışıyor" "$(ps -o user= -p "$(systemctl show -p MainPID --value pixpanel-node-$S)" | tr -d ' ')" admin

say "Ayarlar ve ortam değişkenleri"
cat > /home/admin/sites/uygulama.test/server.js <<'XEOF'
const http = require("node:http");
http.createServer((req, res) => res.end(`${process.env.SELAM}|${process.env.NODE_ENV}|${req.url}`)).listen(process.env.PORT, process.env.HOST);
XEOF
cat > /home/admin/sites/uygulama.test/package.json <<'XEOF'
{"name":"deneme","private":true,"scripts":{"start":"node server.js","build":"echo derlendi > built.txt"}}
XEOF
chown admin:www-data /home/admin/sites/uygulama.test/server.js /home/admin/sites/uygulama.test/package.json
cat > /tmp/px-node.json <<'XEOF'
{"node":{"version":"22","command":"node server.js","memory_mb":256,"env":[{"key":"SELAM","value":"a b\"c$d\\e"},{"key":"NODE_ENV","value":"staging"}]}}
XEOF
check "ayar kaydı" "$(code PATCH /sites/$S -d @/tmp/px-node.json)" 200
sleep 2
check "ortam değişkeni bozulmadan ulaştı" "$(site uygulama.test /yol)" 'a b"c$d\e|staging|/yol'
check "ortam dosyasını yalnızca root okur" "$(stat -c '%a %U' /etc/pixpanel/node/$S.env)" "600 root"
check "bellek sınırı uygulandı" "$(systemctl show -p MemoryMax --value pixpanel-node-$S)" 268435456
check "PORT elle tanımlanamaz" "$(code PATCH /sites/$S -d '{"node":{"version":"22","command":"npm start","env":[{"key":"PORT","value":"80"}]}}')" 400
check "bozuk değişken adı" "$(code PATCH /sites/$S -d '{"node":{"version":"22","command":"npm start","env":[{"key":"A=B\nUser","value":"root"}]}}')" 400
check "satır sonlu komut" "$(code PATCH /sites/$S -d '{"node":{"version":"22","command":"npm start\nUser=root"}}')" 400
check "satır sonlu değer" "$(code PATCH /sites/$S -d '{"node":{"version":"22","command":"npm start","env":[{"key":"A","value":"x\nExecStartPre=/bin/id"}]}}')" 400
check "reddedilen ayarlar servisi bozmadı" "$(site uygulama.test / | cut -d'|' -f2)" staging

say "npm ve servis işlemleri"
check "npm betiği" "$(wait_job "$(api POST /sites/$S/node/run -d '{"script":"build","restart":true}' | jget 'd["job_id"]')")" success
check "betik sitenin kullanıcısıyla çalıştı" "$(stat -c %U /home/admin/sites/uygulama.test/built.txt) $(cat /home/admin/sites/uygulama.test/built.txt)" "admin derlendi"
check "geçersiz betik adı" "$(code POST /sites/$S/node/run -d '{"script":"build; id"}')" 400
check "durdur" "$(api POST /sites/$S/node/stop | jget 'd["state"]')" inactive
check "durmuşken 502" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: uygulama.test' http://127.0.0.1/)" 502
check "başlat" "$(api POST /sites/$S/node/start | jget 'd["state"]')" active
sleep 1
api GET "/logs/read?source=site:$S:app&lines=20" | jget '"uygulama günlüğü satırı:", len(d["lines"]) > 0'
check "PHP sitesinde Node işlemi" "$(code POST /sites/1/node/restart)" 400

say "Yol bazlı vekil kuralları"
docker rm -f pxwho >/dev/null 2>&1; docker run -d --name pxwho -p 127.0.0.1:18082:80 traefik/whoami >/dev/null 2>&1; sleep 2
ST=$(api GET /sites | jget 'next(s["id"] for s in d if s["domain"]=="statik.test")')
R='{"proxy_rules":[{"path":"/api/","target":"http://127.0.0.1:18082","websocket":true,"strip_prefix":true},{"path":"/servis/","target":"http://127.0.0.1:18082","websocket":false,"strip_prefix":false}]}'
check "kural kaydı" "$(code PATCH /sites/$ST -d "$R")" 200
has "önek kırpılarak iletildi" "$(site statik.test /api/kullanici)" "GET /kullanici "
has "önek korunarak iletildi" "$(site statik.test /servis/durum)" "GET /servis/durum "
has "gerçek alan adı iletiliyor" "$(site statik.test /api/x)" "Host: statik.test"
has "sitenin kendi içeriği yerinde" "$(site statik.test /)" "<h1>"
check "kök yol kural olamaz" "$(code PATCH /sites/$ST -d '{"proxy_rules":[{"path":"/","target":"http://127.0.0.1:18082"}]}')" 400
check "boşluklu yol" "$(code PATCH /sites/$ST -d '{"proxy_rules":[{"path":"/a b { return 200; }","target":"http://127.0.0.1:18082"}]}')" 400
check "yollu hedef" "$(code PATCH /sites/$ST -d '{"proxy_rules":[{"path":"/x/","target":"http://127.0.0.1:18082/y"}]}')" 400
check "noktalı virgüllü hedef" "$(code PATCH /sites/$ST -d '{"proxy_rules":[{"path":"/x/","target":"http://127.0.0.1:18082; return 200"}]}')" 400
check "sertifika doğrulama yolu kural olamaz" "$(code PATCH /sites/$ST -d '{"proxy_rules":[{"path":"/.well-known/","target":"http://127.0.0.1:18082"}]}')" 400
has "reddedilen kurallar eskileri bozmadı" "$(site statik.test /api/hala)" "GET /hala "
check "kuralları kaldır" "$(code PATCH /sites/$ST -d '{"proxy_rules":[]}')" 200
check "kural kalkınca 404" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: statik.test' http://127.0.0.1/api/x)" 404
docker rm -f pxwho >/dev/null 2>&1

say "Node.js sürüm güncelleme"
VERS=$(api GET /system/node/versions)
check "sürüm listesi alanları" "$(jget 'sorted(d[0].keys())' <<<"$VERS")" "['checked_at', 'installed', 'latest', 'major', 'security', 'sites', 'update_available']"
check "kurulu sürüm tam numara olarak listelenir" "$(jget 'next(v["installed"] for v in d if v["major"]=="22").count(".")' <<<"$VERS")" 2
check "bu sınamanın sitesi 22 sürümünü kullanıyor sayılır" "$(jget 'next(v["sites"] for v in d if v["major"]=="22") >= 1' <<<"$VERS")" True
check "oturumsuz istek" "$(curl -sk -o /dev/null -w '%{http_code}' "$B/system/node/versions")" 401
check "geçersiz ana sürüm" "$(code POST /system/node/versions/abc/update)" 400
check "yol kaçışlı ana sürüm" "$(code POST '/system/node/versions/..%2Fx/update')" 400
check "kurulu olmayan ana sürüm" "$(code POST /system/node/versions/7/update)" 400

# Gerçek güncelleme: 20 sürümü bu sınamaya aittir. Başkası kullanıyorsa dokunulmaz.
N20=/opt/pixpanel/node/20
USES20=$(api GET /sites | jget 'len([s for s in d if s["kind"]=="node" and s["node"]["version"]=="20"])')
if [ "$USES20" != 0 ]; then
  echo "  ATLANDI Node.js 20'yi kullanan site var; güncelleme sınaması yapılmadı"
else
  WAS20=0; [ -x $N20/bin/node ] && WAS20=1
  if [ $WAS20 = 0 ]; then
    check "Node.js 20 kurulumu" "$(wait_job "$(api POST /components/node-20/install | jget 'd["job_id"]')")" success
  fi
  # Eski bir yama sürümü (resmî derleme, özeti listeyle doğrulanır) yerine konur.
  OLDV=20.10.0
  OLDF=node-v$OLDV-linux-x64.tar.xz
  curl -fsSL -o /tmp/px-node-old.tar.xz https://nodejs.org/dist/v$OLDV/$OLDF
  EXPECT=$(curl -fsSL https://nodejs.org/dist/v$OLDV/SHASUMS256.txt | grep " $OLDF\$" | cut -d' ' -f1)
  check "eski derlemenin özeti" "$(sha256sum /tmp/px-node-old.tar.xz | cut -d' ' -f1)" "$EXPECT"
  downgrade() { rm -rf $N20; mkdir -p $N20; tar -xJf /tmp/px-node-old.tar.xz -C $N20 --strip-components=1 --no-same-owner; }
  downgrade
  check "kurulu eski yama görünür; güncelleme ve güvenlik düzeltmesi var" "$(api GET /system/node/versions | jget 'next((v["installed"], v["update_available"], v["security"], v["sites"]) for v in d if v["major"]=="20")')" "('$OLDV', True, True, 0)"
  H=$(api POST /health/check)
  check "sağlık merkezi güvenlik güncellemesini bildirir" "$(jget "next((f['severity'], f['fix']) for f in d['report']['findings'] if f['key']=='node.update:20')" <<<"$H")" "('warning', 'Güncelle')"

  api DELETE "/sites/$(api GET /sites | jget 'next((s["id"] for s in d if s["domain"]=="node-guncelle.test"), "")')?delete_files=true" >/dev/null 2>&1
  S20=$(api POST /sites -d '{"domain":"node-guncelle.test","kind":"node","node_version":"20"}' | jget 'd["id"]')
  cat > /home/admin/sites/node-guncelle.test/server.js <<'XEOF'
const http = require("node:http");
// Sınama: PX_CRASH tanımlıysa yalnızca eski yama sürümünde ayakta kalır.
if (process.env.PX_CRASH && process.version !== "v20.10.0") process.exit(1);
http.createServer((req, res) => res.end(process.version)).listen(process.env.PORT, process.env.HOST);
XEOF
  chown admin:www-data /home/admin/sites/node-guncelle.test/server.js
  check "ayar kaydı" "$(code PATCH /sites/$S20 -d '{"node":{"version":"20","command":"node server.js"}}')" 200
  sleep 2
  check "güncellemeden önce eski sürümle yayında" "$(site node-guncelle.test /)" "v$OLDV"
  check "sürümü kullanan site sayılır" "$(api GET /system/node/versions | jget 'next(v["sites"] for v in d if v["major"]=="20")')" 1
  PID1=$(systemctl show -p MainPID --value pixpanel-node-$S20)

  UJ=$(api POST /system/node/versions/20/update | jget 'd["job_id"]')
  check "aynı sürümde ikinci güncelleme başlatılamaz" "$(code POST /system/node/versions/20/update)" 409
  check "güncelleme görevi" "$(wait_job "$UJ")" success
  ULOG=$(api GET "/jobs/$UJ" | jget 'd["log"]')
  has "sağlama toplamı doğrulandı" "$ULOG" "Sağlama toplamı doğrulandı"
  has "kullanan servis yeniden başlatıldı" "$ULOG" "node-guncelle.test: servis yeniden başlatılıyor"
  check "yeni yama sürümü kuruldu" "$([ "$($N20/bin/node --version)" != "v$OLDV" ] && echo yeni)" yeni
  check "site yeni sürümle yayında" "$([ "$(site node-guncelle.test /)" = "$($N20/bin/node --version)" ] && echo evet)" evet
  check "servis yeni süreçle çalışıyor" "$([ "$(systemctl show -p MainPID --value pixpanel-node-$S20)" != "$PID1" ] && echo evet)" evet
  check "güncelleme sonrası güncel görünür" "$(api GET /system/node/versions | jget 'next((v["update_available"], v["security"]) for v in d if v["major"]=="20")')" "(False, False)"
  check "geçici klasör kalmadı" "$(ls /opt/pixpanel/node | grep -c '\.\(old\|new\|failed\)$')" 0
  check "denetim günlüğüne yazıldı" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) >= 1 FROM audit_log WHERE action='node.update' AND target='20'")" 1
  H=$(api POST /health/check)
  check "güncellemeden sonra bulgu kalkar" "$(jget "next((f['key'] for f in d['report']['findings'] if f['key']=='node.update:20'), 'yok')" <<<"$H")" yok
  NJ=$(api POST /system/node/versions/20/update | jget 'd["job_id"]')
  check "güncel sürümde yeniden güncelleme bir şey yapmaz" "$(wait_job "$NJ")" success
  has "zaten güncel iletisi" "$(api GET "/jobs/$NJ" | jget 'd["log"]')" "yapılacak bir şey yok"

  # Yeni sürümde çöken uygulama: güncelleme başarısız sayılır ve eski sürüme dönülür.
  downgrade
  check "çökme denemesi için ayar" "$(code PATCH /sites/$S20 -d '{"node":{"version":"20","command":"node server.js","env":[{"key":"PX_CRASH","value":"1"}]}}')" 200
  sleep 2
  check "eski sürümle ayakta" "$(site node-guncelle.test /)" "v$OLDV"
  RJ=$(api POST /system/node/versions/20/update | jget 'd["job_id"]')
  check "yeni sürümle çöken uygulama güncellemeyi başarısız sayar" "$(wait_job "$RJ")" failed
  RLOG=$(api GET "/jobs/$RJ" | jget 'd["log"]')
  has "eski sürüme dönüldü" "$RLOG" "eski sürüme ($OLDV) döndürüldü"
  check "eski sürüm geri kondu" "$($N20/bin/node --version)" "v$OLDV"
  sleep 4
  check "site eski sürümle yeniden yayında" "$(site node-guncelle.test /)" "v$OLDV"
  check "geri dönüşten sonra geçici klasör kalmadı" "$(ls /opt/pixpanel/node | grep -c '\.\(old\|new\|failed\)$')" 0

  check "sınama sitesini sil" "$(code DELETE "/sites/$S20?delete_files=true")" 200
  if [ $WAS20 = 1 ]; then
    # Önceden kuruluydu: en son yama sürümüne geri getirilir.
    check "Node.js 20 eski hâline getirilir" "$(wait_job "$(api POST /system/node/versions/20/update | jget 'd["job_id"]')")" success
  else
    rm -rf $N20
  fi
  rm -f /tmp/px-node-old.tar.xz
fi

say "Temizlik"
check "siteyi sil" "$(code DELETE "/sites/$S?delete_files=true")" 200
check "servis kaldırıldı" "$(systemctl list-unit-files "pixpanel-node-$S.service" --no-legend | wc -l) $(ls /etc/pixpanel/node/ 2>/dev/null | grep -c "^$S\.")" "0 0"
