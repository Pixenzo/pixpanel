#!/usr/bin/env bash
# DKIM imza yetkisi: aynı sunucudaki bir hesap, başka hesabın alan adı adına imzalı posta
# gönderemez. İmza yalnızca alan adının sahibi hesaptan (sendmail), o alan adının kutusuyla oturum
# açan gönderenden (SMTP) ya da kök kullanıcıdan gelen postaya atılır.
# Panelin kurulu olduğu makinede çalışır. Kullanım: PX_USER=... PX_PASS=... bash dkim.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-dkim
HERE="$(cd "$(dirname "$0")" && pwd)"
A=imzabir
O=imzaiki
PASS='Imza-Sinama-Parolasi-1'
export MAIL_PASS='Kutu-Parola-12345'
DOMAIN=imza-posta.test
BOX=/var/vmail/$DOMAIN/kutu/Maildir
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 200)]" ;; esac; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
mail_id() { api GET /mail | jget "next((x['id'] for x in d['domains'] if x['domain']=='$1'), '')"; }
# send_as <kullanıcı> <konu> [ek başlık satırı]: kullanıcının kimliğiyle sendmail çalıştırır.
send_as() {
  local who=$1 subject=$2 extra=${3:-}
  { [ -n "$extra" ] && printf '%s\n' "$extra"; printf 'From: kutu@%s\nTo: kutu@%s\nSubject: %s\n\nmerhaba\n' "$DOMAIN" "$DOMAIN" "$subject"; } |
    runuser -u "$who" -- /usr/sbin/sendmail -f "kutu@$DOMAIN" "kutu@$DOMAIN"
}
# delivered <konu>: iletinin teslim edilen dosyası (bulunana kadar bekler).
delivered() {
  local file=
  for _ in $(seq 1 40); do
    file=$(grep -rlF -- "Subject: $1" $BOX/new $BOX/cur 2>/dev/null | head -1)
    [ -n "$file" ] && { echo "$file"; return; }
    sleep 0.5
  done
}
# verdict <konu>: "imzali", "imzasiz" ya da "teslim-edilmedi"; iç başlık alıcıya ulaştıysa "+ic-baslik".
verdict() {
  local file; file=$(delivered "$1")
  [ -z "$file" ] && { echo teslim-edilmedi; return; }
  local result=imzasiz
  grep -qi "^DKIM-Signature:.* d=$DOMAIN" "$file" && result=imzali
  grep -qi '^X-PixPanel-Uid:' "$file" && result="$result+ic-baslik"
  echo "$result"
}

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
id=$(mail_id $DOMAIN); [ -n "$id" ] && api DELETE "/mail/domains/$id?delete_mail=true" >/dev/null
for name in imza-bir.test imza-iki.test; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
for name in $A $O; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done
T=$(date +%s)

say "Hazırlık"
check "temel yapılandırma" "$(code POST /mail/setup)" 200
check "iki hesap" "$(code POST /users -d "{\"username\":\"$A\",\"password\":\"$PASS\",\"role\":\"user\"}") $(code POST /users -d "{\"username\":\"$O\",\"password\":\"$PASS\",\"role\":\"user\"}")" "200 200"
UA=$(user_id $A); UO=$(user_id $O)
# Linux kullanıcıları ilk siteyle oluşur.
check "hesapların siteleri" "$(code POST /sites -d "{\"domain\":\"imza-bir.test\",\"kind\":\"static\",\"user_id\":$UA}") $(code POST /sites -d "{\"domain\":\"imza-iki.test\",\"kind\":\"static\",\"user_id\":$UO}")" "200 200"
D=$(api POST /mail/domains -d "{\"domain\":\"$DOMAIN\",\"user_id\":$UA}" | jget 'd["id"]')
check "kutu" "$(code POST /mail/domains/$D/mailboxes -d "{\"local\":\"kutu\",\"password\":\"$MAIL_PASS\"}")" 200
MAP=/etc/pixpanel/mail/rspamd/dkim_owners.map
check "eşlemede yalnız sahibin numarası var" "$(grep -F ":$DOMAIN" $MAP)" "$(id -u $A):$DOMAIN"
has "Postfix yerel gönderimi ayrı servisten geçirir" "$(postconf -P 'pickup/unix/cleanup_service_name' 2>/dev/null)" "pixcleanup"
has "asıl servis iç başlığı siler" "$(cat /etc/postfix/pixpanel-header-checks)" "/^X-PixPanel-Uid:/ IGNORE"
sleep 2

say "Sunucunun içinden gönderim (sendmail)"
send_as $A "sahip-$T"
check "alan adının sahibi imza alır" "$(verdict "sahip-$T")" imzali
send_as $O "komsu-$T"
check "başka hesap imza alamaz" "$(verdict "komsu-$T")" imzasiz
send_as $O "sahte-kok-$T" "X-PixPanel-Uid: 0"
check "sahte iç başlık (kök) işe yaramaz" "$(verdict "sahte-kok-$T")" imzasiz
send_as $O "sahte-sahip-$T" "X-PixPanel-Uid: $(id -u $A)"
check "sahte iç başlık (sahibin numarası) işe yaramaz" "$(verdict "sahte-sahip-$T")" imzasiz
send_as $O "sahte-alindi-$T" "Received: by sunucu.test (Postfix, from userid $(id -u $A))"
check "sahte alındı başlığı işe yaramaz" "$(verdict "sahte-alindi-$T")" imzasiz
printf 'From: kutu@%s\nTo: kutu@%s\nSubject: kok-%s\n\nmerhaba\n' $DOMAIN $DOMAIN $T | /usr/sbin/sendmail -f "kutu@$DOMAIN" "kutu@$DOMAIN"
check "kök kullanıcı imza alır" "$(verdict "kok-$T")" imzali

say "SMTP ile gönderim"
check "kutu sahibi oturum açıp gönderir" "$(python3 "$HERE/mailflow.py" send "kutu@$DOMAIN" "kutu@$DOMAIN" "kutu@$DOMAIN" "oturum-$T")" sent
check "oturum açan gönderen imza alır" "$(verdict "oturum-$T")" imzali
# Yerel bağlantı (ör. bir sitenin PHP'si 127.0.0.1:25'e bağlanır) kimliksizdir; sahte başlıkla da imza alamaz.
python3 - "$DOMAIN" "$T" "$(id -u $A)" <<'XEOF'
import smtplib, sys
domain, stamp, uid = sys.argv[1:4]
for subject, extra in (("yerel-smtp-" + stamp, ""), ("yerel-smtp-sahte-" + stamp, "X-PixPanel-Uid: 0\r\nReceived: by sunucu.test (Postfix, from userid %s)\r\n" % uid)):
    body = "%sFrom: kutu@%s\r\nTo: kutu@%s\r\nSubject: %s\r\n\r\nmerhaba\r\n" % (extra, domain, domain, subject)
    with smtplib.SMTP("127.0.0.1", 25, timeout=20) as smtp:
        smtp.sendmail("kutu@" + domain, ["kutu@" + domain], body)
XEOF
check "kimliksiz yerel bağlantı imza alamaz" "$(verdict "yerel-smtp-$T")" imzasiz
check "kimliksiz yerel bağlantı sahte başlıkla da imza alamaz" "$(verdict "yerel-smtp-sahte-$T")" imzasiz

say "Sahiplik değişince"
check "posta alanı silinince eşlemeden çıkar" "$(api DELETE "/mail/domains/$D?delete_mail=true" >/dev/null; grep -cF ":$DOMAIN" $MAP)" 0

say "Temizlik"
for name in imza-bir.test imza-iki.test; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
check "hesapları sil" "$(code DELETE /users/$UA) $(code DELETE /users/$UO)" "200 200"
rm -f $J
echo "bitti"
