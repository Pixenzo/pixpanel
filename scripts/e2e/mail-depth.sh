#!/usr/bin/env bash
# Posta derinliği sınaması: (A) giden posta sınırı (Postfix policy servisi) ve (B) istemci otomatik
# ayarı (autoconfig, autodiscover, mobileconfig). Test makinesinde root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash mail-depth.sh
# Önkoşul: posta kurulu ve çalışıyor, nginx kurulu; `*.test` adları 127.0.0.1'e çözülür.
# Not: sayaçlar panelin belleğindedir; yinelenen koşularda sayaç karışmasın diye her koşu
# kendine ait (zaman damgalı) alan adı ve kutu adları kullanır.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-mdepth
JC=/tmp/pxjar-mdepth-cust
HERE="$(cd "$(dirname "$0")" && pwd)"
T=$(date +%s)
DOM="sl$T.test"
CDOM="slm$T.test"
CUSTOMER="sinirmus"
PASS='Sinir-Sinama-Parolasi-1'
export MAIL_PASS='Kutu-Parola-12345'
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
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
mail_id() { api GET /mail | jget "next((x['id'] for x in d['domains'] if x['domain']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
# mailbox_field <alan adı numarası> <adres> <alan>: kutunun bir alanını okur.
mailbox_field() { api GET /mail/domains/$1 | jget "next(m['$3'] for m in d['mailboxes'] if m['address']=='$2')"; }
# web <Host> <yol> [curl seçenekleri]: nginx'e (80. port) belirli bir ad ile istek atar.
web() { local host=$1 path=$2; shift 2; curl -s -H "Host: $host" "$@" "http://127.0.0.1$path"; }
webcode() { local host=$1 path=$2; shift 2; curl -s -o /dev/null -w '%{http_code}' -H "Host: $host" "$@" "http://127.0.0.1$path"; }
# Sorgu değerini doğru kodlayarak gönderir (satır sonu, tırnak gibi karakterler dahil).
webq() { local host=$1 path=$2 key=$3 value=$4; curl -s -G -H "Host: $host" --data-urlencode "$key=$value" "http://127.0.0.1$path"; }
webqcode() { local host=$1 path=$2 key=$3 value=$4; curl -s -o /dev/null -w '%{http_code}' -G -H "Host: $host" --data-urlencode "$key=$value" "http://127.0.0.1$path"; }
pox() { printf '<?xml version="1.0" encoding="utf-8"?><Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/outlook/requestschema/2006"><Request><EMailAddress>%s</EMailAddress><AcceptableResponseSchema>http://schemas.microsoft.com/exchange/autodiscover/outlook/responseschema/2006a</AcceptableResponseSchema></Request></Autodiscover>' "$1"; }
wait_panel() { for _ in $(seq 1 60); do [ "$(curl -sk -o /dev/null -w '%{http_code}' $B/auth/me)" != 000 ] && return 0; sleep 1; done; return 1; }

cleanup() {
  local id
  for d in "$DOM" "$CDOM" otoayar.test; do id=$(mail_id "$d"); [ -n "$id" ] && api DELETE "/mail/domains/$id?delete_mail=true" >/dev/null; done
  id=$(site_id "$DOM"); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null
  id=$(user_id $CUSTOMER); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
  zid=$(api GET /dns | jget "next((z['id'] for z in d['zones'] if z['domain']=='otoayar.test'), '')"); [ -n "$zid" ] && api DELETE "/dns/zones/$zid" >/dev/null
  api PUT /mail/limits -d '{"default_limit":200}' >/dev/null
}

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Hazırlık"
check "temel yapılandırma" "$(code POST /mail/setup)" 200
HOST=$(api GET /mail | jget 'd["hostname"]')
echo "  posta sunucusu adı: $HOST"
check "varsayılan sınır 200 olarak ayarlı" "$(api PUT /mail/limits -d '{"default_limit":200}' | jget 'd["default_limit"]')" 200
has "Postfix: submission policy servisine bağlı" "$(postconf -P submission/inet/smtpd_recipient_restrictions)" "pixpanel-policy"
has "Postfix: smtps policy servisine bağlı" "$(postconf -P submissions/inet/smtpd_recipient_restrictions)" "pixpanel-policy"
check "Postfix: panel kapalıyken posta durmaz (varsayılan eylem)" "$(postconf -P submission/inet/smtpd_policy_service_default_action | sed 's/.*= *//')" DUNNO
QUEUE_DIR=$(postconf -h queue_directory)
check "policy soketi var ve yalnız postfix'e açık" "$(stat -c '%U %a' "$QUEUE_DIR/private/pixpanel-policy")" "postfix 600"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
UC=$(user_id $CUSTOMER)
# Sitesi olan alan adı: sitenin kendi yolundan otomatik ayar (well-known) de sınanır.
check "alan adının sitesi" "$(code POST /sites -d "{\"domain\":\"$DOM\",\"kind\":\"static\"}")" 200
D=$(api POST /mail/domains -d "{\"domain\":\"$DOM\"}" | jget 'd["id"]')
CD=$(api POST /mail/domains -d "{\"domain\":\"$CDOM\",\"user_id\":$UC}" | jget 'd["id"]')
A1="k1$T"; A2="k2$T"; EX="var$T"
for l in $A1 $A2 $EX; do
  check "kutu $l" "$(code POST /mail/domains/$D/mailboxes -d "{\"local\":\"$l\",\"password\":\"$MAIL_PASS\"}")" 200
done
CM=$(api POST /mail/domains/$CD/mailboxes -d "{\"local\":\"musteri\",\"password\":\"$MAIL_PASS\"}" | jget 'd["id"]')
check "müşteri girişi" "$(ccode POST /auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\"}")" 200
M1=$(api GET /mail/domains/$D | jget "next(m['id'] for m in d['mailboxes'] if m['local']=='$A1')")

say "Giden posta sınırı: kutu"
check "kutu sınırı 3" "$(code PATCH /mail/mailboxes/$M1 -d '{"send_limit":3}')" 200
check "geçerli sınır görünüyor" "$(mailbox_field $D $A1@$DOM effective_limit)" 3
for i in 1 2 3; do
  check "sınıra kadar kabul ($i/3, 587)" "$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "sinir-$i-$T")" sent
done
R=$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "sinir-4-$T")
has "sınırdan sonra geçici ret (450 4.7.1)" "$R" "450"
has "ret iletisi sınırı söylüyor" "$R" "4.7.1"
R=$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "sinir-5-$T" 465)
has "smtps (465) üzerinden de reddedilir" "$R" "450"
check "başka kutu etkilenmez" "$(mf send $A2@$DOM $A2@$DOM $A1@$DOM "komsu-$T")" sent
check "son saatteki sayı arayüz verisinde" "$(mailbox_field $D $A1@$DOM sent_last_hour)" 3
sleep 2
HITS=$(api GET /mail/limit-hits | jget "len([h for h in d if h['address']=='$A1@$DOM' and h['kind']=='mailbox'])")
check "sınıra takılan kayıtlı (saat başına bir kayıt)" "$HITS" 1
check "denetim günlüğüne sistem kaydı düştü" "$(api GET '/system/audit?limit=300' | jget "sum(1 for e in d if e['action']=='mail.limit.hit' and e['target']=='$A1@$DOM')")" 1

say "Giden posta sınırı: alan adı toplamı"
check "alan adı sınırı 2" "$(code PATCH /mail/domains/$D -d '{"send_limit":2}')" 200
EX_ID=$(api GET /mail/domains/$D | jget "next(m['id'] for m in d['mailboxes'] if m['local']=='$EX')")
# Alan adının bu saatteki sayacı (önceki gönderimler dahil) zaten 2'nin üstünde olduğundan yeni kutu da reddedilir.
R=$(mf send $EX@$DOM $EX@$DOM $A1@$DOM "alanadi-$T")
has "alan adı toplamı aşılınca başka kutu da reddedilir" "$R" "450"
check "alan adı sınırını varsayılana al" "$(code PATCH /mail/domains/$D -d '{"send_limit":-1}')" 200
check "sınır kalkınca aynı kutu gönderir" "$(mf send $EX@$DOM $EX@$DOM $A1@$DOM "alanadi-sonra-$T")" sent
check "geçersiz sınır (negatif)" "$(code PATCH /mail/mailboxes/$M1 -d '{"send_limit":-5}')" 400
check "geçersiz sınır (çok büyük)" "$(code PATCH /mail/mailboxes/$M1 -d '{"send_limit":2000000}')" 400
check "geçersiz varsayılan sınır" "$(code PUT /mail/limits -d '{"default_limit":-1}')" 400

say "Yetki: müşteri sınırı değiştiremez"
check "müşteri kendi kutusunun sınırını kaldıramaz" "$(ccode PATCH /mail/mailboxes/$CM -d '{"send_limit":0}')" 403
check "müşteri sınırı yükseltemez" "$(ccode PATCH /mail/mailboxes/$CM -d '{"send_limit":100000}')" 403
check "müşteri alan adı sınırını değiştiremez" "$(ccode PATCH /mail/domains/$CD -d '{"send_limit":0}')" 403
check "müşteri varsayılan sınırı değiştiremez" "$(ccode PUT /mail/limits -d '{"default_limit":0}')" 403
check "müşteri varsayılan sınırı okuyamaz" "$(ccode GET /mail/limits)" 403
check "müşteri başka kutunun sınırına dokunamaz (404)" "$(ccode PATCH /mail/mailboxes/$M1 -d '{"send_limit":0}')" 404
check "müşteri kendi sınırını görür" "$(capi GET /mail/domains/$CD | jget 'd["mailboxes"][0]["effective_limit"]')" 200
check "müşteri sınıra takılanlar listesinde yalnız kendi alanını görür" "$(capi GET /mail/limit-hits | jget "len([h for h in d if h['domain']!='$CDOM'])")" 0
check "değişmeyen değerle (varsayılan) kayıt serbest" "$(ccode PATCH /mail/mailboxes/$CM -d '{"send_limit":-1,"quota_mb":50}')" 200
check "oturumsuz: sınırlar" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/limits)" 401
check "oturumsuz: sınıra takılanlar" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/limit-hits)" 401
check "oturumsuz: istemci ayarları" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/domains/$D/client-settings)" 401

say "Panel durunca posta durmaz"
systemctl stop pixpanel
sleep 1
check "panel kapalıyken sınırı aşmış kutu da gönderir (varsayılan eylem)" "$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "panelkapali-$T")" sent
systemctl start pixpanel
check "panel yeniden açıldı" "$(wait_panel && echo evet)" evet
rm -f $J; api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
sleep 3
check "soket yeniden açıldı" "$(stat -c '%U %a' "$QUEUE_DIR/private/pixpanel-policy")" "postfix 600"
# Sayaçlar bellektedir: yeniden başlayınca sıfırlanır, kutu sınırı (veritabanında) sürer.
for i in 1 2 3; do
  check "yeniden başlatma sonrası sınıra kadar kabul ($i/3)" "$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "yeniden-$i-$T")" sent
done
has "yeniden başlatma sonrası sınır yine işliyor" "$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "yeniden-4-$T")" "450"
check "sınırı varsayılana al (geri alma)" "$(code PATCH /mail/mailboxes/$M1 -d '{"send_limit":-1}')" 200
check "geri alınca gönderim normale döner" "$(mf send $A1@$DOM $A1@$DOM $A2@$DOM "normal-$T")" sent
check "sunucu geneli sınır 0 (sınırsız) kabul edilir" "$(code PUT /mail/limits -d '{"default_limit":0}')" 200
check "varsayılan 0 iken geçerli sınır sınırsız" "$(mailbox_field $D $A2@$DOM effective_limit)" 0
check "varsayılan sınır 200'e döndü" "$(api PUT /mail/limits -d '{"default_limit":200}' | jget 'd["default_limit"]')" 200

say "Otomatik ayar: nginx blokları"
CONF=/etc/nginx/pixpanel/sites/_mail-autoconfig.conf
has "autoconfig sunucu bloğu yazıldı" "$(cat $CONF)" "server_name autoconfig.$DOM;"
has "autodiscover sunucu bloğu yazıldı" "$(cat $CONF)" "server_name autodiscover.$DOM;"
check "nginx yapılandırması geçerli" "$(nginx -t >/dev/null 2>&1 && echo evet)" evet
has "sitenin kendi yapılandırması dahil etme satırını içeriyor" "$(cat /etc/nginx/pixpanel/sites/$DOM.conf)" "mail-wellknown/$DOM.inc*"
check "site için well-known parçası yazıldı" "$(test -s /etc/nginx/pixpanel/mail-wellknown/$DOM.inc && echo evet)" evet

say "Otomatik ayar: Thunderbird (autoconfig)"
X=$(webq autoconfig.$DOM /mail/config-v1.1.xml emailaddress "$A1@$DOM")
has "XML: sunucu adı" "$X" "<hostname>$HOST</hostname>"
has "XML: IMAP 993" "$X" "<port>993</port>"
has "XML: SMTP 587" "$X" "<port>587</port>"
has "XML: SSL/TLS" "$X" "<socketType>SSL</socketType>"
has "XML: STARTTLS" "$X" "<socketType>STARTTLS</socketType>"
has "XML: alan adı" "$X" "<domain>$DOM</domain>"
check "autodiscover adı da aynı yolu sunar" "$(webqcode autodiscover.$DOM /mail/config-v1.1.xml emailaddress "$A1@$DOM")" 200
XW=$(webq $DOM /.well-known/autoconfig/mail/config-v1.1.xml emailaddress "$A1@$DOM")
has "sitenin /.well-known yolundan sunulur" "$XW" "<hostname>$HOST</hostname>"
XH=$(web autoconfig.$DOM /mail/config-v1.1.xml)
has "adres verilmezse alan adı ana makine adından çıkarılır" "$XH" "<domain>$DOM</domain>"
check "panelin genel ucu doğrudan da yanıt verir" "$(curl -sk -o /dev/null -w '%{http_code}' -G --data-urlencode "emailaddress=$A1@$DOM" $B/mail/public/autoconfig)" 200
check "bilinmeyen alan adı: 404 (nginx üzerinden)" "$(webqcode autoconfig.$DOM /mail/config-v1.1.xml emailaddress "biri@bilinmeyen-$T.test")" 404
check "bilinmeyen alan adı: 404 (doğrudan)" "$(curl -sk -o /dev/null -w '%{http_code}' -G --data-urlencode "emailaddress=biri@bilinmeyen-$T.test" $B/mail/public/autoconfig)" 404
check "var olmayan kutu ile var olan kutu aynı yanıtı alır" "$(webq autoconfig.$DOM /mail/config-v1.1.xml emailaddress "yok$T@$DOM" | sha256sum | cut -d' ' -f1)" "$(printf '%s' "$X" | sha256sum | cut -d' ' -f1)"
check "oturum çerezi gerekmez (oturumsuz istek 200)" "$(curl -sk -o /dev/null -w '%{http_code}' -G --data-urlencode "emailaddress=$A1@$DOM" $B/mail/public/autoconfig)" 200

say "Otomatik ayar: Outlook (autodiscover)"
P=$(pox "$A1@$DOM")
O=$(curl -s -H "Host: autodiscover.$DOM" -H 'content-type: text/xml' --data "$P" "http://127.0.0.1/autodiscover/autodiscover.xml")
has "POX: sunucu adı" "$O" "<Server>$HOST</Server>"
has "POX: IMAP 993" "$O" "<Port>993</Port>"
has "POX: SMTP 587" "$O" "<Port>587</Port>"
has "POX: giriş adı adresin tamamı" "$O" "<LoginName>$A1@$DOM</LoginName>"
has "POX: IMAP SSL" "$O" "<SSL>on</SSL>"
has "POX: SMTP STARTTLS" "$O" "<Encryption>TLS</Encryption>"
O2=$(curl -s -H "Host: autodiscover.$DOM" --data "$P" "http://127.0.0.1/Autodiscover/Autodiscover.xml")
has "büyük harfli yol da çalışır" "$O2" "<Server>$HOST</Server>"
O3=$(curl -s -H "Host: $DOM" --data "$P" "http://127.0.0.1/autodiscover/autodiscover.xml")
has "sitenin kendi /autodiscover yolundan sunulur" "$O3" "<Server>$HOST</Server>"
OA=$(curl -s -H "Host: autodiscover.$DOM" --data "$(pox "yok$T@$DOM")" "http://127.0.0.1/autodiscover/autodiscover.xml")
check "var olmayan kutu: yanıt yalnız adres satırında farklı" "$(printf '%s' "$OA" | sed "s/yok$T@$DOM/X/g" | sha256sum | cut -d' ' -f1)" "$(printf '%s' "$O" | sed "s/$A1@$DOM/X/g" | sha256sum | cut -d' ' -f1)"
check "bilinmeyen alan adı: 404" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: autodiscover.$DOM" --data "$(pox "biri@bilinmeyen-$T.test")" http://127.0.0.1/autodiscover/autodiscover.xml)" 404
check "GET ile autodiscover yolu (yöntem uymuyor)" "$(webcode autodiscover.$DOM /autodiscover/autodiscover.xml)" 405

say "Otomatik ayar: Apple (mobileconfig)"
MC=$(curl -si -G -H "Host: autoconfig.$DOM" --data-urlencode "email=$A1@$DOM" http://127.0.0.1/mail/mobileconfig)
has "profil türü" "$MC" "application/x-apple-aspen-config"
has "profil: IMAP portu" "$MC" "<key>IncomingMailServerPortNumber</key><integer>993</integer>"
has "profil: sunucu adı" "$MC" "<string>$HOST</string>"
check "mobileconfig bilinmeyen alan adı: 404" "$(webqcode autoconfig.$DOM /mail/mobileconfig email "biri@bilinmeyen-$T.test")" 404

say "Otomatik ayar: bozuk ve enjeksiyonlu girdiler"
for bad in $'a@'"$DOM"$'\r\nX-Evil: 1' "a b@$DOM" "<x>@$DOM" "a@$DOM'; DROP TABLE users;--" "a@$DOM&x=1" "../../etc/passwd@$DOM" '"a"@'"$DOM" "a@" "@$DOM" "a@$DOM$(printf 'x%.0s' $(seq 1 400))"; do
  check "autoconfig reddeder: $(printf '%s' "$bad" | tr -d '\r\n' | head -c 40)" "$(webqcode autoconfig.$DOM /mail/config-v1.1.xml emailaddress "$bad")" 400
done
check "autoconfig boş adres" "$(webqcode autoconfig.$DOM /mail/config-v1.1.xml emailaddress "")" 400
R=$(webq autoconfig.$DOM /mail/config-v1.1.xml emailaddress $'a@'"$DOM"$'\r\nX-Evil: 1')
hasnt "enjeksiyon yanıta yansımaz" "$R" "X-Evil"
check "mobileconfig bozuk adres" "$(webqcode autoconfig.$DOM /mail/mobileconfig email "<script>@$DOM")" 400
check "mobileconfig adres yok" "$(webcode autoconfig.$DOM /mail/mobileconfig)" 400
check "POX: etiket içinde HTML" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: autodiscover.$DOM" --data "$(pox "&lt;script&gt;@$DOM")" http://127.0.0.1/autodiscover/autodiscover.xml)" 400
check "POX: adreste satır sonu" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: autodiscover.$DOM" --data "$(pox $'a@'"$DOM"$'\nX-Evil: 1')" http://127.0.0.1/autodiscover/autodiscover.xml)" 400
check "POX: adres etiketi yok" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: autodiscover.$DOM" --data '<Autodiscover><Request/></Autodiscover>' http://127.0.0.1/autodiscover/autodiscover.xml)" 400
check "POX: boş gövde" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: autodiscover.$DOM" -X POST http://127.0.0.1/autodiscover/autodiscover.xml)" 400
XXE='<?xml version="1.0"?><!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]><Autodiscover><Request><EMailAddress>'"$A1@$DOM"'</EMailAddress><X>&e;</X></Request></Autodiscover>'
R=$(curl -s -H "Host: autodiscover.$DOM" --data "$XXE" http://127.0.0.1/autodiscover/autodiscover.xml)
hasnt "POX: dış varlık (XXE) işlenmez" "$R" "root:"
BIG=$(head -c 20000 /dev/zero | tr '\0' 'a')
check "POX: aşırı büyük gövde" "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: autodiscover.$DOM" --data "$(pox "$BIG@$DOM")" http://127.0.0.1/autodiscover/autodiscover.xml)" 413
check "tanımsız ad: nginx yanıt vermez (444) ya da 404" "$(case "$(webcode autoconfig.bilinmeyen-$T.test /mail/config-v1.1.xml)" in 000|404) echo evet ;; *) echo hayir ;; esac)" evet

say "DNS önerileri ve arayüz verisi"
DNSNAMES=$(api GET /mail/domains/$D | jget '" ".join(r["kind"] + ":" + r["name"] for r in d["dns"])')
has "autoconfig CNAME önerisi" "$DNSNAMES" "CNAME:autoconfig.$DOM"
has "autodiscover CNAME önerisi" "$DNSNAMES" "CNAME:autodiscover.$DOM"
has "_autodiscover._tcp SRV önerisi" "$DNSNAMES" "SRV:_autodiscover._tcp.$DOM"
has "_imaps._tcp SRV önerisi" "$DNSNAMES" "SRV:_imaps._tcp.$DOM"
has "_submission._tcp SRV önerisi" "$DNSNAMES" "SRV:_submission._tcp.$DOM"
CS=$(api GET /mail/domains/$D/client-settings)
check "istemci ayarları: sunucu adı" "$(printf '%s' "$CS" | jget 'd["hostname"]')" "$HOST"
check "istemci ayarları: portlar" "$(printf '%s' "$CS" | jget '(d["imap_port"], d["submission_port"], d["smtps_port"])')" "(993, 587, 465)"
check "istemci ayarları: iki ad" "$(printf '%s' "$CS" | jget 'len(d["names"])')" 2
check "müşteri başkasının alan adının istemci ayarını göremez" "$(ccode GET /mail/domains/$D/client-settings)" 404

say "DNS: bölge panelde yönetiliyorsa kayıtlar kendiliğinden eklenir"
if [ "$(api GET /dns | jget 'd["local_installed"] and d["local_running"]')" = True ] && [[ "$HOST" == *.* ]]; then
  ZR=$(api POST /dns/zones -d '{"domain":"otoayar.test","provider":"local"}')
  OD=$(api POST /mail/domains -d '{"domain":"otoayar.test"}' | jget 'd["id"]')
  q() { dig +short +time=2 +tries=1 @127.0.0.1 "$@" | tr '\n' ' ' | sed 's/ $//'; }
  sleep 2
  has "_imaps._tcp SRV yayınlandı" "$(q SRV _imaps._tcp.otoayar.test)" " 993 "
  has "_submission._tcp SRV yayınlandı" "$(q SRV _submission._tcp.otoayar.test)" " 587 "
  has "_autodiscover._tcp SRV yayınlandı" "$(q SRV _autodiscover._tcp.otoayar.test)" " 443 autodiscover.otoayar.test."
  has "autoconfig adı çözülüyor" "$(q A autoconfig.otoayar.test)" "."
  RP=$(api POST /mail/domains/$OD/dns/apply | jget '" | ".join(d["report"])')
  has "yeniden uygulama zararsız (zaten doğru)" "$RP" "zaten doğru"
  # Farklı değerdeki var olan kayda dokunulmaz.
  ZID=$(printf '%s' "$ZR" | jget 'd["zone"]["id"]')
  api POST /dns/zones/$ZID/records/delete -d '{"record":{"id":"","type":"SRV","name":"_imaps._tcp.otoayar.test","content":"1 993 '"$HOST"'","ttl":3600,"priority":0}}' >/dev/null
  api POST /dns/zones/$ZID/records -d '{"record":{"id":"","type":"SRV","name":"_imaps._tcp.otoayar.test","content":"5 1993 baska.ornek.net","ttl":3600,"priority":9}}' >/dev/null
  RP=$(api POST /mail/domains/$OD/dns/apply | jget '" | ".join(d["report"])')
  has "farklı değerdeki kayda dokunulmadı" "$RP" "SRV _imaps._tcp.otoayar.test: farklı bir kayıt var, dokunulmadı"
else
  echo "  ATLANDI DNS sunucusu kurulu/çalışır değil; otomatik kayıt denetimi yapılmadı"
fi

say "Posta alanı silinince nginx blokları kalkar"
for d in "$DOM" otoayar.test; do id=$(mail_id "$d"); [ -n "$id" ] && api DELETE "/mail/domains/$id?delete_mail=true" >/dev/null; done
sleep 1
hasnt "autoconfig bloğu kalktı" "$(cat $CONF 2>/dev/null)" "autoconfig.$DOM"
check "well-known parçası kalktı" "$(test -e /etc/nginx/pixpanel/mail-wellknown/$DOM.inc && echo var || echo yok)" yok
check "nginx yapılandırması hâlâ geçerli" "$(nginx -t >/dev/null 2>&1 && echo evet)" evet
check "alan adı kalkınca autoconfig adı yanıt vermez" "$(case "$(webqcode autoconfig.$DOM /mail/config-v1.1.xml emailaddress "$A1@$DOM")" in 000|404) echo evet ;; *) echo hayir ;; esac)" evet

say "Temizlik"
cleanup
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/settings)" 401
rm -f $J $JC
echo "bitti"
