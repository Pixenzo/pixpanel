#!/usr/bin/env bash
# Posta ekleri sınaması: posta listeleri, alan adı başına sertifika (`mail.<alan>`, SNI, `webmail.<alan>`)
# ve ClamAV virüs taraması. Test makinesinde root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash mail-extras.sh
# Önkoşul: posta kurulu ve çalışıyor. Sertifika bölümü ACME sınama sunucusu (Pebble) ister; yoksa atlanır.
# ClamAV bölümü bileşen kuruluysa koşar; değilse "atlandı" diye geçilir (bileşen bu betikçe kurulmaz).
# Kendi posta alanını (mx-ekstra.test) oluşturur ve sonunda siler; başka alanlara dokunmaz.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-mextras
JC=/tmp/pxjar-mextras-cust
HERE="$(cd "$(dirname "$0")" && pwd)"
T=$(date +%s)
DOM="mx-ekstra.test"
CUSTOMER="ekstramusteri"
CPASS='Ekstra-Musteri-Parola-1'
export MAIL_PASS='Kutu-Parola-12345'
MAILHOST=$(hostname -I | awk '{print $1}')
DB=/var/lib/pixpanel/pixpanel.db
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
ccode() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if printf '%s' "$2" | grep -qF -- "$3"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(printf '%s' "$2" | tr '\n' ' ' | head -c 240)]"; fi; }
hasnt() { if printf '%s' "$2" | grep -qF -- "$3"; then echo "  HATA $1: [$3] olmamalıydı"; else echo "  OK   $1"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
mf() { python3 "$HERE/mailflow.py" "$@"; }
wait_job() { for _ in $(seq 1 180); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
mail_id() { api GET /mail | jget "next((x['id'] for x in d['domains'] if x['domain']=='$1'), '')"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
# Konunun $2 klasöründe görünmesini en çok 30 sn bekler; konu listesini yazar.
wait_subject() { local user=$1 folder=$2 subject=$3 list; for _ in $(seq 1 15); do list=$(mf list "$user" "$folder"); grep -qF -- "$subject" <<<"$list" && { echo "$list"; return; }; sleep 2; done; echo "$list"; }
# Verilen portta (starttls: smtp ya da boş) $3 adıyla istenen sertifikanın seri numarası.
serial() { echo | timeout 8 openssl s_client -connect "127.0.0.1:$1" ${2:+-starttls $2} -servername "$3" 2>/dev/null | openssl x509 -noout -serial 2>/dev/null; }
file_serial() { openssl x509 -in "$1" -noout -serial 2>/dev/null; }

cleanup() {
  local id
  id=$(mail_id "$DOM"); [ -n "$id" ] && api DELETE "/mail/domains/$id?delete_mail=true" >/dev/null
  id=$(user_id $CUSTOMER); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
  sed -i '/px-mail-extras/d' /etc/hosts
}

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Hazırlık"
check "temel yapılandırma" "$(code POST /mail/setup)" 200
check "Postfix yapılandırması geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok
check "Dovecot yapılandırması geçerli" "$(doveconf -n >/dev/null 2>&1 && echo ok)" ok
D=$(api POST /mail/domains -d "{\"domain\":\"$DOM\"}" | jget 'd["id"]')
check "posta alanı açıldı" "$([ -n "$D" ] && [ "$D" -gt 0 ] 2>/dev/null && echo var)" var
UA=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"uyea\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
UB=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"uyeb\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
YON=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"yon\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
check "kutular açıldı" "$([ -n "$UA" ] && [ -n "$UB" ] && [ -n "$YON" ] && echo var)" var

say "Posta listeleri: oluşturma ve doğrulama"
L=$(api POST /mail/domains/$D/lists -d '{"local":"duyuru","mode":"members"}' | jget 'd["id"]')
check "liste oluşturuldu" "$([ -n "$L" ] && [ "$L" -gt 0 ] 2>/dev/null && echo var)" var
check "aynı adla ikinci liste" "$(code POST /mail/domains/$D/lists -d '{"local":"duyuru"}')" 409
check "kutuyla çakışan ad" "$(code POST /mail/domains/$D/lists -d '{"local":"uyea"}')" 409
check "listeyle çakışan takma ad" "$(code POST /mail/domains/$D/aliases -d "{\"local\":\"duyuru\",\"destinations\":[\"uyea@$DOM\"]}")" 409
check "geçersiz ad" "$(code POST /mail/domains/$D/lists -d '{"local":"../x"}')" 400
check "geçersiz kip" "$(code POST /mail/domains/$D/lists -d '{"local":"kip","mode":"herkes"}')" 400
check "üyesiz liste: alias satırı yok" "$(grep -c "^duyuru@$DOM " /etc/pixpanel/mail/aliases)" 0
check "geçersiz üye reddedilir" "$(code POST /mail/domains/$D/lists/$L/members -d "{\"text\":\"uyea@$DOM\\nsacma\\nb@@x.test\"}")" 400
check "geçersiz üye listeye girmedi (hiçbiri işlenmez)" "$(api GET /mail/domains/$D/lists/$L | jget 'len(d["members"])')" 0
check "kendi adresi üye olamaz" "$(code POST /mail/domains/$D/lists/$L/members -d "{\"text\":\"duyuru@$DOM\"}")" 400
check "CRLF ya da tırnak içeren üye reddedilir" "$(code POST /mail/domains/$D/lists/$L/members -d "{\"text\":\"a@x.test\\\";rm -rf\"}")" 400
check "toplu üye ekleme" "$(api POST /mail/domains/$D/lists/$L/members -d "{\"text\":\"uyea@$DOM, Ayse Veli <uyeb@$DOM>\\nUYEA@$DOM\"}" | jget '(d["added"], d["existing"])')" "(2, 0)"
check "yinelenen üye tekrar eklenmez" "$(api POST /mail/domains/$D/lists/$L/members -d "{\"text\":\"uyea@$DOM\"}" | jget '(d["added"], d["existing"])')" "(0, 1)"
check "yönetici üye eklendi" "$(api POST /mail/domains/$D/lists/$L/members -d "{\"text\":\"yon@$DOM\",\"admin\":true}" | jget 'd["added"]')" 1
check "üye sayısı" "$(api GET /mail/domains/$D/lists | jget 'd[0]["member_count"]')" 3
has "alias eşlemesinde liste genişlemesi" "$(grep "^duyuru@$DOM " /etc/pixpanel/mail/aliases)" "uyea@$DOM, uyeb@$DOM, yon@$DOM"
check "Postfix liste adresini çözüyor" "$(postmap -q duyuru@$DOM texthash:/etc/pixpanel/mail/aliases | tr -d ' ')" "uyea@$DOM,uyeb@$DOM,yon@$DOM"
has "kısıt sınıfı tanımlı" "$(postconf -h smtpd_restriction_classes)" "pixlist_$L"
has "sınıf tanımı gönderen eşlemesini kullanıyor" "$(postconf -h pixlist_$L)" "lists/$L.senders"
has "25. port kısıt listesinde liste eşlemesi" "$(postconf -h smtpd_recipient_restrictions)" "list-access"
has "587 kısıt listesinde liste eşlemesi" "$(postconf -h -P submission/inet/smtpd_recipient_restrictions)" "list-access"
check "Postfix yapılandırması hâlâ geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok

say "Posta listeleri: gönderim kipleri"
S1="liste-uye-$T"
check "üye (kimlikli) listeye gönderebilir" "$(mf send uyea@$DOM uyea@$DOM duyuru@$DOM "$S1" 587)" sent
has "ileti diğer üyeye ulaştı" "$(wait_subject uyeb@$DOM INBOX "$S1")" "$S1"
has "ileti yöneticiye ulaştı" "$(wait_subject yon@$DOM INBOX "$S1")" "$S1"
S2="liste-disari-$T"
check "üye olmayan (25. port) reddedilir" "$(MAIL_HOST=$MAILHOST mf send - kimse@disarisi.example duyuru@$DOM "$S2" 25 | cut -c1-8)" rejected
sleep 2
hasnt "reddedilen ileti teslim edilmedi" "$(mf list uyeb@$DOM)" "$S2"
check "kip: yalnızca yöneticiler" "$(api PATCH /mail/domains/$D/lists/$L -d '{"mode":"admins"}' | jget 'd["mode"]')" admins
check "yönetici olmayan üye reddedilir" "$(mf send uyea@$DOM uyea@$DOM duyuru@$DOM "liste-yok-$T" 587 | cut -c1-8)" rejected
S3="liste-yon-$T"
check "yönetici gönderebilir" "$(mf send yon@$DOM yon@$DOM duyuru@$DOM "$S3" 587)" sent
has "yönetici iletisi üyeye ulaştı" "$(wait_subject uyeb@$DOM INBOX "$S3")" "$S3"
check "üyeyi yönetici yap" "$(api PATCH /mail/domains/$D/lists/$L/members -d "{\"text\":\"uyea@$DOM\",\"admin\":true}" | jget 'd["changed"]')" 1
S4="liste-uyea-yon-$T"
check "yönetici yapılan üye artık gönderebilir" "$(mf send uyea@$DOM uyea@$DOM duyuru@$DOM "$S4" 587)" sent
check "kip: herkes" "$(api PATCH /mail/domains/$D/lists/$L -d '{"mode":"anyone"}' | jget 'd["mode"]')" anyone
S5="liste-herkes-$T"
check "herkes kipinde dışarıdan gönderim kabul edilir" "$(MAIL_HOST=$MAILHOST mf send - kimse@disarisi.example duyuru@$DOM "$S5" 25)" sent
has "dışarıdan gelen üyeye ulaştı" "$(wait_subject uyeb@$DOM INBOX "$S5")" "$S5"
check "herkes kipinde sınıf kalkar" "$(postconf -h smtpd_restriction_classes | grep -c "pixlist_$L")" 0
check "kip: yalnızca üyeler (geri)" "$(api PATCH /mail/domains/$D/lists/$L -d '{"mode":"members"}' | jget 'd["mode"]')" members

say "Posta listeleri: üye çıkarma, boş liste, silme"
check "üye çıkarıldı" "$(api POST /mail/domains/$D/lists/$L/members/remove -d "{\"text\":\"yon@$DOM\"}" | jget 'd["removed"]')" 1
hasnt "çıkarılan üye genişlemeden düştü" "$(grep "^duyuru@$DOM " /etc/pixpanel/mail/aliases)" "yon@$DOM"
check "var olmayan üyeyi çıkarma zararsız" "$(api POST /mail/domains/$D/lists/$L/members/remove -d "{\"text\":\"yok@$DOM\"}" | jget 'd["removed"]')" 0
L2=$(api POST /mail/domains/$D/lists -d '{"local":"bos"}' | jget 'd["id"]')
check "üyesiz liste iletiyi reddeder" "$(MAIL_HOST=$MAILHOST mf send - kimse@disarisi.example bos@$DOM "liste-bos-$T" 25 | cut -c1-8)" rejected
check "üyesiz liste alias satırı üretmez" "$(grep -c "^bos@$DOM " /etc/pixpanel/mail/aliases)" 0
check "üyeler listeden okunur" "$(api GET /mail/domains/$D/lists/$L | jget 'sorted(m["address"] for m in d["members"])' | tr -d "'[] ")" "uyea@$DOM,uyeb@$DOM"

say "Posta listeleri: başka hesap erişemez"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CPASS\",\"role\":\"user\"}")" 200
check "müşteri girişi" "$(ccode POST /auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CPASS\"}")" 200
check "başkasının alanının listeleri görünmez" "$(ccode GET /mail/domains/$D/lists)" 404
check "başkasının alanında liste oluşturulamaz" "$(ccode POST /mail/domains/$D/lists -d '{"local":"hack"}')" 404
check "başkasının listesine üye eklenemez" "$(ccode POST /mail/domains/$D/lists/$L/members -d '{"text":"a@x.test"}')" 404
check "başkasının listesi silinemez" "$(ccode DELETE /mail/domains/$D/lists/$L)" 404
check "başka alan adının liste kimliği ile erişim" "$(code GET /mail/domains/$D/lists/999999)" 404
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/domains/$D/lists)" 401

check "liste silindi" "$(code DELETE /mail/domains/$D/lists/$L)" 200
check "liste silinince alias satırı kalktı" "$(grep -c "^duyuru@$DOM " /etc/pixpanel/mail/aliases)" 0
check "liste silinince gönderen dosyası kalktı" "$(test -e /etc/pixpanel/mail/lists/$L.senders && echo var || echo yok)" yok
check "liste silinince sınıf kalktı" "$(postconf -h smtpd_restriction_classes | grep -c "pixlist_$L")" 0
check "sınıf parametresi kalktı" "$(postconf -n | grep -c "^pixlist_$L ")" 0
check "silinen liste adresine gönderim reddedilir" "$(MAIL_HOST=$MAILHOST mf send - kimse@disarisi.example duyuru@$DOM "liste-silindi-$T" 25 | cut -c1-8)" rejected
check "Postfix yapılandırması hâlâ geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok
check "denetim: liste oluşturma" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.list.create' AND target='duyuru@$DOM'")" 1
check "denetim: üye ekleme" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.list.members.add' AND target='duyuru@$DOM'")" 1
check "denetim: üye çıkarma" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.list.members.remove' AND target='duyuru@$DOM'")" 1
check "denetim: kip değişikliği" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.list.update' AND target='duyuru@$DOM'")" 1
check "denetim: liste silme" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.list.delete' AND target='duyuru@$DOM'")" 1

say "Alan adı başına sertifika (mail.<alan>)"
if [ -f /etc/systemd/system/pebble-test.service ]; then
  echo "127.0.0.1 mail.$DOM autoconfig.$DOM autodiscover.$DOM webmail.$DOM # px-mail-extras" >> /etc/hosts
  WEBMAIL=$(api GET /mail | jget 'd["webmail"]')
  NGINX=$(command -v nginx >/dev/null && echo yes || echo no)
  echo "  web postası kurulu: $WEBMAIL, nginx: $NGINX"
  WM=false; [ "$WEBMAIL" = "True" ] && [ "$NGINX" = yes ] && WM=true
  OLDHOST=$(api GET /mail/domains/$D/client-settings | jget 'd["hostname"]')
  check "başlangıçta kapalı" "$(api GET /mail/domains/$D/own-name | jget 'd["enabled"]')" False
  has "ad listesi mail. adını içeriyor" "$(api GET /mail/domains/$D/own-name | jget '[n["name"] for n in d["names"]]')" "mail.$DOM"
  has "ad listesi autoconfig ve autodiscover adlarını içeriyor" "$(api GET /mail/domains/$D/own-name | jget '[n["name"] for n in d["names"]]')" "autodiscover.$DOM"
  check "DNS'i panelde olmayan alanda DNS doğrulaması reddedilir" "$(code PUT /mail/domains/$D/own-name -d '{"enabled":true,"dns":true}')" 400
  check "o denemeden sonra açık değil" "$(api GET /mail/domains/$D/own-name | jget 'd["enabled"]')" False
  check "müşteri başkasının alanını açamaz" "$(ccode PUT /mail/domains/$D/own-name -d '{"enabled":true}')" 404
  check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/domains/$D/own-name)" 401
  JOB=$(api PUT /mail/domains/$D/own-name -d "{\"enabled\":true,\"webmail\":$WM}" | jget 'd["job_id"]')
  check "sertifika alındı" "$(wait_job "$JOB")" success
  CERT=/etc/pixpanel/mail/certs/$DOM
  check "sertifika dosyaları yazıldı" "$(test -s $CERT/fullchain.pem && test -s $CERT/privkey.pem && test -s $CERT/combined.pem && echo var)" var
  check "özel anahtar yalnız root'a açık" "$(stat -c '%a %U' $CERT/privkey.pem)" "600 root"
  check "birleşik dosya yalnız root'a açık" "$(stat -c '%a %U' $CERT/combined.pem)" "600 root"
  check "sertifika klasörü yalnız root'a açık" "$(stat -c '%a' $CERT)" 700
  has "sertifika mail. adını kapsıyor" "$(openssl x509 -in $CERT/fullchain.pem -noout -ext subjectAltName 2>/dev/null)" "mail.$DOM"
  has "sertifika autoconfig adını kapsıyor" "$(openssl x509 -in $CERT/fullchain.pem -noout -ext subjectAltName 2>/dev/null)" "autoconfig.$DOM"
  has "sertifika autodiscover adını kapsıyor" "$(openssl x509 -in $CERT/fullchain.pem -noout -ext subjectAltName 2>/dev/null)" "autodiscover.$DOM"
  if [ "$WM" = true ]; then has "sertifika webmail adını kapsıyor" "$(openssl x509 -in $CERT/fullchain.pem -noout -ext subjectAltName 2>/dev/null)" "webmail.$DOM"; fi
  SER=$(file_serial $CERT/fullchain.pem)
  has "Postfix SNI eşlemesi bağlı" "$(postconf -h tls_server_sni_maps)" "/etc/pixpanel/mail/sni"
  has "SNI eşlemesinde alan adı var" "$(cat /etc/pixpanel/mail/sni)" "mail.$DOM"
  check "derlenmiş SNI eşlemesi yalnız root'a açık" "$(stat -c '%a' /etc/pixpanel/mail/sni.db /etc/pixpanel/mail/sni.lmdb 2>/dev/null | sort -u | tr '\n' ' ')" "600 "
  has "Dovecot local_name bloğu" "$(doveconf -n 2>/dev/null | tr '\n' ' ')" "local_name mail.$DOM"
  check "SMTP 587: mail.<alan> kendi sertifikasını sunar" "$(serial 587 smtp mail.$DOM)" "$SER"
  check "SMTP 25: mail.<alan> kendi sertifikasını sunar" "$(serial 25 smtp mail.$DOM)" "$SER"
  check "IMAPS 993: mail.<alan> kendi sertifikasını sunar" "$(serial 993 '' mail.$DOM)" "$SER"
  OTHER587=$(serial 587 smtp baska-ad.test)
  check "başka adla bağlanana panelin sertifikası gelir (587)" "$([ -n "$OTHER587" ] && [ "$OTHER587" != "$SER" ] && echo evet)" evet
  OTHER993=$(serial 993 '' baska-ad.test)
  check "başka adla bağlanana panelin sertifikası gelir (993)" "$([ -n "$OTHER993" ] && [ "$OTHER993" != "$SER" ] && echo evet)" evet
  check "kendi adıyla IMAP girişi çalışıyor" "$(MAIL_HOST=127.0.0.1 mf login uyea@$DOM)" ok
  S6="sni-sonrasi-$T"
  check "SNI açıkken gönderim sürüyor" "$(mf send uyea@$DOM uyea@$DOM uyeb@$DOM "$S6" 587)" sent
  has "SNI açıkken teslimat sürüyor" "$(wait_subject uyeb@$DOM INBOX "$S6")" "$S6"
  check "istemci ayarları mail.<alan> gösterir" "$(api GET /mail/domains/$D/client-settings | jget 'd["hostname"]')" "mail.$DOM"
  check "alan ayrıntısı istemci adını verir" "$(api GET /mail/domains/$D | jget 'd["client_host"]')" "mail.$DOM"
  has "autoconfig yanıtı mail.<alan> adresini içerir" "$(curl -sk "$B/mail/public/autoconfig?emailaddress=uyea@$DOM")" "<hostname>mail.$DOM</hostname>"
  has "autodiscover yanıtı mail.<alan> adresini içerir" "$(curl -sk -X POST "$B/mail/public/autodiscover" -d "<Autodiscover><Request><EMailAddress>uyea@$DOM</EMailAddress></Request></Autodiscover>")" "<Server>mail.$DOM</Server>"
  has "mobileconfig mail.<alan> adresini içerir" "$(curl -sk "$B/mail/public/mobileconfig?email=uyea@$DOM")" "mail.$DOM"
  check "durum: sertifika var" "$(api GET /mail/domains/$D/own-name | jget '(d["enabled"], d["certificate"] is not None, d["last_error"])')" "(True, True, '')"
  if [ "$NGINX" = yes ]; then
    has "autoconfig adı HTTPS'te aynı sertifikayla sunulur" "$(curl -sk --resolve autoconfig.$DOM:443:127.0.0.1 https://autoconfig.$DOM/mail/config-v1.1.xml)" "mail.$DOM"
    check "autoconfig HTTPS sertifikası aynı" "$(serial 443 '' autoconfig.$DOM)" "$SER"
    check "autodiscover HTTPS sertifikası aynı" "$(serial 443 '' autodiscover.$DOM)" "$SER"
  else
    echo "  ATLANDI nginx kurulu değil: autoconfig HTTPS denetimleri"
  fi
  if [ "$WM" = true ]; then
    check "webmail adı HTTPS'te aynı sertifikayla sunulur" "$(serial 443 '' webmail.$DOM)" "$SER"
    check "webmail kökü /webmail/ yoluna yönlenir" "$(curl -sk -o /dev/null -w '%{http_code} %{redirect_url}' --resolve webmail.$DOM:443:127.0.0.1 https://webmail.$DOM/)" "302 https://webmail.$DOM/webmail/"
    has "webmail giriş sayfası açılır" "$(curl -sk --resolve webmail.$DOM:443:127.0.0.1 https://webmail.$DOM/webmail/)" "rcmloginuser"
    check "düz HTTP HTTPS'e yönlenir" "$(curl -s -o /dev/null -w '%{http_code}' --resolve webmail.$DOM:80:127.0.0.1 http://webmail.$DOM/)" 301
    check "webmail adından araç sunucusunun başka yollarına erişilemez" "$(curl -sk -o /dev/null -w '%{http_code}' --resolve webmail.$DOM:443:127.0.0.1 https://webmail.$DOM/tools/phpmyadmin/)" 404
    check "webmail adından panel API'sine erişilemez" "$(curl -sk -o /dev/null -w '%{http_code}' --resolve webmail.$DOM:443:127.0.0.1 https://webmail.$DOM/api/v1/auth/me)" 404
    check "alan ayrıntısı webmail adını verir" "$(api GET /mail/domains/$D | jget 'd["webmail_host"]')" "webmail.$DOM"
  else
    echo "  ATLANDI web postası ya da nginx yok: webmail.<alan> denetimleri"
  fi

  say "Alan adı başına sertifika: yenileme ve haritaların yeniden üretimi"
  JOB=$(api POST /mail/domains/$D/own-name/renew | jget 'd["job_id"]')
  check "sertifika yenilendi" "$(wait_job "$JOB")" success
  NEWSER=$(file_serial $CERT/fullchain.pem)
  check "yeni sertifikanın seri numarası farklı" "$([ -n "$NEWSER" ] && [ "$NEWSER" != "$SER" ] && echo evet)" evet
  check "SMTP yeni sertifikayı sunuyor" "$(serial 587 smtp mail.$DOM)" "$NEWSER"
  check "IMAPS yeni sertifikayı sunuyor" "$(serial 993 '' mail.$DOM)" "$NEWSER"
  check "Postfix yapılandırması hâlâ geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok
  check "Dovecot yapılandırması hâlâ geçerli" "$(doveconf -n >/dev/null 2>&1 && echo ok)" ok
  # Yenileme zamanı: bitimine 30 günden az kalan sertifika günlük denetimde yenilenir; burada yalnızca
  # özelliğin denetim kaydı ve hata alanı sınanır.
  check "yenileme sonrası hata alanı boş" "$(api GET /mail/domains/$D/own-name | jget 'd["last_error"]')" ""

  if [ "$WM" = true ]; then
    say "webmail adresini kapatma"
    check "webmail kapatıldı" "$(code PUT /mail/domains/$D/own-name -d '{"enabled":true,"webmail":false}')" 200
    sleep 2
    check "webmail sunucu bloğu kalktı" "$(grep -c "webmail.$DOM" /etc/nginx/pixpanel/sites/_mail-autoconfig.conf)" 0
    check "webmail adı artık sunulmaz" "$(curl -sk -o /dev/null -w '%{http_code}' --resolve webmail.$DOM:443:127.0.0.1 https://webmail.$DOM/ 2>/dev/null)" 000
    check "mail.<alan> sertifikası sürüyor" "$(serial 587 smtp mail.$DOM | grep -c serial)" 1
    JOB=$(api PUT /mail/domains/$D/own-name -d '{"enabled":true,"webmail":true}' | jget 'd["job_id"]')
    check "webmail yeniden açılınca sertifika adı kapsıyor ya da yeniden alınıyor" "$([ "$JOB" = None ] && echo ok || wait_job "$JOB" | sed 's/success/ok/')" ok
  fi

  say "Alan adı başına sertifika: kapatma"
  check "kapatıldı" "$(code PUT /mail/domains/$D/own-name -d '{"enabled":false}')" 200
  check "sertifika klasörü silindi" "$(test -d $CERT && echo var || echo yok)" yok
  check "SNI eşlemesi Postfix'ten kalktı" "$(postconf -h tls_server_sni_maps)" ""
  hasnt "Dovecot local_name bloğu kalktı" "$(doveconf -n 2>/dev/null | tr '\n' ' ')" "local_name mail.$DOM"
  check "istemci ayarları sunucu adına döndü" "$(api GET /mail/domains/$D/client-settings | jget 'd["hostname"]')" "$OLDHOST"
  check "kapatınca IMAP girişi çalışıyor" "$(mf login uyea@$DOM)" ok
  check "Postfix yapılandırması geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok
  check "denetim: açma" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.domain.own_name.enable' AND target='$DOM'")" 1
  check "denetim: yenileme" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.domain.own_name.renew' AND target='$DOM'")" 1
  check "denetim: kapatma" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='mail.domain.own_name.disable' AND target='$DOM'")" 1

  say "Alan adı başına sertifika: alan silinince temizlik"
  JOB=$(api PUT /mail/domains/$D/own-name -d '{"enabled":true,"webmail":false}' | jget 'd["job_id"]')
  check "yeniden açıldı" "$(wait_job "$JOB")" success
  has "SNI yeniden bağlı" "$(postconf -h tls_server_sni_maps)" "/etc/pixpanel/mail/sni"
  check "alan silindi" "$(code DELETE "/mail/domains/$D?delete_mail=true")" 200
  check "alan silinince sertifika klasörü kalktı" "$(test -d $CERT && echo var || echo yok)" yok
  check "alan silinince SNI eşlemesi kalktı" "$(postconf -h tls_server_sni_maps)" ""
  hasnt "alan silinince Dovecot bloğu kalktı" "$(doveconf -n 2>/dev/null | tr '\n' ' ')" "local_name mail.$DOM"
  check "alan silinince Postfix geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok
  # Sonraki bölümler için alan yeniden açılır.
  D=$(api POST /mail/domains -d "{\"domain\":\"$DOM\"}" | jget 'd["id"]')
  api POST /mail/domains/$D/mailboxes -d "{\"local\":\"uyea\",\"password\":\"$MAIL_PASS\"}" >/dev/null
else
  echo "  ATLANDI ACME sınama sunucusu (Pebble) yok: sertifika bölümü"
fi

say "ClamAV virüs taraması"
CLAM=$(api GET /components | jget "next((c['installed'] for c in d if c['id']=='clamav'), 'yok')")
check "bileşen listesinde clamav var" "$([ "$CLAM" != yok ] && echo var)" var
check "clamav posta kategorisinde" "$(api GET /components | jget "next((c['category'] for c in d if c['id']=='clamav'), '')")" mail
check "clamav kaldırılabilir bileşen" "$(api GET /components | jget "next((c['removable'] for c in d if c['id']=='clamav'), '')")" True
check "durum ucu yönetici dışına kapalı" "$(ccode GET /mail/antivirus)" 403
check "durum ucu oturumsuz kapalı" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/antivirus)" 401
check "durum ucu yöneticiye açık" "$(code GET /mail/antivirus)" 200
check "durum: bellek bilgisi var" "$(api GET /mail/antivirus | jget 'd["memory_mb"] is not None')" True
if [ "$CLAM" = True ]; then
  AV=$(api GET /mail/antivirus)
  check "durum: kurulu" "$(jget 'd["installed"]' <<<"$AV")" True
  check "durum: clamd çalışıyor" "$(jget 'd["running"]' <<<"$AV")" True
  check "durum: Rspamd bağlı" "$(jget 'd["enabled"]' <<<"$AV")" True
  check "durum: imza veritabanı tarihi var" "$(jget 'd["signatures_at"] is not None' <<<"$AV")" True
  check "durum: son 7 gün sayısı var" "$(jget 'd["caught_7d"] is not None' <<<"$AV")" True
  has "antivirus.conf: clamav bölümü" "$(cat /etc/rspamd/local.d/antivirus.conf)" 'type = "clamav"'
  has "antivirus.conf: reject" "$(cat /etc/rspamd/local.d/antivirus.conf)" 'action = "reject"'
  has "antivirus.conf: scan_mime_parts" "$(cat /etc/rspamd/local.d/antivirus.conf)" 'scan_mime_parts = true'
  has "antivirus.conf: unix soketi" "$(cat /etc/rspamd/local.d/antivirus.conf)" 'servers = "/'
  has "_rspamd kullanıcısı clamav grubunda" "$(id -nG _rspamd)" clamav
  check "Rspamd yapılandırması geçerli" "$(rspamadm configtest >/dev/null 2>&1 && echo ok)" ok
  check "yapılandırmayı yeniden yaz" "$(code POST /mail/antivirus/apply)" 200
  check "yeniden yazınca Rspamd yapılandırması geçerli" "$(rspamadm configtest >/dev/null 2>&1 && echo ok)" ok
  check "mail/setup antivirus bağlantısını korur" "$(code POST /mail/setup)" 200
  has "setup sonrası antivirus.conf yerinde" "$(cat /etc/rspamd/local.d/antivirus.conf)" 'type = "clamav"'
  S7="temiz-ileti-$T"
  check "temiz ileti kabul edilir" "$(MAIL_BODY=merhaba MAIL_HOST=$MAILHOST mf send - biri@disarisi.example uyea@$DOM "$S7" 25)" sent
  has "temiz ileti teslim edildi" "$(wait_subject uyea@$DOM INBOX "$S7")" "$S7"
  ACTION=$(api GET /mail/settings | jget 'd["spam"]["reject_action"]')
  # EICAR sınama dizgisi iki parçadan birleştirilir (bu dosya tek başına virüs imzası taşımasın).
  EICAR_RESULT=$(MAIL_HOST=$MAILHOST python3 - "$DOM" "$T" <<'PYEOF'
import os, smtplib, sys
from email.message import EmailMessage
domain, stamp = sys.argv[1], sys.argv[2]
part_a = "X5O!P%@AP[4\\PZX54(P^)7CC)7}"
part_b = "$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*"
message = EmailMessage()
message["From"], message["To"], message["Subject"] = "biri@disarisi.example", "uyea@" + domain, "virus-ileti-" + stamp
message.set_content("ekte sinama dosyasi var")
message.add_attachment((part_a + part_b).encode(), maintype="application", subtype="octet-stream", filename="eicar.com")
try:
    with smtplib.SMTP(os.environ["MAIL_HOST"], 25, timeout=30) as smtp:
        smtp.send_message(message)
    print("sent")
except Exception as error:
    print("rejected: " + str(error)[:120])
PYEOF
)
  if [ "$ACTION" = quarantine ]; then
    check "virüslü ileti kabul edilip karantinaya alınır (karantina kipi açık)" "$EICAR_RESULT" sent
    HELD=""
    for _ in $(seq 1 15); do HELD=$(api GET /mail/quarantine | jget "next((m['id'] for m in d['messages'] if 'virus-ileti-$T' in m['subject']), '')"); [ -n "$HELD" ] && break; sleep 2; done
    check "virüslü ileti karantinada" "$([ -n "$HELD" ] && echo var)" var
    [ -n "$HELD" ] && api DELETE "/mail/quarantine/$HELD" >/dev/null
  else
    check "virüslü ileti SMTP'de reddedilir" "$(cut -c1-8 <<<"$EICAR_RESULT")" rejected
  fi
  sleep 2
  hasnt "virüslü ileti kutuya ulaşmadı" "$(mf list uyea@$DOM)" "virus-ileti-$T"
  sleep 3
  has "Rspamd geçmişinde CLAM_VIRUS simgesi" "$(curl -s http://127.0.0.1:11334/history | tr -d '\n ' | grep -o 'CLAM_VIRUS' | head -1)" CLAM_VIRUS
  check "durum: yakalanan sayı arttı" "$(api GET /mail/antivirus | jget 'd["caught_7d"] >= 1')" True
else
  echo "  ATLANDI ClamAV bileşeni kurulu değil: tarama, EICAR ve Rspamd bağlantısı denetimleri (kurmak için Bileşenler sayfası)"
  check "kurulu değilken durum: kurulu değil" "$(api GET /mail/antivirus | jget 'd["installed"]')" False
  check "kurulu değilken antivirus.conf yok" "$(test -e /etc/rspamd/local.d/antivirus.conf && echo var || echo yok)" yok
  check "kurulu değilken yeniden yazma reddedilir" "$(code POST /mail/antivirus/apply)" 400
fi

say "Temizlik"
cleanup
check "posta alanı silindi" "$(mail_id "$DOM")" ""
check "liste dosyaları kalmadı" "$(ls /etc/pixpanel/mail/lists 2>/dev/null | grep -c senders)" 0
check "Postfix yapılandırması geçerli" "$(postfix check >/dev/null 2>&1 && echo ok)" ok
check "Dovecot yapılandırması geçerli" "$(doveconf -n >/dev/null 2>&1 && echo ok)" ok
check "/etc/hosts temizlendi" "$(grep -c px-mail-extras /etc/hosts)" 0
rm -f $J $JC
echo "bitti"
