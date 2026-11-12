#!/usr/bin/env bash
# Veritabanlarına uzaktan erişim sınaması (MariaDB ve PostgreSQL): sunucu ayarı (yalnızca yönetici),
# kullanıcı başına bağlantı adresi, geçersiz adresler, pg_hba.conf bloğu, geri alma ve eski hâle dönüş.
# Test makinesinde root olarak çalışır; makinenin kendi LAN adresinden kendine bağlanılarak sınanır.
# Kullanım: PX_USER=... PX_PASS=... bash db-remote.sh
#
# Sınama başında iki motorun uzaktan erişim ayarı okunur, sonda AYNEN geri getirilir. Yalnızca "dbrmusteri"
# öneki ve "dbrbaskasi" hesabıyla açılan geçici kaynaklar silinir; var olan veritabanlarına ve kullanıcılara dokunulmaz.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-dbr
JC=/tmp/pxjar-dbr-c
JO=/tmp/pxjar-dbr-o
CUSTOMER=dbrmusteri
OTHER=dbrbaskasi
PASS='Dbr-Sinama-Parolasi-1'
DBPASS='Dbr-Veritabani-Parola-1'
W=/tmp/pxdbr
LAN=$(hostname -I | awk '{print $1}')
# Belgelendirme için ayrılmış, bu makineye ait olmayan adres: "yanlış adres" olarak kullanılır.
WRONG=203.0.113.77
NET24="${LAN%.*}.0/24"
MYCONF=/etc/mysql/mariadb.conf.d/99-pixpanel-remote.cnf
DB=/var/lib/pixpanel/pixpanel.db

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
# Denetim günlüğünde bu işlem kodundan kaç kayıt var?
audit_count() { python3 -c 'import sqlite3,sys; print(sqlite3.connect(sys.argv[1], timeout=10).execute("SELECT COUNT(*) FROM audit_log WHERE action=?", (sys.argv[2],)).fetchone()[0])' $DB "$1"; }
installed() { api GET /components | jget "next(c['installed'] for c in d if c['id']=='$1')"; }
db_id() { api GET /databases | jget "next((x['id'] for x in d['databases'] if x['name']=='$1'), '')"; }
user_id() { api GET /databases | jget "next((x['id'] for x in d['users'] if x['name']=='$1'), '')"; }
user_host() { api GET /databases | jget "next((x['host'] for x in d['users'] if x['name']=='$1'), '(yok)')"; }
# Yönetici durum ucundan bir motorun alanı.
remote_field() { api GET /databases/remote | jget "next(e['$2'] for e in d['engines'] if e['id']=='$1')"; }

# Sunucunun belirli porttaki dinleme adresleri; yerel olmayan (127.0.0.1 ya da ::1 dışı) var mı?
listen_all() { ss -H -ltn "( sport = :$1 )" | awk '{print $4}' | sort | tr '\n' ' '; }
listens_remote() { if ss -H -ltn "( sport = :$1 )" | awk '{print $4}' | grep -qvE '^(127\.0\.0\.1|\[::1\]|::1):'; then echo evet; else echo hayir; fi; }

my() { mariadb -N -B -e "$1" 2>&1; }
# my_can <kullanıcı> <adres> : MariaDB'ye TCP ile bağlanılabiliyor mu? (parola ortam değişkeniyle verilir)
my_can() {
  local out
  out=$(MYSQL_PWD=$DBPASS mariadb --skip-ssl --connect-timeout=5 -h "$2" -P 3306 -u "$1" -N -B -e 'SELECT 1' 2>/dev/null)
  if [ "$out" = 1 ]; then echo bağlandı; else echo reddedildi; fi
}
pg() { runuser -u postgres -- psql -X -At -d "${2:-postgres}" -c "$1" 2>/dev/null; }
# pg_can <kullanıcı> <veritabanı> [adres] [sslmode] : PostgreSQL'e TCP ile bağlanılabiliyor mu?
pg_can() {
  local out
  out=$(PGPASSWORD=$DBPASS psql "host=${3:-$LAN} port=5432 user=$1 dbname=$2 sslmode=${4:-require} connect_timeout=5" -X -At -c 'SELECT session_user' 2>/dev/null)
  if [ "$out" = "$1" ]; then echo bağlandı; else echo reddedildi; fi
}
# Panel bloğunun dışındaki satırlar (önce/sonra karşılaştırması için).
strip_block() { awk '/^# PixPanel başla$/{s=1;next} /^# PixPanel bitti$/{s=0;next} !s' "$1"; }
block_of() { awk '/^# PixPanel başla$/{s=1} s{print} /^# PixPanel bitti$/{s=0}' "$1"; }

BAD_HOSTS=('1.2.3.4; DROP' '1.2.3.4\n' '%%' '1.2.3.4 5' "1.2.3.4'" '1.2.3.4/33' '1.2.3.4/0' '*' 'sunucu.ornek' '1.2.3.4\r\nhostssl all all 0.0.0.0/0 trust' '1.2.3.4#' '1.2.%.4' '0.0.0.0' '::')

PGCONF=""; HBA=""
cleanup() {
  for name in ${CUSTOMER}_my1 ${CUSTOMER}_pg1 ${CUSTOMER}_pg2; do
    id=$(db_id $name); [ -n "$id" ] && api DELETE /databases/$id >/dev/null
  done
  for name in ${CUSTOMER}_myl ${CUSTOMER}_myn ${CUSTOMER}_myy ${CUSTOMER}_pgl ${CUSTOMER}_pgn ${CUSTOMER}_pgy ${CUSTOMER}_bad; do
    id=$(user_id $name); [ -n "$id" ] && api DELETE /databases/users/$id >/dev/null
  done
  for name in $CUSTOMER $OTHER; do
    local uid; uid=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')"); [ -n "$uid" ] && api DELETE "/users/$uid" >/dev/null
  done
}
# Beklenmedik çıkışta bile değişmez bayrak kalkar.
trap '[ -n "$HBA" ] && chattr -i "$HBA" 2>/dev/null' EXIT

rm -rf $W; mkdir -p $W; rm -f $J $JC $JO
curl -sk -o /dev/null -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}"
cleanup
api POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}" >/dev/null
api POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\",\"role\":\"user\"}" >/dev/null
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\"}"
curl -sk -o /dev/null -c $JO -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\"}"
check "müşteri ve diğer hesap hazır" "$(call $JC GET /auth/me | jget 'd["username"]')" $CUSTOMER
echo "  (bu makinenin LAN adresi: $LAN; yanlış adres olarak $WRONG kullanılır)"

say "Yetki denetimi (iki motor)"
check "oturumsuz durum 401" "$(anon GET /databases/remote)" 401
check "oturumsuz ayar 401" "$(anon PUT /databases/remote/mariadb -d '{"enabled":true}')" 401
check "müşteri durumu göremez (403)" "$(status $JC GET /databases/remote)" 403
check "müşteri MariaDB ayarını değiştiremez (403)" "$(status $JC PUT /databases/remote/mariadb -d '{"enabled":true}')" 403
check "müşteri PostgreSQL ayarını değiştiremez (403)" "$(status $JC PUT /databases/remote/postgresql -d '{"enabled":true}')" 403
check "müşteri kapatamaz da (403)" "$(status $JC PUT /databases/remote/mariadb -d '{"enabled":false}')" 403
check "bilinmeyen motor 400" "$(code PUT /databases/remote/oracle -d '{"enabled":true}')" 400
check "oturumsuz adres değiştirme 401" "$(anon PUT /databases/users/1/host -d '{"host":"%"}')" 401
check "olmayan kullanıcının adresi 404" "$(code PUT /databases/users/999999/host -d '{"host":"%"}')" 404
has "durum ucu iki motoru da anlatır" "$(api GET /databases/remote | jget "sorted((e['id'], e['installed']) for e in d['engines'])")" "('mariadb'"
check "durum ucunda güvenlik duvarı bilgisi var" "$(api GET /databases/remote | jget "all('firewall_port_open' in e and 'remote_users' in e and 'remote_enabled' in e for e in d['engines'])")" True

# ===================================================================== MariaDB
if [ "$(installed mariadb)" = True ]; then
  say "MariaDB: başlangıç durumu"
  MY_WAS=false; [ -f $MYCONF ] && MY_WAS=true
  [ -f $MYCONF ] && cp -p $MYCONF $W/my.orig
  MY_USERS0=$(remote_field mariadb remote_users)
  echo "  (başlangıçta uzaktan erişim ayarı: $MY_WAS; uzak adresli kullanıcı: $MY_USERS0)"
  # Sınama "kapalı" durumdan başlar.
  [ "$MY_WAS" = true ] && api PUT /databases/remote/mariadb -d '{"enabled":false}' >/dev/null
  check "ayar kapalı" "$(remote_field mariadb remote_enabled)" False
  check "yalnızca yerel dinleme" "$(listens_remote 3306)" hayir

  say "MariaDB: geçersiz adresler"
  for bad in "${BAD_HOSTS[@]}"; do
    check "kullanıcı oluşturma reddedilir [$(head -c 22 <<<"$bad")]" "$(status $JC POST /databases/users -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_bad\",\"password\":\"$DBPASS\",\"host\":\"$bad\"}")" 400
  done
  check "geçersiz adreslerle kullanıcı açılmadı" "$(user_id ${CUSTOMER}_bad)" ""
  check "kötü adresli hesap sunucuda yok" "$(my "SELECT COUNT(*) FROM mysql.user WHERE User='${CUSTOMER}_bad'")" 0

  say "MariaDB: hazırlık (müşteri kendi kullanıcılarını açar)"
  call $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_my1\"}" >/dev/null
  D=$(db_id ${CUSTOMER}_my1)
  check "veritabanı açıldı" "$([ -n "$D" ] && echo var)" var
  for spec in "myl:localhost" "myn:$LAN" "myy:$WRONG"; do
    call $JC POST /databases/users -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_${spec%%:*}\",\"password\":\"$DBPASS\",\"host\":\"${spec#*:}\"}" >/dev/null
  done
  UL=$(user_id ${CUSTOMER}_myl); UN=$(user_id ${CUSTOMER}_myn); UY=$(user_id ${CUSTOMER}_myy)
  for uid in $UL $UN $UY; do call $JC PUT /databases/$D/grants/$uid -d '{"access":"full"}' >/dev/null; done
  check "üç kullanıcı kaydedildi" "$([ -n "$UL" ] && [ -n "$UN" ] && [ -n "$UY" ] && echo var)" var
  check "adresler kaydedildi" "$(user_host ${CUSTOMER}_myl) $(user_host ${CUSTOMER}_myn) $(user_host ${CUSTOMER}_myy)" "localhost $LAN $WRONG"
  check "hesaplar sunucuda doğru adresle var" "$(my "SELECT GROUP_CONCAT(CONCAT(User,'@',Host) ORDER BY User) FROM mysql.user WHERE User LIKE '${CUSTOMER}\\_my%'")" "${CUSTOMER}_myl@localhost,${CUSTOMER}_myn@$LAN,${CUSTOMER}_myy@$WRONG"
  check "uzak adresli kullanıcı sayısı arttı" "$(remote_field mariadb remote_users)" "$((MY_USERS0 + 2))"

  say "MariaDB: ayar kapalıyken bağlantı reddedilir"
  check "LAN adresinden bağlantı reddedilir" "$(my_can ${CUSTOMER}_myn $LAN)" reddedildi
  check "yerel kullanıcı LAN adresinden bağlanamaz" "$(my_can ${CUSTOMER}_myl $LAN)" reddedildi
  check "müşteri, kapalı ayarı genel görünümde görür" "$(call $JC GET /databases | jget "next(e['remote_enabled'] for e in d['engines'] if e['id']=='mariadb')")" False

  say "MariaDB: yönetici ayarı açar"
  OUT=$(api PUT /databases/remote/mariadb -d '{"enabled":true}')
  check "açma 200 ve değişiklik yapıldı" "$(jget 'd.get("changed")' <<<"$OUT")" True
  check "yapılandırma dosyası yazıldı" "$(test -f $MYCONF && grep -c "^bind-address = 0.0.0.0" $MYCONF)" 1
  check "MariaDB çalışıyor" "$(systemctl is-active mariadb)" active
  check "yerel olmayan arayüzde de dinliyor" "$(listens_remote 3306)" evet
  check "durum ucu açık gösterir" "$(remote_field mariadb remote_enabled) $(remote_field mariadb local_only)" "True False"
  check "durum ucu portu bildirir" "$(remote_field mariadb port)" 3306
  check "müşteri açık ayarı genel görünümde görür" "$(call $JC GET /databases | jget "next(e['remote_enabled'] for e in d['engines'] if e['id']=='mariadb')")" True
  check "ikinci açma değişiklik yapmaz" "$(api PUT /databases/remote/mariadb -d '{"enabled":true}' | jget 'd.get("changed")')" False
  check "panel veritabanlarını listeleyebilir" "$(code GET /databases)" 200

  say "MariaDB: adrese göre bağlantı"
  check "LAN adresine izinli kullanıcı bağlanır" "$(my_can ${CUSTOMER}_myn $LAN)" bağlandı
  check "localhost kullanıcısı LAN adresinden bağlanamaz" "$(my_can ${CUSTOMER}_myl $LAN)" reddedildi
  check "yanlış adrese izinli kullanıcı bağlanamaz" "$(my_can ${CUSTOMER}_myy $LAN)" reddedildi
  # skip-name-resolve sonrası yerel hesapların 127.0.0.1 üzerinden TCP ile bağlanmaya devam ettiği doğrulanır.
  check "localhost kullanıcısı 127.0.0.1 üzerinden bağlanır" "$(my_can ${CUSTOMER}_myl 127.0.0.1)" bağlandı
  check "kök hesabı uzaktan parolasız bağlanamaz" "$(MYSQL_PWD= mariadb --skip-ssl --connect-timeout=5 -h $LAN -u root -N -B -e 'SELECT 1' 2>&1 | grep -c -i 'denied')" 1

  say "MariaDB: kullanıcının adresi değiştirilir"
  check "müşteri başkasının kullanıcısını değiştiremez (404)" "$(status $JO PUT /databases/users/$UN/host -d '{"host":"%"}')" 404
  for bad in "${BAD_HOSTS[@]}"; do
    check "adres değiştirme reddedilir [$(head -c 22 <<<"$bad")]" "$(status $JC PUT /databases/users/$UN/host -d "{\"host\":\"$bad\"}")" 400
  done
  check "reddedilen adresler kaydı bozmadı" "$(user_host ${CUSTOMER}_myn)" "$LAN"
  check "eski adrese bağlantı hâlâ çalışır" "$(my_can ${CUSTOMER}_myn $LAN)" bağlandı
  check "adres yanlış adrese çevrilir" "$(status $JC PUT /databases/users/$UN/host -d "{\"host\":\"$WRONG\"}")" 200
  check "eski adres artık bağlanamaz" "$(my_can ${CUSTOMER}_myn $LAN)" reddedildi
  check "hesap yeni adresle sunucuda var" "$(my "SELECT COUNT(*) FROM mysql.user WHERE User='${CUSTOMER}_myn' AND Host='$WRONG'")" 1
  check "eski adresli hesap sunucudan kalktı" "$(my "SELECT COUNT(*) FROM mysql.user WHERE User='${CUSTOMER}_myn' AND Host='$LAN'")" 0
  has "yetkiler yeni hesaba taşındı" "$(my "SHOW GRANTS FOR '${CUSTOMER}_myn'@'$WRONG'")" "_my1"
  check "panel kaydı güncellendi" "$(user_host ${CUSTOMER}_myn)" "$WRONG"
  check "aynı adresi yeniden kaydetmek zararsız (200)" "$(status $JC PUT /databases/users/$UN/host -d "{\"host\":\"$WRONG\"}")" 200
  check "CIDR aralığına çevrilir" "$(status $JC PUT /databases/users/$UN/host -d "{\"host\":\"$NET24\"}")" 200
  check "CIDR kaydı kanonik yazılır" "$(user_host ${CUSTOMER}_myn)" "$NET24"
  check "CIDR hesabı MariaDB'de ağ/maske olarak var" "$(my "SELECT COUNT(*) FROM mysql.user WHERE User='${CUSTOMER}_myn' AND Host='${NET24%/*}/255.255.255.0'")" 1
  check "aralıktaki adresten bağlanılır" "$(my_can ${CUSTOMER}_myn $LAN)" bağlandı
  check "her yer (%) müşteriye de açık" "$(status $JC PUT /databases/users/$UN/host -d '{"host":"%"}')" 200
  check "her yerden bağlanılır" "$(my_can ${CUSTOMER}_myn $LAN)" bağlandı
  check "yerel adrese döndürülür" "$(status $JC PUT /databases/users/$UN/host -d '{"host":"localhost"}')" 200
  check "yerel adrese dönünce LAN reddedilir" "$(my_can ${CUSTOMER}_myn $LAN)" reddedildi
  check "boş adres yerel sayılır" "$(status $JC PUT /databases/users/$UN/host -d '{"host":""}')" 200
  check "yetki kaybolmadı (yerel soketten giriş)" "$(MYSQL_PWD=$DBPASS mariadb -u ${CUSTOMER}_myn -N -B -e 'SELECT 1' ${CUSTOMER}_my1 2>&1)" 1
  check "adres değişiklikleri denetim günlüğünde (database.user.host)" "$([ "$(audit_count database.user.host)" -gt 0 ] && echo var)" var
  check "açma denetim günlüğünde (database.remote.enable)" "$([ "$(audit_count database.remote.enable)" -gt 0 ] && echo var)" var

  say "MariaDB: yönetici ayarı kapatır"
  OUT=$(api PUT /databases/remote/mariadb -d '{"enabled":false}')
  check "kapatma 200 ve değişiklik yapıldı" "$(jget 'd.get("changed")' <<<"$OUT")" True
  check "yapılandırma dosyası silindi" "$(test -e $MYCONF && echo var || echo yok)" yok
  check "MariaDB çalışıyor" "$(systemctl is-active mariadb)" active
  check "yeniden yalnızca yerel dinleme" "$(listens_remote 3306)" hayir
  check "durum ucu kapalı gösterir" "$(remote_field mariadb remote_enabled)" False
  check "LAN adresinden bağlantı yine reddedilir" "$(my_can ${CUSTOMER}_myy $LAN)" reddedildi
  check "ikinci kapatma değişiklik yapmaz" "$(api PUT /databases/remote/mariadb -d '{"enabled":false}' | jget 'd.get("changed")')" False
  check "kapatma denetim günlüğünde (database.remote.disable)" "$([ "$(audit_count database.remote.disable)" -gt 0 ] && echo var)" var
  check "kullanıcı kayıtları silinmedi" "$(user_host ${CUSTOMER}_myy)" "$WRONG"

  say "MariaDB: başlangıç durumuna dönüş"
  if [ "$MY_WAS" = true ]; then
    api PUT /databases/remote/mariadb -d '{"enabled":true}' >/dev/null
    check "ayar eskisi gibi açık" "$(remote_field mariadb remote_enabled)" True
  else
    check "ayar eskisi gibi kapalı" "$(remote_field mariadb remote_enabled)" False
  fi
else
  echo "  (MariaDB kurulu değil; bölüm atlandı)"
fi

# ================================================================= PostgreSQL
if [ "$(installed postgresql)" = True ]; then
  say "PostgreSQL: başlangıç durumu"
  CL=$(pg_lsclusters -h | awk '$3==5432 {print $1 " " $2; exit}')
  [ -z "$CL" ] && CL=$(pg_lsclusters -h | awk '{print $1 " " $2; exit}')
  PGVER=${CL% *}; PGCLUSTER=${CL#* }
  PGCONF=/etc/postgresql/$PGVER/$PGCLUSTER/conf.d/90-pixpanel.conf
  HBA=/etc/postgresql/$PGVER/$PGCLUSTER/pg_hba.conf
  PG_WAS=false; [ -f $PGCONF ] && PG_WAS=true
  cp -p $HBA $W/hba.orig
  HBA_OWNER=$(stat -c '%U:%G %a' $HBA)
  PG_USERS0=$(remote_field postgresql remote_users)
  echo "  (küme $PGVER/$PGCLUSTER; başlangıçta uzaktan erişim ayarı: $PG_WAS; pg_hba.conf $HBA_OWNER)"
  [ "$PG_WAS" = true ] && api PUT /databases/remote/postgresql -d '{"enabled":false}' >/dev/null
  check "ayar kapalı" "$(remote_field postgresql remote_enabled)" False
  check "yalnızca yerel dinleme" "$(listens_remote 5432)" hayir
  check "kapalıyken pg_hba.conf'ta panel bloğu yok" "$(grep -c '^# PixPanel başla$' $HBA)" 0

  say "PostgreSQL: geçersiz adresler"
  for bad in "${BAD_HOSTS[@]}" '10.%'; do
    check "kullanıcı oluşturma reddedilir [$(head -c 22 <<<"$bad")]" "$(status $JC POST /databases/users -d "{\"engine\":\"postgresql\",\"name\":\"${CUSTOMER}_bad\",\"password\":\"$DBPASS\",\"host\":\"$bad\"}")" 400
  done
  check "geçersiz adreslerle kullanıcı açılmadı" "$(user_id ${CUSTOMER}_bad)" ""
  check "kötü adresli rol sunucuda yok" "$(pg "SELECT COUNT(*) FROM pg_roles WHERE rolname='${CUSTOMER}_bad'")" 0

  say "PostgreSQL: hazırlık"
  call $JC POST /databases -d "{\"engine\":\"postgresql\",\"name\":\"${CUSTOMER}_pg1\"}" >/dev/null
  call $JC POST /databases -d "{\"engine\":\"postgresql\",\"name\":\"${CUSTOMER}_pg2\"}" >/dev/null
  D1=$(db_id ${CUSTOMER}_pg1); D2=$(db_id ${CUSTOMER}_pg2)
  for spec in "pgl:localhost" "pgn:$LAN" "pgy:$WRONG"; do
    call $JC POST /databases/users -d "{\"engine\":\"postgresql\",\"name\":\"${CUSTOMER}_${spec%%:*}\",\"password\":\"$DBPASS\",\"host\":\"${spec#*:}\"}" >/dev/null
  done
  PL=$(user_id ${CUSTOMER}_pgl); PN=$(user_id ${CUSTOMER}_pgn); PY=$(user_id ${CUSTOMER}_pgy)
  for uid in $PL $PN $PY; do call $JC PUT /databases/$D1/grants/$uid -d '{"access":"full"}' >/dev/null; done
  check "veritabanları ve kullanıcılar hazır" "$([ -n "$D1" ] && [ -n "$D2" ] && [ -n "$PL" ] && [ -n "$PN" ] && [ -n "$PY" ] && echo var)" var
  check "adresler kaydedildi" "$(user_host ${CUSTOMER}_pgl) $(user_host ${CUSTOMER}_pgn) $(user_host ${CUSTOMER}_pgy)" "localhost $LAN $WRONG"
  check "uzak adresli kullanıcı sayısı arttı" "$(remote_field postgresql remote_users)" "$((PG_USERS0 + 2))"
  check "kapalıyken blok yazılmadı (adresler yalnızca kayıtlı)" "$(grep -c '^# PixPanel başla$' $HBA)" 0

  say "PostgreSQL: ayar kapalıyken bağlantı reddedilir"
  check "LAN adresinden bağlantı reddedilir" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" reddedildi
  check "yerel kullanıcı 127.0.0.1 üzerinden bağlanır" "$(pg_can ${CUSTOMER}_pgl ${CUSTOMER}_pg1 127.0.0.1 prefer)" bağlandı

  say "PostgreSQL: geri alma (bozuk pg_hba.conf ile açma denemesi)"
  # Dosyaya geçersiz bir satır eklenir; panel değişiklikten sonra pg_hba_file_rules'ta hata görür,
  # dosyayı yazmadan önceki hâline döndürür ve ayarı açmaz.
  printf 'bu gecersiz bir satir\n' >> $HBA
  cp -p $HBA $W/hba.broken
  OUT=$(api PUT /databases/remote/postgresql -d '{"enabled":true}')
  STATUS=$(code PUT /databases/remote/postgresql -d '{"enabled":true}')
  check "bozuk dosyayla açma reddedilir" "$([ "$STATUS" = 400 ] && echo red)" red
  has "hata iletisi dosyanın geçersiz olduğunu söyler" "$OUT" "geçersiz"
  check "dosya değişiklik öncesi hâline döndü" "$(cmp -s $HBA $W/hba.broken && echo aynı)" aynı
  check "ayar açılmadı" "$(test -e $PGCONF && echo var || echo yok)" yok
  check "PostgreSQL çalışıyor" "$(pg 'SELECT 1')" 1
  check "yalnızca yerel dinleme sürüyor" "$(listens_remote 5432)" hayir
  cat $W/hba.orig > $HBA
  check "bozuk satır temizlendi" "$(cmp -s $HBA $W/hba.orig && echo aynı)" aynı
  if chattr +i $HBA 2>/dev/null; then
    # Yazılamayan dosya: işlem hata verir, hiçbir şey yarım kalmaz, sunucu kapanmaz.
    STATUS=$(code PUT /databases/remote/postgresql -d '{"enabled":true}')
    chattr -i $HBA
    check "yazılamayan pg_hba.conf ile açma reddedilir" "$([ "$STATUS" -ge 400 ] && echo red)" red
    check "ayar açılmadı" "$(test -e $PGCONF && echo var || echo yok)" yok
    check "PostgreSQL çalışıyor" "$(pg 'SELECT 1')" 1
    check "dosya aynen kaldı" "$(cmp -s $HBA $W/hba.orig && echo aynı)" aynı
  else
    echo "  (chattr desteklenmiyor; yazılamayan dosya sınaması atlandı; geri alma mantığı birim testlerinde)"
  fi

  say "PostgreSQL: yönetici ayarı açar"
  OUT=$(api PUT /databases/remote/postgresql -d '{"enabled":true}')
  check "açma 200 ve değişiklik yapıldı" "$(jget 'd.get("changed")' <<<"$OUT")" True
  TLS=$(jget 'd.get("tls")' <<<"$OUT")
  KIND=host; [ "$TLS" = True ] && KIND=hostssl
  echo "  (sunucuda TLS: $TLS; satır türü: $KIND)"
  check "yapılandırma dosyası yazıldı" "$(grep -c "^listen_addresses = '\*'$" $PGCONF)" 1
  check "PostgreSQL çalışıyor" "$(pg 'SELECT 1')" 1
  check "yerel olmayan arayüzde de dinliyor" "$(listens_remote 5432)" evet
  check "durum ucu açık gösterir" "$(remote_field postgresql remote_enabled) $(remote_field postgresql local_only)" "True False"
  check "pg_hba.conf'ta hata satırı yok" "$(pg 'SELECT count(*) FROM pg_hba_file_rules WHERE error IS NOT NULL')" 0
  check "panel bloğu var" "$(grep -c '^# PixPanel başla$' $HBA) $(grep -c '^# PixPanel bitti$' $HBA)" "1 1"
  check "LAN kullanıcısı için satır var" "$(block_of $HBA | grep -cF "$KIND \"${CUSTOMER}_pg1\" \"${CUSTOMER}_pgn\" $LAN/32 scram-sha-256")" 1
  check "yanlış adresli kullanıcı için satır var" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pgy\" $WRONG/32")" 1
  check "yerel kullanıcıya satır yazılmadı" "$(block_of $HBA | grep -c "${CUSTOMER}_pgl")" 0
  check "bloğun dışındaki satırlar değişmedi" "$(diff <(strip_block $W/hba.orig) <(strip_block $HBA) | wc -l)" 0
  check "dosyanın sahibi ve izinleri korundu" "$(stat -c '%U:%G %a' $HBA)" "$HBA_OWNER"
  check "ikinci açma değişiklik yapmaz" "$(api PUT /databases/remote/postgresql -d '{"enabled":true}' | jget 'd.get("changed")')" False

  say "PostgreSQL: adrese ve veritabanına göre bağlantı"
  check "LAN adresine izinli kullanıcı bağlanır" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" bağlandı
  check "yetkisiz olduğu veritabanına giremez" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg2)" reddedildi
  check "postgres veritabanına giremez" "$(pg_can ${CUSTOMER}_pgn postgres)" reddedildi
  check "template1 veritabanına giremez" "$(pg_can ${CUSTOMER}_pgn template1)" reddedildi
  check "localhost kullanıcısı LAN adresinden bağlanamaz" "$(pg_can ${CUSTOMER}_pgl ${CUSTOMER}_pg1)" reddedildi
  check "yanlış adrese izinli kullanıcı bağlanamaz" "$(pg_can ${CUSTOMER}_pgy ${CUSTOMER}_pg1)" reddedildi
  check "localhost kullanıcısı 127.0.0.1 üzerinden bağlanır" "$(pg_can ${CUSTOMER}_pgl ${CUSTOMER}_pg1 127.0.0.1 prefer)" bağlandı
  if [ "$KIND" = hostssl ]; then
    check "TLS olmadan bağlantı reddedilir (hostssl)" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1 $LAN disable)" reddedildi
  fi

  say "PostgreSQL: yetki değişince pg_hba satırı güncellenir"
  check "ikinci veritabanına yetki verilir" "$(status $JC PUT /databases/$D2/grants/$PN -d '{"access":"readonly"}')" 200
  check "satır iki veritabanını da kapsar" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pg1\",\"${CUSTOMER}_pg2\" \"${CUSTOMER}_pgn\" $LAN/32")" 1
  check "ikinci veritabanına bağlanılır" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg2)" bağlandı
  check "yetki kaldırılır" "$(status $JC DELETE /databases/$D2/grants/$PN)" 200
  check "satır yalnızca ilk veritabanını kapsar" "$(block_of $HBA | grep -cF "$KIND \"${CUSTOMER}_pg1\" \"${CUSTOMER}_pgn\" $LAN/32")" 1
  check "ikinci veritabanı satırdan çıktı" "$(block_of $HBA | grep -c "${CUSTOMER}_pg2")" 0
  check "ikinci veritabanına bağlanılamaz" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg2)" reddedildi
  check "ilk veritabanına bağlanılır" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" bağlandı
  check "bloğun dışındaki satırlar değişmedi" "$(diff <(strip_block $W/hba.orig) <(strip_block $HBA) | wc -l)" 0

  say "PostgreSQL: kullanıcının adresi değiştirilir"
  check "müşteri başkasının kullanıcısını değiştiremez (404)" "$(status $JO PUT /databases/users/$PN/host -d '{"host":"%"}')" 404
  for bad in "${BAD_HOSTS[@]}" '10.%'; do
    check "adres değiştirme reddedilir [$(head -c 22 <<<"$bad")]" "$(status $JC PUT /databases/users/$PN/host -d "{\"host\":\"$bad\"}")" 400
  done
  check "reddedilen adresler kaydı bozmadı" "$(user_host ${CUSTOMER}_pgn)" "$LAN"
  check "bloğun dışındaki satırlar değişmedi (geçersiz denemelerden sonra)" "$(diff <(strip_block $W/hba.orig) <(strip_block $HBA) | wc -l)" 0
  check "adres yanlış adrese çevrilir" "$(status $JC PUT /databases/users/$PN/host -d "{\"host\":\"$WRONG\"}")" 200
  check "eski adresin satırı kalktı" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pgn\" $LAN/32")" 0
  check "yeni adresin satırı yazıldı" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pgn\" $WRONG/32")" 1
  check "eski adres artık bağlanamaz" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" reddedildi
  check "CIDR aralığına çevrilir" "$(status $JC PUT /databases/users/$PN/host -d "{\"host\":\"$NET24\"}")" 200
  check "CIDR satırı yazıldı" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pgn\" $NET24 scram-sha-256")" 1
  check "aralıktaki adresten bağlanılır" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" bağlandı
  check "her yer (%) müşteriye de açık" "$(status $JC PUT /databases/users/$PN/host -d '{"host":"%"}')" 200
  check "IPv4 için her yer satırı" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pgn\" 0.0.0.0/0 scram-sha-256")" 1
  check "IPv6 için her yer satırı" "$(block_of $HBA | grep -cF "\"${CUSTOMER}_pgn\" ::/0 scram-sha-256")" 1
  check "her yerden bağlanılır" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" bağlandı
  check "yerel adrese döndürülür" "$(status $JC PUT /databases/users/$PN/host -d '{"host":"localhost"}')" 200
  check "satırlar kalktı" "$(block_of $HBA | grep -c "${CUSTOMER}_pgn")" 0
  check "yerel adrese dönünce LAN reddedilir" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" reddedildi
  check "LAN adresine geri alınır" "$(status $JC PUT /databases/users/$PN/host -d "{\"host\":\"$LAN\"}")" 200
  check "yeniden bağlanılır" "$(pg_can ${CUSTOMER}_pgn ${CUSTOMER}_pg1)" bağlandı
  check "kullanıcı silinince satırı kalkar" "$(status $JC DELETE /databases/users/$PN)" 200
  check "silinen kullanıcının satırı yok" "$(block_of $HBA | grep -c "${CUSTOMER}_pgn")" 0
  check "pg_hba.conf'ta hata satırı yok" "$(pg 'SELECT count(*) FROM pg_hba_file_rules WHERE error IS NOT NULL')" 0
  check "bloğun dışındaki satırlar değişmedi" "$(diff <(strip_block $W/hba.orig) <(strip_block $HBA) | wc -l)" 0
  check "adres değişiklikleri denetim günlüğünde (database.user.host)" "$([ "$(audit_count database.user.host)" -gt 0 ] && echo var)" var

  say "PostgreSQL: yönetici ayarı kapatır"
  OUT=$(api PUT /databases/remote/postgresql -d '{"enabled":false}')
  check "kapatma 200 ve değişiklik yapıldı" "$(jget 'd.get("changed")' <<<"$OUT")" True
  check "yapılandırma dosyası silindi" "$(test -e $PGCONF && echo var || echo yok)" yok
  check "panel bloğu kalktı" "$(grep -c '^# PixPanel başla$' $HBA)" 0
  check "pg_hba.conf özgün hâline döndü" "$(cmp -s $HBA $W/hba.orig && echo aynı)" aynı
  check "PostgreSQL çalışıyor" "$(pg 'SELECT 1')" 1
  check "yeniden yalnızca yerel dinleme" "$(listens_remote 5432)" hayir
  check "yanlış adresli kullanıcının kaydı silinmedi" "$(user_host ${CUSTOMER}_pgy)" "$WRONG"
  check "LAN adresinden bağlantı yine reddedilir" "$(pg_can ${CUSTOMER}_pgy ${CUSTOMER}_pg1)" reddedildi
  check "ikinci kapatma değişiklik yapmaz" "$(api PUT /databases/remote/postgresql -d '{"enabled":false}' | jget 'd.get("changed")')" False

  say "PostgreSQL: başlangıç durumuna dönüş"
  if [ "$PG_WAS" = true ]; then
    # Sınama kullanıcıları silinmeden önce açılırsa blokta yer alırlar; bu yüzden önce temizlenir (aşağıda).
    PG_REENABLE=true
  else
    PG_REENABLE=false
    check "ayar eskisi gibi kapalı" "$(remote_field postgresql remote_enabled)" False
  fi
else
  echo "  (PostgreSQL kurulu değil; bölüm atlandı)"
  PG_REENABLE=false
fi

say "Temizlik"
cleanup
if [ "${PG_REENABLE:-false}" = true ]; then
  api PUT /databases/remote/postgresql -d '{"enabled":true}' >/dev/null
  check "PostgreSQL ayarı eskisi gibi açık" "$(remote_field postgresql remote_enabled)" True
fi
if [ -n "$HBA" ]; then
  check "pg_hba.conf sınama öncesiyle aynı" "$(cmp -s $HBA $W/hba.orig && echo aynı || echo farklı)" aynı
  check "pg_hba.conf sahibi ve izinleri aynı" "$(stat -c '%U:%G %a' $HBA)" "$HBA_OWNER"
fi
if [ -f $W/my.orig ] && [ "${MY_WAS:-false}" = true ]; then
  check "MariaDB yapılandırması sınama öncesiyle aynı" "$(grep -c -E '^bind-address = 0\.0\.0\.0(,::)?$' $MYCONF)" 1
fi
check "sınama kaynakları silindi" "$(api GET /databases | jget "[x['name'] for x in d['databases'] if x['name'].startswith('$CUSTOMER')] + [x['name'] for x in d['users'] if x['name'].startswith('$CUSTOMER')]")" "[]"
check "sınama hesapları silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$CUSTOMER', '$OTHER')]")" "[]"
check "MariaDB hesapları sunucudan kalktı" "$(my "SELECT COUNT(*) FROM mysql.user WHERE User LIKE '${CUSTOMER}\\_%'" 2>/dev/null || echo 0)" 0
check "PostgreSQL rolleri sunucudan kalktı" "$(pg "SELECT COUNT(*) FROM pg_roles WHERE rolname LIKE '${CUSTOMER}\\_%'" 2>/dev/null || echo 0)" 0
rm -rf $W $J $JC $JO
echo "bitti"
