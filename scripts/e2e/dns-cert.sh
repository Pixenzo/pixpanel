#!/usr/bin/env bash
# Yalnızca DNS ile sertifika doğrulaması bölümü (dns.sh'in bir parçası): yerel bölgeler (PowerDNS), otomatik kayıtlar, DNS ile
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
say "DNS ile sertifika doğrulaması (joker sertifika)"
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

