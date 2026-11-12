#!/usr/bin/env bash
# Başka panelin yedek dosyasından içe aktarma sınaması (cPanel, Plesk, HestiaCP).
# Betik yedekleri kendisi üretir: yöneticinin gerçek bir WordPress sitesinden (dosyalar + mysqldump
# çıktısı) üç panelin yerleşimini taklit eden arşivler kurar, panel API'siyle inceler ve içe aktarır,
# sonucu doğrular (dosyalar yerinde ve hesabın, veritabanı geldi, wp-config yeni bilgilere çevrildi,
# adresler çevrildi) ve oluşturduğu her şeyi siler. Kalıcı örnek verilere dokunmaz; adlar `bimp-`
# ile başlar. Panelin kurulu olduğu makinede root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash backup-import.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-bimp
JC=/tmp/pxjar-bimp-c
C=bimphesap
PASS='Yedek-Aktarma-Parolasi-1'
WP_PASS='Wp-Yedek-Parolasi-1'
SRC=bimp-kaynak.test
CPMAIN=bimp-cp.test
CPADDON=bimp-ek.test
CPRB=bimp-rb.test
PLDOM=bimp-pl.test
PLTARGET=bimp-pl2.test
HEDOM=bimp-he.test
HOSTILE=bimp-zararli.test
W=/tmp/bimp-work
TMPROOT=/var/lib/pixpanel/tmp
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
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
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
joblog() { api GET "/jobs/$1" | jget 'd["log"]'; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
jstr() { python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$1"; }
# do_inspect <yol>: IID (inceleme kimliği), IJOB, IRES (görev sonucu) değişkenlerini kurar.
do_inspect() {
  local R; R=$(api POST /migrate/backup/inspect -d "{\"path\":$(jstr "$1")}")
  IID=$(jget 'd["inspect_id"]' <<<"$R"); IJOB=$(jget 'd["job_id"]' <<<"$R"); IRES=$(wait_job "$IJOB")
  ALL_IDS="$ALL_IDS $IID"
}
manifest() { api GET /migrate/backup/$IID; }
# do_import <öğeler JSON>: JOB ve RES değişkenlerini kurar.
do_import() {
  local R; R=$(api POST /migrate/backup/import -d "{\"inspect_id\":\"$IID\",\"owner_user_id\":$UC,\"items\":$1}")
  JOB=$(jget 'd["job_id"]' <<<"$R"); RES=$(wait_job "$JOB")
  if [ "$RES" != success ]; then
    echo "        yanıt: $(head -c 300 <<<"$R")"
    case "$JOB" in ''|*[!0-9]*) ;; *) joblog "$JOB" | tail -15 | sed 's/^/        /' ;; esac
  fi
}
# drop_inspect: şu anki incelemenin geçici klasörünü siler (aynı anda en çok 4 inceleme tutulur).
drop_inspect() { api DELETE /migrate/backup/$IID >/dev/null; }
cfgval() { sed -n "s/^define( *'$1', *'\\(.*\\)' *);.*/\\1/p" "$2/wp-config.php"; }
drop_databases() {
  for old in $(api GET /databases | jget "' '.join(str(x['id']) for x in d['databases'] if 'bimp' in x['name'])"); do api DELETE /databases/$old >/dev/null; done
  for old in $(api GET /databases | jget "' '.join(str(x['id']) for x in d['users'] if 'bimp' in x['name'])"); do api DELETE /databases/users/$old >/dev/null; done
}
cleanup_all() {
  for id in $ALL_IDS; do api DELETE /migrate/backup/$id >/dev/null 2>&1; done
  for name in $SRC $CPMAIN $CPADDON $CPRB $PLDOM $PLTARGET $HEDOM $HOSTILE; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
  drop_databases
  id=$(user_id $C); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
  rm -rf $W
}
ALL_IDS=""

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
# Önceki bir koşudan kalan (yalnızca bu betiğin yollarından açılmış) incelemeler silinir.
for dir in $TMPROOT/import-*; do
  [ -f "$dir/meta.json" ] || continue
  if python3 -c "import json,sys; sys.exit(0 if json.load(open('$dir/meta.json'))['source'].startswith(('/tmp/bimp-work','/home/$C/')) else 1)"; then api DELETE "/migrate/backup/${dir##*/import-}" >/dev/null; fi
done
cleanup_all
HAS_ZSTD=$(command -v zstd >/dev/null && echo evet || echo hayir)

say "Hazırlık: gerçek bir WordPress'ten yedek dosyaları kurulur"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$C\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
UC=$(user_id $C)
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$C\",\"password\":\"$PASS\"}"
S=$(api POST /sites -d "{\"domain\":\"$SRC\",\"kind\":\"php\",\"php_version\":\"8.3\"}" | jget 'd["id"]')
JOB=$(api POST /sites/$S/wordpress/install -d "{\"title\":\"Yedek Sitesi\",\"admin_user\":\"yonetici\",\"admin_email\":\"ali@ornek.com\",\"admin_password\":\"$WP_PASS\",\"locale\":\"tr_TR\"}" | jget 'd["job_id"]')
check "kaynak WordPress kuruldu" "$(wait_job "$JOB")" success
SRCDIR=/home/$PX_USER/sites/$SRC/public
echo "yedekten-gelen-dosya" > $SRCDIR/isaret.txt
mkdir -p "$SRCDIR/wp-content/uploads/2026" && echo resim > "$SRCDIR/wp-content/uploads/2026/foto.jpg"
printf '#!/bin/sh\necho suid\n' > $SRCDIR/suid-deneme.sh; chmod 4755 $SRCDIR/suid-deneme.sh
ln -sf /etc/passwd $SRCDIR/disari-bag
SRCDB=$(cfgval DB_NAME $SRCDIR)
SRCDBPASS=$(cfgval DB_PASSWORD $SRCDIR)
mkdir -p $W/dump && chmod 755 $W
mariadb-dump --single-transaction --no-tablespaces $SRCDB > $W/dump/plain.sql
mariadb-dump --single-transaction --no-tablespaces --databases $SRCDB > $W/dump/with-use.sql
check "döküm üretildi, USE satırı içeriyor" "$(grep -c '^USE `' $W/dump/with-use.sql) $(grep -c '^CREATE DATABASE' $W/dump/with-use.sql)" "1 1"
# Site içeriği (bağlar dahil; tar arşive olduğu gibi koyar).
mkdir -p $W/content && cp -a $SRCDIR/. $W/content/
echo "ek-alan-sitesi" > $W/addon-index.html

# --- cPanel: cpmove-bimpcp.tar.gz ---
P=$W/cp/cpmove-bimpcp
mkdir -p $P/cp $P/userdata $P/homedir/public_html $P/homedir/$CPADDON $P/mysql $P/psql $P/homedir/etc/$CPMAIN
cp -a $W/content/. $P/homedir/public_html/
ln -s public_html $P/homedir/www
cp $W/addon-index.html $P/homedir/$CPADDON/index.html
printf 'USER=bimpcp\nDNS=%s\n' $CPMAIN > $P/cp/bimpcp
printf 'main_domain: %s\naddon_domains:\n  %s: ek.%s\nsub_domains: []\n' $CPMAIN $CPADDON $CPMAIN > $P/userdata/main
printf 'documentroot: /home/bimpcp/public_html\nhomedir: /home/bimpcp\nservername: %s\nuser: bimpcp\n' $CPMAIN > $P/userdata/$CPMAIN
printf 'documentroot: /home/bimpcp/%s\nhomedir: /home/bimpcp\nservername: %s\nuser: bimpcp\n' $CPADDON $CPADDON > $P/userdata/$CPADDON
cp $W/dump/plain.sql $P/mysql/$SRCDB.sql
echo "SELECT 1;" > $P/psql/bimp_pg.sql
printf "GRANT ALL PRIVILEGES ON \`%s\`.* TO '%s'@'localhost';\n" "${SRCDB//_/\\\\_}" $SRCDB > $P/mysql.sql
printf 'info:x:1\nsatis:x:2\n' > $P/homedir/etc/$CPMAIN/passwd
tar czf $W/cpmove-bimpcp.tar.gz -C $W/cp cpmove-bimpcp

# --- cPanel: bozuk dökümlü (geri alma sınaması) ---
P=$W/cprb/cpmove-bimprb
mkdir -p $P/cp $P/userdata $P/homedir/public_html $P/mysql
echo "<?php echo 'rb';" > $P/homedir/public_html/index.php
printf 'USER=bimprb\nDNS=%s\n' $CPRB > $P/cp/bimprb
printf 'main_domain: %s\n' $CPRB > $P/userdata/main
printf 'documentroot: /home/bimprb/public_html\nhomedir: /home/bimprb\nservername: %s\n' $CPRB > $P/userdata/$CPRB
echo "BU SQL DEGIL;" > $P/mysql/bimprb_db.sql
tar czf $W/cpmove-bimprb.tar.gz -C $W/cprb cpmove-bimprb

# --- Plesk: backup_bimp.tar ---
P=$W/pl
mkdir -p $P/domains/$PLDOM $P/databases/$SRCDB
cat > $P/backup_info_2611121200.xml <<XML
<?xml version="1.0" encoding="UTF-8"?>
<migration-dump content-included-type="full">
  <domain name="$PLDOM" guid="x">
    <databases><database name="$SRCDB" type="mysql"><dbuser name="${SRCDB}_u"/></database></databases>
    <mailsystem><mailuser name="info"/></mailsystem>
  </domain>
</migration-dump>
XML
mkdir -p $W/pl-data && rm -rf $W/pl-data/* && mkdir -p $W/pl-data/httpdocs && cp -a $W/content/. $W/pl-data/httpdocs/
tar czf $P/domains/$PLDOM/backup_user-data_2611121200.tgz -C $W/pl-data .
mkdir -p $W/pl-sql && cp $W/dump/plain.sql $W/pl-sql/dump.sql
tar czf $P/databases/$SRCDB/backup_sqldump_2611121200.tgz -C $W/pl-sql .
tar cf $W/backup_bimp.tar -C $P .

# --- HestiaCP: bimphestia.2026-11-12_12-00-00.tar ---
P=$W/he
mkdir -p $P/web/$HEDOM $P/db/$SRCDB $P/user $P/pam $P/mail/$HEDOM
mkdir -p $W/he-data/public_html && cp -a $W/content/. $W/he-data/public_html/
if [ $HAS_ZSTD = evet ]; then
  tar -C $W/he-data -cf - . | zstd -q -o $P/web/$HEDOM/domain_data.tar.zst
  zstd -q -c $W/dump/with-use.sql > $P/db/$SRCDB/$SRCDB.mysql.sql.zst
else
  tar czf $P/web/$HEDOM/domain_data.tar.gz -C $W/he-data .
  gzip -c $W/dump/with-use.sql > $P/db/$SRCDB/$SRCDB.mysql.sql.gz
fi
printf "DB='%s' DBUSER='%s_u' TYPE='mysql'\n" $SRCDB $SRCDB > $P/db/$SRCDB/db.conf
printf "ACCOUNT='info' ALIAS=''\n" > $P/mail/$HEDOM/accounts.conf
echo "FNAME='x'" > $P/user/user.conf; echo x > $P/pam/passwd
tar cf $W/bimphestia.2026-11-12_12-00-00.tar -C $P .

# --- Zararlı arşivler ---
python3 - <<PY
import tarfile, io
def add(t, name, data=b'x', kind=None, link=None, mode=None):
    i = tarfile.TarInfo(name)
    if mode: i.mode = mode
    if kind: i.type = kind
    if link: i.linkname = link
    else: i.size = len(data)
    t.addfile(i, None if link else io.BytesIO(data))
with tarfile.open('$W/kacis.tar.gz', 'w:gz') as t:
    add(t, 'cpmove-x/cp/x', b'DNS=$HOSTILE\n')
    add(t, '../bimp-kacis.txt', b'kacis')
with tarfile.open('$W/baglar.tar.gz', 'w:gz') as t:
    add(t, 'cpmove-x/cp/x', b'DNS=$HOSTILE\n')
    add(t, 'cpmove-x/userdata/main', b'main_domain: $HOSTILE\n')
    add(t, 'cpmove-x/homedir/public_html/index.html', b'zararli')
    add(t, 'cpmove-x/homedir/public_html/evil', kind=tarfile.SYMTYPE, link='/etc')
    add(t, 'cpmove-x/homedir/public_html/evil/bimp-pwn.txt', b'pwn')
    add(t, 'cpmove-x/homedir/public_html/aygit', kind=tarfile.CHRTYPE, data=b'')
    add(t, 'cpmove-x/homedir/public_html/suid', data=b'#!/bin/sh\n', mode=0o6755)
PY
echo "tar degil" > $W/duz.txt

say "Yetki ve girdi denetimi"
check "müşteri inceleme başlatamaz" "$(status $JC POST /migrate/backup/inspect -d "{\"path\":\"$W/cpmove-bimpcp.tar.gz\"}")" 403
check "müşteri önizleme okuyamaz" "$(status $JC GET /migrate/backup/0123456789abcdef0123456789abcdef)" 403
check "müşteri içe aktaramaz" "$(status $JC POST /migrate/backup/import -d '{"inspect_id":"0123456789abcdef0123456789abcdef","owner_user_id":1,"items":[]}')" 403
check "müşteri silemez" "$(status $JC DELETE /migrate/backup/0123456789abcdef0123456789abcdef)" 403
check "göreli yol" "$(code POST /migrate/backup/inspect -d '{"path":"yedek.tar.gz"}')" 400
check "üst klasöre çıkan yol" "$(code POST /migrate/backup/inspect -d '{"path":"/tmp/../etc/passwd"}')" 400
check "olmayan dosya" "$(code POST /migrate/backup/inspect -d '{"path":"/tmp/yok-boyle-yedek.tar.gz"}')" 400
check "boş yol" "$(code POST /migrate/backup/inspect -d '{"path":""}')" 400
check "panelin kendi klasörü" "$(code POST /migrate/backup/inspect -d '{"path":"/var/lib/pixpanel"}')" 400
check "olmayan inceleme kimliği" "$(code GET /migrate/backup/0123456789abcdef0123456789abcdef)" 404
check "yol kaçışı içeren kimlik" "$(code GET '/migrate/backup/..%2F..%2Fetc')" 404
check "geçersiz kimlikle silme" "$(code DELETE /migrate/backup/ABC)" 404
check "geçersiz kimlikle içe aktarma" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"../../etc\",\"owner_user_id\":$UC,\"items\":[{\"domain\":\"x.com\"}]}")" 404

say "Zararlı arşivler"
do_inspect $W/duz.txt
check "tar olmayan dosya incelenemez" "$IRES" failed
has "nedeni açıklanır" "$(joblog "$IJOB")" "tar arşivi değil"
check "başarısız inceleme durumu" "$(manifest | jget 'd["status"]')" failed
do_inspect $W/kacis.tar.gz
check "yol kaçışı olan arşiv reddedilir" "$IRES" failed
has "nedeni açıklanır" "$(joblog "$IJOB")" "dışına çıkan"
check "kaçış hedefe yazılmadı" "$(find $TMPROOT /tmp /var/lib/pixpanel -maxdepth 4 -name 'bimp-kacis.txt' 2>/dev/null | wc -l)" 0
do_inspect $W/baglar.tar.gz
check "bağlı arşiv incelenir" "$IRES" success
has "bağ ve özel dosya atlandı" "$(joblog "$IJOB")" "bağ ve 1 özel dosya güvenlik için atlandı"
check "bağın ardına yazılamadı" "$([ -e /etc/bimp-pwn.txt ] && echo var || echo yok)" yok
check "bağ ve aygıt oluşturulmadı" "$(find $TMPROOT/import-$IID/raw -type l -o -type c | wc -l)" 0
check "özel bit atıldı" "$(find $TMPROOT/import-$IID/raw -perm /7000 | wc -l)" 0
check "geçici klasör yalnız root'a açık" "$(stat -c '%a %U' $TMPROOT/import-$IID)" "700 root"
drop_inspect

say "cPanel: inceleme"
mkdir -p /home/$C/tmp && cp $W/cpmove-bimpcp.tar.gz /home/$C/tmp/ && chown $C: /home/$C/tmp/cpmove-bimpcp.tar.gz
do_inspect /home/$C/tmp/cpmove-bimpcp.tar.gz
check "hesap klasöründeki yedek incelendi" "$IRES" success
M=$(manifest)
check "durum ve biçim" "$(jget '(d["status"], d["manifest"]["format"])' <<<"$M")" "('ready', 'cpanel')"
check "alan adları" "$(jget '[x["name"] for x in d["manifest"]["domains"]]' <<<"$M")" "['$CPMAIN', '$CPADDON']"
check "WordPress tanındı" "$(jget '[x["wordpress"] for x in d["manifest"]["domains"]]' <<<"$M")" "[True, False]"
check "veritabanları" "$(jget 'sorted((x["name"], x["engine"]) for x in d["manifest"]["databases"])' <<<"$M")" "[('bimp_pg', 'postgresql'), ('$SRCDB', 'mysql')]"
check "posta kutuları yalnızca listelenir" "$(jget 'd["manifest"]["mailboxes"]' <<<"$M")" "['info@$CPMAIN', 'satis@$CPMAIN']"
check "denetim günlüğünde inceleme" "$(api GET '/system/audit?limit=30' | jget "any(e['action']=='migrate.backup.inspect' for e in d)")" True
CPID=$IID

say "cPanel: içe aktarma"
check "var olmayan alan adı" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"$CPID\",\"owner_user_id\":$UC,\"items\":[{\"domain\":\"yok.test\"}]}")" 400
check "var olmayan veritabanı" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"$CPID\",\"owner_user_id\":$UC,\"items\":[{\"domain\":\"$CPMAIN\",\"databases\":[\"yok_db\"]}]}")" 400
check "geçersiz hedef alan adı" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"$CPID\",\"owner_user_id\":$UC,\"items\":[{\"domain\":\"$CPMAIN\",\"target_domain\":\"a b\"}]}")" 400
check "olmayan hesap" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"$CPID\",\"owner_user_id\":999999,\"items\":[{\"domain\":\"$CPMAIN\"}]}")" 400
check "boş liste" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"$CPID\",\"owner_user_id\":$UC,\"items\":[]}")" 400
IID=$CPID
do_import "[{\"domain\":\"$CPMAIN\",\"databases\":[\"$SRCDB\",\"bimp_pg\"]},{\"domain\":\"$CPADDON\"}]"
check "içe aktarma görevi" "$RES" success
LOG=$(joblog "$JOB")
has "PostgreSQL atlandı ve bildirildi" "$LOG" "PostgreSQL"
check "gizli bilgiler görev günlüğünde yok" "$(grep -cF "$SRCDBPASS" <<<"$LOG")" 0
check "siteler hesapta açıldı" "$(api GET /sites | jget "sorted((s['domain'], s['owner'], s['kind']) for s in d if s['domain'] in ('$CPMAIN','$CPADDON'))")" "[('$CPADDON', '$C', 'static'), ('$CPMAIN', '$C', 'php')]"
DIR=/home/$C/sites/$CPMAIN/public
check "dosyalar yerinde" "$(cat $DIR/isaret.txt) $(cat $DIR/wp-content/uploads/2026/foto.jpg)" "yedekten-gelen-dosya resim"
check "dosyaların tamamı hesabın" "$(find $DIR ! -user $C | wc -l)" 0
check "setuid/setgid yok" "$(find $DIR -perm /6000 | wc -l)" 0
check "sembolik bağlar gelmedi" "$([ -e $DIR/disari-bag ] || [ -L $DIR/disari-bag ] && echo var || echo yok)" yok
check "yayın klasörü başkasına yazılabilir değil" "$(find $DIR -perm /o+w ! -type l | wc -l)" 0
check "ek alan adı dosyası" "$(cat /home/$C/sites/$CPADDON/public/index.html)" "ek-alan-sitesi"
NEWDB=$(cfgval DB_NAME $DIR)
has "wp-config hesabın yeni veritabanını gösteriyor" "$NEWDB" "${C}_"
check "kaynağın veritabanı parolası kalmadı" "$(grep -cF "$SRCDBPASS" $DIR/wp-config.php)" 0
check "yeni veritabanı hesabın" "$(call $JC GET /databases | jget "[x['name'] for x in d['databases']]")" "['$NEWDB']"
check "içerik geldi, adres yeni alan adına çevrildi" "$(mariadb -N -e "SELECT option_value FROM \`$NEWDB\`.wp_options WHERE option_name='siteurl'")" "http://$CPMAIN"
check "kullanıcı yazıları tablosu dolu" "$(mariadb -N -e "SELECT COUNT(*) > 0 FROM \`$NEWDB\`.wp_posts")" 1
has "içe aktarılan site yayında" "$(curl -s -L -H "Host: $CPMAIN" http://127.0.0.1/ | head -c 4000)" "Yedek Sitesi"
check "kaynak veritabanı değişmedi" "$(mariadb -N -e "SELECT option_value FROM \`$SRCDB\`.wp_options WHERE option_name='siteurl'")" "http://$SRC"
check "inceleme geçici klasörü hâlâ duruyor" "$([ -d $TMPROOT/import-$CPID/raw ] && echo var)" var
check "çalışma klasörü temizlendi" "$([ -e $TMPROOT/import-$CPID/work ] && echo var || echo yok)" yok
check "kilit kalmadı" "$([ -e $TMPROOT/import-$CPID/import.lock ] && echo var || echo yok)" yok
check "denetim günlüğünde içe aktarma" "$(api GET '/system/audit?limit=30' | jget "any(e['action']=='migrate.backup.import' for e in d)")" True

say "Geri alma: bozuk dökümlü yedek"
SITES_BEFORE=$(api GET /sites | jget 'len(d)'); DBS_BEFORE=$(call $JC GET /databases | jget "len(d['databases'])")
do_inspect $W/cpmove-bimprb.tar.gz
check "bozuk dökümlü yedek incelenir" "$IRES" success
do_import "[{\"domain\":\"$CPRB\",\"databases\":[\"bimprb_db\"]}]"
check "içe aktarma başarısız olur" "$RES" failed
has "geri alma bildirildi" "$(joblog "$JOB")" "geri alınıyor"
check "site geri alındı" "$(site_id $CPRB)" ""
check "site klasörü silindi" "$([ -e /home/$C/sites/$CPRB ] && echo var || echo yok)" yok
check "site sayısı değişmedi" "$(api GET /sites | jget 'len(d)')" "$SITES_BEFORE"
check "veritabanı sayısı değişmedi" "$(call $JC GET /databases | jget "len(d['databases'])")" "$DBS_BEFORE"
drop_inspect

say "Plesk: inceleme ve farklı alan adına içe aktarma"
do_inspect $W/backup_bimp.tar
check "Plesk yedeği incelendi" "$IRES" success
M=$(manifest)
check "biçim ve alan adı" "$(jget '(d["manifest"]["format"], [x["name"] for x in d["manifest"]["domains"]])' <<<"$M")" "('plesk', ['$PLDOM'])"
check "veritabanı XML'den ve dökümden" "$(jget '[(x["name"], x["engine"], x["dump_path"] is not None, x["domain"]) for x in d["manifest"]["databases"]]' <<<"$M")" "[('$SRCDB', 'mysql', True, '$PLDOM')]"
check "posta kutusu" "$(jget 'd["manifest"]["mailboxes"]' <<<"$M")" "['info@$PLDOM']"
do_import "[{\"domain\":\"$PLDOM\",\"target_domain\":\"$PLTARGET\",\"databases\":[\"$SRCDB\"]}]"
check "Plesk içe aktarma görevi" "$RES" success
DIR=/home/$C/sites/$PLTARGET/public
check "dosyalar httpdocs'tan geldi" "$(cat $DIR/isaret.txt) $([ -d $DIR/httpdocs ] && echo httpdocs-var || echo httpdocs-yok)" "yedekten-gelen-dosya httpdocs-yok"
check "dosyaların tamamı hesabın" "$(find $DIR ! -user $C | wc -l)" 0
NEWDB2=$(cfgval DB_NAME $DIR)
check "ikinci içe aktarma ayrı veritabanı açtı" "$([ "$NEWDB2" != "$NEWDB" ] && [ -n "$NEWDB2" ] && echo evet)" evet
check "adresler hedef alan adına çevrildi" "$(mariadb -N -e "SELECT option_value FROM \`$NEWDB2\`.wp_options WHERE option_name='siteurl'")" "http://$PLTARGET"
check "aynı hedef alan adı tekrar seçilemez" "$(code POST /migrate/backup/import -d "{\"inspect_id\":\"$IID\",\"owner_user_id\":$UC,\"items\":[{\"domain\":\"$PLDOM\",\"target_domain\":\"$CPMAIN\"},{\"domain\":\"$PLDOM\",\"target_domain\":\"$CPMAIN\"}]}")" 400
drop_inspect

say "HestiaCP: var olan siteye içe aktarma"
HE=$(api POST /sites -d "{\"domain\":\"$HEDOM\",\"kind\":\"php\",\"php_version\":\"8.3\",\"user_id\":$UC}" | jget 'd["id"]')
check "hedef site önceden açıldı" "$(case "$HE" in ''|'('*) echo hayir ;; *) echo evet ;; esac)" evet
do_inspect $W/bimphestia.2026-11-12_12-00-00.tar
check "Hestia yedeği incelendi" "$IRES" success
M=$(manifest)
check "biçim ve alan adı" "$(jget '(d["manifest"]["format"], [x["name"] for x in d["manifest"]["domains"]])' <<<"$M")" "('hestia', ['$HEDOM'])"
check "veritabanı ve kullanıcısı" "$(jget '[(x["name"], x["engine"], x["users"]) for x in d["manifest"]["databases"]]' <<<"$M")" "[('$SRCDB', 'mysql', ['${SRCDB}_u'])]"
check "posta kutusu" "$(jget 'd["manifest"]["mailboxes"]' <<<"$M")" "['info@$HEDOM']"
SITES_BEFORE=$(api GET /sites | jget 'len(d)')
do_import "[{\"domain\":\"$HEDOM\",\"databases\":[\"$SRCDB\"]}]"
check "Hestia içe aktarma görevi (USE satırlı döküm)" "$RES" success
has "var olan site kullanıldı" "$(joblog "$JOB")" "zaten var"
check "yeni site açılmadı" "$(api GET /sites | jget 'len(d)')" "$SITES_BEFORE"
DIR=/home/$C/sites/$HEDOM/public
check "dosyalar public_html'den geldi" "$(cat $DIR/isaret.txt) $([ -d $DIR/public_html ] && echo klasor-var || echo klasor-yok)" "yedekten-gelen-dosya klasor-yok"
NEWDB3=$(cfgval DB_NAME $DIR)
check "veritabanı yüklendi" "$(mariadb -N -e "SELECT option_value FROM \`$NEWDB3\`.wp_options WHERE option_name='siteurl'")" "http://$HEDOM"
has "içe aktarılan site yayında" "$(curl -s -L -H "Host: $HEDOM" http://127.0.0.1/ | head -c 4000)" "Yedek Sitesi"
drop_inspect

say "zstd"
if [ $HAS_ZSTD = hayir ]; then
  # zstd yokken zstd imzalı bir yedek anlaşılır hata vermeli.
  printf '\x28\xb5\x2f\xfd\x00\x00\x00\x00' > $W/sahte.tar.zst
  do_inspect $W/sahte.tar.zst
  check "zstd yokken inceleme başarısız" "$IRES" failed
  has "nedeni anlaşılır" "$(joblog "$IJOB")" "zstd"
else
  echo "  (zstd kurulu: Hestia yedeği zstd ile sıkıştırıldı ve yukarıda içe aktarıldı)"
fi

say "Geçici dosyaların silinmesi"
check "inceleme silinir" "$(code DELETE /migrate/backup/$CPID)" 200
check "silinen inceleme okunamaz" "$(code GET /migrate/backup/$CPID)" 404
check "geçici klasör kalmadı" "$([ -e $TMPROOT/import-$CPID ] && echo var || echo yok)" yok
check "denetim günlüğünde silme" "$(api GET '/system/audit?limit=60' | jget "any(e['action']=='migrate.backup.delete' for e in d)")" True
# Bayat inceleme: oluşturulma zamanı 25 saat öncesine çekilirse temizlik döngüsü (10 dakikada bir) siler.
# Beklemek istemiyorsanız BIMP_SKIP_EXPIRY=1 verin.
if [ "${BIMP_SKIP_EXPIRY:-0}" != 1 ]; then
  do_inspect $W/cpmove-bimprb.tar.gz
  python3 - "$TMPROOT/import-$IID/meta.json" <<'PY'
import json, sys, time
p = sys.argv[1]
d = json.load(open(p)); d['created_at'] = int(time.time()) - 25 * 3600
json.dump(d, open(p, 'w'))
PY
  for _ in $(seq 1 130); do [ -e $TMPROOT/import-$IID ] || break; sleep 5; done
  check "24 saati geçen geçici klasör kendiliğinden silindi" "$([ -e $TMPROOT/import-$IID ] && echo var || echo yok)" yok
fi

say "Temizlik"
cleanup_all
LEFT=0; for id in $ALL_IDS; do [ -e $TMPROOT/import-$id ] && LEFT=$((LEFT+1)); done
check "bu sınamanın geçici klasörlerinin hepsi silindi" "$LEFT" 0
check "kaynak site silindi" "$(site_id $SRC)" ""
check "hesap silindi" "$(user_id $C)" ""
rm -f $J $JC
echo "bitti"
