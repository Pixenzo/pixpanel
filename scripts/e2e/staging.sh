#!/usr/bin/env bash
# WordPress hazırlık ortamı sınaması: asıl sitenin bağlı kopyası açılır, kopyadaki değişiklikler
# canlıya aktarılır, kopya canlıdan yenilenir.
# Test makinesinde çalışır; WordPress'i internetten indirir.
# Kullanım: PX_USER=... PX_PASS=... bash staging.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-staging
LIVE=hazirlik-asil.test
STG=staging.hazirlik-asil.test
PLAIN=hazirlik-duz.test
WP_PASS='Wp-Hazirlik-Parola-97531'
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
wait_job() { for _ in $(seq 1 400); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
job_log() { api GET "/jobs/$1" | jget 'd["log"][-1800:]' | sed 's/^/        /'; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
page() { local host=$1 path=$2; shift 2; curl -s -H "Host: $host" "$@" "http://127.0.0.1$path"; }
st() { curl -s -o /dev/null -w '%{http_code}' -H "Host: $1" "http://127.0.0.1$2"; }
wp() { local domain=$1; shift; runuser -u "$PX_USER" -- php8.3 /usr/local/lib/pixpanel/wp-cli.phar --no-color --path=/home/$PX_USER/sites/$domain/public "$@" 2>/dev/null; }
dbname() { sed -n "s/^define( *'DB_NAME', *'\\(.*\\)' *);.*/\\1/p" /home/$PX_USER/sites/$1/public/wp-config.php; }
asuser() { runuser -u "$PX_USER" -- "$@"; }
cleanup() {
  for name in $STG $LIVE $PLAIN; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
  for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["databases"] if "hazirlik_asil" in x["name"])'); do api DELETE /databases/$old >/dev/null; done
  for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["users"] if "hazirlik_asil" in x["name"])'); do api DELETE /databases/users/$old >/dev/null; done
}

rm -f $J
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' "$B/sites/1/wordpress/staging")" 401
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Hazırlık"
P=$(api POST /sites -d "{\"domain\":\"$PLAIN\",\"kind\":\"php\",\"php_version\":\"8.3\"}" | jget 'd["id"]')
L=$(api POST /sites -d "{\"domain\":\"$LIVE\",\"kind\":\"php\",\"php_version\":\"8.3\"}" | jget 'd["id"]')
JOB=$(api POST /sites/$L/wordpress/install -d "{\"title\":\"Canlı Site\",\"admin_user\":\"yonetici\",\"admin_email\":\"ali@ornek.com\",\"admin_password\":\"$WP_PASS\",\"locale\":\"tr_TR\"}" | jget 'd["job_id"]')
check "canlı sitede WordPress kurulumu" "$(wait_job "$JOB")" success
LROOT=/home/$PX_USER/sites/$LIVE/public
SROOT=/home/$PX_USER/sites/$STG/public
wp $LIVE post create --post_title="Canlı yazı" --post_status=publish >/dev/null
LIVE_DB=$(dbname $LIVE)

say "Doğrulama"
check "WordPress olmayan sitede hazırlık ortamı açılmaz" "$(code POST /sites/$P/wordpress/staging -d '{"domain":"staging.hazirlik-duz.test"}')" 400
check "olmayan site" "$(code POST /sites/999999/wordpress/staging -d "{\"domain\":\"$STG\"}")" 404
check "geçersiz alan adı" "$(code POST /sites/$L/wordpress/staging -d '{"domain":"../x"}')" 400
check "var olan alan adı" "$(code POST /sites/$L/wordpress/staging -d "{\"domain\":\"$PLAIN\"}")" 409
check "hazırlık ortamı olmayan sitede aktarım" "$(code POST /sites/$L/wordpress/staging/sync -d '{"direction":"push","files":true,"database":true}')" 400
has "önerilen adres" "$(api GET /sites/$L/wordpress/staging | jget 'd["suggested"]')" "$STG"

say "Hazırlık ortamını açma"
R=$(api POST /sites/$L/wordpress/staging -d "{\"domain\":\"$STG\"}")
CJOB=$(jget 'd["job_id"]' <<<"$R"); S=$(jget 'd["site_id"]' <<<"$R")
RESULT=$(wait_job "$CJOB")
check "hazırlık ortamı görevi" "$RESULT" success
[ "$RESULT" = success ] || job_log "$CJOB"
check "asıl site kopyasını gösterir" "$(api GET /sites/$L/wordpress/staging | jget '[c["domain"] for c in d["copies"]]')" "['$STG']"
check "kopya asıl sitesini gösterir" "$(api GET /sites/$S/wordpress/staging | jget 'd["live"]["domain"]') $(api GET /sites/$S | jget 'd["staging_of"]')" "$LIVE $L"
check "kopyanın adresi kendi alan adı" "$(wp $STG option get siteurl)" "http://$STG"
check "kopya arama motorlarına kapalı, canlı açık" "$(wp $STG option get blog_public) $(wp $LIVE option get blog_public)" "0 1"
check "kopyanın veritabanı ayrı" "$([ -n "$(dbname $STG)" ] && [ "$(dbname $STG)" != "$LIVE_DB" ] && echo ayrı)" ayrı
has "kopya açılıyor" "$(page $STG /)" "Canlı yazı"
check "hazırlık ortamının hazırlık ortamı açılmaz" "$(code POST /sites/$S/wordpress/staging -d '{"domain":"staging2.hazirlik-asil.test"}')" 400
check "geçersiz yön" "$(code POST /sites/$S/wordpress/staging/sync -d '{"direction":"sideways","files":true,"database":true}')" 400
check "hiçbir şey seçilmeden aktarım" "$(code POST /sites/$S/wordpress/staging/sync -d '{"direction":"push"}')" 400

say "Canlıya aktarma"
# Hazırlık ortamında değişiklikler: yeni yazı (adresli), ad, yeni eklenti dosyası, silinen dosya.
wp $STG post create --post_title="Hazırlık yazısı" --post_status=publish --post_content="Görsel: http://$STG/wp-content/uploads/yeni.png" >/dev/null
wp $STG option update blogname "Yeni Tasarım" >/dev/null
wp $STG option update pix_json '{"adres":"http:\/\/'$STG'\/sayfa"}' >/dev/null
asuser mkdir -p $SROOT/wp-content/plugins/deneme-eklenti
echo '<?php // deneme' | asuser tee $SROOT/wp-content/plugins/deneme-eklenti/deneme.php >/dev/null
asuser rm -f $SROOT/readme.html
# Canlıda bu arada: yeni yüklenen dosya (kalmalı) ve artık kullanılmayan dosya (silinmeli).
asuser mkdir -p $LROOT/wp-content/uploads
echo "canlıda yüklendi" | asuser tee $LROOT/wp-content/uploads/canli-sonradan.txt >/dev/null
echo "eski" | asuser tee $LROOT/wp-content/eski.txt >/dev/null
[ -f $LROOT/readme.html ] || echo "x" | asuser tee $LROOT/readme.html >/dev/null
JOB=$(api POST /sites/$S/wordpress/staging/sync -d '{"direction":"push","files":true,"database":true}' | jget 'd["job_id"]')
check "aynı anda ikinci aktarım reddedilir" "$(code POST /sites/$S/wordpress/staging/sync -d '{"direction":"refresh","files":true,"database":true}')" 409
RESULT=$(wait_job "$JOB")
check "canlıya aktarma görevi" "$RESULT" success
[ "$RESULT" = success ] || job_log "$JOB"
has "canlı site yeni yazıyı gösteriyor" "$(page $LIVE /)" "Hazırlık yazısı"
check "canlı sitenin adı değişti" "$(wp $LIVE option get blogname)" "Yeni Tasarım"
check "canlı sitenin adresi kendi adresi" "$(wp $LIVE option get siteurl) $(wp $LIVE option get home)" "http://$LIVE http://$LIVE"
has "içerikteki adresler canlı adrese çevrildi" "$(wp $LIVE post list --post_type=post --field=post_content)" "http://$LIVE/wp-content/uploads/yeni.png"
has "JSON içindeki kaçışlı adres de çevrildi" "$(wp $LIVE option get pix_json)" "http:\\/\\/$LIVE\\/sayfa"
check "canlıda hazırlık adresi kalmadı (kalıcı kimlikler dışında)" "$(wp $LIVE db search "$STG" --all-tables | grep -E '^[a-z_0-9]+:[a-z_]+$' | grep -vc ':guid$')" 0
check "canlı site arama motorlarına açık kaldı" "$(wp $LIVE option get blog_public)" 1
check "canlı site kendi veritabanında kaldı" "$(dbname $LIVE)" "$LIVE_DB"
check "yeni eklenti dosyası geldi" "$(cat $LROOT/wp-content/plugins/deneme-eklenti/deneme.php)" "<?php // deneme"
check "hazırlıkta silinen dosya canlıda da silindi" "$([ -e $LROOT/readme.html ] && echo var || echo yok) $([ -e $LROOT/wp-content/eski.txt ] && echo var || echo yok)" "yok yok"
check "canlıda sonradan yüklenen dosya korundu" "$(cat $LROOT/wp-content/uploads/canli-sonradan.txt)" "canlıda yüklendi"
check "bakım kipi kapandı" "$([ -e $LROOT/.maintenance ] && echo açık || echo kapalı) $(st $LIVE /)" "kapalı 200"
check "döküm dosyaları geride kalmadı" "$(ls /home/$PX_USER/tmp/ 2>/dev/null | grep -c 'pixpanel-hazirlik\|pixpanel-onceki')" 0
check "dosyalar hesabın kullanıcısına ait" "$(find $LROOT -not -user $PX_USER | wc -l)" 0
check "hazırlık ortamı değişmedi" "$(wp $STG option get siteurl) $(wp $STG option get blog_public)" "http://$STG 0"
check "veritabanı parolası günlüğe yazılmadı" "$(api GET "/jobs/$JOB" | jget 'd["log"]' | grep -c "$(sed -n "s/^define( *'DB_PASSWORD', *'\\(.*\\)' *);.*/\\1/p" $LROOT/wp-config.php)")" 0
check "denetim günlüğüne yazıldı" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='wordpress.staging.push'")" 1

say "Canlıdan yenileme (yalnız veritabanı)"
wp $LIVE option update blogname "Canlıda Değişti" >/dev/null
echo "yalnız hazırlıkta" | asuser tee $SROOT/wp-content/yalniz-hazirlik.txt >/dev/null
JOB=$(api POST /sites/$S/wordpress/staging/sync -d '{"direction":"refresh","database":true}' | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "yenileme görevi" "$RESULT" success
[ "$RESULT" = success ] || job_log "$JOB"
check "hazırlık ortamı canlının verisini aldı" "$(wp $STG option get blogname)" "Canlıda Değişti"
check "hazırlık ortamının adresi ve gizliliği korundu" "$(wp $STG option get siteurl) $(wp $STG option get blog_public)" "http://$STG 0"
check "dosyalar seçilmediği için dokunulmadı" "$(cat $SROOT/wp-content/yalniz-hazirlik.txt)" "yalnız hazırlıkta"
check "canlı site yenilemeden etkilenmedi" "$(wp $LIVE option get siteurl) $(wp $LIVE option get blog_public)" "http://$LIVE 1"

say "Canlıdan yenileme (dosyalar)"
JOB=$(api POST /sites/$S/wordpress/staging/sync -d '{"direction":"refresh","files":true}' | jget 'd["job_id"]')
check "dosya yenileme görevi" "$(wait_job "$JOB")" success
check "hazırlıkta fazladan duran dosya silindi" "$([ -e $SROOT/wp-content/yalniz-hazirlik.txt ] && echo var || echo yok)" yok
check "hazırlık ortamı kendi veritabanında kaldı" "$([ "$(dbname $STG)" != "$LIVE_DB" ] && echo ayrı) $(st $STG /)" "ayrı 200"

say "Asıl site silinince"
api DELETE "/sites/$L?delete_files=true" >/dev/null
check "kopya sıradan siteye döner" "$(api GET /sites/$S | jget 'd["staging_of"]') $(api GET /sites/$S/wordpress/staging | jget 'd["live"]')" "None None"
check "bağı kopan kopyada aktarım reddedilir" "$(code POST /sites/$S/wordpress/staging/sync -d '{"direction":"push","files":true}')" 400

say "Temizlik"
cleanup
check "siteler silindi" "$(api GET /sites | jget "len([s for s in d if 'hazirlik-' in s['domain']])")" 0
rm -f $J
echo "bitti"
