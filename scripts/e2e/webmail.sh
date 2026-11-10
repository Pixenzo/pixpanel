#!/usr/bin/env bash
# Web postasının (Roundcube) sınaması: panel üzerinden erişim ve bir kutuyla oturum açma.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash webmail.sh
set -uo pipefail
P=https://127.0.0.1:8443
B=$P/api/v1
J=/tmp/pxjar-webmail
W=/tmp/pxjar-roundcube
BOX_PASS='Kutu-Parola-13579'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 240)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }

rm -f $J $W
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
check "panel web postasını kurulu görüyor" "$(api GET /mail | jget 'd["webmail"]')" True

# Sınama kutusu
OLD=$(api GET /mail | jget 'next((x["id"] for x in d["domains"] if x["domain"]=="webposta.test"), "")')
[ -n "$OLD" ] && api DELETE "/mail/domains/$OLD?delete_mail=true" >/dev/null
D=$(api POST /mail/domains -d '{"domain":"webposta.test"}' | jget 'd["id"]')
api POST /mail/domains/$D/mailboxes -d "{\"local\":\"ayse\",\"password\":\"$BOX_PASS\"}" >/dev/null
sleep 1

check "sondaki eğik çizgisiz adres yönlenir" "$(curl -sk -o /dev/null -w '%{http_code}' $P/webmail)" 308
PAGE=$(curl -sk -c $W "$P/webmail/")
has "giriş sayfası panel oturumu olmadan açılıyor" "$PAGE" "PixPanel Web Postası"
check "yapılandırma klasörü kapalı" "$(curl -sk -o /dev/null -w '%{http_code}' $P/webmail/config/config.inc.php)" 404
check "stil dosyaları sunuluyor" "$(curl -sk -o /dev/null -w '%{http_code}' "$P/webmail/$(grep -o 'skins/elastic/[a-z/._-]*\.css[^"]*' <<<"$PAGE" | head -1)")" 200
TOKEN=$(grep -o 'name="_token" value="[^"]*"' <<<"$PAGE" | cut -d'"' -f4)

BAD=$(curl -sk -b $W -c $W -o /dev/null -w '%{http_code}' "$P/webmail/?_task=login" --data-urlencode "_token=$TOKEN" -d '_task=login&_action=login&_timezone=Europe/Istanbul' --data-urlencode '_user=ayse@webposta.test' --data-urlencode '_pass=yanlis-parola')
check "yanlış parola reddedilir" "$BAD" 401

PAGE=$(curl -sk -c $W "$P/webmail/"); TOKEN=$(grep -o 'name="_token" value="[^"]*"' <<<"$PAGE" | cut -d'"' -f4)
LOGIN=$(curl -sk -b $W -c $W -o /dev/null -w '%{http_code} %{redirect_url}' "$P/webmail/?_task=login" --data-urlencode "_token=$TOKEN" -d '_task=login&_action=login&_timezone=Europe/Istanbul' --data-urlencode '_user=ayse@webposta.test' --data-urlencode "_pass=$BOX_PASS")
has "doğru parolayla oturum açılır" "$LOGIN" "302"
has "posta görünümüne yönlenir" "$LOGIN" "_task=mail"
INBOX=$(curl -sk -b $W -c $W "$P/webmail/?_task=mail&_mbox=INBOX")
has "gelen kutusu açıldı" "$INBOX" "ayse@webposta.test"
check "panelin oturum çerezi web postasına sızmıyor" "$(grep -c pixpanel_session $W)" 0

api DELETE "/mail/domains/$D?delete_mail=true" >/dev/null
echo "bitti"
