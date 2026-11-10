#!/usr/bin/env bash
# PHP genel ayarları, Git ile yayınlama, bildirimler ve Docker uçlarının sınaması.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash newtest.sh [bölüm...]
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=$3 gelen=$2"; fi; }
wait_job() { for _ in $(seq 1 120); do s=$(api GET "/jobs/$1" | python3 -c 'import sys,json; print(json.load(sys.stdin)["job"]["status"])' 2>/dev/null); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('  (JSON degil:', raw[:200], ')'); sys.exit()
try: print($1)
except Exception as e: print('  (alan yok:', e, raw[:200], ')')"; }

api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
SECTIONS="${*:-php git docker}"

if [[ $SECTIONS == *php* ]]; then
say "PHP genel ayarları"
api GET /php | jget '[(v["version"], v["running"], v["site_count"], v["settings"]["memory_limit"]) for v in d]'
G='{"memory_limit":"384M","upload_max_filesize":"80M","post_max_size":"80M","max_execution_time":90,"max_input_vars":4000,"display_errors":false,"timezone":"Europe/Istanbul","opcache":true,"opcache_memory":128,"custom":[]}'
check "genel ayar kaydı" "$(code PUT /php/8.3/settings -d "$G")" 200
# Site ayarlarını devralır hâle getir
check "site ayarlarını sıfırla" "$(code PATCH /sites/1 -d '{"php_settings":{}}')" 200
printf '<?php echo ini_get("memory_limit"),"|",ini_get("upload_max_filesize"),"|",date_default_timezone_get(),"|",ini_get("max_execution_time");' > /home/admin/sites/deneme.test/public/t.php
chown admin:www-data /home/admin/sites/deneme.test/public/t.php
check "site geneli devralıyor" "$(curl -sk https://deneme.test/t.php)" "384M|80M|Europe/Istanbul|90"
check "komut satırı saat dilimi" "$(php8.3 -r 'echo date_default_timezone_get();')" "Europe/Istanbul"
check "site kendi değerini eziyor" "$(code PATCH /sites/1 -d '{"php_settings":{"memory_limit":"512M"}}')" 200
check "ezilen değer geçerli" "$(curl -sk https://deneme.test/t.php)" "512M|80M|Europe/Istanbul|90"
check "geçersiz saat dilimi" "$(code PUT /php/8.3/settings -d "${G/Europe\/Istanbul/..\/..\/etc\/passwd}")" 400
check "satır sonlu yönerge" "$(code PUT /php/8.3/settings -d "${G/\"custom\":[]/\"custom\":[{\"key\":\"a\",\"value\":\"1\\nextension=evil.so\"}]}")" 400
check "olmayan sürüm" "$(code PUT /php/9.9/settings -d "$G")" 404
api GET /php/8.3/extensions | jget '"kurulu:", len(d["installed"]), "kurulabilir:", d["installable"][:6]'
check "eklenti kapat" "$(code POST /php/8.3/extensions/redis -d '{"enabled":false}')" 200
check "redis kapalı" "$(php8.3 -m | grep -c '^redis$')" 0
check "eklenti aç" "$(code POST /php/8.3/extensions/redis -d '{"enabled":true}')" 200
check "redis açık" "$(php8.3 -m | grep -c '^redis$')" 1
check "eklenti adı enjeksiyonu" "$(code POST '/php/8.3/extensions/..%2Fx' -d '{"enabled":true}')" 400
check "müşteri erişemez" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer nope" $B/php)" 401
rm -f /home/admin/sites/deneme.test/public/t.php
fi

if [[ $SECTIONS == *git* ]]; then
say "Bildirim ayarları"
: > /tmp/px-mock.log
check "smtp kaydı" "$(code PUT /notifications/smtp -d '{"host":"127.0.0.1","port":2525,"security":"none","username":"","from":"PixPanel <panel@pix.test>"}')" 200
check "kanal kaydı" "$(code PUT /notifications -d '{"email_enabled":true,"email_to":"yonetici@pix.test","telegram_enabled":true,"telegram_token":"123456:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi","telegram_chat_id":"-100123","events":["deploy_failed","deploy_success"]}')" 200
api GET /notifications | jget '"anahtar gizli:", "telegram_token" not in d, d["telegram_token_set"]'
check "e-posta denemesi" "$(code POST /notifications/test -d '{"channel":"email"}')" 200
check "telegram denemesi" "$(code POST /notifications/test -d '{"channel":"telegram"}')" 200
check "bozuk bot anahtarı" "$(code PUT /notifications -d '{"email_enabled":false,"email_to":"","telegram_enabled":true,"telegram_token":"x/../y","telegram_chat_id":"1","events":[]}')" 400
cat /tmp/px-mock.log

say "Git deposu hazırlığı"
id gitsrv >/dev/null 2>&1 || useradd -m -s /usr/bin/git-shell gitsrv
rm -rf /home/gitsrv/demo.git /tmp/demo-work
runuser -u gitsrv -- git init -q --bare -b main /home/gitsrv/demo.git
git init -q -b main /tmp/demo-work && cd /tmp/demo-work
git config user.email t@pix.test; git config user.name Deneme
mkdir public && echo '<h1>surum 1</h1>' > public/index.html && echo 'a' > README.md
git add . && git commit -qm "ilk sürüm" && git -c safe.directory='*' push -q /home/gitsrv/demo.git main 2>&1 | tail -1
chown -R gitsrv: /home/gitsrv/demo.git; cd /

say "Git ile yayınlama"
check "komut taşıyıcılı adres" "$(code PUT /sites/2/git -d '{"repo_url":"ext::sh -c id","branch":"main"}')" 400
check "seçenek gibi adres" "$(code PUT /sites/2/git -d '{"repo_url":"--upload-pack=id","branch":"main"}')" 400
check "yerel dosya adresi" "$(code PUT /sites/2/git -d '{"repo_url":"file:///etc","branch":"main"}')" 400
check "parolalı adres" "$(code PUT /sites/2/git -d '{"repo_url":"https://u:p@github.com/a/b.git","branch":"main"}')" 400
check "seçenek gibi dal" "$(code PUT /sites/2/git -d '{"repo_url":"gitsrv@localhost:demo.git","branch":"--exec=id"}')" 400
cat > /tmp/px-script.sh <<'XEOF'
echo "betik: $PIXPANEL_DOMAIN $PIXPANEL_BRANCH ${PIXPANEL_COMMIT:0:7} (onceki: ${PIXPANEL_PREVIOUS_COMMIT:0:7})"
id -un
echo yayinlandi > public/durum.txt
XEOF
python3 -c 'import json; print(json.dumps({"repo_url":"gitsrv@localhost:demo.git","branch":"main","script":open("/tmp/px-script.sh").read()}))' > /tmp/px-body.json
V=$(api PUT /sites/2/git -d @/tmp/px-body.json)
echo "$V" | jget '"anahtar:", d["config"]["public_key"][:40], "kanca:", d["webhook_path"][:28]'
HOOK=$(echo "$V" | jget 'd["webhook_path"]')
mkdir -p /home/gitsrv/.ssh && echo "$V" | jget 'd["config"]["public_key"]' > /home/gitsrv/.ssh/authorized_keys
chown -R gitsrv: /home/gitsrv/.ssh && chmod 700 /home/gitsrv/.ssh && chmod 600 /home/gitsrv/.ssh/authorized_keys
ls -la /home/admin/.ssh/ | grep -c pixpanel-deploy | sed 's/^/  anahtar dosyası sayısı: /'

deploy_and_wait() {
  local dep; dep=$(api POST /sites/2/git/deploy -d "${1:-{\}}" | jget 'd.get("deployment")')
  for _ in $(seq 1 60); do
    st=$(api GET "/sites/2/git/deployments/$dep" | jget 'd["deployment"]["status"]'); [ "$st" != running ] && break; sleep 1
  done
  echo "$dep $st"
}
R=$(deploy_and_wait); check "ilk yayın" "${R#* }" success
api GET "/sites/2/git/deployments/${R% *}" | jget 'd["log"]'
check "site yeni içeriği sunuyor" "$(curl -s -H 'Host: statik.test' http://127.0.0.1/)" "<h1>surum 1</h1>"
check "betik dosya yazdı" "$(cat /home/admin/sites/statik.test/public/durum.txt)" yayinlandi
check "dosya sahibi" "$(stat -c %U /home/admin/sites/statik.test/public/durum.txt)" admin
check ".git dışarıya kapalı" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: statik.test' http://127.0.0.1/.git/config)" 404

say "Web kancası"
cd /tmp/demo-work && echo '<h1>surum 2</h1>' > public/index.html && git commit -qam "ikinci sürüm" && git -c safe.directory='*' push -q /home/gitsrv/demo.git main; cd /
chown -R gitsrv: /home/gitsrv/demo.git
check "yanlış anahtar" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST https://127.0.0.1:8443/api/v1/hooks/deploy/000000000000000000000000000000000000000000000000)" 404
curl -sk -X POST "https://127.0.0.1:8443$HOOK" -H 'content-type: application/json' -d '{"ref":"refs/heads/baska"}' | jget '"başka dal:", d["result"]'
curl -sk -X POST "https://127.0.0.1:8443$HOOK" -H 'x-github-event: ping' -d '{}' | jget '"ping:", d["result"]'
curl -sk -X POST "https://127.0.0.1:8443$HOOK" -H 'content-type: application/json' -d '{"ref":"refs/heads/main"}' | jget '"ana dal:", d["result"]'
curl -sk -X POST "https://127.0.0.1:8443$HOOK" -H 'content-type: application/json' -d '{"ref":"refs/heads/main"}' | jget '"hemen ardından:", d["result"]'
sleep 6
check "kanca sonrası içerik" "$(curl -s -H 'Host: statik.test' http://127.0.0.1/)" "<h1>surum 2</h1>"
api GET /sites/2/git | jget '[(x["source"], x["status"], x["commit_subject"]) for x in d["deployments"]]'

say "Eski sürüme dönüş ve başarısız yayın"
FIRST=$(git -C /tmp/demo-work rev-list --max-parents=0 HEAD)
R=$(deploy_and_wait "{\"commit\":\"$FIRST\"}"); check "eski sürüme dönüş" "${R#* }" success
check "eski içerik" "$(curl -s -H 'Host: statik.test' http://127.0.0.1/)" "<h1>surum 1</h1>"
api PUT /sites/2/git -d '{"repo_url":"gitsrv@localhost:demo.git","branch":"main","script":"echo basliyor\nfalse\necho buraya gelmemeli"}' >/dev/null
: > /tmp/px-mock.log
R=$(deploy_and_wait); check "hatalı betik başarısız sayılır" "${R#* }" failed
api GET "/sites/2/git/deployments/${R% *}" | jget '"gelmemeli" not in d["log"].split("basliyor")[-1]'
sleep 2; cat /tmp/px-mock.log
check "müşteri başkasının yayınını göremez" "$(curl -sk -o /dev/null -w '%{http_code}' -b /tmp/pxjar2 $B/sites/2/git)" 401
fi

if [[ $SECTIONS == *docker* ]]; then
say "Docker"
api GET /docker | jget 'd'
docker rm -f pxtest >/dev/null 2>&1
check "seçenek gibi imaj" "$(code POST /docker/containers -d '{"image":"--privileged"}')" 400
check "seçenek gibi ad" "$(code POST /docker/containers -d '{"image":"nginx:alpine","name":"-v/:/host"}')" 400
check "birim kaçışı" "$(code POST /docker/containers -d '{"image":"nginx:alpine","volumes":[{"source":"/etc:/x","target":"/data"}]}')" 400
cat > /tmp/px-run.json <<'XEOF'
{"image":"nginx:alpine","name":"pxtest","ports":[{"host_port":18080,"container_port":80}],"env":[{"key":"SIR","value":"a b$c\"d"}],"memory_mb":64}
XEOF
JOB=$(api POST /docker/containers -d @/tmp/px-run.json | jget 'd["job_id"]')
check "konteyner oluşturma" "$(wait_job "$JOB")" success
api GET /docker/containers | jget '[(c["name"], c["state"], [(p["host_ip"], p["host_port"]) for p in c["ports"]]) for c in d]'
check "konteyner yanıt veriyor" "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18080/)" 200
check "ortam değişkeni bozulmadı" "$(docker exec pxtest printenv SIR)" 'a b$c"d'
check "görev günlüğünde sır yok" "$(api GET "/jobs/$JOB" | grep -c 'a b')" 0
api GET /docker/containers/stats | jget '[(s["cpu"], s["memory"]) for s in d][:2]'
api GET /docker/containers/pxtest/logs?tail=20 | jget 'len(d["lines"]), "satır günlük"'
check "durdur" "$(code POST /docker/containers/pxtest/stop)" 200
check "geçersiz işlem" "$(code POST /docker/containers/pxtest/exec)" 400
check "sil" "$(code DELETE /docker/containers/pxtest)" 200
api GET /docker/images | jget '[(i["repository"], i["tag"], i["size"]) for i in d][:4]'
check "birim oluştur" "$(code POST /docker/volumes -d '{"name":"pxvol"}')" 200
check "birim sil" "$(code DELETE /docker/volumes/pxvol)" 200

say "Compose yığını"
api DELETE /docker/stacks/deneme >/dev/null
check "bozuk compose" "$(code POST /docker/stacks -d '{"name":"deneme","content":"services:\n  web:\n    imagee: [bozuk"}')" 400
check "yol kaçışlı ad" "$(code POST /docker/stacks -d '{"name":"../x","content":"services: {}"}')" 400
C='services:\n  web:\n    image: traefik/whoami\n    ports:\n      - \"127.0.0.1:18081:80\"\n'
check "yığın oluştur" "$(code POST /docker/stacks -d "{\"name\":\"deneme\",\"content\":\"$C\"}")" 200
check "yığını başlat" "$(wait_job "$(api POST /docker/stacks/deneme/up | jget 'd["job_id"]')")" success
api GET /docker/stacks | jget 'd'
check "yığın yanıt veriyor" "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18081/)" 200
check "yığını kaldır" "$(code DELETE /docker/stacks/deneme)" 200
check "klasör temizlendi" "$(ls /opt/pixpanel/stacks 2>/dev/null | wc -l)" 0
fi
