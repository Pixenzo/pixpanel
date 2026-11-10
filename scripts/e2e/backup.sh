#!/usr/bin/env bash
# Yedekleme ve geri yüklemenin sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash backup.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-backup
REPO=/var/backups/pixpanel-e2e
SFTP_REPO=/var/backups/pixpanel-e2e-sftp
DOMAIN=yedek-e2e.test
CLONE=yedek-klon.test
DBNAME=yedek_e2e
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 240)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
job_log() { api GET "/jobs/$1" | jget 'd["log"]'; }
target_id() { api GET /backup | jget "next((t['id'] for t in d['targets'] if t['name']=='$1'), '')"; }
plan_id() { api GET /backup | jget "next((p['id'] for p in d['plans'] if p['name']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
db_id() { api GET /databases | jget "next((x['id'] for x in d['databases'] if x['name']=='$1'), '')"; }
# Verilen türde ve addaki en yeni yedeğin kimliği.
snap() { api GET "/backup/targets/$T/snapshots" | jget "next((s['id'] for s in d['snapshots'] if s['kind']=='$1' and s['name']=='$2'), '')"; }
count() { api GET "/backup/targets/$T/snapshots" | jget "len([s for s in d['snapshots'] if s['kind']=='$1' and s['name']=='$2'])"; }
restore() { wait_job "$(api POST /backup/restore -d "$1" | jget 'd["job_id"]')"; }
sql() { mariadb -N -B -e "$1" 2>&1; }

cleanup() {
  for name in e2e-plan; do id=$(plan_id $name); [ -n "$id" ] && api DELETE /backup/plans/$id >/dev/null; done
  for name in e2e-yerel e2e-bozuk e2e-sftp e2e-yeniden e2e-yanlis; do id=$(target_id $name); [ -n "$id" ] && api DELETE /backup/targets/$id >/dev/null; done
  for name in $DOMAIN $CLONE; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
  id=$(db_id $DBNAME); [ -n "$id" ] && api DELETE /databases/$id >/dev/null
  rm -rf $REPO $SFTP_REPO /var/backups/pixpanel-e2e-bozuk
  sed -i '/pixpanel-backup$/d' /root/.ssh/authorized_keys 2>/dev/null
}

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Kurulum"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="restic")')" != True ]; then
  check "restic kurulumu" "$(wait_job "$(api POST /components/restic/install | jget 'd["job_id"]')")" success
fi
check "kurulu ve sürümü yeterli" "$(api GET /backup | jget '(d["installed"], d["supported"])')" "(True, True)"
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/backup)" 401

say "Hedef doğrulaması"
tbody() { python3 -c 'import json,sys; print(json.dumps({"name":sys.argv[1],"kind":sys.argv[2],"config":json.loads(sys.argv[3]),"secret_key":sys.argv[4] if len(sys.argv)>4 else "","password":sys.argv[5] if len(sys.argv)>5 else ""}))' "$@"; }
check "site klasörlerinin içi hedef olamaz" "$(code POST /backup/targets -d "$(tbody x local '{"path":"/home/yedek"}')")" 400
check "üst klasöre çıkan yol" "$(code POST /backup/targets -d "$(tbody x local '{"path":"/var/backups/../../etc"}')")" 400
check "göreli yerel yol" "$(code POST /backup/targets -d "$(tbody x local '{"path":"yedek"}')")" 400
check "seçenek gibi görünen SFTP sunucusu" "$(code POST /backup/targets -d "$(tbody x sftp '{"host":"-oProxyCommand=id","user":"root","path":"y"}')")" 400
check "boşluklu SFTP kullanıcısı" "$(code POST /backup/targets -d "$(tbody x sftp '{"host":"127.0.0.1","user":"root -o x","path":"y"}')")" 400
check "kimlik gömülü S3 adresi" "$(code POST /backup/targets -d "$(tbody x s3 '{"endpoint":"https://a:b@s3.test","bucket":"kova","access_key":"k"}' gizli)")" 400
check "gizli anahtarsız S3" "$(code POST /backup/targets -d "$(tbody x s3 '{"endpoint":"https://s3.test","bucket":"kova","access_key":"k"}')")" 400
check "bilinmeyen tür" "$(code POST /backup/targets -d "$(tbody x ftp '{"path":"/x"}')")" 400

say "Yerel hedef"
check "oluştur" "$(code POST /backup/targets -d "$(tbody e2e-yerel local "{\"path\":\"$REPO\"}")")" 200
T=$(target_id e2e-yerel)
check "aynı adla ikinci hedef" "$(code POST /backup/targets -d "$(tbody e2e-yerel local "{\"path\":\"$REPO\"}")")" 409
check "bağlantı denemesi depoyu kurar" "$(api POST /backup/targets/$T/check | jget "next((t['last_check_ok'], t['last_check_error']) for t in d['targets'] if t['id']==$T)")" "(True, '')"
check "depo diskte" "$([ -f $REPO/config ] && echo var)" var
PASSWORD=$(api GET /backup/targets/$T/password | jget 'd["password"]')
check "depo parolası üretildi" "${#PASSWORD}" 40
check "parola görünümde yok" "$(api GET /backup | grep -c "$PASSWORD")" 0

say "Sınama verisi"
S=$(api POST /sites -d "{\"domain\":\"$DOMAIN\",\"kind\":\"static\"}" | jget 'd["id"]')
ROOT=/home/$PX_USER/sites/$DOMAIN
echo "ilk hali" > $ROOT/public/index.html
mkdir -p $ROOT/public/alt && echo "alt dosya" > $ROOT/public/alt/veri.txt
chown -R $PX_USER:www-data $ROOT/public
api POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DBNAME\"}" >/dev/null
D=$(db_id $DBNAME)
sql "CREATE TABLE $DBNAME.notlar (id INT PRIMARY KEY, metin VARCHAR(50)); INSERT INTO $DBNAME.notlar VALUES (1, 'yedekten önce');" >/dev/null
M=$(api GET /backup | jget 'next((m["id"] for m in d["sources"]["mail_domains"]), "")')
check "site ve veritabanı oluştu" "$([ -n "$S" ] && [ -n "$D" ] && echo evet)" evet

say "Elle yedek"
SCOPE="{\"site_ids\":[$S],\"database_ids\":[$D],\"panel\":true$([ -n "$M" ] && echo ",\"mail_domain_ids\":[$M]")}"
check "boş kapsam" "$(code POST /backup/run -d "{\"target_id\":$T,\"scope\":{}}")" 400
JOB=$(api POST /backup/run -d "{\"target_id\":$T,\"scope\":$SCOPE}" | jget 'd["job_id"]')
check "yedekleme görevi" "$(wait_job "$JOB")" success
[ "$(api GET /jobs/$JOB | jget 'd["job"]["status"]')" = success ] || job_log "$JOB" | tail -20
check "site yedeği" "$(count site $DOMAIN)" 1
check "veritabanı yedeği" "$(count db $DBNAME)" 1
check "panel yedeği" "$(count panel panel)" 1
check "yedek elle alınmış sayılır" "$(api GET "/backup/targets/$T/snapshots" | jget 'set(s["plan_id"] for s in d["snapshots"])')" "{None}"
check "yedekte sahip ve boyut bilgisi" "$(api GET "/backup/targets/$T/snapshots" | jget "next((s['owner'], s['size'] > 0) for s in d['snapshots'] if s['kind']=='site')")" "('$PX_USER', True)"
check "depodaki veri şifreli" "$(grep -rl 'ilk hali' $REPO | wc -l)" 0
check "geçici panel kopyası silindi" "$(ls /var/lib/pixpanel/backup/panel 2>/dev/null | wc -l)" 0
SNAP=$(snap site $DOMAIN)
DBSNAP=$(snap db $DBNAME)

say "Yedeğin içinde gezinme"
check "kök klasör" "$(api GET "/backup/targets/$T/snapshots/$SNAP/files" | jget 'sorted(e["name"] for e in d["entries"])')" "['logs', 'public']"
check "alt klasör" "$(api GET "/backup/targets/$T/snapshots/$SNAP/files?path=$ROOT/public" | jget '[(e["name"], e["kind"]) for e in d["entries"]]')" "[('alt', 'dir'), ('index.html', 'file')]"
check "kapsam dışı yol" "$(code GET "/backup/targets/$T/snapshots/$SNAP/files?path=/etc")" 400
check "üst klasöre çıkma" "$(code GET "/backup/targets/$T/snapshots/$SNAP/files?path=$ROOT/../..")" 400
check "veritabanı yedeğinde gezilemez" "$(code GET "/backup/targets/$T/snapshots/$DBSNAP/files")" 400
check "geçersiz yedek kimliği" "$(code GET "/backup/targets/$T/snapshots/latest/files")" 400
check "olmayan yedek" "$(code GET "/backup/targets/$T/snapshots/00000000ffff/files")" 404

say "Yerinde geri yükleme"
echo "bozuldu" > $ROOT/public/index.html
echo "bozuldu" > $ROOT/public/alt/veri.txt
echo "sonradan" > $ROOT/public/yeni.txt
check "tek dosya geri yükleme" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\",\"paths\":[\"$ROOT/public/index.html\"]}")" success
check "seçilen dosya geri geldi" "$(cat $ROOT/public/index.html)" "ilk hali"
check "seçilmeyen dosyaya dokunulmadı" "$(cat $ROOT/public/alt/veri.txt)" "bozuldu"
check "tam geri yükleme" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\"}")" success
check "her dosya geri geldi" "$(cat $ROOT/public/alt/veri.txt)" "alt dosya"
check "fazladan dosya duruyor" "$([ -f $ROOT/public/yeni.txt ] && echo var)" var
check "birebir geri yükleme" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\",\"delete_extra\":true}")" success
check "fazladan dosya silindi" "$([ -f $ROOT/public/yeni.txt ] || echo yok)" yok
check "sahiplik korundu" "$(stat -c '%U:%G' $ROOT/public/index.html)" "$PX_USER:www-data"
check "kapsam dışı yol reddedilir" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\",\"paths\":[\"/etc/passwd\"]}")" failed
check "üst klasöre çıkan yol reddedilir" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\",\"paths\":[\"$ROOT/../../../../etc/passwd\"]}")" failed
check "panel yedeği arayüzden geri yüklenmez" "$(code POST /backup/restore -d "{\"target_id\":$T,\"snapshot\":\"$(snap panel panel)\"}")" 400

say "Veritabanı geri yükleme"
sql "UPDATE $DBNAME.notlar SET metin = 'bozuldu'; CREATE TABLE $DBNAME.fazla (id INT);" >/dev/null
check "aynı veritabanına" "$(restore "{\"target_id\":$T,\"snapshot\":\"$DBSNAP\"}")" success
check "veri geri geldi" "$(sql "SELECT metin FROM $DBNAME.notlar")" "yedekten önce"
api DELETE /databases/$D >/dev/null
check "veritabanı silindi" "$(sql "SHOW DATABASES LIKE '$DBNAME'")" ""
check "silinmiş veritabanı geri yüklenir" "$(restore "{\"target_id\":$T,\"snapshot\":\"$DBSNAP\"}")" success
check "veri yerinde" "$(sql "SELECT metin FROM $DBNAME.notlar")" "yedekten önce"
D=$(db_id $DBNAME)
check "panel kaydı da geri geldi" "$([ -n "$D" ] && echo evet)" evet

say "Silinmiş siteyi geri getirme"
api DELETE "/sites/$S?delete_files=true" >/dev/null
check "site silindi" "$([ -d $ROOT ] || echo yok)" yok
JOB=$(api POST /backup/restore -d "{\"target_id\":$T,\"snapshot\":\"$SNAP\"}" | jget 'd["job_id"]')
check "geri yükleme" "$(wait_job "$JOB")" success
[ "$(api GET /jobs/$JOB | jget 'd["job"]["status"]')" = success ] || job_log "$JOB" | tail -20
S=$(site_id $DOMAIN)
check "site kaydı geri geldi" "$([ -n "$S" ] && echo evet)" evet
check "dosyalar geri geldi" "$(cat $ROOT/public/alt/veri.txt 2>&1)" "alt dosya"
check "karşılama sayfası yerine yedekteki sayfa" "$(curl -s -H "Host: $DOMAIN" http://127.0.0.1/)" "ilk hali"
check "sahiplik" "$(stat -c '%U:%G' $ROOT/public/index.html)" "$PX_USER:www-data"

say "Başka siteye yükleme"
C=$(api POST /sites -d "{\"domain\":\"$CLONE\",\"kind\":\"static\"}" | jget 'd["id"]')
check "klon" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\",\"destination_site_id\":$C}")" success
check "klonda dosyalar" "$(cat /home/$PX_USER/sites/$CLONE/public/alt/veri.txt 2>&1)" "alt dosya"
check "klon yayında" "$(curl -s -H "Host: $CLONE" http://127.0.0.1/)" "ilk hali"
check "olmayan hedef site" "$(restore "{\"target_id\":$T,\"snapshot\":\"$SNAP\",\"destination_site_id\":999999}")" failed

say "Plan, saklama ilkesi ve zamanlama"
pbody() { echo "{\"name\":\"e2e-plan\",\"target_id\":$T,\"scope\":{\"site_ids\":[$S],\"database_ids\":[$D]},\"frequency\":\"$1\",\"time\":\"$2\",\"keep_daily\":1,\"keep_weekly\":0,\"keep_monthly\":0,\"enabled\":true}"; }
check "geçersiz saat" "$(code POST /backup/plans -d "$(pbody daily 25:00)")" 400
check "geçersiz sıklık" "$(code POST /backup/plans -d "$(pbody hourly 03:00)")" 400
check "plan oluştur" "$(code POST /backup/plans -d "$(pbody daily 03:00)")" 200
P=$(plan_id e2e-plan)
check "sıradaki çalışma hesaplandı" "$(api GET /backup | jget "next(p['next_run_at'] > 0 for p in d['plans'] if p['id']==$P)")" True
check "kullanılan hedef silinemez" "$(code DELETE /backup/targets/$T)" 409
check "ilk koşu" "$(wait_job "$(api POST /backup/plans/$P/run | jget 'd["job_id"]')")" success
echo "ikinci hali" > $ROOT/public/index.html
JOB=$(api POST /backup/plans/$P/run | jget 'd["job_id"]')
check "ikinci koşu" "$(wait_job "$JOB")" success
check "plan durumu" "$(api GET /backup | jget "next((p['last_status'], p['last_job_id']) for p in d['plans'] if p['id']==$P)")" "('success', $JOB)"
check "saklama ilkesi eski plan yedeğini sildi" "$(api GET "/backup/targets/$T/snapshots" | jget "len([s for s in d['snapshots'] if s['plan_id']==$P and s['kind']=='site'])")" 1
check "elle alınan yedek duruyor" "$(api GET "/backup/targets/$T/snapshots" | jget "len([s for s in d['snapshots'] if s['plan_id'] is None and s['kind']=='site'])")" 1
check "kalan plan yedeği en yenisi" "$(restore "{\"target_id\":$T,\"snapshot\":\"$(snap site $DOMAIN)\"}" >/dev/null; cat $ROOT/public/index.html)" "ikinci hali"
# Zamanı geçmiş plan kendiliğinden başlar.
sqlite3 /var/lib/pixpanel/pixpanel.db "UPDATE backup_plans SET next_run_at = strftime('%s','now') - 5 WHERE id = $P"
STARTED=""
for _ in $(seq 1 50); do
  NOW_JOB=$(api GET /backup | jget "next(p['last_job_id'] for p in d['plans'] if p['id']==$P)")
  [ "$NOW_JOB" != "$JOB" ] && { STARTED=$NOW_JOB; break; }
  sleep 1
done
check "zamanlayıcı planı başlattı" "$([ -n "$STARTED" ] && wait_job "$STARTED")" success
check "sıradaki çalışma ileri alındı" "$(api GET /backup | jget "next(p['next_run_at'] > $(date +%s) for p in d['plans'] if p['id']==$P)")" True
check "plan kapatılınca sıradaki çalışma kalkar" "$(api PUT /backup/plans/$P -d "$(pbody manual 03:00)" | jget "next(p['next_run_at'] for p in d['plans'] if p['id']==$P)")" None

say "Tek yedeği silme"
OLD=$(snap db $DBNAME)
BEFORE=$(count db $DBNAME)
check "silme görevi" "$(wait_job "$(api DELETE /backup/targets/$T/snapshots/$OLD | jget 'd["job_id"]')")" success
check "yedek bir azaldı" "$(count db $DBNAME)" "$((BEFORE - 1))"

say "Bozuk hedef"
touch /var/backups/pixpanel-e2e-bozuk
api POST /backup/targets -d "$(tbody e2e-bozuk local '{"path":"/var/backups/pixpanel-e2e-bozuk"}')" >/dev/null
X=$(target_id e2e-bozuk)
check "bağlantı denemesi hatayı kaydeder" "$(api POST /backup/targets/$X/check | jget "next((t['last_check_ok'], len(t['last_check_error']) > 0) for t in d['targets'] if t['id']==$X)")" "(False, True)"
JOB=$(api POST /backup/run -d "{\"target_id\":$X,\"scope\":{\"panel\":true}}" | jget 'd["job_id"]')
check "bozuk hedefte görev başarısız" "$(wait_job "$JOB")" failed
has "günlükte neden yazıyor" "$(job_log "$JOB")" "Yedek hedefine ulaşılamadı"

say "Var olan depoyu yeniden bağlama"
api POST /backup/targets -d "$(tbody e2e-yanlis local "{\"path\":\"$REPO\"}" "" yanlisparola)" >/dev/null
W=$(target_id e2e-yanlis)
check "yanlış parolayla açılmaz" "$(api POST /backup/targets/$W/check | jget "next(t['last_check_ok'] for t in d['targets'] if t['id']==$W)")" False
api POST /backup/targets -d "$(tbody e2e-yeniden local "{\"path\":\"$REPO\"}" "" "$PASSWORD")" >/dev/null
R=$(target_id e2e-yeniden)
check "doğru parolayla açılır" "$(api POST /backup/targets/$R/check | jget "next(t['last_check_ok'] for t in d['targets'] if t['id']==$R)")" True
check "eski yedekler görünür" "$(api GET "/backup/targets/$R/snapshots" | jget 'len(d["snapshots"]) > 3')" True

say "SFTP hedefi"
api POST /backup/targets -d "$(tbody e2e-sftp sftp "{\"host\":\"127.0.0.1\",\"port\":22,\"user\":\"root\",\"path\":\"$SFTP_REPO\"}")" >/dev/null
F=$(target_id e2e-sftp)
KEY=$(api GET /backup | jget "next(t['public_key'] for t in d['targets'] if t['id']==$F)")
has "açık anahtar üretildi" "$KEY" "ssh-ed25519 "
check "anahtar eklenmeden bağlanamaz" "$(api POST /backup/targets/$F/check | jget "next(t['last_check_ok'] for t in d['targets'] if t['id']==$F)")" False
echo "$KEY" >> /root/.ssh/authorized_keys
check "anahtar eklenince bağlanır" "$(api POST /backup/targets/$F/check | jget "next((t['last_check_ok'], t['last_check_error']) for t in d['targets'] if t['id']==$F)")" "(True, '')"
check "SFTP'ye yedek" "$(wait_job "$(api POST /backup/run -d "{\"target_id\":$F,\"scope\":{\"site_ids\":[$S]}}" | jget 'd["job_id"]')")" success
check "yedek uzak klasörde" "$([ -f $SFTP_REPO/config ] && api GET "/backup/targets/$F/snapshots" | jget 'len(d["snapshots"])')" 1
echo "sftp öncesi" > $ROOT/public/index.html
T_OLD=$T; T=$F
check "SFTP'den geri yükleme" "$(restore "{\"target_id\":$F,\"snapshot\":\"$(snap site $DOMAIN)\"}")" success
T=$T_OLD
check "dosya geri geldi" "$(cat $ROOT/public/index.html)" "ikinci hali"
check "özel anahtar yalnız root'a açık" "$(stat -c '%a' /etc/pixpanel/backup/target-$F.key)" 600

say "Temizlik"
check "hedef silinince anahtarı da silinir" "$(api DELETE /backup/targets/$F >/dev/null; [ -f /etc/pixpanel/backup/target-$F.key ] || echo yok)" yok
cleanup
check "hedef ve plan kalmadı" "$(api GET /backup | jget '(len([t for t in d["targets"] if t["name"].startswith("e2e-")]), len(d["plans"]))')" "(0, 0)"
echo "bitti"
