#!/usr/bin/env bash
# Güvenlik duvarının sınaması. Diğer sınamalardan farklı olarak GELİŞTİRME
# MAKİNESİNDE çalışır: engellemenin gerçekten işlediğini görmek için bağlantılar
# test makinesine dışarıdan denenir.
# Kullanım: scripts/e2e/firewall.sh   (panel bilgileri .env dosyasından okunur)
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
set -a; . "$ROOT/.env"; set +a
HOST=192.168.1.25
P=https://$HOST:8443/api/v1
J=$(mktemp)
SSH="ssh -o BatchMode=yes -o ConnectTimeout=8 root@$HOST"
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
# Dışarıdan TCP bağlantısı kurulabiliyor mu?
probe() { if timeout 3 bash -c "</dev/tcp/$HOST/$1" 2>/dev/null; then echo acik; else echo kapali; fi; }
settings() { python -c "
import json,sys
base=json.load(open(sys.argv[1]))
exec(sys.argv[2])
print(json.dumps({'settings': base}))" "$BASE" "$1"; }

api POST /auth/login -d "{\"username\":\"$PIXPANEL_TEST_USER\",\"password\":\"$PIXPANEL_TEST_PASSWORD\"}" >/dev/null
BASE=$(mktemp)

say "Hazırlık"
api POST /firewall/revert >/dev/null
$SSH 'systemctl stop pxfw-http 2>/dev/null; systemd-run --quiet --unit pxfw-http python3 -m http.server 18099 --bind 0.0.0.0; docker rm -f pxfw >/dev/null 2>&1; docker run -d --name pxfw -p 0.0.0.0:18098:80 traefik/whoami >/dev/null 2>&1; sleep 2'
V=$(api GET /firewall)
echo "$V" | jget '("nftables:", d["available"], "adresiniz:", d["your_ip"], "hazır kümeler:", [p["key"] for p in d["presets"]])'
echo "$V" | jget 'json.dumps(dict(d["settings"], enabled=False, rules=[], blocked=[]))' > "$BASE"
# Bilinen bir başlangıç: kapalı ve onaylı.
api PUT /firewall -d "$(settings 'pass')" >/dev/null; api POST /firewall/confirm >/dev/null
check "kapalıyken sınama portu dışarıdan açık" "$(probe 18099)" acik

say "Etkinleştirme ve onaysız geri dönüş"
check "etkinleştir (onay bekler)" "$(api PUT /firewall -d "$(settings 'base["enabled"]=True')" | jget 'd["pending"] is not None')" True
check "kuralı olmayan port kapandı" "$(probe 18099)" kapali
check "SSH açık kaldı" "$(probe 22)" acik
check "panel açık kaldı" "$(probe 8443)" acik
check "web açık kaldı" "$(probe 80)" acik
check "Docker'ın yayınladığı port etkilenmedi" "$(probe 18098)" acik
api GET /firewall | jget '[(l["port"], l["verdict"]) for l in d["listeners"] if l["port"] in (22, 80, 18099)]'
echo "  onaylanmadan $(api GET /firewall | jget 'd["confirm_seconds"]') saniye bekleniyor..."
sleep 66
check "onaylanmayınca önceki duruma dönüldü" "$(probe 18099)" acik
check "bekleyen değişiklik kalmadı" "$(api GET /firewall | jget 'd["pending"]')" None
check "tablo kaldırıldı" "$($SSH 'nft list tables | grep -c pixpanel')" 0
check "süre dolduktan sonra onay reddedilir" "$(code POST /firewall/confirm)" 400

say "Onaylı kurallar"
RULES='base["enabled"]=True; base["rules"]=[{"label":"deneme","protocol":"tcp","ports":"18099","source":"","action":"accept"}]; base["blocked"]=["203.0.113.9","2001:db8::/32"]'
api PUT /firewall -d "$(settings "$RULES")" >/dev/null
check "özel kuralla port açıldı" "$(probe 18099)" acik
check "onayla" "$(code POST /firewall/confirm)" 200
check "kalıcı kural dosyası yazıldı" "$($SSH 'grep -c "tcp dport { 18099 } accept" /etc/pixpanel/firewall.nft')" 1
check "açılış birimi etkin" "$($SSH 'systemctl is-enabled pixpanel-firewall.service')" enabled
check "engelli adres kümede" "$($SSH 'nft list set inet pixpanel blocked4 | grep -c 203.0.113.9')" 1
$SSH 'systemctl restart pixpanel; sleep 3'
api POST /auth/login -d "{\"username\":\"$PIXPANEL_TEST_USER\",\"password\":\"$PIXPANEL_TEST_PASSWORD\"}" >/dev/null
check "panel yeniden başlayınca kurallar yerinde" "$(probe 18099) $($SSH 'nft list tables | grep -c pixpanel')" "acik 1"

say "Erişimi daraltma ve kilitlenme denetimi"
check "FTP'yi başka bir adrese daralt" "$(api PUT /firewall -d "$(settings "$RULES"'; [p.update(source="10.9.9.9") for p in base["presets"] if p["key"]=="ftp"]')" | jget 'd["pending"] is not None')" True
check "FTP dışarıdan kapandı" "$(probe 21)" kapali
api GET /firewall | jget '[(l["port"], l["verdict"]) for l in d["listeners"] if l["port"] == 21]'
api POST /firewall/revert >/dev/null
check "geri alınca FTP yeniden açık" "$(probe 21)" acik
check "paneli kendi adresine kapatan kural reddedilir" "$(code PUT /firewall -d "$(settings "$RULES"'; [p.update(source="10.0.0.0/8") for p in base["presets"] if p["key"]=="panel"]')")" 400
check "panel kümesini kapatmak reddedilir" "$(code PUT /firewall -d "$(settings "$RULES"'; [p.update(enabled=False) for p in base["presets"] if p["key"]=="panel"]')")" 400
MYIP=$(api GET /firewall | jget 'd["your_ip"]')
check "kendi adresini engellemek reddedilir" "$(code PUT /firewall -d "$(settings "$RULES"'; base["blocked"]=["'"$MYIP"'"]')")" 400
check "panel portunu engelleyen özel kural reddedilir" "$(code PUT /firewall -d "$(settings 'base["enabled"]=True; base["rules"]=[{"label":"x","protocol":"tcp","ports":"8000-9000","source":"","action":"drop"}]')")" 400
check "reddedilen denemeler kuralları değiştirmedi" "$(probe 8443) $(probe 18099)" "acik acik"

say "Geçersiz girdiler"
check "port alanında komut" "$(code PUT /firewall -d "$(settings 'base["enabled"]=True; base["rules"]=[{"label":"x","protocol":"tcp","ports":"80; flush ruleset","source":"","action":"accept"}]')")" 400
check "adres alanında komut" "$(code PUT /firewall -d "$(settings 'base["enabled"]=True; base["rules"]=[{"label":"x","protocol":"tcp","ports":"80","source":"1.2.3.4 accept; flush ruleset","action":"accept"}]')")" 400
check "engelli listede küme kaçışı" "$(code PUT /firewall -d "$(settings 'base["enabled"]=True; base["blocked"]=["1.2.3.4 } ; flush ruleset"]')")" 400
check "geçersiz protokol" "$(code PUT /firewall -d "$(settings 'base["enabled"]=True; base["rules"]=[{"label":"x","protocol":"icmp","ports":"80","source":"","action":"accept"}]')")" 400
check "Docker tabloları yerinde" "$($SSH 'nft list tables | grep -c "ip nat"')" 1
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $P/firewall)" 401

say "Son durum"
# Güvenlik duvarı varsayılan kümelerle açık ve onaylı bırakılır.
api PUT /firewall -d "$(settings 'base["enabled"]=True')" >/dev/null
check "varsayılan kurallarla onayla" "$(code POST /firewall/confirm)" 200
check "sınama portu kapalı" "$(probe 18099)" kapali
$SSH 'systemctl stop pxfw-http 2>/dev/null; docker rm -f pxfw >/dev/null 2>&1'
rm -f "$J" "$BASE"
echo "bitti (güvenlik duvarı açık bırakıldı)"
