#!/usr/bin/env bash
# SQL içe aktarmanın güvenlik sınaması: yüklenen döküm güvenilmeyen girdidir. Döküm başka
# veritabanına erişememeli, kullanıcı açamamalı, sunucuda komut çalıştıramamalı; meşru dökümler
# (görünüm, tetikleyici, başka sunucudan gelen DEFINER ifadeleri) ise sorunsuz yüklenmelidir.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash dbimport.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-dbi
JC=/tmp/pxjar-dbi-c
CUSTOMER=dbimusteri
PASS='Dbi-Sinama-Parolasi-1'
DBPASS='Dbi-Veritabani-Parola-1'
W=/tmp/pxdbi
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
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
wait_job() { for _ in $(seq 1 120); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
db_id() { api GET /databases | jget "next((x['id'] for x in d['databases'] if x['name']=='$1'), '')"; }
user_id() { api GET /databases | jget "next((x['id'] for x in d['users'] if x['name']=='$1'), '')"; }
# upload <kimlik çerezi> <veritabanı no> <dosya>: dökümü yükler, görevin sonucunu yazar.
upload() { local job; job=$(curl -sk -b $1 -c $1 -H 'x-pixpanel-csrf: 1' -F "file=@$3" "$B/databases/$2/import" | jget 'd["job_id"]'); wait_job "$job"; }
my() { mariadb -N -B -e "$1" 2>&1; }
pg() { runuser -u postgres -- psql -X -At -d "${2:-postgres}" -c "$1" 2>&1; }
installed() { api GET /components | jget "next(c['installed'] for c in d if c['id']=='$1')"; }

rm -rf $W; mkdir -p $W; rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
# Önceki koşudan kalanlar.
for name in ${CUSTOMER}_hedef ${CUSTOMER}_pg dbi_kurban dbi_pgkurban; do id=$(db_id $name); [ -n "$id" ] && api DELETE /databases/$id >/dev/null; done
for name in ${CUSTOMER}_uyg; do id=$(user_id $name); [ -n "$id" ] && api DELETE /databases/users/$id >/dev/null; done
OLD=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$CUSTOMER'), '')"); [ -n "$OLD" ] && api DELETE "/users/$OLD" >/dev/null
rm -f /root/pwned-* /tmp/pwned-* /var/lib/mysql/pwned-* 2>/dev/null
api POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}" >/dev/null
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\"}"

if [ "$(installed mariadb)" = True ]; then
  say "MariaDB: hazırlık"
  api POST /databases -d '{"engine":"mariadb","name":"dbi_kurban"}' >/dev/null
  my "CREATE TABLE dbi_kurban.gizli (deger VARCHAR(40)); INSERT INTO dbi_kurban.gizli VALUES ('baska-musterinin-verisi');" >/dev/null
  T=$(call $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_hedef\",\"user\":{\"name\":\"${CUSTOMER}_uyg\",\"password\":\"$DBPASS\"}}" | jget 'd["id"]')
  [ "$T" -gt 0 ] 2>/dev/null || T=$(db_id ${CUSTOMER}_hedef)
  check "müşterinin veritabanı açıldı" "$([ -n "$T" ] && echo var)" var

  say "MariaDB: saldırı dökümleri"
  printf 'CREATE TABLE ilk (id INT);\n\\! touch /root/pwned-mariadb-sh\nsystem touch /root/pwned-mariadb-system\n' > $W/kabuk.sql
  upload $JC $T $W/kabuk.sql >/dev/null
  check "döküm panelin kimliğiyle komut çalıştıramaz" "$(ls /root/pwned-mariadb-* 2>/dev/null | wc -l)" 0
  printf "CREATE USER 'dbi_kotu'@'localhost' IDENTIFIED BY 'x';\nGRANT ALL PRIVILEGES ON *.* TO 'dbi_kotu'@'localhost' WITH GRANT OPTION;\n" > $W/kullanici.sql
  check "kullanıcı açan döküm reddedilir" "$(upload $JC $T $W/kullanici.sql)" failed
  check "yetkili kullanıcı oluşmadı" "$(my "SELECT COUNT(*) FROM mysql.user WHERE User='dbi_kotu'")" 0
  printf 'CREATE TABLE calinan AS SELECT * FROM dbi_kurban.gizli;\n' > $W/okuma.sql
  check "başka veritabanını okuyan döküm reddedilir" "$(upload $JC $T $W/okuma.sql)" failed
  check "başkasının verisi kopyalanmadı" "$(my "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='${CUSTOMER}_hedef' AND table_name='calinan'")" 0
  printf 'USE dbi_kurban;\nDROP TABLE gizli;\n' > $W/silme.sql
  upload $JC $T $W/silme.sql >/dev/null
  check "başka veritabanı değiştirilemez" "$(my "SELECT deger FROM dbi_kurban.gizli")" baska-musterinin-verisi
  printf "SELECT 'x' INTO OUTFILE '/var/lib/mysql/pwned-outfile';\n" > $W/dosya.sql
  upload $JC $T $W/dosya.sql >/dev/null
  check "sunucuya dosya yazılamaz" "$(test -e /var/lib/mysql/pwned-outfile && echo var || echo yok)" yok
  printf "CREATE TABLE okunan (satir TEXT);\nLOAD DATA INFILE '/etc/passwd' INTO TABLE okunan;\n" > $W/yukle.sql
  upload $JC $T $W/yukle.sql >/dev/null
  check "sunucudan dosya okunamaz" "$(my "SELECT COUNT(*) FROM ${CUSTOMER}_hedef.okunan" | grep -c '^0$\|doesn.t exist')" 1
  check "müşteri başkasının veritabanına yükleyemez" "$(curl -sk -o /dev/null -w '%{http_code}' -b $JC -H 'x-pixpanel-csrf: 1' -F "file=@$W/okuma.sql" "$B/databases/$(db_id dbi_kurban)/import")" 404

  say "MariaDB: meşru dökümler"
  cat > $W/gercek.sql <<'XEOF'
-- Başka bir sunucudan alınmış döküm: tanımlayan hesaplar bu sunucuda yok.
DROP TABLE IF EXISTS `urunler`;
CREATE TABLE `urunler` (`id` int NOT NULL AUTO_INCREMENT, `ad` varchar(80) NOT NULL, `fiyat` int NOT NULL, PRIMARY KEY (`id`)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
INSERT INTO `urunler` VALUES (1,'Çay DEFINER=`eski`@`sunucu` bardağı',40),(2,'Şeker',25);
DROP TABLE IF EXISTS `gunluk`;
CREATE TABLE `gunluk` (`ileti` varchar(80)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
/*!50001 DROP VIEW IF EXISTS `pahali`*/;
/*!50003 DROP PROCEDURE IF EXISTS `say` */;
/*!50001 CREATE ALGORITHM=UNDEFINED */
/*!50013 DEFINER=`eskiroot`@`10.0.0.%` SQL SECURITY DEFINER */
/*!50001 VIEW `pahali` AS select `urunler`.`ad` AS `ad` from `urunler` where `urunler`.`fiyat` > 30 */;
DELIMITER ;;
/*!50003 CREATE*/ /*!50017 DEFINER=`eskiroot`@`10.0.0.%`*/ /*!50003 TRIGGER `urun_eklendi` AFTER INSERT ON `urunler` FOR EACH ROW INSERT INTO gunluk VALUES (CONCAT('eklendi: ', NEW.ad)) */;;
CREATE DEFINER=`eskiroot`@`10.0.0.%` PROCEDURE `say`()
SELECT COUNT(*) FROM urunler ;;
DELIMITER ;
XEOF
  check "görünüm, tetikleyici ve yordam içeren döküm yüklenir" "$(upload $JC $T $W/gercek.sql)" success
  APP() { mariadb -N -B --default-character-set=utf8mb4 -u ${CUSTOMER}_uyg -p"$DBPASS" ${CUSTOMER}_hedef -e "$1" 2>&1; }
  check "veri bozulmadan geldi (içindeki benzer metin dahil)" "$(APP "SELECT ad FROM urunler WHERE id=1")" 'Çay DEFINER=`eski`@`sunucu` bardağı'
  check "görünüm uygulamanın kullanıcısıyla çalışır" "$(APP "SELECT ad FROM pahali")" 'Çay DEFINER=`eski`@`sunucu` bardağı'
  check "tetikleyici çalışır" "$(APP "INSERT INTO urunler (ad, fiyat) VALUES ('Kahve', 90); SELECT ileti FROM gunluk")" "eklendi: Kahve"
  check "yordam çalışır" "$(APP "CALL say()")" 3
  gzip -c $W/gercek.sql > $W/gercek.sql.gz
  check "sıkıştırılmış döküm yüklenir" "$(upload $JC $T $W/gercek.sql.gz) $(APP "SELECT COUNT(*) FROM urunler")" "success 2"
  printf 'bu bir sql degil' | gzip -c | head -c 12 > $W/bozuk.sql.gz
  check "bozuk arşiv hata verir" "$(upload $JC $T $W/bozuk.sql.gz)" failed
  IMP=$(my "SELECT User FROM mysql.user WHERE User LIKE 'pxi\_%' LIMIT 1")
  check "içe aktarma hesabı yükleme dışında kilitli" "$(mariadb -u "$IMP" -px ${CUSTOMER}_hedef -e 'SELECT 1' 2>&1 | grep -c 'locked\|Access denied')" 1
  check "içe aktarma hesabının genel yetkisi yok" "$(my "SELECT COUNT(*) FROM information_schema.user_privileges WHERE grantee LIKE '%pxi\_%' AND privilege_type != 'USAGE'")" 0
  check "veritabanı silinince hesabı da kalkar" "$(code DELETE /databases/$T) $(my "SELECT COUNT(*) FROM mysql.user WHERE User='$IMP'")" "200 0"
  api DELETE /databases/$(db_id dbi_kurban) >/dev/null
else
  echo "  (MariaDB kurulu değil; bölüm atlandı)"
fi

if [ "$(installed postgresql)" = True ]; then
  say "PostgreSQL"
  api POST /databases -d '{"engine":"postgresql","name":"dbi_pgkurban"}' >/dev/null
  pg "CREATE TABLE gizli (deger text); INSERT INTO gizli VALUES ('baska-musterinin-verisi');" dbi_pgkurban >/dev/null
  call $JC POST /databases -d "{\"engine\":\"postgresql\",\"name\":\"${CUSTOMER}_pg\"}" >/dev/null
  P=$(db_id ${CUSTOMER}_pg)
  printf '\\! touch /root/pwned-pg-sh\nCREATE TABLE ilk (id int);\n' > $W/pgkabuk.sql
  upload $JC $P $W/pgkabuk.sql >/dev/null
  check "döküm panelin ya da veritabanı sunucusunun kimliğiyle komut çalıştıramaz" "$(ls /root/pwned-pg-* /var/lib/postgresql/pwned-* 2>/dev/null | wc -l)" 0
  printf "RESET ROLE;\nSET ROLE postgres;\nCREATE ROLE dbi_pgkotu SUPERUSER LOGIN PASSWORD 'x';\n" > $W/pgrol.sql
  upload $JC $P $W/pgrol.sql >/dev/null
  check "yönetici rolüne geçilemez, yetkili rol açılamaz" "$(pg "SELECT COUNT(*) FROM pg_roles WHERE rolname='dbi_pgkotu'")" 0
  printf "COPY (SELECT 1) TO PROGRAM 'touch /tmp/pwned-pg-program';\nCOPY (SELECT 1) TO '/tmp/pwned-pg-dosya';\n" > $W/pgprogram.sql
  upload $JC $P $W/pgprogram.sql >/dev/null
  check "sunucuda program çalıştırılamaz, dosya yazılamaz" "$(ls /tmp/pwned-pg-* 2>/dev/null | wc -l)" 0
  printf '\\connect dbi_pgkurban\nDROP TABLE gizli;\n' > $W/pgbaska.sql
  upload $JC $P $W/pgbaska.sql >/dev/null
  check "başka veritabanına geçilemez" "$(pg "SELECT deger FROM gizli" dbi_pgkurban)" baska-musterinin-verisi
  printf "CREATE TABLE urunler (id serial PRIMARY KEY, ad text);\nINSERT INTO urunler (ad) VALUES ('Çay'), ('Şeker');\n" > $W/pggercek.sql
  check "meşru döküm yüklenir" "$(upload $JC $P $W/pggercek.sql) $(pg "SELECT COUNT(*) FROM urunler" ${CUSTOMER}_pg)" "success 2"
  check "nesnelerin sahibi veritabanının ortak rolü" "$(pg "SELECT tableowner FROM pg_tables WHERE tablename='urunler'" ${CUSTOMER}_pg)" "pixpanel_own_${CUSTOMER}_pg"
  check "geçici giriş rolü geride kalmadı" "$(pg "SELECT COUNT(*) FROM pg_roles WHERE rolname LIKE 'pxi\_%'")" 0
  api DELETE /databases/$P >/dev/null; api DELETE /databases/$(db_id dbi_pgkurban) >/dev/null
else
  echo "  (PostgreSQL kurulu değil; bölüm atlandı)"
fi

say "Temizlik"
id=$(user_id ${CUSTOMER}_uyg); [ -n "$id" ] && api DELETE /databases/users/$id >/dev/null
check "hesabı sil" "$(code DELETE "/users/$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")")" 200
rm -rf $W $J $JC /tmp/pwned-* 2>/dev/null
echo "bitti"
