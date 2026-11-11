#!/usr/bin/env bash
# Site kuralları sınaması: yönlendirme, parola korumalı klasör, adres engelleme, bakım kipi,
# WWW tercihi, görsel koruması, özel hata sayfaları. Her kuralın web sunucusundaki davranışı denenir.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash rules.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-rules
SITE=kural-sinamasi.test
PHP=kural-php.test
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 200)]" ;; esac; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
# Sitenin yanıtı: durum kodu, gövde, yönlendirme hedefi. Ek curl seçenekleri sona yazılır.
st() { local host=$1 path=$2; shift 2; curl -s -o /dev/null -w '%{http_code}' -H "Host: $host" "$@" "http://127.0.0.1$path"; }
body() { local host=$1 path=$2; shift 2; curl -s -H "Host: $host" "$@" "http://127.0.0.1$path"; }
loc() { local host=$1 path=$2; shift 2; curl -s -o /dev/null -w '%{http_code} %{redirect_url}' -H "Host: $host" "$@" "http://127.0.0.1$path"; }
# rules <site no> <JSON>: kuralların tamamını yazar; yanıtın kurallar kısmını döner.
rules() { api PATCH "/sites/$1" -d "{\"rules\":$2}"; }
rcode() { code PATCH "/sites/$1" -d "{\"rules\":$2}"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in $SITE $PHP; do
  OLD=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$name'), '')")
  [ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
done
PHP_VERSION=$(api GET /sites/options | jget 'd["php_versions"][0]')
S=$(api POST /sites -d "{\"domain\":\"$SITE\",\"kind\":\"static\",\"aliases\":[\"www.$SITE\"]}" | jget 'd["id"]')
ROOT=/home/$PX_USER/sites/$SITE/public
mkdir -p $ROOT/gizli $ROOT/yeni-klasor/a
echo ana > $ROOT/index.html; echo yeni > $ROOT/yeni.html; echo "gizli içerik" > $ROOT/gizli/index.html
echo derin > $ROOT/yeni-klasor/a/b.html; echo "bulunamadı sayfası" > $ROOT/404.html; printf 'GIF89a' > $ROOT/resim.gif
chown -R $PX_USER:www-data $ROOT
mkdir -p /var/lib/pixpanel/acme/.well-known/acme-challenge && echo dogrulama > /var/lib/pixpanel/acme/.well-known/acme-challenge/kural-sinamasi

say "Başlangıç"
check "yeni sitede kural yok" "$(api GET /sites/$S | jget '(d["rules"]["redirects"], d["rules"]["protected"], d["rules"]["maintenance"], d["rules"]["www"])')" "([], [], False, '')"

say "Yönlendirmeler"
R='{"redirects":[{"from":"/eski","to":"/yeni.html","code":301,"exact":true},{"from":"/eski-klasor","to":"/yeni-klasor","code":302,"exact":false},{"from":"/dis","to":"https://ornek.org/karsilama?kaynak=site","code":301,"exact":true}]}'
check "kaydet" "$(rules $S "$R" | jget 'len(d["rules"]["redirects"])')" 3
check "tam eşleşme kalıcı yönlenir" "$(loc $SITE /eski)" "301 http://$SITE/yeni.html"
check "tam eşleşme altını etkilemez" "$(st $SITE /eski/alt)" 404
check "klasör yönlendirmesi altındaki yolu taşır" "$(loc $SITE /eski-klasor/a/b.html)" "302 http://$SITE/yeni-klasor/a/b.html"
check "klasör yönlendirmesi sorgu dizgesini taşır" "$(loc $SITE '/eski-klasor/a?x=1&y=2')" "302 http://$SITE/yeni-klasor/a?x=1&y=2"
check "yolda satır başı yanıt başlığını bölmez" "$(curl -s -D - -o /dev/null -H "Host: $SITE" 'http://127.0.0.1/eski-klasor/a%0d%0aX-Eklenen:%201' | tr -d '' | grep -ci '^x-eklenen')" 0
check "klasörün kendisi de yönlenir" "$(loc $SITE /eski-klasor)" "302 http://$SITE/yeni-klasor"
check "benzer adlı adres yönlenmez" "$(st $SITE /eski-klasorum)" 404
check "dış adrese yönlendirme" "$(loc $SITE /dis)" "301 https://ornek.org/karsilama?kaynak=site"
check "öteki adresler olduğu gibi" "$(body $SITE /)" ana
check "yapılandırmayı bozacak hedef" "$(rcode $S '{"redirects":[{"from":"/a","to":"/b; return 200 x","code":301,"exact":true}]}')" 400
check "değişken içeren hedef" "$(rcode $S '{"redirects":[{"from":"/a","to":"/$host","code":301,"exact":true}]}')" 400
check "geçersiz kaynak" "$(rcode $S '{"redirects":[{"from":"/a b","to":"/b","code":301,"exact":true}]}')" 400
check "betik şeması" "$(rcode $S '{"redirects":[{"from":"/a","to":"javascript:alert(1)","code":301,"exact":true}]}')" 400
check "geçersiz tür" "$(rcode $S '{"redirects":[{"from":"/a","to":"/b","code":307,"exact":true}]}')" 400
check "aynı kaynak iki kez" "$(rcode $S '{"redirects":[{"from":"/a","to":"/b","code":301,"exact":true},{"from":"/a","to":"/c","code":301,"exact":true}]}')" 400
check "reddedilen istek öncekini bozmaz" "$(loc $SITE /eski)" "301 http://$SITE/yeni.html"

say "Parola korumalı klasör"
R='{"protected":[{"path":"/gizli","label":"Gizli alan","users":[{"name":"ali","password":"Ali-Parolasi-1"}]}]}'
check "kaydet; yanıt parola ya da özet taşımaz" "$(rules $S "$R" | jget '(d["rules"]["protected"][0]["users"], "hash" in json.dumps(d), "Parolasi" in json.dumps(d))')" "([{'name': 'ali'}], False, False)"
check "yönlendirmeler kuralların tamamı yazıldığı için kalktı" "$(st $SITE /eski)" 404
check "parolasız istek reddedilir" "$(st $SITE /gizli/)" 401
has "tarayıcıya koruma adı bildirilir" "$(curl -s -D - -o /dev/null -H "Host: $SITE" http://127.0.0.1/gizli/ | tr -d '\r')" 'Basic realm="Gizli alan"'
check "doğru parola" "$(body $SITE /gizli/ -u ali:Ali-Parolasi-1)" "gizli içerik"
check "yanlış parola" "$(st $SITE /gizli/ -u ali:yanlis)" 401
check "klasörün dışı açık" "$(st $SITE /)" 200
check "benzer adlı yol korunmaz" "$(st $SITE /gizlilik)" 404
check "parola dosyasını yalnız web sunucusu okur" "$(stat -c '%a %U:%G' /etc/nginx/pixpanel/htpasswd/$S-0)" "640 root:www-data"
check "parola açık yazılmaz" "$(grep -c 'Ali-Parolasi' /etc/nginx/pixpanel/htpasswd/$S-0) $(cut -d: -f2 /etc/nginx/pixpanel/htpasswd/$S-0 | cut -c1-3)" '0 $6$'
R='{"protected":[{"path":"/gizli","label":"Gizli alan","users":[{"name":"ali"},{"name":"veli","password":"Veli-Parolasi-2"}]}]}'
check "parolası verilmeyen kullanıcının parolası korunur" "$(rcode $S "$R") $(st $SITE /gizli/ -u ali:Ali-Parolasi-1) $(st $SITE /gizli/ -u veli:Veli-Parolasi-2)" "200 200 200"
check "yeni kullanıcı parolasız eklenemez" "$(rcode $S '{"protected":[{"path":"/gizli","label":"x","users":[{"name":"ayse"}]}]}')" 400
check "kısa parola" "$(rcode $S '{"protected":[{"path":"/gizli","label":"x","users":[{"name":"ali","password":"kisa"}]}]}')" 400
check "korumayı kapatan ad kullanılamaz" "$(rcode $S '{"protected":[{"path":"/gizli","label":"off","users":[{"name":"ali","password":"Ali-Parolasi-1"}]}]}')" 400
check "sitenin tamamı korunabilir; sertifika doğrulaması açık kalır" "$(rcode $S '{"protected":[{"path":"/","label":"Site","users":[{"name":"ali","password":"Ali-Parolasi-1"}]}]}') $(st $SITE /) $(st $SITE / -u ali:Ali-Parolasi-1) $(st $SITE /.well-known/acme-challenge/kural-sinamasi)" "200 401 200 200"
check "koruma kaldırılınca parola dosyası silinir" "$(rcode $S '{}') $(st $SITE /gizli/) $(ls /etc/nginx/pixpanel/htpasswd/ 2>/dev/null | grep -c "^$S-")" "200 200 0"

say "PHP sitesinde koruma ve yönlendirme"
P=$(api POST /sites -d "{\"domain\":\"$PHP\",\"kind\":\"php\",\"php_version\":\"$PHP_VERSION\"}" | jget 'd["id"]')
PROOT=/home/$PX_USER/sites/$PHP/public
mkdir -p $PROOT/yonetim
echo '<?php echo "yonetim-calisti";' > $PROOT/yonetim/index.php; echo '<?php echo "komsu-calisti";' > $PROOT/eskimis.php
chown -R $PX_USER:www-data $PROOT
R='{"protected":[{"path":"/yonetim","label":"Yönetim","users":[{"name":"ali","password":"Ali-Parolasi-1"}]}],"redirects":[{"from":"/eski","to":"/yeni","code":301,"exact":false}]}'
check "kaydet" "$(rcode $P "$R")" 200
check "korumalı klasörde PHP parolasız çalışmaz" "$(st $PHP /yonetim/index.php)" 401
check "betik adı öne eklenerek koruma atlatılamaz" "$(st $PHP /index.php/yonetim/) $(st $PHP /eskimis.php/yonetim/index.php)" "401 401"
check "büyük harfle yazılan yol da korunur" "$(st $PHP /YONETIM/index.php)" 401
check "parolayla PHP çalışır (kaynak olarak sunulmaz)" "$(body $PHP /yonetim/index.php -u ali:Ali-Parolasi-1)" yonetim-calisti
check "yönlendirmeye komşu PHP dosyası çalışmayı sürdürür" "$(body $PHP /eskimis.php)" komsu-calisti
check "yönlendirme PHP sitesinde de çalışır" "$(loc $PHP /eski/sayfa)" "301 http://$PHP/yeni/sayfa"

say "Adres engelleme"
check "engellenen adres" "$(rcode $S '{"blocked":["127.0.0.1","203.0.113.0/24"]}') $(st $SITE /)" "200 403"
check "engelliyken sertifika doğrulaması açık" "$(body $SITE /.well-known/acme-challenge/kural-sinamasi)" dogrulama
check "yalnız izinli adresler: listede olmayan giremez" "$(rcode $S '{"allowed":["198.51.100.7"]}') $(st $SITE /)" "200 403"
check "izinli adres girer" "$(rcode $S '{"allowed":["127.0.0.1","198.51.100.7"]}') $(st $SITE /)" "200 200"
check "adres olmayan değer" "$(rcode $S '{"blocked":["1.2.3.4; allow all"]}')" 400
check "geçersiz ağ" "$(rcode $S '{"allowed":["203.0.113.5/40"]}')" 400

say "Bakım kipi"
check "aç" "$(rcode $S '{"maintenance":true}') $(st $SITE /) $(st $SITE /yeni.html)" "200 503 503"
has "bakım sayfası gösterilir" "$(body $SITE /)" "bakım"
has "arama motorlarına yeniden deneme süresi bildirilir" "$(curl -s -D - -o /dev/null -H "Host: $SITE" http://127.0.0.1/ | tr -d '\r')" "Retry-After: 600"
check "bakımdayken sertifika doğrulaması açık" "$(st $SITE /.well-known/acme-challenge/kural-sinamasi)" 200
check "izinli adres siteyi görmeye devam eder" "$(rcode $S '{"maintenance":true,"maintenance_allowed":["127.0.0.1"]}') $(body $SITE /)" "200 ana"
check "kapat" "$(rcode $S '{}') $(body $SITE /)" "200 ana"

say "WWW tercihi"
check "www'siz ad asıl ad" "$(rcode $S '{"www":"apex"}') $(loc www.$SITE /yeni.html?a=1)" "200 301 http://$SITE/yeni.html?a=1"
check "asıl ad yönlenmez" "$(body $SITE /)" ana
check "www'li ad asıl ad" "$(rcode $S '{"www":"www"}') $(loc $SITE /yeni.html) $(body www.$SITE /)" "200 301 http://www.$SITE/yeni.html ana"
check "www takma adı olmayan sitede seçilemez" "$(rcode $P '{"www":"www"}')" 400
check "geçersiz tercih" "$(rcode $S '{"www":"hepsi"}')" 400

say "Görsel koruması ve hata sayfaları"
check "aç" "$(rcode $S '{"hotlink_protection":true,"error_pages":{"not_found":"/404.html"}}')" 200
check "başka siteden gösterim engellenir" "$(st $SITE /resim.gif -H 'Referer: https://baska-site.example/sayfa')" 403
check "sitenin kendi sayfasından gösterim" "$(st $SITE /resim.gif -H "Referer: http://$SITE/")" 200
check "doğrudan açma" "$(st $SITE /resim.gif)" 200
check "öteki dosyalar etkilenmez" "$(st $SITE /yeni.html -H 'Referer: https://baska-site.example/')" 200
check "özel 404 sayfası (durum kodu korunur)" "$(st $SITE /olmayan) $(body $SITE /olmayan)" "404 bulunamadı sayfası"
check "hata sayfası site dışını gösteremez" "$(rcode $S '{"error_pages":{"not_found":"/../../etc/passwd"}}')" 400
check "hata sayfası PHP olamaz" "$(rcode $S '{"error_pages":{"not_found":"/hata.php"}}')" 400

say "Öteki ayarlarla birlikte"
check "askıya alınan sitede kurallar yapılandırmayı bozmaz" "$(rcode $S '{"maintenance":true,"protected":[{"path":"/gizli","label":"x","users":[{"name":"ali","password":"Ali-Parolasi-1"}]}]}') $(code PATCH /sites/$S -d '{"status":"suspended"}') $(st $SITE /)" "200 200 503"
check "yeniden yayına alınca kurallar geri gelir" "$(code PATCH /sites/$S -d '{"status":"active"}') $(st $SITE /gizli/)" "200 503"
check "başka ayar değişince kurallar korunur" "$(code PATCH /sites/$S -d '{"hsts":false}') $(api GET /sites/$S | jget '(d["rules"]["maintenance"], len(d["rules"]["protected"]))')" "200 (True, 1)"
check "yapılandırma geçerli" "$(nginx -t 2>&1 | grep -c 'test is successful')" 1

say "Temizlik"
check "siteleri sil" "$(code DELETE "/sites/$S?delete_files=true") $(code DELETE "/sites/$P?delete_files=true")" "200 200"
check "parola dosyaları kalmadı" "$(ls /etc/nginx/pixpanel/htpasswd/ 2>/dev/null | grep -c -e "^$S-" -e "^$P-")" 0
rm -f /var/lib/pixpanel/acme/.well-known/acme-challenge/kural-sinamasi $J
echo "bitti"
