#!/usr/bin/env bash
# Taşıma aracı: başka sunucudaki site SSH ile çekilir. "Karşı sunucu" olarak bu makinenin ağ adresi
# kullanılır: kaynak, yöneticinin WordPress sitesidir (anahtarla, root olarak okunur) ve parolayla
# giren geçici bir Linux kullanıcısının klasörüdür. Hedef, bir müşteri hesabının siteleridir.
# Karşı sunucu yerel ağda olduğu için taşımayı yönetici başlatır (müşteri özel ağ adreslerine
# bağlanamaz); iş yine müşterinin Linux kullanıcısıyla çalışır.
# Panelin kurulu olduğu makinede çalışır. Kullanım: PX_USER=... PX_PASS=... bash migrate.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-mig
JC=/tmp/pxjar-mig-c
C=tasiyan
PASS='Tasima-Sinama-Parolasi-1'
WP_PASS='Wp-Tasima-Parolasi-1'
SRC=tasima-kaynak.test
DST=tasima-hedef.test
STATIC=tasima-duz.test
OLDUSER=pxeskisunucu
OLDPASS='Eski-Sunucu-Parolasi-9'
KEY=/tmp/pxmig-key
ADDR=$(hostname -I | awk '{print $1}')
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
# body <kullanıcı> <yol> [ek alanlar]: taşıma isteğinin gövdesi (ek alanlar python sözlüğü olarak).
body() { python3 -c "
import json, sys
d = {'host': '$ADDR', 'username': sys.argv[1], 'path': sys.argv[2]}
d.update(eval(sys.argv[3]) if len(sys.argv) > 3 else {})
print(json.dumps(d))" "$@"; }
keybody() { python3 -c "
import json, sys
d = {'host': '$ADDR', 'username': 'root', 'path': sys.argv[1], 'private_key': open('$KEY').read()}
d.update(eval(sys.argv[2]) if len(sys.argv) > 2 else {})
print(json.dumps(d))" "$@"; }
cleanup_remote() {
  sed -i '/pixpanel-tasima-sinamasi/d' /root/.ssh/authorized_keys 2>/dev/null
  rm -f $KEY $KEY.pub
  # Geçici kullanıcının oturum yöneticisi kapanmadan öldürülürse "başarısız birim" olarak kalır.
  if id $OLDUSER >/dev/null 2>&1; then
    local uid; uid=$(id -u $OLDUSER)
    loginctl terminate-user $OLDUSER 2>/dev/null; systemctl stop "user@$uid.service" "user-runtime-dir@$uid.service" 2>/dev/null
    pkill -KILL -u $OLDUSER 2>/dev/null; userdel -r $OLDUSER 2>/dev/null
    systemctl reset-failed "user@$uid.service" "user-runtime-dir@$uid.service" 2>/dev/null
  fi
  # Soketle başlatılan sshd yeniden yüklenemez (portu soket birimi tutar); yeniden başlatılır.
  if [ -e /etc/ssh/sshd_config.d/00-pxmig-sinama.conf ]; then rm -f /etc/ssh/sshd_config.d/00-pxmig-sinama.conf; systemctl restart ssh 2>/dev/null; fi
  fail2ban-client unban "$ADDR" >/dev/null 2>&1
}
drop_wp_databases() {
  for old in $(api GET /databases | jget "' '.join(str(x['id']) for x in d['databases'] if 'tasima_' in x['name'])"); do api DELETE /databases/$old >/dev/null; done
  for old in $(api GET /databases | jget "' '.join(str(x['id']) for x in d['users'] if 'tasima_' in x['name'])"); do api DELETE /databases/users/$old >/dev/null; done
}

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in $SRC $DST $STATIC; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
drop_wp_databases
id=$(user_id $C); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
cleanup_remote

say "Hazırlık"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$C\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
UC=$(user_id $C)
S=$(api POST /sites -d "{\"domain\":\"$SRC\",\"kind\":\"php\",\"php_version\":\"8.3\"}" | jget 'd["id"]')
D=$(api POST /sites -d "{\"domain\":\"$DST\",\"kind\":\"php\",\"php_version\":\"8.3\",\"user_id\":$UC}" | jget 'd["id"]')
T=$(api POST /sites -d "{\"domain\":\"$STATIC\",\"kind\":\"static\",\"user_id\":$UC}" | jget 'd["id"]')
JOB=$(api POST /sites/$S/wordpress/install -d "{\"title\":\"Eski Site\",\"admin_user\":\"yonetici\",\"admin_email\":\"ali@ornek.com\",\"admin_password\":\"$WP_PASS\",\"locale\":\"tr_TR\"}" | jget 'd["job_id"]')
check "kaynak WordPress kuruldu" "$(wait_job "$JOB")" success
SRCDIR=/home/$PX_USER/sites/$SRC/public
DSTDIR=/home/$C/sites/$DST/public
echo "tasinan-dosya" > $SRCDIR/isaret.txt
mkdir -p "$SRCDIR/wp-content/uploads/2026" && echo resim > "$SRCDIR/wp-content/uploads/2026/foto.jpg"
ln -sf /etc/passwd $SRCDIR/disari-bag
chown -R -h $PX_USER: $SRCDIR/isaret.txt $SRCDIR/wp-content/uploads $SRCDIR/disari-bag
SRCDB=$(sed -n "s/^define( *'DB_NAME', *'\\(.*\\)' *);.*/\\1/p" $SRCDIR/wp-config.php)
SRCDBPASS=$(sed -n "s/^define( *'DB_PASSWORD', *'\\(.*\\)' *);.*/\\1/p" $SRCDIR/wp-config.php)
# "Karşı sunucu"ya giriş: root için geçici anahtar, parolayla giren geçici kullanıcı.
ssh-keygen -q -t ed25519 -N '' -C pixpanel-tasima-sinamasi -f $KEY
mkdir -p /root/.ssh && cat $KEY.pub >> /root/.ssh/authorized_keys
useradd -m -s /bin/bash $OLDUSER && echo "$OLDUSER:$OLDPASS" | chpasswd
mkdir -p /home/$OLDUSER/public_html/alt && echo "eski-duz-site" > /home/$OLDUSER/public_html/index.html && echo "alt-dosya" > /home/$OLDUSER/public_html/alt/sayfa.html
chown -R $OLDUSER: /home/$OLDUSER/public_html
# Sınama bilerek yanlış girişler dener; sshd'nin kaynak başına geçici engeli sonraki adımları kesmesin.
echo "PerSourcePenalties no" > /etc/ssh/sshd_config.d/00-pxmig-sinama.conf; systemctl restart ssh; sleep 1
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$C\",\"password\":\"$PASS\"}"

say "Girdi denetimi"
check "kabuk karakterli yol" "$(code POST /sites/$D/migrate/probe -d "$(keybody '/var/www/$(id)')")" 400
check "göreli yol" "$(code POST /sites/$D/migrate/probe -d "$(keybody 'public_html')")" 400
check "üst klasöre çıkan yol" "$(code POST /sites/$D/migrate/probe -d "$(keybody '/var/../etc')")" 400
check "seçenek gibi sunucu adı" "$(code POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR "{'host': '-oProxyCommand=id'}")")" 400
check "seçenek gibi kullanıcı adı" "$(code POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR "{'username': '-oProxyCommand=id'}")")" 400
check "yerel adrese bağlanılamaz" "$(code POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR "{'host': '127.0.0.1'}")")" 400
check "yerel adrese çözülen ad" "$(code POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR "{'host': 'localhost'}")")" 400
check "parola ve anahtar birlikte" "$(code POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR "{'password': 'x'}")")" 400
check "kimlik bilgisi yok" "$(code POST /sites/$D/migrate/probe -d "$(body root $SRCDIR)")" 400
check "anahtar olmayan metin" "$(code POST /sites/$D/migrate/probe -d "$(body root $SRCDIR "{'private_key': 'merhaba'}")")" 400
has "müşteri yerel ağdaki sunucuya bağlanamaz" "$(call $JC POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR)")" "Bu adrese bağlanılamaz"
check "başkasının sitesine taşıma başlatılamaz" "$(status $JC POST /sites/$S/migrate/probe -d "$(keybody $SRCDIR)")" 404
check "onaysız anahtarla taşıma başlamaz" "$(R=$(api POST /sites/$D/migrate -d "$(keybody $SRCDIR)"); wait_job "$(jget 'd["job_id"]' <<<"$R")")" failed

say "Bağlantı denemesi"
R=$(call $J POST /sites/$D/migrate/probe -d "$(keybody $SRCDIR)")
check "klasör bulundu, WordPress tanındı" "$(jget '(d["path_found"], d["wordpress"]["database"], d["can_dump"])' <<<"$R")" "(True, '$SRCDB', True)"
has "sunucu anahtarının parmak izi" "$(jget 'd["fingerprints"]' <<<"$R")" "SHA256:$(ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub | awk '{print $2}' | cut -d: -f2)"
check "veritabanı parolası yanıtta yok" "$(grep -cF "$SRCDBPASS" <<<"$R")" 0
HOSTKEY=$(jget 'd["host_key"]' <<<"$R")
check "olmayan klasör" "$(call $J POST /sites/$D/migrate/probe -d "$(keybody /yok/boyle/klasor)" | jget 'd["path_found"]')" False
has "yanlış anahtar anlaşılır hata verir" "$(ssh-keygen -q -t ed25519 -N '' -f /tmp/pxmig-yanlis <<<y >/dev/null; call $J POST /sites/$D/migrate/probe -d "$(KEY=/tmp/pxmig-yanlis; python3 -c "import json; print(json.dumps({'host':'$ADDR','username':'root','path':'$SRCDIR','private_key':open('/tmp/pxmig-yanlis').read()}))")"; rm -f /tmp/pxmig-yanlis /tmp/pxmig-yanlis.pub)" "girişi reddetti"
has "yanlış parola anlaşılır hata verir" "$(call $J POST /sites/$T/migrate/probe -d "$(body $OLDUSER '~/public_html' "{'password': 'yanlis-parola'}")")" "girişi reddetti"
has "kapalı bağlantı noktası anlaşılır hata verir" "$(call $J POST /sites/$T/migrate/probe -d "$(body $OLDUSER '~/public_html' "{'password': 'x', 'port': 2}")")" "yanıt vermedi"

say "WordPress taşıma (anahtarla)"
JOB=$(call $J POST /sites/$D/migrate -d "$(keybody $SRCDIR "{'host_key': '''$HOSTKEY''', 'database': True}")" | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "taşıma görevi" "$RESULT" success
[ "$RESULT" = success ] || joblog "$JOB" | tail -25 | sed 's/^/        /'
check "dosyalar geldi" "$(cat $DSTDIR/isaret.txt 2>/dev/null) $(cat $DSTDIR/wp-content/uploads/2026/foto.jpg 2>/dev/null)" "tasinan-dosya resim"
check "dosyaların tamamı hesabın" "$(find $DSTDIR ! -user $C | wc -l)" 0
check "karşılama sayfası kaldırıldı" "$([ -e $DSTDIR/index.html ] && echo var || echo yok)" yok
check "sembolik bağ bağ olarak geldi" "$(readlink $DSTDIR/disari-bag)" /etc/passwd
NEWDB=$(sed -n "s/^define( *'DB_NAME', *'\\(.*\\)' *);.*/\\1/p" $DSTDIR/wp-config.php)
has "ayar dosyası hesabın yeni veritabanını gösteriyor" "$NEWDB" "${C}_"
check "yeni veritabanı panelde hesabın" "$(call $JC GET /databases | jget "[x['name'] for x in d['databases']]")" "['$NEWDB']"
check "kaynağın veritabanı parolası hedefte kalmadı" "$(grep -cF "$SRCDBPASS" $DSTDIR/wp-config.php)" 0
check "içerik taşındı, adres yeni alan adına çevrildi" "$(mariadb -N -e "SELECT option_value FROM \`$NEWDB\`.wp_options WHERE option_name='siteurl'")" "http://$DST"
has "taşınan site yayında" "$(curl -s -L -H "Host: $DST" http://127.0.0.1/ | head -c 4000)" "Eski Site"
has "kaynak site yerinde" "$(curl -s -L -H "Host: $SRC" http://127.0.0.1/ | head -c 4000)" "Eski Site"
check "kaynağın adresi değişmedi" "$(mariadb -N -e "SELECT option_value FROM \`$SRCDB\`.wp_options WHERE option_name='siteurl'")" "http://$SRC"
LOG=$(joblog "$JOB")
check "gizli bilgiler görev günlüğünde yok" "$(grep -cF -e "$SRCDBPASS" -e "PRIVATE KEY" <<<"$LOG")" 0
check "geçici bağlantı dosyaları silindi" "$(ls /run/pixpanel-migrate 2>/dev/null | wc -l)" 0
has "denetim günlüğüne yazıldı" "$(api GET '/system/audit?limit=20' | jget "[e['target'] for e in d if e['action']=='site.migrate']")" "$DST"

say "Değişen sunucu anahtarı"
FAKE="pixpanel-migrate ssh-ed25519 $(ssh-keygen -q -t ed25519 -N '' -f /tmp/pxmig-sahte <<<y >/dev/null; awk '{print $2}' /tmp/pxmig-sahte.pub; rm -f /tmp/pxmig-sahte /tmp/pxmig-sahte.pub)"
JOB=$(call $J POST /sites/$T/migrate -d "$(keybody $SRCDIR "{'host_key': '$FAKE'}")" | jget 'd["job_id"]')
check "onaylanandan farklı anahtarda taşıma durur" "$(wait_job "$JOB")" failed
has "nedeni açıklanır" "$(joblog "$JOB")" "anahtarı onayladığınızla uyuşmuyor"
check "hiçbir dosya gelmedi" "$([ -e /home/$C/sites/$STATIC/public/isaret.txt ] && echo var || echo yok)" yok

say "Düz site taşıma (parolayla)"
R=$(call $J POST /sites/$T/migrate/probe -d "$(body $OLDUSER '~/public_html' "{'password': '$OLDPASS'}")")
check "klasör bulundu, WordPress yok" "$(jget '(d["path_found"], d["wordpress"])' <<<"$R")" "(True, None)"
JOB=$(call $J POST /sites/$T/migrate -d "$(body $OLDUSER '~/public_html' "{'password': '$OLDPASS', 'host_key': '''$(jget 'd["host_key"]' <<<"$R")'''}")" | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "taşıma görevi" "$RESULT" success
[ "$RESULT" = success ] || joblog "$JOB" | tail -25 | sed 's/^/        /'
has "taşınan site yayında" "$(curl -s -H "Host: $STATIC" http://127.0.0.1/)" "eski-duz-site"
has "alt klasörler geldi" "$(curl -s -H "Host: $STATIC" http://127.0.0.1/alt/sayfa.html)" "alt-dosya"
check "parola görev günlüğünde yok" "$(joblog "$JOB" | grep -cF "$OLDPASS")" 0

say "WordPress dışı uygulamanın veritabanı (bilgiler elle)"
RDB=pxmig_duz; RDBPASS='Eski"Db\Parola#7'
mariadb -e "DROP DATABASE IF EXISTS $RDB; CREATE DATABASE $RDB; DROP USER IF EXISTS '$RDB'@'localhost'; CREATE USER '$RDB'@'localhost' IDENTIFIED BY 'Eski\"Db\\\\Parola#7'; GRANT ALL ON $RDB.* TO '$RDB'@'localhost'; CREATE TABLE $RDB.notlar (id INT PRIMARY KEY, metin VARCHAR(50)); INSERT INTO $RDB.notlar VALUES (1, 'tasinan-kayit');"
printf 'APP_NAME=Eski\nDB_HOST=localhost\nDB_DATABASE=%s\nDB_USERNAME=%s\nDB_PASSWORD="gizli-eski"\nSON=1\n' $RDB $RDB > /home/$OLDUSER/public_html/.env
chown $OLDUSER: /home/$OLDUSER/public_html/.env
HK=$(jget 'd["host_key"]' <<<"$R")
rdb() { python3 -c "
import json, sys
d = {'host': '$ADDR', 'username': '$OLDUSER', 'path': '~/public_html', 'password': '$OLDPASS', 'host_key': sys.argv[1], 'database': True, 'remote_database': json.loads(sys.argv[2])}
print(json.dumps(d))" "$HK" "$1"; }
check "veritabanı adı boşsa reddedilir" "$(status $J POST /sites/$T/migrate -d "$(rdb '{"name":"","user":"x"}')")" 400
JOB=$(call $J POST /sites/$T/migrate -d "$(rdb '{"name":"pxmig_duz","user":"pxmig_duz","password":"yanlis"}')" | jget 'd["job_id"]')
check "yanlış veritabanı parolasında görev başarısız" "$(wait_job "$JOB")" failed
check "yarım kalan veritabanı geri alındı" "$(call $JC GET /databases | jget "len([x for x in d['databases'] if '_db_' in x['name']])")" 0
JOB=$(call $J POST /sites/$T/migrate -d "$(rdb '{"name":"pxmig_duz;id","user":"pxmig_duz","password":"x"}')" | jget 'd["job_id"]')
check "kabuk karakterli veritabanı adı çalıştırılmaz" "$(wait_job "$JOB") $(call $JC GET /databases | jget "len([x for x in d['databases'] if '_db_' in x['name']])")" "failed 0"
JOB=$(call $J POST /sites/$T/migrate -d "$(rdb "$(python3 -c "import json; print(json.dumps({'name':'pxmig_duz','user':'pxmig_duz','password':'Eski\"Db\\\\Parola#7','host':'localhost'}))")")" | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "veritabanlı taşıma görevi" "$RESULT" success
[ "$RESULT" = success ] || joblog "$JOB" | tail -25 | sed 's/^/        /'
PLAINDB=$(call $JC GET /databases | jget "next((x['name'] for x in d['databases'] if '_db_' in x['name']), '')")
has "yeni veritabanı hesabın adıyla açıldı" "$PLAINDB" "${C}_db_"
check "kayıtlar taşındı" "$(mariadb -N -e "SELECT metin FROM \`$PLAINDB\`.notlar WHERE id=1")" "tasinan-kayit"
TDIR=/home/$C/sites/$STATIC/public
check ".env yeni veritabanına çevrildi" "$(grep -c "^DB_DATABASE=$PLAINDB$" $TDIR/.env) $(grep -c 'gizli-eski' $TDIR/.env) $(grep -c '^SON=1$' $TDIR/.env)" "1 0 1"
NOTE=/home/$C/tmp/veritabani-$PLAINDB.txt
check "bağlantı bilgileri dosyası yalnız hesabın" "$(stat -c '%a %U' $NOTE 2>/dev/null)" "600 $C"
NEWPASS=$(sed -n 's/^Parola: //p' $NOTE 2>/dev/null)
check "dosyadaki parola çalışıyor" "$(mariadb -u "$PLAINDB" -p"$NEWPASS" -N -e "SELECT COUNT(*) FROM \`$PLAINDB\`.notlar" 2>/dev/null)" 1
check ".env içindeki parola da aynı" "$(grep -c "^DB_PASSWORD=$NEWPASS$" $TDIR/.env)" 1
check "parolalar görev günlüğünde yok" "$(joblog "$JOB" | grep -cF -e "$NEWPASS" -e 'Parola#7' -e "$OLDPASS")" 0
mariadb -e "DROP DATABASE IF EXISTS $RDB; DROP USER IF EXISTS '$RDB'@'localhost';"

say "Temizlik"
cleanup_remote
for name in $SRC $DST $STATIC; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
drop_wp_databases
check "hesabı sil" "$(code DELETE /users/$UC)" 200
rm -f $J $JC
echo "bitti"
