#!/usr/bin/env bash
# fail2ban sınaması. GELİŞTİRME MAKİNESİNDE çalışır: hatalı girişler test
# makinesine dışarıdan yapılır ve bu makinenin adresinin engellendiği doğrulanır.
# Engel sırasında makineye SSH ile ulaşılamayacağı için durum Proxmox üzerinden
# (pct exec) okunur ve engel oradan kaldırılır.
# Kullanım: scripts/e2e/fail2ban.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
set -a; . "$ROOT/.env"; set +a
HOST=192.168.1.25
CT=105
P=https://$HOST:8443/api/v1
J=$(mktemp)
SSH="ssh -o BatchMode=yes -o ConnectTimeout=8 root@$HOST"
PCT="ssh -o BatchMode=yes -o ConnectTimeout=8 proxmox pct exec $CT --"
api() { local m=$1 p=$2; shift 2; curl -sk -m 20 -b "$J" -c "$J" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$P$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -m 20 -o /dev/null -w '%{http_code}' -b "$J" -c "$J" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$P$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
probe() { if timeout 3 bash -c "</dev/tcp/$HOST/$1" 2>/dev/null; then echo acik; else echo kapali; fi; }
login() { api POST /auth/login -d "{\"username\":\"$PIXPANEL_TEST_USER\",\"password\":\"$PIXPANEL_TEST_PASSWORD\"}" >/dev/null; }
banned() { $PCT fail2ban-client status "$1" 2>/dev/null | grep -c "$MYIP"; }
# Engelin kalkmasını bekler (en çok 40 sn); kalkmazsa Proxmox üzerinden kaldırır.
release() { for _ in $(seq 1 20); do [ "$(probe 8443)" = acik ] && return; sleep 2; done; $PCT fail2ban-client unban --all >/dev/null; sleep 1; }
settings() { python -c "
import json,sys
base=json.load(open(sys.argv[1]))
exec(sys.argv[2])
print(json.dumps({'settings': base}))" "$BASE" "$1"; }

login
BASE=$(mktemp)

say "Kurulum"
wait_job() { for _ in $(seq 1 240); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="fail2ban")')" != True ]; then
  check "fail2ban kurulumu" "$(wait_job "$(api POST /components/fail2ban/install | jget 'd["job_id"]')")" success
fi
# Posta sayfasına bakmak, web postasının güncel yapılandırmasının yazılmasını sağlar.
api GET /mail >/dev/null
V=$(api GET /fail2ban)
MYIP=$(echo "$V" | jget 'd["your_ip"]')
echo "$V" | jget '("çalışıyor:", d["running"], "hapisler:", [(j["key"], j["available"], j["active"]) for j in d["jails"]])'
echo "$V" | jget 'json.dumps(d["settings"])' > "$BASE"
# Sınama için kısa süreler: 3 denemede 20 saniyelik engel, süre uzatma kapalı.
FAST='base["escalate"]=False; base["whitelist"]=[]; [j.update(enabled=True, max_retry=3, find_time=120, ban_time=20) for j in base["jails"]]'
check "ayarları uygula" "$(code PUT /fail2ban -d "$(settings "$FAST")")" 200
sleep 3
check "hapisler çalışıyor" "$(api GET /fail2ban | jget 'all(j["active"] for j in d["jails"] if j["available"])')" True

say "Geçersiz girdiler"
check "beyaz listede komut" "$(code PUT /fail2ban -d "$(settings "$FAST"'; base["whitelist"]=["1.2.3.4\nbantime = 1"]')")" 400
check "sıfır deneme" "$(code PUT /fail2ban -d "$(settings "$FAST"'; base["jails"][0]["max_retry"]=0')")" 400
check "geçersiz adresin engelini kaldırma" "$(code POST /fail2ban/unban -d '{"ip":"1.2.3.4; id"}')" 400
check "bilinmeyen hapis" "$(code POST /fail2ban/ban -d '{"ip":"203.0.113.77","jail":"sshd; id"}')" 400
check "kendi adresini elle engelleme" "$(code POST /fail2ban/ban -d "{\"ip\":\"$MYIP\",\"jail\":\"sshd\"}")" 400
check "elle engelleme" "$(code POST /fail2ban/ban -d '{"ip":"203.0.113.77","jail":"sshd"}')" 200
check "engelli listede görünüyor" "$(api GET /fail2ban | jget '"203.0.113.77" in next(j["banned"] for j in d["jails"] if j["key"]=="sshd")')" True
check "engeli kaldır" "$(api POST /fail2ban/unban -d '{"ip":"203.0.113.77"}' | jget '"203.0.113.77" in next(j["banned"] for j in d["jails"] if j["key"]=="sshd")')" False

say "Panel girişi"
for i in 1 2 3; do curl -sk -m 5 -o /dev/null -H 'content-type: application/json' "$P/auth/login" -d '{"username":"admin","password":"yanlis-parola-deneme"}'; done
sleep 3
check "hatalı girişlerden sonra adres engellendi" "$(banned pixpanel)" 1
check "engelli adres hiçbir porta ulaşamıyor" "$(probe 8443) $(probe 22) $(probe 80)" "kapali kapali kapali"
release
check "süre dolunca erişim geri geldi" "$(probe 8443)" acik

say "Kullanıcı adına gömülen sahte adres"
# Saldırgan, kullanıcı adına başka bir adres yazarak o adresi engelletmeye çalışır.
for i in 1 2 3; do curl -sk -m 5 -o /dev/null -H 'content-type: application/json' "$P/auth/login" -d '{"username":"x ip=203.0.113.200 username=y","password":"yanlis-parola-deneme"}'; done
sleep 3
check "sahte adres engellenmedi" "$($PCT fail2ban-client status pixpanel | grep -c 203.0.113.200)" 0
check "gerçek adres engellendi" "$(banned pixpanel)" 1
release

say "FTP"
python - "$HOST" <<'PY' >/dev/null 2>&1
import ftplib, sys
for _ in range(4):
    try:
        f = ftplib.FTP(); f.connect(sys.argv[1], 21, timeout=5); f.login("olmayan-hesap", "yanlis-parola")
    except Exception:
        pass
PY
sleep 3
check "hatalı FTP girişlerinden sonra engellendi" "$(banned pixpanel-ftp)" 1
release

say "Posta kutusu (IMAP)"
python - "$HOST" <<'PY' >/dev/null 2>&1
import imaplib, ssl, sys
for _ in range(4):
    try:
        m = imaplib.IMAP4_SSL(sys.argv[1], 993, ssl_context=ssl._create_unverified_context(), timeout=5); m.login("yok@deneme.test", "yanlis-parola")
    except Exception:
        pass
PY
sleep 4
check "hatalı IMAP girişlerinden sonra engellendi" "$(banned pixpanel-dovecot)" 1
release

say "Web postası"
# Üçüncü denemede engel başlar; dördüncüsü zaman aşımına takılıp engel süresini tüketirdi.
for i in 1 2 3; do
  W=$(mktemp); PAGE=$(curl -sk -m 8 -c "$W" "https://$HOST:8443/webmail/"); TOKEN=$(grep -o 'name="_token" value="[^"]*"' <<<"$PAGE" | cut -d'"' -f4)
  curl -sk -m 8 -o /dev/null -b "$W" "https://$HOST:8443/webmail/?_task=login" --data-urlencode "_token=$TOKEN" -d '_task=login&_action=login&_timezone=UTC' --data-urlencode '_user=yok@deneme.test' --data-urlencode '_pass=yanlis-parola'; rm -f "$W"
done
sleep 4
check "web postası gerçek adresi günlüğe yazıyor" "$($PCT journalctl -t roundcube --no-pager -n 20 | grep -c "from $MYIP" | awk '{print ($1 >= 3) ? "evet" : "hayir"}')" evet
check "hatalı web postası girişlerinden sonra engellendi" "$(banned pixpanel-webmail)" 1
release

say "Beyaz liste"
login
check "kendi adresini beyaz listeye al" "$(code PUT /fail2ban -d "$(settings "$FAST"'; base["whitelist"]=["'"$MYIP"'"]')")" 200
sleep 3
for i in 1 2 3 4 5; do curl -sk -m 5 -o /dev/null -H 'content-type: application/json' "$P/auth/login" -d '{"username":"admin","password":"yanlis-parola-deneme"}'; done
sleep 3
check "beyaz listedeki adres engellenmedi" "$(probe 8443) $(banned pixpanel)" "acik 0"

say "Son durum"
# Varsayılan sürelerle bırakılır; geliştirme makinesinin adresi beyaz listede kalır.
login
check "varsayılan ayarlar" "$(code PUT /fail2ban -d "$(settings 'base["escalate"]=True; base["whitelist"]=["'"$MYIP"'"]; [j.update(enabled=True, max_retry=5, find_time=600, ban_time=3600) for j in base["jails"]]')")" 200
rm -f "$J" "$BASE"
echo "bitti"
