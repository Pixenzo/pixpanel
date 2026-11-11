#!/usr/bin/env bash
# Site şablonlarının sınaması: şablon değişince web sunucusunun davranışı değişir.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash templates.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-tpl
STATIC=sablon-statik.test
PHP=sablon-php.test
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
# get <alan adı> <yol>: sitenin o yoldaki yanıtının durum kodu ve gövdesi.
status() { curl -s -o /dev/null -w '%{http_code}' -H "Host: $1" "http://127.0.0.1$2"; }
body() { curl -s -H "Host: $1" "http://127.0.0.1$2"; }
template() { api PATCH "/sites/$1" -d "{\"template\":\"$2\"}" | jget 'd["template"]'; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in $STATIC $PHP; do
  OLD=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$name'), '')")
  [ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
done
PHP_VERSION=$(api GET /sites/options | jget 'd["php_versions"][0]')

say "Durağan site"
S=$(api POST /sites -d "{\"domain\":\"$STATIC\",\"kind\":\"static\"}" | jget 'd["id"]')
ROOT=/home/$PX_USER/sites/$STATIC/public
echo "ana sayfa" > $ROOT/index.html; echo "hakkında sayfası" > $ROOT/hakkinda.html; chown $PX_USER:www-data $ROOT/*.html
check "yeni site varsayılan şablonla açılır" "$(api GET /sites/$S | jget 'd["template"]')" default
check "varsayılan: uzantısız adres bulunamaz" "$(status $STATIC /hakkinda)" 404
check "varsayılan: olmayan adres 404" "$(status $STATIC /uygulama/yol)" 404
check "temiz adresler şablonu" "$(template $S clean_urls)" clean_urls
check "temiz adresler: /hakkinda, hakkinda.html dosyasını sunar" "$(body $STATIC /hakkinda)" "hakkında sayfası"
check "temiz adresler: olmayan adres yine 404" "$(status $STATIC /yok)" 404
check "tek sayfalık uygulama şablonu" "$(template $S spa)" spa
check "tek sayfalık uygulama: bilinmeyen adres uygulamaya düşer" "$(body $STATIC /uygulama/yol)" "ana sayfa"
check "tek sayfalık uygulama: var olan dosya yine kendisi" "$(body $STATIC /hakkinda.html)" "hakkında sayfası"
check "PHP şablonu durağan sitede kullanılamaz" "$(code PATCH /sites/$S -d '{"template":"wordpress"}')" 400
check "olmayan şablon" "$(code PATCH /sites/$S -d '{"template":"../nginx"}')" 400
check "reddedilen şablon öncekini bozmaz" "$(api GET /sites/$S | jget 'd["template"]') $(body $STATIC /uygulama/yol)" "spa ana sayfa"
check "varsayılana dönüş" "$(template $S default) $(status $STATIC /uygulama/yol)" "default 404"

say "PHP sitesi"
P=$(api POST /sites -d "{\"domain\":\"$PHP\",\"kind\":\"php\",\"php_version\":\"$PHP_VERSION\"}" | jget 'd["id"]')
PROOT=/home/$PX_USER/sites/$PHP/public
mkdir -p $PROOT/wp-content/uploads $PROOT/storage
echo '<?php echo "calisti";' > $PROOT/wp-content/uploads/kabuk.php
echo '<?php echo "xmlrpc";' > $PROOT/xmlrpc.php
echo "gizli günlük" > $PROOT/storage/laravel.log
echo "APP_KEY=gizli" > $PROOT/artisan
chown -R $PX_USER:www-data $PROOT
check "varsayılan: yükleme klasöründeki PHP çalışır (koruma yok)" "$(body $PHP /wp-content/uploads/kabuk.php)" calisti
check "WordPress şablonu" "$(template $P wordpress)" wordpress
check "WordPress: yükleme klasöründeki PHP çalışmaz" "$(status $PHP /wp-content/uploads/kabuk.php)" 403
check "WordPress: xmlrpc kapalı" "$(status $PHP /xmlrpc.php)" 403
check "WordPress: öteki PHP dosyaları çalışır" "$(echo '<?php echo "tamam";' > $PROOT/index.php; chown $PX_USER:www-data $PROOT/index.php; body $PHP /)" tamam
check "Laravel şablonu" "$(template $P laravel)" laravel
check "Laravel: iç klasörler sunulmaz" "$(status $PHP /storage/laravel.log) $(status $PHP /artisan)" "404 404"
check "Laravel: xmlrpc kuralı artık yok" "$(body $PHP /xmlrpc.php)" xmlrpc
check "durağan şablon PHP sitesinde kullanılamaz" "$(code PATCH /sites/$P -d '{"template":"spa"}')" 400
check "site türü değişince geçersiz kalan şablon varsayılan gibi davranır" "$(code PATCH /sites/$P -d '{"kind":"static"}') $(status $PHP /storage/laravel.log)" "200 200"

say "Temizlik"
check "siteleri sil" "$(code DELETE "/sites/$S?delete_files=true") $(code DELETE "/sites/$P?delete_files=true")" "200 200"
echo "bitti"
