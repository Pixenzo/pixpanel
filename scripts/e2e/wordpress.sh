#!/usr/bin/env bash
# WordPress araçlarının sınaması: kurulum, durum, tek kullanımlık giriş, eklentiler,
# bakım kipi ve güncelleme. Test makinesinde çalışır; WordPress'i internetten indirir.
# Kullanım: PX_USER=... PX_PASS=... bash wordpress.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-wp
DOMAIN=wp-e2e.test
WP_PASS='Wp-Yonetici-Parola-97531'
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
# site <yol> [curl seçenekleri]: siteye sunucunun içinden istek atar.
site() { local path=$1; shift; curl -s -H "Host: $DOMAIN" "$@" "http://127.0.0.1$path"; }
body() { printf '{"title":"Sınama Sitesi","admin_user":"%s","admin_email":"%s","admin_password":"%s","locale":"%s"}' "$1" "$2" "$3" "${4:-tr_TR}"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Hazırlık"
OLD=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$DOMAIN'), '')")
[ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["databases"] if x["name"].startswith("wp_wp_e2e"))'); do api DELETE /databases/$old >/dev/null; done
for old in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["users"] if x["name"].startswith("wp_wp_e2e"))'); do api DELETE /databases/users/$old >/dev/null; done
S=$(api POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"php\",\"php_version\":\"8.3\"}" | jget 'd["id"]')
STATIC=$(api GET /sites | jget 'next(s["id"] for s in d if s["kind"]=="static")')

say "Kurulum öncesi"
R=$(api GET /sites/$S/wordpress)
check "boş sitede kurulu değil, kurulabilir" "$(jget '(d["installed"], d["can_install"], d["database_ready"])' <<<"$R")" "(False, True, True)"
check "wp-cli özetiyle doğrulanmış" "$(sha256sum /usr/local/lib/pixpanel/wp-cli.phar | cut -c1-16)" "ce34ddd838f7351d"
check "PHP olmayan sitede" "$(code GET /sites/$STATIC/wordpress)" 400
check "seçenek gibi yönetici adı" "$(code POST /sites/$S/wordpress/install -d "$(body -x ali@ornek.com "$WP_PASS")")" 400
check "komut eklenmiş e-posta" "$(code POST /sites/$S/wordpress/install -d "$(body yonetici 'ali@ornek.com --allow-root' "$WP_PASS")")" 400
check "kısa parola" "$(code POST /sites/$S/wordpress/install -d "$(body yonetici ali@ornek.com kisa)")" 400
check "yol kaçışlı dil kodu" "$(code POST /sites/$S/wordpress/install -d "$(body yonetici ali@ornek.com "$WP_PASS" '../x')")" 400

say "Kurulum"
JOB=$(api POST /sites/$S/wordpress/install -d "$(body yonetici ali@ornek.com "$WP_PASS")" | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "kurulum görevi" "$RESULT" success
[ "$RESULT" = success ] || api GET "/jobs/$JOB" | jget 'd["log"][-1200:]' | sed 's/^/        /'
check "yönetici parolası görev günlüğüne yazılmadı" "$(api GET "/jobs/$JOB" | jget 'd["log"]' | grep -c "$WP_PASS")" 0
DBPASS=$(sed -n "s/^define( *'DB_PASSWORD', *'\\(.*\\)' *);.*/\\1/p" /home/$PX_USER/sites/$DOMAIN/public/wp-config.php)
check "veritabanı parolası görev günlüğüne yazılmadı" "$([ ${#DBPASS} -ge 16 ] && api GET "/jobs/$JOB" | jget 'd["log"]' | grep -c "$DBPASS")" 0
R=$(api GET /sites/$S/wordpress)
check "kurulu ve sorunsuz" "$(jget '(d["installed"], d["problem"], d["version"] != "")' <<<"$R")" "(True, None, True)"
check "başlık ve adres" "$(jget '(d["title"], d["url"])' <<<"$R")" "('Sınama Sitesi', 'http://$DOMAIN')"
check "eklentiler listelendi" "$(jget 'len(d["plugins"]) >= 1 and len(d["themes"]) >= 1' <<<"$R")" True
has "site WordPress sunuyor" "$(site /)" "Sınama Sitesi"
check "dosyalar sitenin kullanıcısına ait" "$(stat -c '%U' /home/$PX_USER/sites/$DOMAIN/public/wp-load.php)" "$PX_USER"
check "wp-config yalnızca sahibine ve web sunucusuna açık" "$(stat -c '%a %U %G' /home/$PX_USER/sites/$DOMAIN/public/wp-config.php)" "640 $PX_USER www-data"
check "veritabanı parolası süreç listesine düşmedi" "$(grep -c "DB_PASSWORD" /home/$PX_USER/sites/$DOMAIN/public/wp-config.php)" 1
check "panelden dosya düzenleme kapalı" "$(grep -c "DISALLOW_FILE_EDIT" /home/$PX_USER/sites/$DOMAIN/public/wp-config.php)" 1
check "karşılama sayfası kaldırıldı" "$([ -e /home/$PX_USER/sites/$DOMAIN/public/index.html ] || echo yok)" yok
check "veritabanı panelde kayıtlı" "$(api GET /databases | jget 'any(x["name"]=="wp_wp_e2e_test" for x in d["databases"])')" True
check "ikinci kurulum reddedilir" "$(code POST /sites/$S/wordpress/install -d "$(body yonetici ali@ornek.com "$WP_PASS")")" 409

say "Tek kullanımlık giriş"
URL=$(api POST /sites/$S/wordpress/login | jget 'd["url"]')
has "giriş adresi" "$URL" "http://$DOMAIN/?pixpanel_login="
TOKEN=${URL##*=}
HEAD=$(site "/?pixpanel_login=$TOKEN" -D - -o /dev/null)
has "yönetim paneline yönlendirir" "$HEAD" "wp-admin/"
has "oturum çerezi verilir" "$HEAD" "wordpress_logged_in"
check "aynı bağlantı ikinci kez çalışmaz" "$(site "/?pixpanel_login=$TOKEN" -o /dev/null -w '%{http_code}')" 403
URL=$(api POST /sites/$S/wordpress/login | jget 'd["url"]')
check "yanlış anahtar reddedilir" "$(site "/?pixpanel_login=0000" -o /dev/null -w '%{http_code}')" 403
check "yanlış deneme asıl bağlantıyı da geçersiz kılar" "$(site "/?pixpanel_login=${URL##*=}" -o /dev/null -w '%{http_code}')" 403
# Site sahibi veritabanındaki adresi değiştirebilir; giriş adresi ona güvenmemeli.
mariadb -e "UPDATE wp_wp_e2e_test.wp_options SET option_value='javascript:alert(document.cookie)//' WHERE option_name='siteurl'"
has "zararlı site adresi giriş adresine taşınmaz" "$(api POST /sites/$S/wordpress/login | jget 'd["url"]')" "http://$DOMAIN/?pixpanel_login="
mariadb -e "UPDATE wp_wp_e2e_test.wp_options SET option_value='https://kotu.example' WHERE option_name='siteurl'"
has "yabancı alan adı giriş adresine taşınmaz" "$(api POST /sites/$S/wordpress/login | jget 'd["url"]')" "http://$DOMAIN/?pixpanel_login="
mariadb -e "UPDATE wp_wp_e2e_test.wp_options SET option_value='http://$DOMAIN' WHERE option_name='siteurl'"
# Yukarıda üretilip kullanılmayan giriş kaydı bir denemeyle tüketilir.
site "/?pixpanel_login=0000" -o /dev/null
check "panelin giriş eklentisi eklenti listesinde görünmez" "$(api GET /sites/$S/wordpress | jget '[p["name"] for p in d["plugins"] if "pixpanel" in p["name"]]')" "[]"
check "anahtar veritabanında açık durmaz" "$(mariadb -N -e "SELECT COUNT(*) FROM wp_wp_e2e_test.wp_options WHERE option_name='pixpanel_login_token'")" 0

say "Eklentiler ve ayarlar"
PLUGIN=$(jget 'd["plugins"][0]["name"]' <<<"$R")
check "eklentiyi etkinleştir" "$(api POST /sites/$S/wordpress/plugins/$PLUGIN -d '{"action":"activate"}' | jget "next(p['status'] for p in d['plugins'] if p['name']=='$PLUGIN')")" active
check "eklentiyi kapat" "$(api POST /sites/$S/wordpress/plugins/$PLUGIN -d '{"action":"deactivate"}' | jget "next(p['status'] for p in d['plugins'] if p['name']=='$PLUGIN')")" inactive
check "kendiliğinden güncellemeyi aç" "$(api POST /sites/$S/wordpress/plugins/$PLUGIN -d '{"action":"auto-on"}' | jget "next(p['auto_update'] for p in d['plugins'] if p['name']=='$PLUGIN')")" on
check "seçenek gibi eklenti adı" "$(code POST /sites/$S/wordpress/plugins/--all -d '{"action":"delete"}')" 400
check "geçersiz işlem" "$(code POST /sites/$S/wordpress/plugins/$PLUGIN -d '{"action":"eval"}')" 400
check "geçersiz tür" "$(code POST /sites/$S/wordpress/core/$PLUGIN -d '{"action":"update"}')" 404
check "bakım kipini aç" "$(api PUT /sites/$S/wordpress/settings -d '{"maintenance":true}' | jget 'd["maintenance"]')" True
check "bakımdayken site 503 döner" "$(site / -o /dev/null -w '%{http_code}')" 503
check "bakım kipini kapat" "$(api PUT /sites/$S/wordpress/settings -d '{"maintenance":false}' | jget 'd["maintenance"]')" False
check "site yeniden yayında" "$(site / -o /dev/null -w '%{http_code}')" 200
check "çekirdek güncellemesi: hepsi" "$(api PUT /sites/$S/wordpress/settings -d '{"auto_core":"all"}' | jget 'd["auto_core"]')" all
check "çekirdek güncellemesi: kapalı" "$(api PUT /sites/$S/wordpress/settings -d '{"auto_core":"off"}' | jget 'd["auto_core"]')" off
check "geçersiz seçenek" "$(code PUT /sites/$S/wordpress/settings -d '{"auto_core":"true); system(id"}')" 400

say "Güncelleme"
check "geçersiz kapsam" "$(code POST /sites/$S/wordpress/update -d '{"what":"--all"}')" 400
UJOB=$(api POST /sites/$S/wordpress/update -d '{"what":"all"}' | jget 'd["job_id"]')
check "aynı sitede ikinci güncelleme başlatılamaz" "$(code POST /sites/$S/wordpress/update -d '{"what":"all"}')" 409
check "güncelleme görevi" "$(wait_job "$UJOB")" success
ULOG=$(api GET "/jobs/$UJOB" | jget 'd["log"]')
if [ "$(api GET /backup | jget 'len(d["targets"]) > 0')" = True ]; then
  has "güncellemeden önce yedek alındı" "$ULOG" "Güncellemeden önce yedek alınıyor"
  T=$(api GET /backup | jget 'd["targets"][0]["id"]')
  check "yedekte site ve veritabanı var" "$(api GET "/backup/targets/$T/snapshots" | jget "sorted({s['kind'] for s in d['snapshots'] if s['name'] in ('$DOMAIN', 'wp_wp_e2e_test')})")" "['db', 'site']"
else
  has "yedek yeri yokken uyarır" "$ULOG" "yedek alınamadı"
fi
has "güncellemeden sonra site yoklandı" "$ULOG" "Site yanıt veriyor"
check "güncellemeden sonra site yayında" "$(site / -o /dev/null -w '%{http_code}')" 200

say "wp-cli bütünlüğü"
echo "bozuk" >> /usr/local/lib/pixpanel/wp-cli.phar
check "değiştirilmiş wp-cli çalıştırılmaz, yeniden indirilir" "$(api GET /sites/$S/wordpress | jget 'd["installed"]') $(sha256sum /usr/local/lib/pixpanel/wp-cli.phar | cut -c1-16)" "True ce34ddd838f7351d"

say "Temizlik"
check "site sil" "$(code DELETE "/sites/$S?delete_files=true")" 200
D=$(api GET /databases | jget 'next(x["id"] for x in d["databases"] if x["name"]=="wp_wp_e2e_test")')
check "veritabanını sil" "$(code DELETE /databases/$D)" 200
U=$(api GET /databases | jget 'next((x["id"] for x in d["users"] if x["name"]=="wp_wp_e2e_test"), "")')
[ -n "$U" ] && check "veritabanı kullanıcısını sil" "$(code DELETE /databases/users/$U)" 200
echo "bitti"
