#!/usr/bin/env bash
# DNS yönetiminin sınaması: yerel bölgeler (PowerDNS), otomatik kayıtlar, DNS ile
# sertifika doğrulaması ve Cloudflare (sahte API'ye karşı). Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash dns.sh
# Önkoşul: /etc/pixpanel/config.toml içinde cloudflare_api = "http://127.0.0.1:9912"
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-dns
HERE="$(cd "$(dirname "$0")" && pwd)"
CF_TOKEN=cf_test_token_0123456789abcdefghij
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 260)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 240); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
q() { dig +short +time=2 +tries=1 @127.0.0.1 "$@" | tr '\n' ' ' | sed 's/ $//'; }
rec() { python3 -c 'import json,sys; a=sys.argv; print(json.dumps({"record":{"id":"","type":a[1],"name":a[2],"content":a[3],"ttl":int(a[4]) if len(a)>4 else 3600,"priority":int(a[5]) if len(a)>5 and a[5] else None,"proxied":(a[6]=="1") if len(a)>6 else None}}))' "$@"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Kurulum"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="dns")')" != True ]; then
  check "DNS sunucusu kurulumu" "$(wait_job "$(api POST /components/dns/install | jget 'd["job_id"]')")" success
else
  check "yapılandırma" "$(code POST /dns/setup)" 200
fi
api GET /dns | jget '(d["local_installed"], d["local_running"], d["server_ip"], d["available_domains"])'
for old in $(api GET /dns | jget '" ".join(str(z["id"]) for z in d["zones"])'); do api DELETE /dns/zones/$old >/dev/null; done
IP=$(api GET /dns | jget 'd["server_ip"]')

say "Yerel bölge ve otomatik kayıtlar"
check "geçersiz alan adı" "$(code POST /dns/zones -d '{"domain":"../x","provider":"local"}')" 400
R=$(api POST /dns/zones -d '{"domain":"deneme.test","provider":"local"}')
Z=$(echo "$R" | jget 'd["zone"]["id"]')
echo "$R" | jget '"\n".join("    " + line for line in d["report"])'
check "aynı bölge ikinci kez" "$(code POST /dns/zones -d '{"domain":"deneme.test","provider":"local"}')" 409
check "kök A kaydı" "$(q A deneme.test)" "$IP"
check "www A kaydı" "$(q A www.deneme.test)" "$IP"
check "alt alan adı sitesinin A kaydı" "$(q A uygulama.deneme.test)" "$IP"
has "ad sunucuları" "$(q NS deneme.test)" "ns1.deneme.test."
check "SOA var" "$(q SOA deneme.test | awk '{print $1}')" "ns1.deneme.test."
has "posta: MX" "$(q MX deneme.test)" "10 "
has "posta: SPF" "$(q TXT deneme.test)" "v=spf1 mx ~all"
has "posta: DKIM (uzun kayıt parçalı yayınlanıyor)" "$(q TXT pix._domainkey.deneme.test)" "v=DKIM1; k=rsa; p="
check "DKIM değeri panel kaydıyla aynı" "$(q TXT pix._domainkey.deneme.test | sed 's/" "//g; s/"//g')" "$(api GET /mail | jget 'next(x["dkim_record"] for x in d["domains"] if x["domain"]=="deneme.test")')"
has "posta: DMARC" "$(q TXT _dmarc.deneme.test)" "v=DMARC1"
MD=$(api GET /mail | jget 'next(x["id"] for x in d["domains"] if x["domain"]=="deneme.test")')
check "posta sayfası bölgeyi yönetilen görüyor" "$(api GET /mail/domains/$MD | jget 'd["dns_managed"]')" True
has "posta kayıtlarını yeniden uygula" "$(api POST /mail/domains/$MD/dns/apply | jget '" | ".join(d["report"])')" "zaten doğru"

say "Kayıt işlemleri"
check "A ekle" "$(code POST /dns/zones/$Z/records -d "$(rec A api 203.0.113.7 300)")" 200
check "yayınlandı" "$(q A api.deneme.test)" "203.0.113.7"
check "aynı ada ikinci A" "$(code POST /dns/zones/$Z/records -d "$(rec A api 203.0.113.8 300)")" 200
check "iki değer birlikte" "$(q A api.deneme.test | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//')" "203.0.113.7 203.0.113.8"
check "aynı kayıt ikinci kez" "$(code POST /dns/zones/$Z/records -d "$(rec A api 203.0.113.7 300)")" 409
check "güncelle" "$(code PUT /dns/zones/$Z/records -d "{\"old\":$(rec A api 203.0.113.8 300 | jget 'json.dumps(d["record"])'),\"new\":$(rec A api 203.0.113.9 600 | jget 'json.dumps(d["record"])')}")" 200
check "güncellendi" "$(q A api.deneme.test | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//')" "203.0.113.7 203.0.113.9"
check "sil" "$(code POST /dns/zones/$Z/records/delete -d "$(rec A api 203.0.113.7 300)")" 200
check "silindi" "$(q A api.deneme.test)" "203.0.113.9"
check "CNAME" "$(code POST /dns/zones/$Z/records -d "$(rec CNAME blog hedef.ornek.net 3600)")" 200
check "CNAME yayınlandı" "$(q CNAME blog.deneme.test)" "hedef.ornek.net."
check "tırnaklı TXT" "$(code POST /dns/zones/$Z/records -d "$(rec TXT dogrulama 'site-verification="abc\def"; x' 3600)")" 200
check "TXT bozulmadan yayınlandı" "$(q TXT dogrulama.deneme.test)" '"site-verification=\"abc\\def\"; x"'
check "SRV" "$(code POST /dns/zones/$Z/records -d "$(rec SRV _sip._tcp '10 5060 sip.deneme.test' 3600 5)")" 200
check "SRV yayınlandı" "$(q SRV _sip._tcp.deneme.test)" "5 10 5060 sip.deneme.test."
check "CAA" "$(code POST /dns/zones/$Z/records -d "$(rec CAA @ '0 issue "letsencrypt.org"' 3600)")" 200
check "joker A" "$(code POST /dns/zones/$Z/records -d "$(rec A '*.alt' 203.0.113.50 3600)")" 200
check "joker yanıt veriyor" "$(q A herhangi.alt.deneme.test)" "203.0.113.50"
check "geçersiz IP" "$(code POST /dns/zones/$Z/records -d "$(rec A x 999.1.1.1)")" 400
check "boşluklu ad" "$(code POST /dns/zones/$Z/records -d "$(rec A 'a b' 1.2.3.4)")" 400
check "bölge dışı ad bölgeye eklenir" "$(api POST /dns/zones/$Z/records -d "$(rec A baska.com 1.2.3.4)" | jget 'any(r["name"]=="baska.com.deneme.test" for r in d["records"])')" True
check "satır sonlu TXT" "$(code POST /dns/zones/$Z/records -d '{"record":{"id":"","type":"TXT","name":"x","content":"a\nb","ttl":3600}}')" 400
check "köke CNAME" "$(code POST /dns/zones/$Z/records -d "$(rec CNAME @ hedef.ornek.net)")" 400
check "desteklenmeyen tür" "$(code POST /dns/zones/$Z/records -d "$(rec PTR x y.ornek.net)")" 400
api POST /dns/zones/$Z/records/delete -d "$(rec NS @ ns2.deneme.test 3600)" >/dev/null
check "son ad sunucusu kaydı silinemez" "$(code POST /dns/zones/$Z/records/delete -d "$(rec NS @ ns1.deneme.test 3600)")" 400

say "Yeni site ve posta alan adında otomatik kayıt"
OLD=$(api GET /sites | jget 'next((s["id"] for s in d if s["domain"]=="yeni.deneme.test"), "")'); [ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
NS=$(api POST /sites -d '{"domain":"yeni.deneme.test","kind":"static"}' | jget 'd["id"]')
check "yeni sitenin A kaydı kendiliğinden eklendi" "$(q A yeni.deneme.test)" "$IP"
api DELETE "/sites/$NS?delete_files=true" >/dev/null

say "DNS ile sertifika doğrulaması (joker sertifika)"
# ACME sınama sunucusu (Pebble) yalnızca geliştirme makinesinde kurulu; yoksa bölüm atlanır.
if [ -f /etc/systemd/system/pebble-test.service ]; then
# Pebble doğrulama sorgularını yerel DNS sunucusuna yöneltir.
PEBBLE=$(grep ^ExecStart /etc/systemd/system/pebble-test.service)
if ! grep -q dnsserver <<<"$PEBBLE"; then sed -i 's|^ExecStart=\(.*\)$|ExecStart=\1 -dnsserver 127.0.0.1:53|' /etc/systemd/system/pebble-test.service; systemctl daemon-reload; systemctl restart pebble-test; sleep 3; fi
check "bölgesi olmayan sitede DNS doğrulaması reddedilir" "$(code POST /sites/2/ssl/letsencrypt -d '{"wildcard":true}')" 400
JOB=$(api POST /sites/1/ssl/letsencrypt -d '{"wildcard":true}' | jget 'd["job_id"]')
check "joker sertifika alındı" "$(wait_job "$JOB")" success
api GET "/jobs/$JOB" | jget '"\n".join("    " + l for l in d["log"].splitlines()[-6:])'
has "sertifika joker adı içeriyor" "$(openssl x509 -in /etc/pixpanel/ssl/deneme.test/fullchain.pem -noout -ext subjectAltName 2>/dev/null)" "DNS:*.deneme.test"
check "doğrulama kaydı temizlendi" "$(q TXT _acme-challenge.deneme.test)" ""
check "site kipi kaydedildi" "$(api GET /sites/1 | jget 'd["ssl_mode"]')" letsencrypt_wildcard
check "site yeni sertifikayla yayında" "$(curl -sk -o /dev/null -w '%{http_code}' --resolve deneme.test:443:127.0.0.1 https://deneme.test/)" 200
# Sonraki sınamalar için Pebble eski hâline döner.
sed -i 's| -dnsserver 127.0.0.1:53||' /etc/systemd/system/pebble-test.service; systemctl daemon-reload; systemctl restart pebble-test
else echo "  (Pebble yok; bölüm atlandı)"; fi

say "Cloudflare (sahte API)"
pkill -f "python3 $HERE/[m]ock_cloudflare" 2>/dev/null; : > /tmp/px-cf.log
(setsid python3 "$HERE/mock_cloudflare.py" >/dev/null 2>&1 &); sleep 1
api DELETE /dns/cloudflare >/dev/null
check "biçimi bozuk anahtar" "$(code PUT /dns/cloudflare -d '{"token":"x y"}')" 400
check "Cloudflare'in reddettiği anahtar" "$(code PUT /dns/cloudflare -d '{"token":"yanlis_anahtar_0123456789abcdefghij"}')" 400
check "geçerli anahtar" "$(code PUT /dns/cloudflare -d "{\"token\":\"$CF_TOKEN\"}")" 200
api GET /dns/cloudflare | jget '(d["connected"], [(z["name"], z["linked"]) for z in d["zones"]])'
check "anahtar yanıtlarda görünmüyor" "$(api GET /dns/cloudflare | grep -c "$CF_TOKEN")" 0
check "hesapta olmayan bölge" "$(code POST /dns/zones -d '{"domain":"yok.test","provider":"cloudflare"}')" 400
CZ=$(api POST /dns/zones -d '{"domain":"bulut.test","provider":"cloudflare"}' | jget 'd["zone"]["id"]')
check "vekilli A ekle" "$(code POST /dns/zones/$CZ/records -d "$(rec A @ 203.0.113.20 3600 '' 1)")" 200
check "MX ekle" "$(code POST /dns/zones/$CZ/records -d "$(rec MX @ posta.bulut.test 3600 10)")" 200
check "SRV panelden yazılamaz" "$(code POST /dns/zones/$CZ/records -d "$(rec SRV _sip._tcp '10 5060 sip.bulut.test' 3600 5)")" 400
D=$(api GET /dns/zones/$CZ)
echo "$D" | jget '[(r["type"], r["name"], r["content"], r["ttl"], r["proxied"], r["priority"]) for r in d["records"]]'
echo "$D" | jget '("ayarlar:", d["cloudflare"]["ssl_mode"], d["cloudflare"]["development_mode"], d["nameservers"])'
A=$(echo "$D" | jget 'json.dumps(next(r for r in d["records"] if r["type"]=="A"))')
N=$(echo "$A" | jget 'd.update(content="203.0.113.21", proxied=False) or json.dumps(d)')
check "kaydı güncelle" "$(code PUT /dns/zones/$CZ/records -d "{\"old\":$A,\"new\":$N}")" 200
check "SSL kipi" "$(code POST /dns/zones/$CZ/cloudflare -d '{"ssl_mode":"strict","development_mode":true}')" 200
check "geçersiz SSL kipi" "$(code POST /dns/zones/$CZ/cloudflare -d '{"ssl_mode":"x"}')" 400
check "önbelleği temizle" "$(code POST /dns/zones/$CZ/purge)" 200
check "yerel bölgede Cloudflare işlemi" "$(code POST /dns/zones/$Z/purge)" 400
check "kaydı sil" "$(code POST /dns/zones/$CZ/records/delete -d "{\"record\":$N}")" 200
sed 's/^/    /' /tmp/px-cf.log
check "bağlı bölge varken bağlantı kesilemez" "$(code DELETE /dns/cloudflare)" 409
check "bölge bağlantısını kaldır" "$(code DELETE /dns/zones/$CZ)" 200
check "bağlantıyı kes" "$(code DELETE /dns/cloudflare)" 200

say "Gerçek ziyaretçi adresi ve ayarlar"
S='{"server_ip":"203.0.113.99","nameservers":["ns1.pixhost.test","ns2.pixhost.test"],"cloudflare_real_ip":true}'
check "ayar kaydı" "$(code PUT /dns/settings -d "$S")" 200
check "nginx dosyası yazıldı" "$(grep -c '^set_real_ip_from' /etc/nginx/conf.d/pixpanel-cloudflare.conf)" 11
check "başlık tanımlı" "$(grep -c 'real_ip_header CF-Connecting-IP;' /etc/nginx/conf.d/pixpanel-cloudflare.conf)" 1
check "güvenilmeyen kaynaktan gelen başlık yok sayılır" "$(curl -s -o /dev/null -H 'Host: statik.test' -H 'CF-Connecting-IP: 6.6.6.6' http://127.0.0.1/; tail -1 /var/log/nginx/statik.test.access.log | awk '{print $1}')" "127.0.0.1"
check "tek ad sunucusu" "$(code PUT /dns/settings -d '{"server_ip":"","nameservers":["ns1.pixhost.test"],"cloudflare_real_ip":false}')" 400
check "geçersiz sunucu adresi" "$(code PUT /dns/settings -d '{"server_ip":"abc","nameservers":[],"cloudflare_real_ip":false}')" 400
check "ayarları sıfırla" "$(code PUT /dns/settings -d '{"server_ip":"","nameservers":[],"cloudflare_real_ip":false}')" 200
check "nginx dosyası kaldırıldı" "$(ls /etc/nginx/conf.d/pixpanel-cloudflare.conf 2>/dev/null | wc -l)" 0
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/dns)" 401
pkill -f "python3 $HERE/[m]ock_cloudflare" 2>/dev/null
echo "bitti (deneme.test bölgesi sonraki denemeler için yerinde bırakıldı)"
