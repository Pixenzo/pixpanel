#!/usr/bin/env bash
# Veritabanı derinliği sınaması: boyutlar ve tablo sayısı, veritabanı kopyalama (içerik, bağımsızlık,
# kota, yetki yükselmesi, geri alma), denetim/onarım/iyileştirme görevleri ve yönetici yavaş sorgu günlüğü.
# Test makinesinde root olarak çalışır; MariaDB ve PostgreSQL kuruludur.
# Kullanım: PX_USER=... PX_PASS=... bash db-depth.sh
# İsteğe bağlı: PX_MEASURE=1 -> paneli yeniden başlatıp saatlik disk ölçümünün veritabanı kalemini bekler (~3 dk).
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-dbd
JC=/tmp/pxjar-dbd-c
JO=/tmp/pxjar-dbd-o
DB=/var/lib/pixpanel/pixpanel.db
CUSTOMER=dbdmusteri
OTHER=dbdbaskasi
PASS='Dbd-Sinama-Parolasi-1'
DBPASS='Dbd-Veritabani-Parola-1'
PKG=e2e-dbd
CONF=/etc/mysql/mariadb.conf.d/99-pixpanel-slow.cnf
W=/tmp/pxdbd
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
anon() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qiF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 300)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
sql() { python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1], timeout=10); c.execute(sys.argv[2]); c.commit()' $DB "$1"; }
wait_job() { for _ in $(seq 1 180); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
job_log() { api GET "/jobs/$1" | jget 'd["log"]'; }
# Son başlatılan görevin numarası (copy_job/maint_job alt kabukta koştuğundan dosyadan okunur).
last_job() { cat $W/lastjob; }
db_id() { api GET /databases | jget "next((x['id'] for x in d['databases'] if x['name']=='$1'), '')"; }
db_field() { api GET /databases | jget "next((x['$2'] for x in d['databases'] if x['name']=='$1'), '(yok)')"; }
user_id() { api GET /databases | jget "next((x['id'] for x in d['users'] if x['name']=='$1'), '')"; }
installed() { api GET /components | jget "next(c['installed'] for c in d if c['id']=='$1')"; }
# copy_job <çerez> <veritabanı no> <yeni ad>: kopyalamayı başlatır, görevin sonucunu yazar (başlatılamazsa HTTP hata kodu).
copy_job() {
  local out job
  out=$(call $1 POST "/databases/$2/copy" -d "{\"name\":\"$3\"}")
  job=$(jget 'd["job_id"]' <<<"$out")
  case "$job" in ''|'('*) echo "red:$(jget 'd["error"]["code"]' <<<"$out")"; return;; esac
  echo "$job" > $W/lastjob
  wait_job "$job"
}
# maint_job <veritabanı no> <işlem>: bakım görevini başlatıp sonucunu yazar.
maint_job() {
  local out job
  out=$(api POST "/databases/$1/maintenance" -d "{\"action\":\"$2\"}")
  job=$(jget 'd["job_id"]' <<<"$out")
  case "$job" in ''|'('*) echo "red:$(jget 'd["error"]["code"]' <<<"$out")"; return;; esac
  echo "$job" > $W/lastjob
  wait_job "$job"
}
my() { mariadb --default-character-set=utf8mb4 -N -B -e "$1" 2>&1; }
mydb() { mariadb --default-character-set=utf8mb4 "$1" 2>&1; }
# Müşterinin uygulama kullanıcısıyla bağlanır.
APPK() { mariadb -N -B --default-character-set=utf8mb4 -u ${CUSTOMER}_uyg -p"$DBPASS" "$1" -e "$2" 2>&1; }
pg() { runuser -u postgres -- psql -X -At -d "${2:-postgres}" -c "$1" 2>/dev/null; }
PGAPP() { PGPASSWORD=$DBPASS psql -X -At -h 127.0.0.1 -U ${CUSTOMER}_pguyg -d "$1" -c "$2" 2>&1; }
# upload <çerez> <veritabanı no> <dosya>: dökümü yükler (PostgreSQL kaynağını müşteri sahipliğinde kurmak için).
upload() { local job; job=$(curl -sk -b $1 -c $1 -H 'x-pixpanel-csrf: 1' -F "file=@$3" "$B/databases/$2/import" | jget 'd["job_id"]'); wait_job "$job"; }

cleanup() {
  for name in ${CUSTOMER}_kopya ${CUSTOMER}_kopya2 ${CUSTOMER}_zararli_kopya ${CUSTOMER}_zararli ${CUSTOMER}_kaynak ${CUSTOMER}_dolu ${CUSTOMER}_pgkopya ${CUSTOMER}_pgkaynak dbd_kurban; do
    id=$(db_id $name); [ -n "$id" ] && api DELETE /databases/$id >/dev/null
  done
  for name in ${CUSTOMER}_uyg ${CUSTOMER}_pguyg; do id=$(user_id $name); [ -n "$id" ] && api DELETE /databases/users/$id >/dev/null; done
  for name in $CUSTOMER $OTHER; do
    local uid; uid=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')"); [ -n "$uid" ] && api DELETE "/users/$uid" >/dev/null
  done
  for package in $(api GET /packages | jget "' '.join(str(p['id']) for p in d if p['name']=='$PKG')"); do api DELETE /packages/$package >/dev/null; done
  pg "DROP ROLE IF EXISTS dbd_pgkotu" >/dev/null
}

rm -rf $W; mkdir -p $W; rm -f $J $JC $JO
curl -sk -o /dev/null -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}"
cleanup
ORIG_SLOW=0; ORIG_LQT=10
if [ "$(installed mariadb)" = True ]; then
  ORIG_SLOW=$(my "SELECT @@global.slow_query_log" | tail -1); ORIG_LQT=$(my "SELECT @@global.long_query_time" | tail -1)
fi

# Müşteri (paketli: en çok 4 veritabanı, 1000 MB disk) ve başka bir müşteri.
PKGID=$(api POST /packages -d "{\"name\":\"$PKG\",\"limits\":{\"databases\":4,\"disk_mb\":1000}}" | jget 'd["id"]')
api POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":$PKGID}" >/dev/null
api POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\",\"role\":\"user\"}" >/dev/null
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\"}"
curl -sk -o /dev/null -c $JO -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\"}"
CID=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
check "müşteri ve paketi hazır" "$([ -n "$CID" ] && [ "$PKGID" -gt 0 ] 2>/dev/null && echo var)" var

if [ "$(installed mariadb)" = True ]; then
  say "MariaDB: hazırlık"
  api POST /databases -d '{"engine":"mariadb","name":"dbd_kurban"}' >/dev/null
  my "CREATE TABLE dbd_kurban.gizli (deger VARCHAR(40)); CREATE TABLE dbd_kurban.sir (deger VARCHAR(40)); INSERT INTO dbd_kurban.sir VALUES ('baska-musterinin-verisi');" >/dev/null
  call $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_kaynak\",\"user\":{\"name\":\"${CUSTOMER}_uyg\",\"password\":\"$DBPASS\"}}" >/dev/null
  SRC=$(db_id ${CUSTOMER}_kaynak)
  check "kaynak veritabanı açıldı" "$([ -n "$SRC" ] && echo var)" var
  # Tablolar ve veri; ardından yönetici (root) tanımlı görünüm ve tetikleyiciler (başka sunucudan gelmiş gibi).
  mydb ${CUSTOMER}_kaynak >/dev/null <<'XEOF'
CREATE TABLE urun (id INT AUTO_INCREMENT PRIMARY KEY, ad VARCHAR(40) NOT NULL) ENGINE=InnoDB;
INSERT INTO urun (ad) VALUES ('cay'), ('seker'), ('kahve');
CREATE TABLE eski (id INT PRIMARY KEY, deger VARCHAR(20)) ENGINE=MyISAM;
INSERT INTO eski VALUES (1, 'a'), (2, 'b');
CREATE TABLE log (ileti VARCHAR(40)) ENGINE=InnoDB;
CREATE TABLE tetikli (id INT) ENGINE=InnoDB;
CREATE DEFINER=`root`@`localhost` SQL SECURITY DEFINER VIEW v_urun AS SELECT ad FROM urun;
CREATE DEFINER=`root`@`localhost` TRIGGER urun_log AFTER INSERT ON urun FOR EACH ROW INSERT INTO log VALUES (NEW.ad);
CREATE DEFINER=`root`@`localhost` TRIGGER tetik_disari AFTER INSERT ON tetikli FOR EACH ROW INSERT INTO dbd_kurban.gizli VALUES ('tetik-calisti');
XEOF
  check "kaynağın tabloları kuruldu" "$(my "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='${CUSTOMER}_kaynak' AND table_type='BASE TABLE'")" 4

  say "MariaDB: kopyalama"
  check "kopyalama görevi başarıyla biter" "$(copy_job $JC $SRC ${CUSTOMER}_kopya)" success
  has "görev günlüğü kopyalamayı anlatır" "$(job_log $(last_job))" "kopyalan"
  K=$(db_id ${CUSTOMER}_kopya)
  check "kopya müşterinin hesabında görünür" "$(db_field ${CUSTOMER}_kopya owner)" $CUSTOMER
  check "tablo sayısı (görünüm hariç) kaynakla aynı" "$(db_field ${CUSTOMER}_kopya tables) $(db_field ${CUSTOMER}_kaynak tables)" "4 4"
  check "boyutlar listede görünür" "$(api GET /databases | jget "all(x['size'] and x['size']>0 for x in d['databases'] if x['name'] in ('${CUSTOMER}_kaynak','${CUSTOMER}_kopya'))")" True
  check "kopyanın verisi aynı (InnoDB)" "$(my "SELECT GROUP_CONCAT(ad ORDER BY id) FROM ${CUSTOMER}_kopya.urun")" cay,seker,kahve
  check "kopyanın verisi aynı (MyISAM)" "$(my "SELECT COUNT(*) FROM ${CUSTOMER}_kopya.eski")" 2
  check "görünüm, tetikleyici ve tablo kopyada var" "$(my "SELECT (SELECT COUNT(*) FROM information_schema.views WHERE table_schema='${CUSTOMER}_kopya'), (SELECT COUNT(*) FROM information_schema.triggers WHERE trigger_schema='${CUSTOMER}_kopya')")" "$(printf '1\t2')"
  check "kopyadaki görünümün tanımlayanı root değil" "$(my "SELECT COUNT(*) FROM information_schema.views WHERE table_schema='${CUSTOMER}_kopya' AND definer LIKE 'root@%'")" 0
  check "kopyadaki tetikleyicilerin tanımlayanı root değil" "$(my "SELECT COUNT(*) FROM information_schema.triggers WHERE trigger_schema='${CUSTOMER}_kopya' AND definer LIKE 'root@%'")" 0

  say "MariaDB: kopya bağımsız ve yetkisi sınırlı"
  UID_APP=$(user_id ${CUSTOMER}_uyg)
  check "müşteri kopyaya kullanıcı yetkisi verebilir" "$(status $JC PUT /databases/$K/grants/$UID_APP -d '{"access":"full"}')" 200
  APPK ${CUSTOMER}_kopya "INSERT INTO urun (ad) VALUES ('yeni')" >/dev/null
  check "kopyaya yazmak kaynağı etkilemez" "$(my "SELECT COUNT(*) FROM ${CUSTOMER}_kaynak.urun") $(my "SELECT COUNT(*) FROM ${CUSTOMER}_kopya.urun")" "3 4"
  check "kopyanın tetikleyicisi kendi tablosuna yazar" "$(my "SELECT COUNT(*) FROM ${CUSTOMER}_kaynak.log") $(my "SELECT COUNT(*) FROM ${CUSTOMER}_kopya.log")" "0 1"
  my "DELETE FROM ${CUSTOMER}_kaynak.eski" >/dev/null
  check "kaynaktan silmek kopyayı etkilemez" "$(my "SELECT COUNT(*) FROM ${CUSTOMER}_kopya.eski")" 2
  has "başka veritabanına yazan tetikleyici kopyada çalışmaz" "$(APPK ${CUSTOMER}_kopya "INSERT INTO tetikli VALUES (1)")" "denied"
  check "başkasının tablosuna hiçbir şey yazılmadı" "$(my "SELECT COUNT(*) FROM dbd_kurban.gizli")" 0
  has "root tanımlı görünüm kopyada çalışır (kendi veritabanı içinde)" "$(APPK ${CUSTOMER}_kopya "SELECT COUNT(*) FROM v_urun")" 4

  say "MariaDB: zararlı görünümlü kaynak"
  call $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_zararli\"}" >/dev/null
  ZSRC=$(db_id ${CUSTOMER}_zararli)
  mydb ${CUSTOMER}_zararli >/dev/null <<'XEOF'
CREATE TABLE t (id INT) ENGINE=InnoDB;
CREATE DEFINER=`root`@`localhost` SQL SECURITY DEFINER VIEW sizinti AS SELECT deger FROM dbd_kurban.sir;
XEOF
  RESULT=$(copy_job $JC $ZSRC ${CUSTOMER}_zararli_kopya)
  if [ "$RESULT" = failed ]; then
    echo "  OK   başkasının verisine uzanan görünüm kopyada kurulamaz, kopyalama başarısız olur"
    check "başarısız kopya geri alındı (panel kaydı)" "$(db_id ${CUSTOMER}_zararli_kopya)" ""
    check "başarısız kopya geri alındı (sunucuda)" "$(my "SHOW DATABASES LIKE '${CUSTOMER}_zararli_kopya'")" ""
    has "görev günlüğü geri almayı belirtir" "$(job_log $(last_job))" "geri alın"
  else
    ZK=$(db_id ${CUSTOMER}_zararli_kopya)
    call $JC PUT /databases/$ZK/grants/$UID_APP -d '{"access":"full"}' >/dev/null
    has "kopyalansa bile görünüm başkasının verisini okutmaz" "$(APPK ${CUSTOMER}_zararli_kopya 'SELECT * FROM sizinti')" "denied"
    check "(geri alma bu senaryoda denenemedi)" "$RESULT" success
  fi
  check "başkasının verisi yerinde" "$(my "SELECT deger FROM dbd_kurban.sir")" baska-musterinin-verisi

  say "MariaDB: ad, yetki ve kota denetimleri"
  check "var olan adla kopyalama" "$(copy_job $JC $SRC ${CUSTOMER}_kopya)" red:conflict
  for bad in 'mysql' '../etc' 'dbdmusteri_a b' 'dbdmusteri_x`; DROP DATABASE mysql; --' '-dbdmusteri_x' 'baska_ad' 'pxi_abc' 'dbdmusteri_' '' 'dbdmusteri_a\nb' "dbdmusteri_$(printf 'x%.0s' $(seq 1 45))"; do
    check "geçersiz ad reddedilir [$(head -c 24 <<<"$bad")]" "$(status $JC POST /databases/$SRC/copy -d "{\"name\":\"$bad\"}")" 400
  done
  check "geçersiz adlar veritabanı yaratmadı" "$(api GET /databases | jget "sorted(x['name'] for x in d['databases'] if x['owner']=='$CUSTOMER')")" "['${CUSTOMER}_kaynak', '${CUSTOMER}_kopya', '${CUSTOMER}_zararli']"
  check "başka hesabın veritabanında 404 (kopyalama)" "$(status $JO POST /databases/$SRC/copy -d "{\"name\":\"${OTHER}_x\"}")" 404
  check "başka hesabın veritabanında 404 (bakım)" "$(status $JO POST /databases/$SRC/maintenance -d '{"action":"check"}')" 404
  check "olmayan veritabanında 404" "$(code POST /databases/999999/copy -d '{"name":"x"}')" 404
  check "oturumsuz kopyalama 401" "$(anon POST /databases/$SRC/copy -d '{"name":"x"}')" 401
  check "oturumsuz bakım 401" "$(anon POST /databases/$SRC/maintenance -d '{"action":"check"}')" 401
  # Disk kotası dolu: yeni veritabanı açılamaz. Ölçüm saatte bir yapıldığından kayıt doğrudan yazılır.
  sql "INSERT INTO account_usage (user_id, disk_bytes, measured_at) VALUES ($CID, 2097152000, strftime('%s','now')) ON CONFLICT(user_id) DO UPDATE SET disk_bytes = excluded.disk_bytes"
  check "kota doluyken kopya reddedilir" "$(copy_job $JC $SRC ${CUSTOMER}_kopya2)" red:bad_request
  has "kota doluyken neden açıklanır" "$(call $JC POST /databases/$SRC/copy -d "{\"name\":\"${CUSTOMER}_kopya2\"}")" "Disk kotası doldu"
  # Kotaya 1 bayt kala: kopya kaynağın boyutu kadar yer isteyeceğinden sığmaz.
  sql "UPDATE account_usage SET disk_bytes = 1048575999 WHERE user_id = $CID"
  has "kopya kotayı aşacaksa baştan reddedilir" "$(call $JC POST /databases/$SRC/copy -d "{\"name\":\"${CUSTOMER}_kopya2\"}")" "kotasını aşar"
  sql "UPDATE account_usage SET disk_bytes = 1048576 WHERE user_id = $CID"
  check "reddedilen kopyalar veritabanı bırakmadı" "$(db_id ${CUSTOMER}_kopya2)" ""
  # Veritabanı sayısı sınırı (paket: 4): 3 var, dördüncüyü açıp sınırı doldur.
  call $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_dolu\"}" >/dev/null
  has "veritabanı sayısı sınırında kopya reddedilir" "$(call $JC POST /databases/$SRC/copy -d "{\"name\":\"${CUSTOMER}_kopya2\"}")" "en çok 4 veritabanı"
  api DELETE /databases/$(db_id ${CUSTOMER}_dolu) >/dev/null

  say "MariaDB: denetim, onarım, iyileştirme"
  check "denetim görevi başarıyla biter" "$(maint_job $SRC check)" success
  has "denetim çıktısı tabloları listeler" "$(job_log $(last_job))" "${CUSTOMER}_kaynak.urun"
  check "onarım görevi başarıyla biter" "$(maint_job $SRC repair)" success
  check "iyileştirme görevi başarıyla biter" "$(maint_job $SRC optimize)" success
  check "geçersiz işlem 400" "$(code POST /databases/$SRC/maintenance -d '{"action":"drop"}')" 400
  check "boş işlem 400" "$(code POST /databases/$SRC/maintenance -d '{"action":""}')" 400
  check "işlem adı komut gibi görünemez" "$(code POST /databases/$SRC/maintenance -d '{"action":"--all-databases"}')" 400
  # Aynı veritabanında eşzamanlı işlem: tablo kilidi tutan bir oturum iyileştirmeyi bekletir.
  (mariadb -e "LOCK TABLES ${CUSTOMER}_kaynak.urun READ; SELECT SLEEP(6); UNLOCK TABLES;" >/dev/null 2>&1) &
  LOCKER=$!
  sleep 1
  OUT=$(api POST /databases/$SRC/maintenance -d '{"action":"optimize"}'); SLOW_JOB=$(jget 'd["job_id"]' <<<"$OUT")
  sleep 1
  check "aynı veritabanında ikinci bakım 409" "$(code POST /databases/$SRC/maintenance -d '{"action":"check"}')" 409
  check "bakım sürerken kopyalama 409" "$(copy_job $JC $SRC ${CUSTOMER}_kopya2)" red:conflict
  check "bakım sürerken silme 409" "$(code DELETE /databases/$SRC)" 409
  check "başka veritabanında bakım engellenmez" "$(maint_job $ZSRC check)" success
  check "bekleyen iyileştirme kilit kalkınca biter" "$(wait_job "$SLOW_JOB")" success
  wait $LOCKER 2>/dev/null
  check "iş bitince veritabanı yeniden kullanılabilir" "$(maint_job $SRC check)" success

  say "MariaDB: yavaş sorgu günlüğü"
  check "müşteri günlüğü göremez (GET)" "$(status $JC GET /databases/slow-log)" 403
  check "müşteri günlüğü açamaz (PUT)" "$(status $JC PUT /databases/slow-log -d '{"enabled":true,"threshold":1}')" 403
  check "oturumsuz günlük 401" "$(anon GET /databases/slow-log)" 401
  check "geçersiz eşik 400" "$(code PUT /databases/slow-log -d '{"enabled":true,"threshold":3}')" 400
  check "sıfır eşik 400" "$(code PUT /databases/slow-log -d '{"enabled":true,"threshold":0}')" 400
  BAD_CODE=$(code PUT /databases/slow-log -d '{"enabled":true,"threshold":"1; SET GLOBAL read_only=1"}')
  check "eşik metin olamaz (400/422)" "$([ "$BAD_CODE" = 400 ] || [ "$BAD_CODE" = 422 ] && echo red)" red
  check "reddedilen eşik sunucuyu değiştirmedi" "$(my "SELECT @@global.read_only")" 0
  check "yönetici günlüğü açar (1 sn)" "$(code PUT /databases/slow-log -d '{"enabled":true,"threshold":1}')" 200
  check "sunucuda anında açıldı" "$(my "SELECT @@global.slow_query_log, @@global.long_query_time = 1")" "$(printf '1\t1')"
  check "kalıcı dosya yazıldı" "$(test -f $CONF && grep -c '^long_query_time = 1$' $CONF)" 1
  check "ayar uçtan okunur" "$(api GET /databases/slow-log | jget "(d['enabled'], d['threshold'], d['persistent'])")" "(True, 1.0, True)"
  mariadb -e "SELECT SLEEP(2)" >/dev/null 2>&1
  sleep 1
  LOG=$(api GET /databases/slow-log)
  check "SELECT SLEEP(2) günlükte görünür" "$(jget "any('SLEEP(2)' in e['query'].upper() and e['query_time'] >= 1.9 and e['time'] > 0 for e in d['entries'])" <<<"$LOG")" True
  check "kayıtta kullanıcı ve süre bilgisi var" "$(jget "all(e['user'] and e['query_time'] > 0 for e in d['entries'][:3])" <<<"$LOG")" True
  check "müşteri açıkken de göremez" "$(status $JC GET /databases/slow-log)" 403
  check "eşik 5 sn olarak değiştirilir" "$(code PUT /databases/slow-log -d '{"enabled":true,"threshold":5}') $(my "SELECT @@global.long_query_time = 5")" "200 1"
  check "yönetici günlüğü kapatır" "$(code PUT /databases/slow-log -d '{"enabled":false,"threshold":5}')" 200
  check "kapatınca sunucuda kapalı" "$(my "SELECT @@global.slow_query_log")" 0
  check "kapatınca kalıcı dosya kalkar" "$(test -e $CONF && echo var || echo yok)" yok
  check "kapalı ayar uçtan okunur" "$(api GET /databases/slow-log | jget "(d['enabled'], d['persistent'])")" "(False, False)"
  # Sınama öncesi durum geri konur.
  if [ "$ORIG_SLOW" = 1 ]; then my "SET GLOBAL long_query_time = $ORIG_LQT; SET GLOBAL slow_query_log = 1" >/dev/null; else my "SET GLOBAL long_query_time = $ORIG_LQT" >/dev/null; fi

  say "MariaDB: disk kullanımında veritabanı kalemi"
  check "kullanım görünümünde veritabanı kalemi alanı var" "$(call $JC GET /packages/usage | jget '"database_bytes" in d')" True
  if [ "${PX_MEASURE:-}" = 1 ]; then
    systemctl restart pixpanel; sleep 5
    [ "$(code GET /auth/me)" = 200 ] || curl -sk -o /dev/null -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}"
    for _ in $(seq 1 60); do
      v=$(api GET /users/$CID/usage | jget 'd["database_bytes"]')
      [ "$v" != None ] && [ "$v" -gt 0 ] 2>/dev/null && break
      sleep 5
    done
    check "ölçüm veritabanı payını yazdı" "$([ "$v" -gt 0 ] 2>/dev/null && echo var)" var
    check "disk kullanımı veritabanı payını kapsar" "$(api GET /users/$CID/usage | jget "d['disk_bytes'] >= d['database_bytes'] > 0")" True
  else
    echo "  (saatlik ölçüm sınaması atlandı; PX_MEASURE=1 ile açılır)"
  fi
else
  echo "  (MariaDB kurulu değil; bölüm atlandı)"
fi

if [ "$(installed postgresql)" = True ]; then
  say "PostgreSQL: hazırlık"
  call $JC POST /databases -d "{\"engine\":\"postgresql\",\"name\":\"${CUSTOMER}_pgkaynak\",\"user\":{\"name\":\"${CUSTOMER}_pguyg\",\"password\":\"$DBPASS\"}}" >/dev/null
  PS=$(db_id ${CUSTOMER}_pgkaynak)
  check "PostgreSQL kaynağı açıldı" "$([ -n "$PS" ] && echo var)" var
  # Nesneler yönetici rolüyle kurulur: tetikleyici işlevi SECURITY DEFINER ve sahibi postgres (yetki yükseltme denemesi).
  pg "CREATE TABLE urunler (id serial PRIMARY KEY, ad text); INSERT INTO urunler (ad) VALUES ('cay'), ('seker'), ('kahve'); CREATE TABLE tetikli (id int); CREATE FUNCTION yukselt() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER AS \$fn\$ BEGIN CREATE ROLE dbd_pgkotu SUPERUSER LOGIN PASSWORD 'x'; RETURN NEW; END \$fn\$; CREATE TRIGGER tetik AFTER INSERT ON tetikli FOR EACH ROW EXECUTE FUNCTION yukselt(); CREATE VIEW v_urun AS SELECT ad FROM urunler;" ${CUSTOMER}_pgkaynak >/dev/null
  check "kaynağın tabloları kuruldu" "$(pg "SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'" ${CUSTOMER}_pgkaynak)" 2

  say "PostgreSQL: kopyalama"
  check "kopyalama görevi başarıyla biter" "$(copy_job $JC $PS ${CUSTOMER}_pgkopya)" success
  PK=$(db_id ${CUSTOMER}_pgkopya)
  check "kopya müşterinin hesabında görünür" "$(db_field ${CUSTOMER}_pgkopya owner)" $CUSTOMER
  check "tablo sayısı kaynakla aynı" "$(db_field ${CUSTOMER}_pgkopya tables) $(db_field ${CUSTOMER}_pgkaynak tables)" "2 2"
  check "boyutlar listede görünür" "$(api GET /databases | jget "all(x['size'] and x['size']>0 for x in d['databases'] if x['name'] in ('${CUSTOMER}_pgkaynak','${CUSTOMER}_pgkopya'))")" True
  check "kopyanın verisi aynı" "$(pg "SELECT string_agg(ad, ',' ORDER BY id) FROM urunler" ${CUSTOMER}_pgkopya)" cay,seker,kahve
  check "görünüm kopyada çalışır" "$(pg "SELECT COUNT(*) FROM v_urun" ${CUSTOMER}_pgkopya)" 3
  check "kopyadaki nesnelerin sahibi veritabanının ortak rolü" "$(pg "SELECT DISTINCT tableowner FROM pg_tables WHERE schemaname='public'" ${CUSTOMER}_pgkopya)" "pixpanel_own_${CUSTOMER}_pgkopya"
  check "SECURITY DEFINER işlevinin sahibi yönetici değil" "$(pg "SELECT r.rolsuper FROM pg_proc p JOIN pg_roles r ON r.oid = p.proowner WHERE p.proname = 'yukselt'" ${CUSTOMER}_pgkopya)" f
  check "geçici giriş rolü geride kalmadı" "$(pg "SELECT COUNT(*) FROM pg_roles WHERE rolname LIKE 'pxi\_%'")" 0
  PUID=$(user_id ${CUSTOMER}_pguyg)
  check "müşteri kopyaya kullanıcı yetkisi verebilir" "$(status $JC PUT /databases/$PK/grants/$PUID -d '{"access":"full"}')" 200
  has "tetikleyici kopyada yönetici yetkisiyle çalışmaz" "$(PGAPP ${CUSTOMER}_pgkopya "INSERT INTO tetikli VALUES (1)")" "ERROR"
  check "yetkili rol açılamadı" "$(pg "SELECT COUNT(*) FROM pg_roles WHERE rolname='dbd_pgkotu'")" 0
  pg "INSERT INTO urunler (ad) VALUES ('yeni')" ${CUSTOMER}_pgkopya >/dev/null
  check "kopyaya yazmak kaynağı etkilemez" "$(pg "SELECT COUNT(*) FROM urunler" ${CUSTOMER}_pgkaynak) $(pg "SELECT COUNT(*) FROM urunler" ${CUSTOMER}_pgkopya)" "3 4"
  check "var olan adla kopyalama" "$(copy_job $JC $PS ${CUSTOMER}_pgkopya)" red:conflict
  check "başka hesabın veritabanında 404" "$(status $JO POST /databases/$PS/copy -d "{\"name\":\"${OTHER}_x\"}")" 404

  say "PostgreSQL: bakım"
  check "denetim PostgreSQL'de yok (400)" "$(code POST /databases/$PS/maintenance -d '{"action":"check"}')" 400
  check "onarım PostgreSQL'de yok (400)" "$(code POST /databases/$PS/maintenance -d '{"action":"repair"}')" 400
  check "iyileştirme görevi başarıyla biter" "$(maint_job $PS optimize)" success
  has "iyileştirme çıktısı VACUUM ayrıntısını içerir" "$(job_log $(last_job))" "vacuum"
  check "geçersiz işlem 400" "$(code POST /databases/$PS/maintenance -d '{"action":"x"}')" 400
  check "başka hesabın veritabanında 404 (bakım)" "$(status $JO POST /databases/$PS/maintenance -d '{"action":"optimize"}')" 404
else
  echo "  (PostgreSQL kurulu değil; bölüm atlandı)"
fi

say "Temizlik"
cleanup
check "sınama kaynakları silindi" "$(api GET /databases | jget "[x['name'] for x in d['databases'] if x['name'].startswith('dbd')]")" "[]"
check "sınama hesabı ve paketi silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$CUSTOMER', '$OTHER')]") $(api GET /packages | jget "[p['name'] for p in d if p['name']=='$PKG']")" "[] []"
rm -rf $W $J $JC $JO
echo "bitti"
