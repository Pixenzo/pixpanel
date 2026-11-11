#!/usr/bin/env bash
# Site kopyalama sınaması: dosyalar ve ayarlar yeni alan adına çoğaltılır; WordPress'te
# veritabanı da kopyalanır ve kopya asıl siteden bağımsız çalışır.
# Test makinesinde çalışır; WordPress'i internetten indirir.
# Kullanım: PX_USER=... PX_PASS=... bash clone.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-clone
SRC=kopya-asil.test
DST=kopya-yeni.test
WSRC=kopya-wp.test
WDST=kopya-wp-deneme.test
WP_PASS='Wp-Kopya-Parola-86420'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 300)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
page() { local host=$1 path=$2; shift 2; curl -s -H "Host: $host" "$@" "http://127.0.0.1$path"; }
st() { curl -s -o /dev/null -w '%{http_code}' -H "Host: $1" "http://127.0.0.1$2"; }
wp() { local domain=$1; shift; runuser -u "$PX_USER" -- php8.3 /usr/local/lib/pixpanel/wp-cli.phar --no-color --path=/home/$PX_USER/sites/$domain/public "$@" 2>/dev/null; }
dbname() { sed -n "s/^define( *'DB_NAME', *'\\(.*\\)' *);.*/\\1/p" /home/$PX_USER/sites/$1/public/wp-config.php; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in $SRC $DST $WSRC $WDST; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["databases"] if x["name"].startswith("wp_kopya_wp"))'); do api DELETE /databases/$old >/dev/null; done
for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["users"] if x["name"].startswith("wp_kopya_wp"))'); do api DELETE /databases/users/$old >/dev/null; done

say "Durağan site"
S=$(api POST /sites -d "{\"domain\":\"$SRC\",\"kind\":\"static\"}" | jget 'd["id"]')
ROOT=/home/$PX_USER/sites/$SRC
mkdir -p $ROOT/public/alt/derin
echo "asıl ana sayfa" > $ROOT/public/index.html; echo "derin dosya" > $ROOT/public/alt/derin/sayfa.html
echo "gizli not" > $ROOT/ozel-not.txt; chmod 600 $ROOT/ozel-not.txt; ln -s alt/derin/sayfa.html $ROOT/public/kisayol.html
chown -R -h $PX_USER:www-data $ROOT/public $ROOT/ozel-not.txt
api PATCH /sites/$S -d '{"template":"spa","rules":{"redirects":[{"from":"/eski","to":"/alt/derin/sayfa.html","code":301,"exact":true}],"maintenance":true}}' >/dev/null
check "geçersiz alan adı" "$(code POST /sites/$S/clone -d '{"domain":"../x"}')" 400
check "var olan alan adı" "$(code POST /sites/$S/clone -d "{\"domain\":\"$SRC\"}")" 409
check "olmayan site" "$(code POST /sites/999999/clone -d "{\"domain\":\"$DST\"}")" 404
R=$(api POST /sites/$S/clone -d "{\"domain\":\"$DST\"}")
D=$(jget 'd["site_id"]' <<<"$R")
check "kopyalama görevi" "$(wait_job "$(jget 'd["job_id"]' <<<"$R")")" success
check "kopya yayında, içerik aynı" "$(page $DST /) | $(page $DST /alt/derin/sayfa.html)" "asıl ana sayfa | derin dosya"
check "sembolik bağ bağ olarak kopyalandı" "$(readlink /home/$PX_USER/sites/$DST/public/kisayol.html)" alt/derin/sayfa.html
check "yayın klasörü dışındaki dosya ve izni" "$(cat /home/$PX_USER/sites/$DST/ozel-not.txt) $(stat -c '%a %U' /home/$PX_USER/sites/$DST/ozel-not.txt)" "gizli not 600 $PX_USER"
check "dosyalar hesabın kullanıcısına ait" "$(find /home/$PX_USER/sites/$DST -not -user $PX_USER | wc -l)" 0
check "ayarlar devralındı (şablon, yönlendirme)" "$(api GET /sites/$D | jget '(d["template"], d["kind"], len(d["rules"]["redirects"]))') $(st $DST /eski)" "('spa', 'static', 1) 301"
check "bakım kipi kopyaya taşınmaz" "$(api GET /sites/$D | jget 'd["rules"]["maintenance"]') $(st $DST /)" "False 200"
echo "kopyada değişti" > /home/$PX_USER/sites/$DST/public/index.html
check "kopya bağımsız: asıl site etkilenmez" "$(cat $ROOT/public/index.html)" "asıl ana sayfa"
check "aynı ada ikinci kopya" "$(code POST /sites/$S/clone -d "{\"domain\":\"$DST\"}")" 409
api PATCH /sites/$S -d '{"status":"suspended"}' >/dev/null
check "askıdaki site kopyalanamaz" "$(code POST /sites/$S/clone -d '{"domain":"kopya-ucuncu.test"}')" 400

say "WordPress"
W=$(api POST /sites -d "{\"domain\":\"$WSRC\",\"kind\":\"php\",\"php_version\":\"8.3\"}" | jget 'd["id"]')
JOB=$(api POST /sites/$W/wordpress/install -d "{\"title\":\"Asıl Site\",\"admin_user\":\"yonetici\",\"admin_email\":\"ali@ornek.com\",\"admin_password\":\"$WP_PASS\",\"locale\":\"tr_TR\"}" | jget 'd["job_id"]')
check "kaynak WordPress kurulumu" "$(wait_job "$JOB")" success
wp $WSRC post create --post_title="Kopyalanacak yazı" --post_status=publish --post_content="Görsel: http://$WSRC/wp-content/uploads/resim.png" >/dev/null
R=$(api POST /sites/$W/clone -d "{\"domain\":\"$WDST\"}")
CJOB=$(jget 'd["job_id"]' <<<"$R"); WD=$(jget 'd["site_id"]' <<<"$R")
RESULT=$(wait_job "$CJOB")
check "kopyalama görevi" "$RESULT" success
[ "$RESULT" = success ] || api GET "/jobs/$CJOB" | jget 'd["log"][-1500:]' | sed 's/^/        /'
check "kopyanın ayrı veritabanı var" "$([ -n "$(dbname $WDST)" ] && [ "$(dbname $WDST)" != "$(dbname $WSRC)" ] && echo ayrı)" ayrı
check "veritabanı panelde hesabın adına kayıtlı" "$(api GET /databases | jget "len([x for x in d['databases'] if x['name']=='$(dbname $WDST)'])")" 1
check "veritabanı parolası görev günlüğüne yazılmadı" "$(api GET "/jobs/$CJOB" | jget 'd["log"]' | grep -c "$(sed -n "s/^define( *'DB_PASSWORD', *'\\(.*\\)' *);.*/\\1/p" /home/$PX_USER/sites/$WDST/public/wp-config.php)")" 0
check "döküm dosyası geride kalmadı" "$(ls /home/$PX_USER/tmp/ 2>/dev/null | grep -c pixpanel-kopya)" 0
check "kopyanın adresi yeni alan adı" "$(wp $WDST option get siteurl) $(wp $WDST option get home)" "http://$WDST http://$WDST"
check "asıl sitenin adresi değişmedi" "$(wp $WSRC option get siteurl)" "http://$WSRC"
has "kopya açılıyor ve yazıyı taşıyor" "$(page $WDST /)" "Kopyalanacak yazı"
has "içerikteki adresler çevrildi" "$(wp $WDST post list --post_type=post --field=post_content)" "http://$WDST/wp-content/uploads/resim.png"
# Yazıların `guid` sütunu kalıcı kimliktir; WordPress değiştirilmemesini ister, bilerek dışarıda tutulur.
check "kopyada asıl sitenin adresi kalmadı (kalıcı kimlikler dışında)" "$(wp $WDST db search "$WSRC" --all-tables | grep -E '^[a-z_0-9]+:[a-z_]+$' | grep -vc ':guid$')" 0
wp $WDST option update blogname "Deneme Kopyası" >/dev/null
check "kopyadaki değişiklik asıl siteye yansımaz" "$(wp $WSRC option get blogname)" "Asıl Site"
check "panel kopyayı WordPress olarak tanır" "$(api GET /sites/$WD/wordpress | jget '(d["installed"], d["problem"], d["title"], d["url"])')" "(True, None, 'Deneme Kopyası', 'http://$WDST')"
check "WordPress şablonu devralındı" "$(api GET /sites/$WD | jget 'd["template"]') $(st $WDST /xmlrpc.php)" "wordpress 403"

say "Temizlik"
for name in $SRC $DST $WSRC $WDST; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["databases"] if x["name"].startswith("wp_kopya_wp"))'); do api DELETE /databases/$old >/dev/null; done
for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["users"] if x["name"].startswith("wp_kopya_wp"))'); do api DELETE /databases/users/$old >/dev/null; done
check "siteler silindi" "$(api GET /sites | jget "len([s for s in d if s['domain'].startswith('kopya-')])")" 0
rm -f $J
echo "bitti"
