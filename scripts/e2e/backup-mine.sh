#!/usr/bin/env bash
# Hesabın kendi yedeklerinin sınaması: müşteri kendi sitesini ve veritabanını yedekler,
# içinde gezer, geri yükler, siler; başkasının yedeğine ulaşamaz. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash backup-mine.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-bm
JC=/tmp/pxjar-bm-customer
JO=/tmp/pxjar-bm-other
CUSTOMER=yedekmusteri
OTHER=yedekkomsu
PASS='Yedek-Parola-13579'
DOMAIN=yedek-musteri.test
DBNAME=${CUSTOMER}_veri
REPO=/var/backups/pixpanel-e2e-mine
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
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
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
# wait_job <çerez> <görev no>
wait_job() { for _ in $(seq 1 300); do s=$(call "$1" GET "/jobs/$2" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
# snap <tür>: müşterinin o türdeki en yeni yedeğinin "hedef/kimlik" yolu.
snap() { call $JC GET /backup/mine | jget "next((str(s['target_id']) + '/' + s['id'] for s in d['snapshots'] if s['kind']=='$1'), '')"; }

cleanup() {
  for name in $CUSTOMER $OTHER; do
    local id; id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')")
    [ -z "$id" ] && continue
    for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['owner']=='$name')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
    for database in $(api GET /databases | jget "' '.join(str(x['id']) for x in d['databases'] if x['owner']=='$name')"); do api DELETE /databases/$database >/dev/null; done
    api DELETE /users/$id >/dev/null
  done
  local target; target=$(api GET /backup | jget "next((t['id'] for t in d['targets'] if t['name']=='e2e-mine'), '')")
  [ -n "$target" ] && api DELETE /backup/targets/$target >/dev/null
  rm -rf $REPO
}

rm -f $J $JC $JO
login $J "$PX_USER" "$PX_PASS" >/dev/null
cleanup

say "Hazırlık"
# Müşteri yedekleri ilk yedek yerine gider; makinede hiç yoksa sınama için bir tane açılır.
if [ "$(api GET /backup | jget 'len(d["targets"])')" = 0 ]; then
  api POST /backup/targets -d "{\"name\":\"e2e-mine\",\"kind\":\"local\",\"config\":{\"path\":\"$REPO\"}}" >/dev/null
fi
T=$(api GET /backup | jget 'd["targets"][0]["id"]')
check "yedek yeri var" "$([[ "$T" =~ ^[0-9]+$ ]] && echo evet)" evet
check "müşteri ve komşusu" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}") $(code POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\",\"role\":\"user\"}")" "200 200"
check "girişler" "$(login $JC $CUSTOMER "$PASS") $(login $JO $OTHER "$PASS")" "200 200"
S=$(call $JC POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"static\"}" | jget 'd["id"]')
check "veritabanı" "$(status $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DBNAME\"}")" 200
D=$(call $JC GET /databases | jget "next(x['id'] for x in d['databases'] if x['name']=='$DBNAME')")
ROOT=/home/$CUSTOMER/sites/$DOMAIN/public
echo "ilk hali" > $ROOT/index.html; chown $CUSTOMER:www-data $ROOT/index.html
mariadb -e "CREATE TABLE $DBNAME.notlar (metin TEXT); INSERT INTO $DBNAME.notlar VALUES ('yedekten önce')"
ADMIN_SITE=$(api GET /sites | jget "next(s['id'] for s in d if s['owner']=='$PX_USER')")

say "Erişim"
check "müşteri yöneticinin yedek ekranına giremez" "$(status $JC GET /backup)" 403
check "müşteri yedek yerinin parolasını göremez" "$(status $JC GET /backup/targets/$T/password)" 403
check "müşteri bütün yedekleri listeleyemez" "$(status $JC GET /backup/targets/$T/snapshots)" 403
R=$(call $JC GET /backup/mine)
check "kendi görünümü: kullanılabilir, yedek yok" "$(jget '(d["available"], len(d["snapshots"]), [s["name"] for s in d["sites"]])' <<<"$R")" "(True, 0, ['$DOMAIN'])"
check "yedek yerinin ayrıntısı müşteriye verilmez" "$(grep -c 'config\|password\|public_key\|/var/backups' <<<"$R")" 0

say "Yedek alma"
check "boş seçim" "$(status $JC POST /backup/mine/run -d '{}')" 400
check "başkasının sitesi" "$(status $JC POST /backup/mine/run -d "{\"site_ids\":[$ADMIN_SITE]}")" 404
check "kendi sitesiyle karışık başkasınınki" "$(status $JC POST /backup/mine/run -d "{\"site_ids\":[$S,$ADMIN_SITE]}")" 404
JOB=$(call $JC POST /backup/mine/run -d "{\"site_ids\":[$S],\"database_ids\":[$D]}" | jget 'd["job_id"]')
check "yedekleme görevi" "$(wait_job $JC "$JOB")" success
R=$(call $JC GET /backup/mine)
check "site ve veritabanı yedeği görünür, elle alınmış sayılır" "$(jget 'sorted((s["kind"], s["name"], s["manual"]) for s in d["snapshots"])' <<<"$R")" "[('db', '$DBNAME', True), ('site', '$DOMAIN', True)]"
check "elle alınan yedek sayısı" "$(jget '(d["manual_count"], d["manual_limit"] >= d["manual_count"])' <<<"$R")" "(2, True)"
SITE_SNAP=$(snap site); DB_SNAP=$(snap db)

say "Başkasının yedeği"
check "komşu bu yedekleri görmez" "$(call $JO GET /backup/mine | jget 'len(d["snapshots"])')" 0
check "komşu içinde gezemez" "$(status $JO GET /backup/mine/snapshots/$SITE_SNAP/files)" 404
check "komşu geri yükleyemez" "$(status $JO POST /backup/mine/restore -d "{\"target_id\":${SITE_SNAP%/*},\"snapshot\":\"${SITE_SNAP#*/}\"}")" 404
check "komşu silemez" "$(status $JO DELETE /backup/mine/snapshots/$SITE_SNAP)" 404
check "olmayan yedek" "$(status $JC GET /backup/mine/snapshots/$T/deadbeef/files)" 404

say "Gezinme ve geri yükleme"
check "kök klasör" "$(call $JC GET /backup/mine/snapshots/$SITE_SNAP/files | jget 'sorted(e["name"] for e in d["entries"])')" "['logs', 'public']"
check "üst klasöre çıkma" "$(status $JC GET "/backup/mine/snapshots/$SITE_SNAP/files?path=/home/$CUSTOMER/sites/$DOMAIN/../../../$PX_USER")" 400
check "kapsam dışı yol" "$(status $JC GET "/backup/mine/snapshots/$SITE_SNAP/files?path=/etc")" 400
echo "bozuldu" > $ROOT/index.html; echo "fazladan" > $ROOT/fazla.txt; chown $CUSTOMER:www-data $ROOT/fazla.txt
JOB=$(call $JC POST /backup/mine/restore -d "{\"target_id\":${SITE_SNAP%/*},\"snapshot\":\"${SITE_SNAP#*/}\",\"delete_extra\":true}" | jget 'd["job_id"]')
check "site geri yükleme görevi" "$(wait_job $JC "$JOB")" success
check "dosya geri geldi, fazladan dosya silindi" "$(cat $ROOT/index.html) $([ -e $ROOT/fazla.txt ] || echo yok)" "ilk hali yok"
check "sahiplik korundu" "$(stat -c '%U:%G' $ROOT/index.html)" "$CUSTOMER:www-data"
mariadb -e "UPDATE $DBNAME.notlar SET metin = 'bozuldu'"
JOB=$(call $JC POST /backup/mine/restore -d "{\"target_id\":${DB_SNAP%/*},\"snapshot\":\"${DB_SNAP#*/}\"}" | jget 'd["job_id"]')
check "veritabanı geri yükleme görevi" "$(wait_job $JC "$JOB")" success
check "veri geri geldi" "$(mariadb -N -e "SELECT metin FROM $DBNAME.notlar")" "yedekten önce"
check "başka siteye yükleme müşteriye kapalı (alan yok sayılır, yerine yüklenir)" "$(call $JC POST /backup/mine/restore -d "{\"target_id\":${SITE_SNAP%/*},\"snapshot\":\"${SITE_SNAP#*/}\",\"destination_site_id\":$ADMIN_SITE}" | jget '"job_id" in d')" True
sleep 3
check "yöneticinin sitesine dokunulmadı" "$(ls /home/$PX_USER/sites/*/public/fazla.txt 2>/dev/null | wc -l)" 0

say "Sembolik bağla dışarı yazdırma denemesi"
# Müşteri, yedek alındıktan sonra bir klasörünü sistemdeki başka bir yere bağlar ve yalnızca
# onun altındaki bir yolu geri yükletir. Geri yükleme root olarak o bağı izlememelidir.
SROOT=/home/$CUSTOMER/sites/$DOMAIN
rm -rf /tmp/geri-hedef; mkdir -p /tmp/geri-hedef
mv $SROOT/public $SROOT/public.asil; ln -s /tmp/geri-hedef $SROOT/public; chown -h $CUSTOMER $SROOT/public
JOB=$(call $JC POST /backup/mine/restore -d "{\"target_id\":${SITE_SNAP%/*},\"snapshot\":\"${SITE_SNAP#*/}\",\"paths\":[\"$SROOT/public/index.html\"]}" | jget 'd["job_id"]')
wait_job $JC "$JOB" >/dev/null
check "geri yükleme site klasörünün dışına dosya yazmaz" "$(ls -A /tmp/geri-hedef | wc -l)" 0
check "ara klasör geride kalmaz" "$(ls -A /var/lib/pixpanel/restore 2>/dev/null | wc -l)" 0
rm -f $SROOT/public; mv $SROOT/public.asil $SROOT/public; rm -rf /tmp/geri-hedef
JOB=$(call $JC POST /backup/mine/restore -d "{\"target_id\":${SITE_SNAP%/*},\"snapshot\":\"${SITE_SNAP#*/}\",\"paths\":[\"$SROOT/public/index.html\"]}" | jget 'd["job_id"]')
check "tek dosya geri yükleme (bağ yokken) çalışır; dosya hesabın" "$(wait_job $JC "$JOB") $(stat -c %U $SROOT/public/index.html) $(stat -c %a $SROOT)" "success $CUSTOMER $(stat -c %a /home/$CUSTOMER/sites/$DOMAIN)"

say "El değiştiren kaynak"
# Site silinirse eski yedeği müşteriye görünmez olur: silinmiş siteyi yeniden kurmak yöneticinin işidir.
check "siteyi sil" "$(status $JC DELETE "/sites/$S?delete_files=true")" 200
check "silinen sitenin yedeği listeden düşer" "$(call $JC GET /backup/mine | jget '[s["kind"] for s in d["snapshots"]]')" "['db']"
check "silinen sitenin yedeği geri yüklenemez" "$(status $JC POST /backup/mine/restore -d "{\"target_id\":${SITE_SNAP%/*},\"snapshot\":\"${SITE_SNAP#*/}\"}")" 404

say "Silme"
JOB=$(call $JC DELETE /backup/mine/snapshots/$DB_SNAP | jget 'd["job_id"]')
check "kendi yedeğini siler" "$(wait_job $JC "$JOB")" success
check "yedek kalmadı" "$(call $JC GET /backup/mine | jget 'len(d["snapshots"])')" 0

say "Temizlik"
# Silinen sitenin yöneticiye görünen yedeği de kaldırılır.
for id in $(api GET "/backup/targets/$T/snapshots" | jget "' '.join(s['id'] for s in d['snapshots'] if s['owner']=='$CUSTOMER')"); do
  wait_job $J "$(api DELETE "/backup/targets/$T/snapshots/$id" | jget 'd["job_id"]')" >/dev/null
done
cleanup
check "hesaplar silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$CUSTOMER', '$OTHER')]")" "[]"
echo "bitti"
