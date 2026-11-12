#!/usr/bin/env bash
# Site başarım ayarları sınaması: tarayıcı önbellek başlıkları, sunucu önbelleği (PHP'de FastCGI,
# vekil sitede vekil önbelleği; HIT/MISS/BYPASS, temizleme, sızıntı koruması) ve "yalnızca Cloudflare" kipi.
# Test makinesinde root olarak çalışır. Kullanım: PX_USER=... PX_PASS=... bash site-perf.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-perf
JO=/tmp/pxjar-perf-other
STATIC=perf-static.test
PHP=perf-php.test
PROXY=perf-proxy.test
OTHER=perfkomsu
OTHER_PASS='Perf-Komsu-Parola-135'
BACKEND_PORT=18931
BACKEND_PY=/tmp/px-perf-backend.py
CACHE_ROOT=/var/cache/nginx/pixpanel
ACME_FILE=/var/lib/pixpanel/acme/.well-known/acme-challenge/perf-sinama

call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 200)]" ;; esac; }
differs() { if [ "$2" != "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: iki değer aynı: [$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }

# İstekler web sunucusuna yerel adresten, Host başlığıyla yapılır. Ek curl seçenekleri sona yazılır.
st() { local host=$1 path=$2; shift 2; curl -s -o /dev/null -w '%{http_code}' -H "Host: $host" "$@" "http://127.0.0.1$path"; }
body() { local host=$1 path=$2; shift 2; curl -s -H "Host: $host" "$@" "http://127.0.0.1$path"; }
# Yanıt başlıklarının tamamı (satır sonları temizlenmiş).
heads() { local host=$1 path=$2; shift 2; curl -s -D - -o /dev/null -H "Host: $host" "$@" "http://127.0.0.1$path" | tr -d '\r'; }
# Tek bir yanıt başlığının değeri (yoksa boş).
hv() { local name=$1; shift; heads "$@" | awk -F': ' -v n="$name" 'tolower($1)==tolower(n){print $2}'; }
xcache() { hv X-Cache "$@"; }
# patch <site no> <kurallar JSON>: kuralların tamamını yazar.
rules() { api PATCH "/sites/$1" -d "{\"rules\":$2}"; }
rcode() { code PATCH "/sites/$1" -d "{\"rules\":$2}"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
settle() { sleep 0.4; }

BACKEND_PID=
cleanup_user() {
  local id; id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$OTHER'), '')")
  [ -n "$id" ] && api DELETE /users/$id >/dev/null
}
finish() {
  [ -n "$BACKEND_PID" ] && kill $BACKEND_PID 2>/dev/null
  rm -f $BACKEND_PY $ACME_FILE $J $JO
}
trap finish EXIT

rm -f $J $JO
login $J "$PX_USER" "$PX_PASS" >/dev/null
for name in $STATIC $PHP $PROXY; do
  OLD=$(site_id $name)
  [ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
done
cleanup_user

say "Hazırlık"
PHP_VERSION=$(api GET /sites/options | jget 'd["php_versions"][0]')
SS=$(api POST /sites -d "{\"domain\":\"$STATIC\",\"kind\":\"static\"}" | jget 'd["id"]')
SP=$(api POST /sites -d "{\"domain\":\"$PHP\",\"kind\":\"php\",\"php_version\":\"$PHP_VERSION\"}" | jget 'd["id"]')

SROOT=/home/$PX_USER/sites/$STATIC/public
echo ana > $SROOT/index.html; echo 'body{}' > $SROOT/stil.css
chown -R $PX_USER:www-data $SROOT
PROOT=/home/$PX_USER/sites/$PHP/public
mkdir -p $PROOT/wp-admin
echo '<?php echo "sayfa-" . microtime(true) . "-" . mt_rand();' > $PROOT/sayfa.php
echo '<?php echo "yonetim-" . microtime(true) . "-" . mt_rand();' > $PROOT/wp-admin/index.php
echo '<?php echo "giris-" . microtime(true) . "-" . mt_rand();' > $PROOT/wp-login.php
echo '<?php setcookie("ziyaret", "1"); echo "cerezli-" . microtime(true) . "-" . mt_rand();' > $PROOT/cerez.php
echo '<?php header("Cache-Control: private"); echo "ozel-" . microtime(true) . "-" . mt_rand();' > $PROOT/ozel.php
echo '<?php echo "ana-" . microtime(true) . "-" . mt_rand();' > $PROOT/index.php
echo 'body{}' > $PROOT/stil.css
chown -R $PX_USER:www-data $PROOT
mkdir -p /var/lib/pixpanel/acme/.well-known/acme-challenge && echo dogrulama > $ACME_FILE

cat > $BACKEND_PY <<'PYEOF'
import http.server, itertools

counter = itertools.count(1)

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        text = ("n=%d path=%s" % (next(counter), self.path)).encode()
        self.send_response(200)
        if self.path.startswith("/gizli"):
            self.send_header("Cache-Control", "private")
        elif self.path.startswith("/cerez"):
            self.send_header("Set-Cookie", "x=1; Path=/")
        elif self.path.endswith(".css"):
            self.send_header("Cache-Control", "max-age=5")
            self.send_header("Content-Type", "text/css")
        self.send_header("Content-Length", str(len(text)))
        self.end_headers()
        self.wfile.write(text)

    do_GET = reply
    do_POST = reply

http.server.ThreadingHTTPServer(("127.0.0.1", 18931), Handler).serve_forever()
PYEOF
python3 $BACKEND_PY &
BACKEND_PID=$!
sleep 1
SX=$(api POST /sites -d "{\"domain\":\"$PROXY\",\"kind\":\"proxy\",\"proxy_target\":\"http://127.0.0.1:$BACKEND_PORT\"}" | jget 'd["id"]')
check "üç site açıldı" "$([ -n "$SS" ] && [ -n "$SP" ] && [ -n "$SX" ] && echo evet)" evet
check "yeni sitede ayarlar kapalı" "$(api GET /sites/$SP | jget '(d["rules"]["browser_cache"], d["rules"]["server_cache"]["enabled"], d["rules"]["cloudflare_only"])')" "('', False, False)"

say "Tarayıcı önbellek başlıkları (durağan site)"
check "varsayılan 30 gün" "$(hv Cache-Control $STATIC /stil.css)" "max-age=2592000"
check "1 saat" "$(rcode $SS '{"browser_cache":"1h"}') $(hv Cache-Control $STATIC /stil.css)" "200 max-age=3600"
check "1 gün" "$(rcode $SS '{"browser_cache":"1d"}') $(hv Cache-Control $STATIC /stil.css)" "200 max-age=86400"
check "7 gün" "$(rcode $SS '{"browser_cache":"7d"}') $(hv Cache-Control $STATIC /stil.css)" "200 max-age=604800"
check "1 yıl" "$(rcode $SS '{"browser_cache":"1y"}') $(hv Cache-Control $STATIC /stil.css)" "200 max-age=31536000"
has "Expires başlığı da gelir" "$(heads $STATIC /stil.css)" "Expires:"
check "kapalı: süre bildirilmez" "$(rcode $SS '{"browser_cache":"off"}') [$(hv Cache-Control $STATIC /stil.css)]" "200 []"
check "sayfalar etkilenmez" "$(body $STATIC /)" ana
check "sunucu önbelleği durağan sitede açılamaz" "$(rcode $SS '{"server_cache":{"enabled":true,"ttl":"10m"}}')" 400

say "Geçersiz değerler ve geri alma (durağan site)"
check "ayarla" "$(rcode $SS '{"browser_cache":"7d"}')" 200
for bad in '7 gün' '1w' '7' 'off; return 200' '$host' '1h"'; do
  check "geçersiz tarayıcı süresi [$bad]" "$(rcode $SS "{\"browser_cache\":\"$bad\"}")" 400
done
check "reddedilen istek öncekini bozmaz" "$(hv Cache-Control $STATIC /stil.css) $(api GET /sites/$SS | jget 'd["rules"]["browser_cache"]')" "max-age=604800 7d"
check "yapılandırma geçerli" "$(nginx -t 2>&1 | grep -c 'test is successful')" 1

say "Tarayıcı önbelleği (PHP sitesi)"
check "7 gün" "$(rcode $SP '{"browser_cache":"7d"}') $(hv Cache-Control $PHP /stil.css)" "200 max-age=604800"

say "FastCGI önbelleği: HIT ve MISS"
check "geçersiz süre reddedilir" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"2m"}}') $(rcode $SP '{"server_cache":{"enabled":true,"ttl":"10m; x"}}') $(rcode $SP '{"server_cache":{"enabled":true,"ttl":"10"}}')" "400 400 400"
check "kapalıyken X-Cache başlığı yok" "[$(xcache $PHP /sayfa.php)]" "[]"
check "aç" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"10m"},"browser_cache":"7d"}')" 200
check "süre verilmezse 10 dakika" "$(rcode $SP '{"server_cache":{"enabled":true},"browser_cache":"7d"}') $(api GET /sites/$SP | jget 'd["rules"]["server_cache"]["ttl"]')" "200 10m"
check "önbellek klasörü site için açıldı (yalnız web sunucusu kullanıcısı)" "$(stat -c '%a %U' $CACHE_ROOT/$SP)" "700 www-data"
check "ilk istek MISS" "$(xcache $PHP /sayfa.php)" MISS
settle
A=$(body $PHP /sayfa.php)
check "ikinci istek HIT" "$(xcache $PHP /sayfa.php)" HIT
check "önbellekten gelen içerik aynı" "$(body $PHP /sayfa.php)" "$A"
check "adres ayrı önbelleklenir (sorgu dizgesi)" "$(xcache $PHP '/sayfa.php?a=1')" MISS
settle
check "aynı sorgu HIT" "$(xcache $PHP '/sayfa.php?a=1')" HIT
check "kök adres (index.php) önbelleklenir" "$(xcache $PHP /) $(sleep 0.4; xcache $PHP /)" "MISS HIT"
check "ek istek başlıkları anahtarı değiştirmez" "$(xcache $PHP /sayfa.php -H 'X-Deneme: 1')" HIT
check "durağan dosyalar önbellek başlığı taşımaz (doğrudan sunulur)" "[$(xcache $PHP /stil.css)]" "[]"
check "tarayıcı önbelleği hâlâ geçerli" "$(hv Cache-Control $PHP /stil.css)" "max-age=604800"

say "FastCGI önbelleği: önbelleğe alınmayacaklar"
check "POST: BYPASS" "$(xcache $PHP /sayfa.php -X POST -d a=1)" BYPASS
check "PHPSESSID çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: PHPSESSID=abc123')" BYPASS
check "wordpress_logged_in_ çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: wordpress_logged_in_abc=kullanici%7C123')" BYPASS
check "wp-postpass_ çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: wp-postpass_abc=x')" BYPASS
check "comment_author_ çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: comment_author_abc=ali')" BYPASS
check "woocommerce_ çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: woocommerce_items_in_cart=1')" BYPASS
check "laravel_session çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: laravel_session=abc')" BYPASS
check "çerezlerden biri oturum çerezi ise: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: tema=koyu; PHPSESSID=abc')" BYPASS
check "büyük harfli oturum çerezi: BYPASS" "$(xcache $PHP /sayfa.php -H 'Cookie: phpsessid=abc')" BYPASS
differs "çerezli istek önbellekteki kopyayı almaz" "$(body $PHP /sayfa.php -H 'Cookie: PHPSESSID=abc')" "$A"
check "Authorization başlığı: BYPASS" "$(xcache $PHP /sayfa.php -H 'Authorization: Bearer abc')" BYPASS
check "API anahtarı başlığı: BYPASS" "$(xcache $PHP /sayfa.php -H 'X-API-Key: abc')" BYPASS
check "/wp-admin/: BYPASS" "$(xcache $PHP /wp-admin/index.php)" BYPASS
check "/wp-login.php: BYPASS" "$(xcache $PHP /wp-login.php)" BYPASS
check "büyük harfli /WP-ADMIN/: BYPASS" "$(xcache $PHP /WP-ADMIN/index.php)" BYPASS
check "wc-ajax: BYPASS" "$(xcache $PHP '/sayfa.php?wc-ajax=get_refreshed_fragments')" BYPASS
check "zararsız çerez önbelleği engellemez" "$(xcache $PHP /sayfa.php -H 'Cookie: tema=koyu')" HIT
C1=$(body $PHP /cerez.php); C2=$(body $PHP /cerez.php)
check "Set-Cookie dönen yanıt önbelleğe alınmaz (MISS)" "$(xcache $PHP /cerez.php) $(xcache $PHP /cerez.php)" "MISS MISS"
differs "Set-Cookie dönen yanıt her seferinde yeniden üretilir" "$C1" "$C2"
check "Cache-Control: private yanıt önbelleğe alınmaz" "$(xcache $PHP /ozel.php) $(xcache $PHP /ozel.php)" "MISS MISS"
differs "özel yanıt başkasına verilmez" "$(body $PHP /ozel.php)" "$(body $PHP /ozel.php)"

say "FastCGI önbelleği: temizleme"
BEFORE=$(body $PHP /sayfa.php)
check "temizle" "$(code POST /sites/$SP/cache/purge)" 200
check "klasör yerinde kalır, içi boşalır" "$(test -d $CACHE_ROOT/$SP && echo var) $(find $CACHE_ROOT/$SP -type f | wc -l)" "var 0"
check "temizlikten sonra MISS" "$(xcache $PHP /sayfa.php)" MISS
settle
check "sonra yine HIT" "$(xcache $PHP /sayfa.php)" HIT
differs "içerik yeniden üretildi" "$BEFORE" "$(body $PHP /sayfa.php)"
check "denetim günlüğüne işlendi" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='site.cache.purge' AND target='$PHP'")" 1
check "önbelleği kapalı sitede temizleme zararsız" "$(code POST /sites/$SS/cache/purge)" 200

say "Yetki"
check "oturumsuz temizleme 401" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/sites/$SP/cache/purge -d '{}')" 401
check "komşu hesap oluştu" "$(code POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$OTHER_PASS\",\"role\":\"user\"}") $(login $JO $OTHER "$OTHER_PASS")" "200 200"
check "başka hesap temizleyemez (404)" "$(status $JO POST /sites/$SP/cache/purge -d '{}')" 404
check "başka hesap ayarı değiştiremez (404)" "$(status $JO PATCH /sites/$SP -d '{"rules":{"server_cache":{"enabled":false}}}')" 404
check "temizlik komşu isteğinden etkilenmedi" "$(xcache $PHP /sayfa.php)" HIT
check "geçersiz site numarası" "$(code POST /sites/999999/cache/purge)" 404
check "sayı olmayan site numarası" "$(code POST /sites/abc/cache/purge)" 400

say "Geçersiz istek öncekini bozmaz"
check "geçersiz süre" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"3h"},"browser_cache":"1d"}')" 400
check "ayar ve önbellek bozulmadı" "$(api GET /sites/$SP | jget '(d["rules"]["server_cache"]["ttl"], d["rules"]["browser_cache"])') $(xcache $PHP /sayfa.php)" "('10m', '7d') HIT"
check "geçersiz tarayıcı süresi" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"10m"},"browser_cache":"2y"}')" 400
check "yapılandırma geçerli" "$(nginx -t 2>&1 | grep -c 'test is successful')" 1
check "süre değişir (1 dakika)" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"1m"},"browser_cache":"7d"}') $(grep -c 'fastcgi_cache_valid 200 301 302 1m' /etc/nginx/pixpanel/sites/$PHP.conf)" "200 1"

say "Vekil önbelleği"
check "aç" "$(rcode $SX '{"server_cache":{"enabled":true,"ttl":"10m"}}')" 200
check "vekil önbellek klasörü açıldı" "$(test -d $CACHE_ROOT/$SX && echo var)" var
check "ilk istek MISS" "$(xcache $PROXY /sayfa)" MISS
settle
P1=$(body $PROXY /sayfa)
check "ikinci istek HIT" "$(xcache $PROXY /sayfa)" HIT
check "uygulamaya tek kez gidildi (içerik aynı)" "$(body $PROXY /sayfa)" "$P1"
check "POST: BYPASS" "$(xcache $PROXY /sayfa -X POST -d a=1)" BYPASS
check "oturum çerezi: BYPASS" "$(xcache $PROXY /sayfa -H 'Cookie: connect.sid=abc')" BYPASS
check "Authorization: BYPASS" "$(xcache $PROXY /sayfa -H 'Authorization: Bearer abc')" BYPASS
check "API anahtarı: BYPASS" "$(xcache $PROXY /sayfa -H 'X-API-Key: abc')" BYPASS
check "WebSocket yükseltme isteği: BYPASS" "$(xcache $PROXY /sayfa -H 'Connection: Upgrade' -H 'Upgrade: websocket')" BYPASS
check "uygulamanın private yanıtı saklanmaz" "$(xcache $PROXY /gizli) $(xcache $PROXY /gizli)" "MISS MISS"
differs "private yanıt başkasına verilmez" "$(body $PROXY /gizli)" "$(body $PROXY /gizli)"
check "Set-Cookie dönen yanıt saklanmaz" "$(xcache $PROXY /cerez) $(xcache $PROXY /cerez)" "MISS MISS"
check "temizle" "$(code POST /sites/$SX/cache/purge)" 200
check "temizlikten sonra MISS" "$(xcache $PROXY /sayfa)" MISS
differs "uygulamaya yeniden gidildi" "$P1" "$(body $PROXY /sayfa -H 'Cookie: sid=1')"

say "Tarayıcı önbelleği (vekil site)"
has "ayar yokken uygulamanın kendi başlığı geçer" "$(heads $PROXY /a.css)" "max-age=5"
check "1 gün" "$(rcode $SX '{"browser_cache":"1d","server_cache":{"enabled":true,"ttl":"10m"}}') $(hv Cache-Control $PROXY /a.css)" "200 max-age=86400"
check "sayfalara dokunulmaz" "$(heads $PROXY /sayfa | grep -ci 'max-age=86400')" 0
check "durağan dosya da önbelleklenir" "$(xcache $PROXY /b.css) $(sleep 0.4; xcache $PROXY /b.css)" "MISS HIT"
check "sayfa önbelleği çalışmayı sürdürür" "$(xcache $PROXY /sayfa2) $(sleep 0.4; xcache $PROXY /sayfa2)" "MISS HIT"

say "Önbellek kapanınca temizlik"
check "kapat (PHP)" "$(rcode $SP '{"browser_cache":"7d"}')" 200
check "klasör silindi" "$(test -d $CACHE_ROOT/$SP && echo var || echo yok)" yok
check "X-Cache başlığı kalktı" "[$(xcache $PHP /sayfa.php)]" "[]"
check "sayfa yine çalışıyor" "$(st $PHP /sayfa.php)" 200
check "kapat (vekil)" "$(rcode $SX '{}') $(test -d $CACHE_ROOT/$SX && echo var || echo yok)" "200 yok"
check "yapılandırmada önbellek kalmadı" "$(grep -c 'cache_path' /etc/nginx/pixpanel/sites/$PHP.conf /etc/nginx/pixpanel/sites/$PROXY.conf | tr '\n' ' ')" "/etc/nginx/pixpanel/sites/$PHP.conf:0 /etc/nginx/pixpanel/sites/$PROXY.conf:0 "
check "tekrar aç" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"10m"}}') $(xcache $PHP /sayfa.php)" "200 MISS"
check "askıya alınca önbellek kalkar" "$(code PATCH /sites/$SP -d '{"status":"suspended"}') $(test -d $CACHE_ROOT/$SP && echo var || echo yok)" "200 yok"
check "yeniden yayına alınca geri gelir" "$(code PATCH /sites/$SP -d '{"status":"active"}') $(test -d $CACHE_ROOT/$SP && echo var || echo yok)" "200 var"

say "Yalnızca Cloudflare kipi"
NIP=$(hostname -I | tr ' ' '\n' | grep -E '^[0-9]+\.' | grep -v '^127\.' | head -1)
far() { local path=$1; shift; curl -s -o /dev/null -w '%{http_code}' -H "Host: $STATIC" "$@" "http://$NIP$path"; }
if [ -z "$NIP" ]; then
  echo "  ATLANDI sunucuda yerel olmayan bir IPv4 adresi yok"
else
  check "kipten önce yerel olmayan adresten erişilir" "$(far /)" 200
  check "aç" "$(rcode $SS '{"cloudflare_only":true}')" 200
  check "ayar kaydedildi" "$(api GET /sites/$SS | jget 'd["rules"]["cloudflare_only"]')" True
  check "Cloudflare adres aralıkları dosyası yazıldı" "$(grep -c '^[0-9a-f.:]*/[0-9]* 1;$' /etc/nginx/pixpanel/cloudflare-ranges.conf | awk '{print ($1>=10) ? "yeterli" : "az"}')" yeterli
  has "yapılandırma gerçek bağlantı adresine bakar" "$(cat /etc/nginx/pixpanel/sites/$STATIC.conf)" 'geo $realip_remote_addr'
  check "doğrudan sunucu adresinden gelen reddedilir" "$(far /)" 403
  check "sayfa adresleri de reddedilir" "$(far /stil.css)" 403
  check "CF-Connecting-IP başlığı taklit edilse de reddedilir" "$(far / -H 'CF-Connecting-IP: 104.16.0.1')" 403
  check "X-Forwarded-For taklit edilse de reddedilir" "$(far / -H 'X-Forwarded-For: 104.16.0.1')" 403
  check "sertifika doğrulama yolu açık kalır" "$(far /.well-known/acme-challenge/perf-sinama)" 200
  check "doğrulama dosyasının içeriği gelir" "$(curl -s -H "Host: $STATIC" http://$NIP/.well-known/acme-challenge/perf-sinama)" dogrulama
  check "doğrulama yolunun dışı kapalı" "$(far /.well-known/baska)" 403
  check "yerel yoklama (127.0.0.1) açık kalır" "$(st $STATIC /)" 200
  check "yerel yoklama başlık taklidine gerek duymaz" "$(st $STATIC /stil.css)" 200
  check "yapılandırma geçerli" "$(nginx -t 2>&1 | grep -c 'test is successful')" 1
  check "kapat" "$(rcode $SS '{}') $(far /)" "200 200"
  check "kapatınca yapılandırmada kalmadı" "$(grep -c 'realip_remote_addr' /etc/nginx/pixpanel/sites/$STATIC.conf)" 0
  say "Yalnızca Cloudflare: PHP sitesinde ve öteki ayarlarla birlikte"
  far_php() { local path=$1; shift; curl -s -o /dev/null -w '%{http_code}' -H "Host: $PHP" "$@" "http://$NIP$path"; }
  check "önbellek ve Cloudflare kipi birlikte" "$(rcode $SP '{"server_cache":{"enabled":true,"ttl":"10m"},"cloudflare_only":true}') $(far_php /sayfa.php) $(xcache $PHP /sayfa.php)" "200 403 MISS"
  check "geçersiz tür reddedilir" "$([ "$(rcode $SP '{"cloudflare_only":"evet"}')" != 200 ] && echo red)" red
  check "reddedilen istek kipi bozmaz" "$(far_php /sayfa.php)" 403
  check "kapat" "$(rcode $SP '{}') $(far_php /sayfa.php)" "200 200"
fi

say "Temizlik"
check "siteleri sil" "$(code DELETE "/sites/$SS?delete_files=true") $(code DELETE "/sites/$SP?delete_files=true") $(code DELETE "/sites/$SX?delete_files=true")" "200 200 200"
check "önbellek klasörleri kalmadı" "$(ls $CACHE_ROOT 2>/dev/null | grep -c -x -e "$SS" -e "$SP" -e "$SX")" 0
check "yapılandırma geçerli" "$(nginx -t 2>&1 | grep -c 'test is successful')" 1
cleanup_user
echo "bitti"
