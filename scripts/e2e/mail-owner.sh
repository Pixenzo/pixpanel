#!/usr/bin/env bash
# Kutu sahibinin kendi işleri (parola değiştirme, ManageSieve, panel betiğinin korunması) ve
# karantina sınaması. Test makinesinde root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash mail-owner.sh
# Önkoşul: posta kurulu ve çalışıyor; web postası (Roundcube) kuruluysa onun bölümü de koşar.
# Not: deneme sınırı sayaçları panelin belleğindedir (5 dakika); sınama her koşuda kendine ait
# (zaman damgalı) istemci adresleri ve alan adı kullanır, böylece art arda koşular birbirini etkilemez.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
P=https://127.0.0.1:8443
J=/tmp/pxjar-mowner
JC=/tmp/pxjar-mowner-cust
W=/tmp/pxjar-mowner-rc
HERE="$(cd "$(dirname "$0")" && pwd)"
T=$(date +%s)
DOM="ko$T.test"
CDOM="musk$T.test"
CUSTOMER="kutusahibi"
CPASS='Kutu-Sahibi-Parolasi-1'
export MAIL_PASS='Kutu-Parola-12345'
NEWPASS='Yeni-Sahip-Parola-42'
GTUBE='XJS*C4JDBQADN1.NSBN3*2IDNEN*GTUBE-STANDARD-ANTI-UBE-TEST-EMAIL*C.34X'
MAILHOST=$(hostname -I | awk '{print $1}')
# Bu koşuya özel sahte istemci adresi öneki (203.0.113.0/24 ve 198.51.100.0/24 belge amaçlı ağlardır).
OCT=$((T % 240 + 10))
IPA="100.$((T % 60 + 64)).$((T / 60 % 250))"
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
capi() { local m=$1 p=$2; shift 2; curl -sk -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
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
ms() { python3 "$HERE/managesieve.py" "$@"; }
mail_id() { api GET /mail | jget "next((x['id'] for x in d['domains'] if x['domain']=='$1'), '')"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
# Doğru kutu parolası için sonucu bekler: ileti $2 klasöründe görünene kadar en çok 30 sn.
wait_subject() { local user=$1 folder=$2 subject=$3 list; for _ in $(seq 1 15); do list=$(mf list "$user" "$folder"); grep -qF -- "$subject" <<<"$list" && { echo "$list"; return; }; sleep 2; done; echo "$list"; }
# Karantina listesinde konuyu bekler; kimliği döner (bulunamazsa boş). $1: curl işlevi (api ya da capi).
wait_held() { local fn=$1 subject=$2 id; for _ in $(seq 1 15); do id=$($fn GET /mail/quarantine | jget "next((m['id'] for m in d['messages'] if '$subject' in m['subject']), '')"); [ -n "$id" ] && { echo "$id"; return; }; sleep 2; done; echo ""; }
# Oturumsuz parola değiştirme: pw <adres> <geçerli> <yeni> [istemci adresi]. Gövdeyi yazar; kod ve süre /tmp/pw.meta'ya gider.
KEYFILE=/var/lib/pixpanel/tools/roundcube/api_key
pwcall() {
  local address=$1 current=$2 new=$3 client=${4:-}
  local headers=()
  if [ -n "$client" ] && [ -r "$KEYFILE" ]; then headers=(-H "X-PixPanel-Webmail-Key: $(cat $KEYFILE)" -H "X-PixPanel-Client-Ip: $client"); fi
  python3 - "$address" "$current" "$new" >/tmp/pw.json <<'PYEOF'
import json, sys
print(json.dumps({"address": sys.argv[1], "current_password": sys.argv[2], "new_password": sys.argv[3]}))
PYEOF
  curl -sk -o /tmp/pw.body -w '%{http_code} %{time_total}' -X POST "$B/mail/public/password" -H 'content-type: application/json' "${headers[@]}" -d @/tmp/pw.json >/tmp/pw.meta
}
pwcode() { awk '{print $1}' /tmp/pw.meta; }
pwtime() { awk '{print $2}' /tmp/pw.meta; }
pwbody() { cat /tmp/pw.body; }

cleanup() {
  local id
  for d in "$DOM" "$CDOM"; do id=$(mail_id "$d"); [ -n "$id" ] && api DELETE "/mail/domains/$id?delete_mail=true" >/dev/null; done
  id=$(user_id $CUSTOMER); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
}
purge_held() {
  # Sınamanın bıraktığı karantina iletilerini siler (yalnız bu betiğin konu önekini taşıyanlar).
  local ids
  ids=$(api GET /mail/quarantine | jget "json.dumps([m['id'] for m in d['messages'] if 'ownerq' in m['subject']])" 2>/dev/null || echo '[]')
  case "$ids" in '['*) ;; *) return ;; esac
  [ "$ids" = '[]' ] || api POST /mail/quarantine/bulk-delete -d "{\"ids\":$ids}" >/dev/null
}

rm -f $J $JC $W
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup
ORIG=$(api GET /mail/settings)

say "Hazırlık"
check "temel yapılandırma (yeni Dovecot düzeni + var olan kutuların geçişi)" "$(code POST /mail/setup)" 200
WEBMAIL=$(api GET /mail | jget 'd["webmail"]')
echo "  web postası kurulu: $WEBMAIL"
check "Dovecot yapılandırması geçerli" "$(doveconf -n >/dev/null 2>&1 && echo ok)" ok
has "ManageSieve yalnız 127.0.0.1'de dinliyor" "$(ss -ltnH 'sport = :4190' | awk '{print $4}')" "127.0.0.1:4190"
hasnt "ManageSieve herkese açık değil (0.0.0.0)" "$(ss -ltnH 'sport = :4190' | awk '{print $4}')" "0.0.0.0:4190"
hasnt "ManageSieve herkese açık değil (*)" "$(ss -ltnH 'sport = :4190' | awk '{print $4}')" "*:4190"
check "ManageSieve yerelde STARTTLS bildiriyor" "$(ms - ping)" ok
check "ManageSieve sunucunun ağ adresinden kapalı" "$(MANAGESIEVE_HOST=$MAILHOST ms - ping)" fail
check "Sieve düzeni geçiş işareti yazıldı" "$(test -f /etc/pixpanel/mail/sieve-layout && echo var)" var
has "Dovecot: panel betiği kutu başına before betiği" "$(doveconf -n | tr '\n' ' ')" "sieve-before"

D=$(api POST /mail/domains -d "{\"domain\":\"$DOM\"}" | jget 'd["id"]')
AYSE=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"ayse\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
VELI=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"veli\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
check "yeni kutuda panel betik klasörü hazır" "$(test -d /var/vmail/$DOM/ayse/sieve-before && test -d /var/vmail/$DOM/ayse/sieve && echo var)" var
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CPASS\",\"role\":\"user\"}")" 200
UC=$(user_id $CUSTOMER)
CD=$(api POST /mail/domains -d "{\"domain\":\"$CDOM\",\"user_id\":$UC}" | jget 'd["id"]')
MUS=$(api POST /mail/domains/$CD/mailboxes -d "{\"local\":\"musteri\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
check "müşteri girişi" "$(ccode POST /auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CPASS\"}")" 200

say "Parola değiştirme (oturumsuz uç)"
check "yanlış parola" "$(pwcall veli@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.1"; pwcode)" 400
WRONG_BODY=$(pwbody); WRONG_TIME=$(pwtime)
check "var olmayan adres" "$(pwcall yok@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.2"; pwcode)" 400
MISSING_BODY=$(pwbody); MISSING_TIME=$(pwtime)
check "yanlış parola ve var olmayan adres AYNI yanıtı verir" "$MISSING_BODY" "$WRONG_BODY"
check "yanıt süreleri benzer (adres sızmaz)" "$(awk -v a="$WRONG_TIME" -v b="$MISSING_TIME" 'BEGIN{d=a-b; if(d<0)d=-d; print (d<0.5)?"benzer":"farkli: " a " / " b}')" benzer
has "ortak hata iletisi" "$WRONG_BODY" "Adres ya da geçerli parola hatalı"
check "kutu kapatıldı" "$(code PATCH /mail/mailboxes/$VELI -d '{"enabled":false}')" 200
check "kapalı kutu da aynı yanıtı verir" "$(pwcall veli@$DOM "$MAIL_PASS" "$NEWPASS" "$IPA.3"; pwcode)" 400
check "kapalı kutunun yanıtı aynı" "$(pwbody)" "$WRONG_BODY"
check "kutu yeniden açıldı" "$(code PATCH /mail/mailboxes/$VELI -d '{"enabled":true}')" 200
check "geçersiz adres biçimi" "$(pwcall 'veli@ornekalan.test;rm' "$MAIL_PASS" "$NEWPASS" "$IPA.4"; pwcode)" 400
check "satır sonlu adres" "$(pwcall $'veli@ornekalan.test\nx@y.z' "$MAIL_PASS" "$NEWPASS" "$IPA.4"; pwcode)" 400
check "boş geçerli parola" "$(pwcall veli@$DOM '' "$NEWPASS" "$IPA.4"; pwcode)" 400
check "zayıf yeni parola reddedilir" "$(pwcall veli@$DOM "$MAIL_PASS" kisa "$IPA.5"; pwcode)" 400
has "zayıf parola iletisi" "$(pwbody)" "en az 10 karakter"
check "yeni parola eskisiyle aynı olamaz" "$(pwcall veli@$DOM "$MAIL_PASS" "$MAIL_PASS" "$IPA.5"; pwcode)" 400
check "zayıf denemeden sonra eski parola hâlâ geçerli" "$(mf login veli@$DOM)" ok
check "büyük gövde reddedilir (413)" "$(python3 -c "print('{\"address\":\"veli@$DOM\",\"current_password\":\"' + 'a'*9000 + '\",\"new_password\":\"x\"}')" | curl -sk -o /dev/null -w '%{http_code}' -X POST $B/mail/public/password -H 'content-type: application/json' --data-binary @-)" 413
check "çok uzun parola" "$(pwcall veli@$DOM "$(python3 -c "print('a'*300)")" "$NEWPASS" "$IPA.5"; pwcode)" 400
check "gövdesiz istek" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST $B/mail/public/password -H 'content-type: application/json' -d '{}' | awk '{print ($1>=400 && $1<500)?"4xx":$1}')" 4xx
check "GET ile erişilemez" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/public/password)" 405
check "doğru parolayla değişir" "$(pwcall veli@$DOM "$MAIL_PASS" "$NEWPASS" "$IPA.6"; pwcode)" 200
check "eski parolayla IMAP girişi kesildi" "$(mf login veli@$DOM)" fail
check "yeni parolayla IMAP girişi" "$(MAIL_PASS=$NEWPASS mf login veli@$DOM)" ok
has "denetim günlüğü" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT username || ' ' || action || ' ' || target FROM audit_log WHERE action='mail.mailbox.self_password' ORDER BY id DESC LIMIT 1")" "veli@$DOM mail.mailbox.self_password veli@$DOM"
hasnt "parola özeti ya da parolası denetim kaydında yok" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT detail FROM audit_log WHERE action='mail.mailbox.self_password' ORDER BY id DESC LIMIT 1")" "$NEWPASS"
check "Dovecot kullanıcı dosyasında kutunun tek satırı var" "$(grep -c "^veli@$DOM:" /etc/pixpanel/mail/dovecot-users)" 1
# Parolayı panelden eski değerine döndür (sonraki bölümler MAIL_PASS ile girer).
check "panelden parola geri alındı" "$(code PATCH /mail/mailboxes/$VELI -d "{\"password\":\"$MAIL_PASS\"}")" 200

say "Deneme sınırı (kaba kuvvet koruması)"
if [ -r "$KEYFILE" ]; then
  # Adres başına: 8 farklı istemciden 8 yanlış deneme, 9. istek (yeni bir istemciden) 429.
  for i in $(seq 1 8); do pwcall ayse@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.$((100 + i))"; done
  check "8 yanlış denemeden sonra adres kilitlenir (429)" "$(pwcall ayse@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.120"; pwcode)" 429
  check "kilitliyken doğru parola da bekletilir" "$(pwcall ayse@$DOM "$MAIL_PASS" "$NEWPASS" "$IPA.121"; pwcode)" 429
  check "kilit yalnız o adresi etkiler" "$(pwcall veli@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.122"; pwcode)" 400
  check "kilitli adreste IMAP girişi etkilenmez" "$(mf login ayse@$DOM)" ok
  # İstemci başına: aynı istemciden 8 farklı (var olmayan) adrese deneme, 9. istek başka adrese 429.
  for i in $(seq 1 8); do pwcall "nobody$i@$DOM" yanlis-parola-123 "$NEWPASS" "$IPA.200"; done
  check "istemci 8 yanlış denemeden sonra kilitlenir (429)" "$(pwcall nobody9@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.200"; pwcode)" 429
  check "başka istemci etkilenmez" "$(pwcall nobody9@$DOM yanlis-parola-123 "$NEWPASS" "$IPA.201"; pwcode)" 400
else
  echo "  ATLANDI web postası kurulu değil: paylaşılan anahtar olmadan istemci adresi bildirilemez"
fi
# Başarısız denemeler fail2ban'in panel hapsinin okuduğu biçimde günlüğe düşer.
sleep 1
LOGLINE=$(journalctl -u pixpanel.service --since "-5min" -o cat --no-pager | grep "başarısız posta kutusu parola denemesi" | grep "username=veli@$DOM" | tail -1)
has "günlük satırı (adres ve kullanıcı alanı)" "$LOGLINE" "ip="
has "günlük satırı panelin giriş hedefiyle aynı" "$LOGLINE" "pixpanel::api::auth"
if command -v fail2ban-regex >/dev/null 2>&1 && [ -f /etc/fail2ban/filter.d/pixpanel.conf ]; then
  has "fail2ban süzgeci satırı tanıyor" "$(fail2ban-regex "$LOGLINE" /etc/fail2ban/filter.d/pixpanel.conf 2>&1 | grep 'Lines:')" "1 matched"
else
  echo "  ATLANDI fail2ban süzgeci denetimi (fail2ban-regex yok)"
fi
# Anahtarsız istek: bildirilen istemci adresi yok sayılır, günlükte gerçek bağlantı adresi görünür.
curl -sk -o /dev/null -X POST "$B/mail/public/password" -H 'content-type: application/json' -H "X-PixPanel-Client-Ip: 192.0.2.77" -H 'X-PixPanel-Webmail-Key: yanlis-anahtar' \
  -d "{\"address\":\"veli@$DOM\",\"current_password\":\"yanlis-parola-123\",\"new_password\":\"$NEWPASS\"}"
sleep 1
SPOOF=$(journalctl -u pixpanel.service --since "-1min" -o cat --no-pager | grep "başarısız posta kutusu parola denemesi" | grep "ip=127.0.0.1" | tail -1)
has "yanlış anahtarla bildirilen istemci adresi kabul edilmez" "$SPOOF" "ip=127.0.0.1"
hasnt "bildirilen sahte adres günlüğe girmedi" "$(journalctl -u pixpanel.service --since '-1min' -o cat --no-pager | grep 'başarısız posta kutusu')" "192.0.2.77"

say "Roundcube içinden parola değiştirme"
if [ "$WEBMAIL" = "True" ]; then
  has "parola eklentisi etkin" "$(cat /etc/roundcube/config.inc.php)" "'password'"
  has "managesieve eklentisi etkin" "$(cat /etc/roundcube/config.inc.php)" "'managesieve'"
  check "sürücü dosyası kurulu" "$(test -f /usr/share/roundcube/plugins/password/drivers/pixpanel.php && echo var)" var
  check "yapılandırma dosyası yalnız web grubuna okunur" "$(stat -c '%U %a' /etc/roundcube/config.inc.php)" "root 640"
  RCPASS='Roundcube-Parola-77'
  PAGE=$(curl -sk -c $W "$P/webmail/"); TOKEN=$(grep -o 'name="_token" value="[^"]*"' <<<"$PAGE" | cut -d'"' -f4)
  LOGIN=$(curl -sk -b $W -c $W -o /dev/null -w '%{http_code} %{redirect_url}' "$P/webmail/?_task=login" --data-urlencode "_token=$TOKEN" -d '_task=login&_action=login&_timezone=Europe/Istanbul' --data-urlencode "_user=veli@$DOM" --data-urlencode "_pass=$MAIL_PASS")
  has "web postasına giriş" "$LOGIN" "_task=mail"
  FORM=$(curl -sk -b $W -c $W "$P/webmail/?_task=settings&_action=plugin.password")
  has "parola formu açıldı" "$FORM" "_newpasswd"
  RCTOKEN=$(grep -o 'name="_token" value="[^"]*"' <<<"$FORM" | head -1 | cut -d'"' -f4)
  curl -sk -b $W -c $W -o /tmp/rc-weak.html "$P/webmail/?_task=settings&_action=plugin.password-save" --data-urlencode "_token=$RCTOKEN" \
    --data-urlencode "_curpasswd=$MAIL_PASS" --data-urlencode '_newpasswd=kisa' --data-urlencode '_confpasswd=kisa'
  check "zayıf parola web postasından da değişmez" "$(mf login veli@$DOM)" ok
  curl -sk -b $W -c $W -o /tmp/rc-save.html "$P/webmail/?_task=settings&_action=plugin.password-save" --data-urlencode "_token=$RCTOKEN" \
    --data-urlencode "_curpasswd=$MAIL_PASS" --data-urlencode "_newpasswd=$RCPASS" --data-urlencode "_confpasswd=$RCPASS"
  check "web postasından parola değişti (eski parola kesildi)" "$(mf login veli@$DOM)" fail
  check "web postasından parola değişti (yeni parola geçerli)" "$(MAIL_PASS=$RCPASS mf login veli@$DOM)" ok
  check "panelden parola geri alındı" "$(code PATCH /mail/mailboxes/$VELI -d "{\"password\":\"$MAIL_PASS\"}")" 200
  SIEVEPAGE=$(curl -sk -b $W -c $W "$P/webmail/?_task=settings&_action=plugin.managesieve")
  has "filtreler sayfası açılıyor" "$SIEVEPAGE" "managesieve"
else
  echo "  ATLANDI web postası kurulu değil"
fi

say "Otomatik yanıt ve filtreler: panel betiği ile kutu sahibinin betiği"
FILTERS='{"filters":[{"name":"Faturalar","enabled":true,"match_all":true,"stop":true,"conditions":[{"field":"subject","header":"","op":"contains","value":"fatura"}],"actions":[{"kind":"move","value":"Faturalar"}]}],"auto_reply":{"enabled":true,"subject":"Tatildeyim","body":"Yakinda donerim.","start_date":"","end_date":""}}'
check "panel kuralları (filtre + otomatik yanıt)" "$(code PATCH /mail/mailboxes/$AYSE -d "$FILTERS")" 200
check "panel betiği before klasöründe" "$(test -f /var/vmail/$DOM/ayse/sieve-before/pixpanel.sieve && echo var)" var
check "panel betiği kutu sahibinin klasöründe değil" "$(ls /var/vmail/$DOM/ayse/sieve | grep -c pixpanel)" 0
check "eski düzenin etkin betiği yok" "$(ls -a /var/vmail/$DOM/ayse | grep -c '^.dovecot.sieve$')" 0
check "kutu sahibi betik listesinde panel betiğini görmez" "$(ms ayse@$DOM list | grep -c pixpanel)" 0
check "panel betiğini okuyamaz" "$(ms ayse@$DOM get pixpanel | cut -c1-3)" "no:"
check "panel betiğini silemez" "$(ms ayse@$DOM delete pixpanel | cut -c1-3)" "no:"
cat > /tmp/px-own.sieve <<'SEOF'
require ["fileinto", "mailbox"];
if header :contains "subject" "kisisel" {
    fileinto :create "Kisisel";
}
SEOF
check "kutu sahibi kendi betiğini yükler" "$(ms ayse@$DOM put benim /tmp/px-own.sieve)" ok
check "betiği etkinleştirir" "$(ms ayse@$DOM activate benim)" ok
has "betik listesi" "$(ms ayse@$DOM list)" "benim ACTIVE"
echo 'if { bozuk' | ms ayse@$DOM put kotu - >/dev/null 2>&1
check "kötü sözdizimli betik kaydedilmez" "$(ms ayse@$DOM list | grep -c '^kotu')" 0
# Kullanıcı aynı adlı kendi betiğiyle panel betiğini ezmeye çalışır: bu yalnız kendi klasörüne yazar.
cat > /tmp/px-evil.sieve <<'SEOF'
discard;
stop;
SEOF
check "pixpanel adlı kendi betiğini yükleyebilir (kendi klasörü)" "$(ms ayse@$DOM put pixpanel /tmp/px-evil.sieve)" ok
check "onu etkinleştirir" "$(ms ayse@$DOM activate pixpanel)" ok
check "panelin betiği hâlâ yerinde" "$(grep -c 'Tatildeyim' /var/vmail/$DOM/ayse/sieve-before/pixpanel.sieve)" 1
mf send veli@$DOM veli@$DOM ayse@$DOM "fatura-ownerq-$T" >/dev/null
mf send veli@$DOM veli@$DOM ayse@$DOM "soru-ownerq-$T" >/dev/null
has "panel filtresi çalışıyor (fatura klasöre taşındı)" "$(wait_subject ayse@$DOM Faturalar "fatura-ownerq-$T")" "fatura-ownerq-$T"
has "otomatik yanıt çalışıyor (gönderene döndü)" "$(wait_subject veli@$DOM INBOX Tatildeyim)" "Tatildeyim"
hasnt "kutu sahibinin discard betiği panelin filtresini ezemedi" "$(mf list ayse@$DOM)" "fatura-ownerq-$T"
check "kutu sahibi kendi betiğini geri alır" "$(ms ayse@$DOM activate benim)" ok
mf send veli@$DOM veli@$DOM ayse@$DOM "kisisel-ownerq-$T" >/dev/null
has "kutu sahibinin kendi filtresi de çalışıyor" "$(wait_subject ayse@$DOM Kisisel "kisisel-ownerq-$T")" "kisisel-ownerq-$T"
check "kutu sahibinin etkin betiği panel güncellemesinde bozulmadı" "$(code PATCH /mail/mailboxes/$AYSE -d "$FILTERS")" 200
has "etkin betik hâlâ kutu sahibinin" "$(ms ayse@$DOM list)" "benim ACTIVE"
check "kuralları kaldırınca yalnız panel betiği silinir" "$(code PATCH /mail/mailboxes/$AYSE -d '{"filters":[],"auto_reply":{"enabled":false,"subject":"","body":"","start_date":"","end_date":""}}')" 200
check "panel betiği silindi" "$(test -e /var/vmail/$DOM/ayse/sieve-before/pixpanel.sieve && echo var || echo yok)" yok
has "kutu sahibinin betikleri durur" "$(ms ayse@$DOM list)" "benim ACTIVE"
check "panel kurallarını geri yükle" "$(code PATCH /mail/mailboxes/$AYSE -d "$FILTERS")" 200

say "Var olan kutuların yeni düzene geçişi"
# Eski düzeni taklit et: panel betiği kutu sahibinin klasöründe, .dovecot.sieve ona işaret ediyor.
check "veli için otomatik yanıt" "$(code PATCH /mail/mailboxes/$VELI -d '{"auto_reply":{"enabled":true,"subject":"Eski duzen","body":"gecis","start_date":"","end_date":""}}')" 200
runuser -u vmail -- sh -c "cp /var/vmail/$DOM/veli/sieve-before/pixpanel.sieve /var/vmail/$DOM/veli/sieve/pixpanel.sieve && rm -f /var/vmail/$DOM/veli/sieve-before/pixpanel.sieve && ln -sfn sieve/pixpanel.sieve /var/vmail/$DOM/veli/.dovecot.sieve"
rm -f /etc/pixpanel/mail/sieve-layout
check "geçiş: temel yapılandırma yeniden uygulanır" "$(code POST /mail/setup)" 200
check "geçiş: eski panel betiği kutu sahibinin klasöründen kalktı" "$(test -e /var/vmail/$DOM/veli/sieve/pixpanel.sieve && echo var || echo yok)" yok
check "geçiş: eski etkin bağ kalktı" "$(test -L /var/vmail/$DOM/veli/.dovecot.sieve && echo var || echo yok)" yok
check "geçiş: panel betiği yeni yerde" "$(grep -c 'Eski duzen' /var/vmail/$DOM/veli/sieve-before/pixpanel.sieve)" 1
check "geçiş: kutu sahibinin etkin betiği korundu" "$(basename "$(readlink -f /var/vmail/$DOM/ayse/.dovecot.sieve)")" "benim.sieve"
check "geçiş işareti yeniden yazıldı" "$(test -f /etc/pixpanel/mail/sieve-layout && echo var)" var
mf send ayse@$DOM ayse@$DOM veli@$DOM "gecis-ownerq-$T" >/dev/null
has "geçişten sonra otomatik yanıt çalışıyor" "$(wait_subject ayse@$DOM INBOX 'Eski duzen')" "Eski duzen"

say "Karantina: ayar"
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/quarantine)" 401
check "geçersiz işlem değeri" "$(code PUT /mail/settings -d "$(printf '%s' "$ORIG" | python3 -c 'import sys,json; d=json.load(sys.stdin); d["spam"]["reject_action"]="sil"; print(json.dumps(d))')")" 400
# Rspamd, GTUBE sınama dizgisini puana bakmadan doğrudan reddeder ve karantina ayarını uygulamaz;
# bu yüzden sınama puanla reddedilen gerçek duruma dayanır: gönderen kara listeye alınır.
BSET=$(printf '%s' "$ORIG" | python3 -c 'import sys,json; d=json.load(sys.stdin); d["spam"]["blacklist"]=d["spam"]["blacklist"]+["biri@disarisi.example"]; print(json.dumps(d))')
QSET=$(printf '%s' "$BSET" | python3 -c 'import sys,json; d=json.load(sys.stdin); d["spam"]["reject_action"]="quarantine"; print(json.dumps(d))')
check "gönderen kara listeye alındı" "$(code PUT /mail/settings -d "$BSET")" 200
sleep 3
check "varsayılan kip reddetme" "$(printf '%s' "$ORIG" | jget 'd["spam"]["reject_action"]')" reject
check "kara listedeki gönderen kip kapalıyken SMTP'de reddedilir" "$(MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example ayse@$DOM "gtube-reddet-ownerq-$T" 25 | cut -c1-8)" rejected
check "karantina kipini aç" "$(code PUT /mail/settings -d "$QSET")" 200
check "Rspamd ayarı yazıldı" "$(grep -c 'quarantine_on_reject = true' /etc/rspamd/local.d/worker-proxy.inc)" 1
check "Rspamd yapılandırması geçerli" "$(rspamadm configtest >/dev/null 2>&1 && echo ok)" ok
check "ayar kaydı okunuyor" "$(api GET /mail/settings | jget 'd["spam"]["reject_action"]')" quarantine
sleep 3

say "Karantina: yönetici"
SUBJ="gtube-ownerq-$T"
check "ileti kabul edilir (reddedilmez)" "$(MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example ayse@$DOM "$SUBJ" 25)" sent
HID=$(wait_held api "$SUBJ")
check "ileti karantina listesinde" "$([ -n "$HID" ] && echo var)" var
check "kimlik biçimi" "$(grep -cE '^[0-9A-Za-z]{6,20}$' <<<"$HID")" 1
sleep 3
hasnt "ileti alıcıya teslim edilmedi (gelen kutusu)" "$(mf list ayse@$DOM)" "$SUBJ"
hasnt "ileti Junk'a da düşmedi" "$(mf list ayse@$DOM Junk)" "$SUBJ"
api GET /mail/quarantine | jget '"mod:", d["mode"], "saklama (gün):", d["retention_days"], "puan:", [m["score"] for m in d["messages"] if m["id"]=="'$HID'"]'
check "kip alanı" "$(api GET /mail/quarantine | jget 'd["mode"]')" quarantine
check "liste alanları" "$(api GET /mail/quarantine | jget "sorted(k for k in next(m for m in d['messages'] if m['id']=='$HID'))" | tr -d "[]' ")" "arrived_at,id,recipients,score,sender,size,subject"
check "gönderen ve alıcı" "$(api GET /mail/quarantine | jget "(lambda m: (m['sender'], m['recipients']))(next(m for m in d['messages'] if m['id']=='$HID'))")" "('biri@disarisi.example', ['ayse@$DOM'])"
VIEW=$(api GET /mail/quarantine/$HID)
has "ileti görüntülenir (başlıklar)" "$(printf '%s' "$VIEW" | jget 'd["raw"]')" "Subject: $SUBJ"
has "ileti görüntülenir (gövde)" "$(printf '%s' "$VIEW" | jget 'd["raw"]')" "karantina-govdesi"
check "küçük ileti kesilmedi" "$(printf '%s' "$VIEW" | jget 'd["truncated"]')" False
check "oturumsuz görüntüleme" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/quarantine/$HID)" 401

say "Karantina: geçersiz kimlikler"
for bad in ab 12345 'ALL;ls' '..%2F..%2Fetc' 'ABC%0Adef' 'A1B2C3D4E5F6G7H8I9J0K'; do
  check "GET /quarantine/$bad" "$(code GET "/mail/quarantine/$bad")" 400
done
check "DELETE geçersiz kimlik" "$(code DELETE /mail/quarantine/ALL)" 400
check "bırak geçersiz kimlik" "$(code POST /mail/quarantine/ab/release)" 400
check "biçimi geçerli ama olmayan kimlik (görüntüle)" "$(code GET /mail/quarantine/Zz9Zz9Zz9)" 404
check "biçimi geçerli ama olmayan kimlik (sil)" "$(code DELETE /mail/quarantine/Zz9Zz9Zz9)" 404
check "toplu silme: geçersiz kimlik tümünü reddettirir" "$(code POST /mail/quarantine/bulk-delete -d "{\"ids\":[\"$HID\",\"ab;ls\"]}")" 400
check "toplu silme: boş liste" "$(code POST /mail/quarantine/bulk-delete -d '{"ids":[]}')" 400
check "geçersiz kimlikte ileti silinmedi" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$HID'])")" 1
check "özel değerli ALL toplu silme kimliği" "$(code POST /mail/quarantine/bulk-delete -d '{"ids":["ALL"]}')" 400

say "Karantina: başka hesap"
check "müşteri listesi yalnız kendi alan adlarını gösterir (yöneticinin iletisi yok)" "$(capi GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$HID'])")" 0
check "müşteri başkasının iletisini göremez (404)" "$(ccode GET /mail/quarantine/$HID)" 404
check "müşteri başkasının iletisini bırakamaz (404)" "$(ccode POST /mail/quarantine/$HID/release)" 404
check "müşteri başkasının iletisini silemez (404)" "$(ccode DELETE /mail/quarantine/$HID)" 404
check "müşteri toplu silmede başkasının iletisini atlar" "$(capi POST /mail/quarantine/bulk-delete -d "{\"ids\":[\"$HID\"]}" | jget '(d["deleted"], d["skipped"])')" "(0, 1)"
check "ileti müşteriye dokunulmadan duruyor" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$HID'])")" 1
CSUBJ="musteri-ownerq-$T"
check "müşteri alan adına reddedilecek ileti" "$(MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example musteri@$CDOM "$CSUBJ" 25)" sent
CHID=$(wait_held capi "$CSUBJ")
check "müşteri kendi iletisini listede görür" "$([ -n "$CHID" ] && echo var)" var
has "müşteri kendi iletisinin içeriğini görür" "$(capi GET /mail/quarantine/$CHID | jget 'd["raw"]')" "Subject: $CSUBJ"
check "yönetici de görür" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$CHID'])")" 1
MSUBJ="karma-ownerq-$T"
check "iki hesabın alıcısı olan ileti" "$(MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example "musteri@$CDOM, ayse@$DOM" "$MSUBJ" 25)" sent
MHID=$(wait_held api "$MSUBJ")
check "karma alıcılı ileti yöneticide listelenir" "$([ -n "$MHID" ] && echo var)" var
check "karma alıcılı ileti müşteriye gösterilmez (liste)" "$(capi GET /mail/quarantine | jget "len([m for m in d['messages'] if '$MSUBJ' in m['subject']])")" 0
check "karma alıcılı ileti müşteriye gösterilmez (içerik 404)" "$(ccode GET /mail/quarantine/$MHID)" 404
check "karma alıcılı ileti müşteri tarafından bırakılamaz" "$(ccode POST /mail/quarantine/$MHID/release)" 404
check "karma alıcılı ileti müşteri tarafından silinemez" "$(ccode DELETE /mail/quarantine/$MHID)" 404
check "müşteri kendi iletisini bırakır" "$(ccode POST /mail/quarantine/$CHID/release)" 200
has "bırakılan ileti kutuya düştü" "$(wait_subject musteri@$CDOM INBOX "$CSUBJ")" "$CSUBJ"
check "bırakılan ileti karantinadan kalktı" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$CHID'])")" 0

say "Karantina: bırakma, silme, toplu silme (yönetici)"
check "yönetici iletiyi bırakır" "$(code POST /mail/quarantine/$HID/release)" 200
has "bırakılan ileti gelen kutusuna düştü" "$(wait_subject ayse@$DOM INBOX "$SUBJ")" "$SUBJ"
check "bırakılan ileti listeden kalktı" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$HID'])")" 0
check "ikinci kez bırakma 404" "$(code POST /mail/quarantine/$HID/release)" 404
DSUBJ="sil-ownerq-$T"
MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example ayse@$DOM "$DSUBJ" 25 >/dev/null
DHID=$(wait_held api "$DSUBJ")
check "silinecek ileti listede" "$([ -n "$DHID" ] && echo var)" var
check "yönetici iletiyi siler" "$(code DELETE /mail/quarantine/$DHID)" 200
check "silinen ileti listeden kalktı" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id']=='$DHID'])")" 0
check "silinen ileti Postfix kuyruğunda da yok" "$(postqueue -j | grep -cE "\"queue_id\": *\"$DHID\"")" 0
sleep 2
hasnt "silinen ileti teslim edilmedi" "$(mf list ayse@$DOM)" "$DSUBJ"
for n in 1 2; do MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example ayse@$DOM "toplu$n-ownerq-$T" 25 >/dev/null; done
T1=$(wait_held api "toplu1-ownerq-$T"); T2=$(wait_held api "toplu2-ownerq-$T")
check "toplu silme: iki ileti listede" "$([ -n "$T1" ] && [ -n "$T2" ] && echo var)" var
check "toplu silme (artı bir olmayan kimlik)" "$(api POST /mail/quarantine/bulk-delete -d "{\"ids\":[\"$T1\",\"$T2\",\"Zz9Zz9Zz9\"]}" | jget '(d["deleted"], d["skipped"])')" "(2, 1)"
check "toplu silinenler kalktı" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if m['id'] in ('$T1','$T2')])")" 0
check "denetim: bırakma" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='mail.quarantine.release' AND target='$HID'")" 1
check "denetim: silme" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='mail.quarantine.delete' AND target='$DHID'")" 1
check "denetim: toplu silme" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='mail.quarantine.bulk_delete'")" 1
check "denetim: ayar değişikliği" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='mail.settings' AND detail LIKE '%quarantine%'")" 1

say "Karantina: kip kapatılınca yeniden reddedilir"
purge_held
check "karantina kipini kapat" "$(code PUT /mail/settings -d "$BSET")" 200
check "Rspamd bloğu kalktı" "$(grep -qs 'quarantine_on_reject' /etc/rspamd/local.d/worker-proxy.inc && echo var || echo yok)" yok
check "Rspamd yapılandırması geçerli" "$(rspamadm configtest >/dev/null 2>&1 && echo ok)" ok
sleep 3
check "kip kapanınca yeniden SMTP'de reddedilir" "$(MAIL_BODY=karantina-govdesi MAIL_HOST=$MAILHOST mf send - biri@disarisi.example ayse@$DOM "gtube-son-ownerq-$T" 25 | cut -c1-8)" rejected
check "kip kapalıyken liste boş (bu betiğin iletileri)" "$(api GET /mail/quarantine | jget "len([m for m in d['messages'] if 'ownerq' in m['subject']])")" 0
check "ayarlar eski hâline döndü" "$(api GET /mail/settings | jget 'd["spam"]["reject_action"]')" reject
check "kara liste eski hâline döndü" "$(code PUT /mail/settings -d "$ORIG")" 200

say "Temizlik"
purge_held
cleanup
check "ayarlar özgün hâlde" "$(api GET /mail/settings | jget 'd["spam"]["reject_action"]')" reject
rm -f /tmp/px-own.sieve /tmp/px-evil.sieve /tmp/pw.json /tmp/pw.body /tmp/pw.meta /tmp/rc-weak.html /tmp/rc-save.html $J $JC $W
echo "bitti"
