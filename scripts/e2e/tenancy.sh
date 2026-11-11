#!/usr/bin/env bash
# Hesaplar arası sınırların sınaması: bir müşteri iç servisleri dışarı açamaz, başkasının alan
# adının altına yerleşemez, sahibi olduğunu kanıtlamadığı alan adına posta açamaz; askıya alınan
# ya da silinen hesabın sunucudaki erişimi de kapanır.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash tenancy.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
DB=/var/lib/pixpanel/pixpanel.db
J=/tmp/pxjar-ten
JA=/tmp/pxjar-ten-a
JB=/tmp/pxjar-ten-b
A=sinirbir
O=siniriki
PASS='Sinir-Sinama-Parolasi-1'
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
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
login() { curl -sk -o /dev/null -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$PASS\"}"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
proxy() { status $1 POST /sites -d "{\"domain\":\"$2\",\"kind\":\"proxy\",\"proxy_target\":\"$3\"}"; }

rm -f $J $JA $JB
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in sinir-a.test alt.sinir-a.test giris.sinir-a.test sinir-b.test vekil-ic.test vekil-dis.test vekil-yonetici.test kendi.sinir-b.test; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
for zone in $(api GET /dns | jget '" ".join(str(z["id"]) for z in d["zones"] if "sinir-" in z["domain"])'); do api DELETE /dns/zones/$zone >/dev/null; done
for name in $A $O; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done

say "Hazırlık"
check "iki müşteri" "$(code POST /users -d "{\"username\":\"$A\",\"password\":\"$PASS\",\"role\":\"user\"}") $(code POST /users -d "{\"username\":\"$O\",\"password\":\"$PASS\",\"role\":\"user\"}")" "200 200"
login $JA $A; login $JB $O
check "birincinin sitesi" "$(status $JA POST /sites -d '{"domain":"sinir-a.test","kind":"static"}')" 200
check "ikincinin sitesi" "$(status $JB POST /sites -d '{"domain":"sinir-b.test","kind":"static"}')" 200

say "İç servisleri dışarı açma (vekil hedefi)"
PANEL_PORT=8443
check "panelin kendisi" "$(proxy $JB vekil-ic.test https://127.0.0.1:$PANEL_PORT)" 400
check "posta süzgecinin yönetim ucu" "$(proxy $JB vekil-ic.test http://127.0.0.1:11334)" 400
check "veritabanı araçları sunucusu" "$(proxy $JB vekil-ic.test http://localhost:8446)" 400
check "web sunucusunun kendisi (port verilmeden)" "$(proxy $JB vekil-ic.test http://127.0.0.1)" 400
check "başka hesabın Redis portu" "$(proxy $JB vekil-ic.test http://127.0.0.1:16400)" 400
check "bulut metaveri adresi" "$(proxy $JB vekil-ic.test http://169.254.169.254)" 400
check "iç ağ" "$(proxy $JB vekil-ic.test http://10.0.0.5:8080) $(proxy $JB vekil-ic.test http://192.168.1.1)" "400 400"
check "IPv6 yerel adres" "$(proxy $JB vekil-ic.test 'http://[::1]:8446')" 400
has "neden açıkça söylenir" "$(call $JB POST /sites -d '{"domain":"vekil-ic.test","kind":"proxy","proxy_target":"http://127.0.0.1:11334"}')" "yalnızca yönetici"
check "kendi uygulamasının portu serbest" "$(proxy $JB vekil-dis.test http://127.0.0.1:3000)" 200
check "dış adres serbest" "$(status $JB PATCH "/sites/$(site_id vekil-dis.test)" -d '{"proxy_target":"https://ornek.org"}')" 200
check "sonradan iç adrese çevrilemez" "$(status $JB PATCH "/sites/$(site_id vekil-dis.test)" -d '{"proxy_target":"http://127.0.0.1:8446"}')" 400
check "yol kuralıyla da açılamaz" "$(status $JB PATCH "/sites/$(site_id sinir-b.test)" -d '{"proxy_rules":[{"path":"/ic/","target":"http://127.0.0.1:11334","websocket":false,"strip_prefix":true}]}')" 400
check "yönetici iç adresi hedef gösterebilir" "$(proxy $J vekil-yonetici.test http://127.0.0.1:8446)" 200

say "Başkasının alan adının altına yerleşme"
check "başka hesabın alan adının altında site" "$(status $JB POST /sites -d '{"domain":"giris.sinir-a.test","kind":"static"}')" 400
check "takma ad olarak da eklenemez" "$(status $JB PATCH "/sites/$(site_id sinir-b.test)" -d '{"aliases":["giris.sinir-a.test"]}')" 400
check "kopyalayarak da alınamaz" "$(status $JB POST "/sites/$(site_id sinir-b.test)/clone" -d '{"domain":"kopya.sinir-a.test"}')" 400
check "hesap kendi alan adının altını kullanır" "$(status $JB POST /sites -d '{"domain":"kendi.sinir-b.test","kind":"static"}')" 200
check "alan adının sahibi kendi altını kullanır" "$(status $JA POST /sites -d '{"domain":"alt.sinir-a.test","kind":"static"}')" 200
check "yönetici istediği adı açar" "$(code POST /sites -d '{"domain":"giris.sinir-a.test","kind":"static"}')" 200
api DELETE "/sites/$(site_id giris.sinir-a.test)?delete_files=true" >/dev/null

if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="dns")')" = True ]; then
  say "DNS bölgesi"
  check "yalnız alt alan adı olan hesap, içinde başkasının sitesi bulunan üst adın bölgesini açamaz" "$(api DELETE "/sites/$(site_id alt.sinir-a.test)?delete_files=true" >/dev/null; code POST /sites -d "{\"domain\":\"alt.sinir-a.test\",\"kind\":\"static\",\"user_id\":$(user_id $O)}" >/dev/null; status $JB POST /dns/zones -d '{"domain":"sinir-a.test","provider":"local"}')" 400
  check "alan adının sahibi kendi bölgesini açar" "$(status $JA POST /dns/zones -d '{"domain":"sinir-a.test","provider":"local"}')" 200
  api DELETE "/sites/$(site_id alt.sinir-a.test)?delete_files=true" >/dev/null
fi

if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="mail")')" = True ]; then
  say "Posta alanı"
  has "bu sunucuyu göstermeyen alan adına müşteri posta açamaz" "$(call $JB POST /mail/domains -d '{"domain":"sinir-b.test"}')" "henüz bu sunucuyu göstermiyor"
  check "posta alanı açılmadı" "$(sqlite3 $DB "SELECT count(*) FROM mail_domains WHERE domain='sinir-b.test'")" 0
  check "yönetici müşteri adına açabilir" "$(code POST /mail/domains -d "{\"domain\":\"sinir-b.test\",\"user_id\":$(user_id $O)}")" 200
  api DELETE "/mail/domains/$(api GET /mail | jget "next(x['id'] for x in d['domains'] if x['domain']=='sinir-b.test')")" >/dev/null
fi

say "Ayrılmış adlar"
check "içe aktarma hesabı adıyla veritabanı kullanıcısı açılamaz" "$(status $JB POST /databases/users -d "{\"engine\":\"mariadb\",\"name\":\"pxi_0123456789abcdef01234567\",\"password\":\"Bir-Parola-123456\"}")" 400
check "sistem hesabı adıyla panel hesabı açılamaz" "$(code POST /users -d "{\"username\":\"postgres\",\"password\":\"$PASS\",\"role\":\"user\"}")" 400

say "Askıya alınan hesap"
call $JB POST /cron -d '{"schedule":"0 4 1 1 *","command":"echo sinir","comment":""}' >/dev/null
check "zamanlı görev dosyası var" "$(test -e /etc/cron.d/pixpanel-$O && echo var || echo yok)" var
check "askıya al" "$(code PATCH /users/$(user_id $O) -d '{"status":"suspended"}')" 200
check "Linux hesabının süresi dolmuş (SSH/SFTP ve görevler kapalı)" "$(getent shadow $O | cut -d: -f8 | sed 's/^$/suresiz/')" 1
check "zamanlı görevler durduruldu" "$(test -e /etc/cron.d/pixpanel-$O && echo var || echo yok)" yok
check "sitesi yayında kalır" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: sinir-b.test' http://127.0.0.1/)" 200
check "yeniden etkinleştir" "$(code PATCH /users/$(user_id $O) -d '{"status":"active"}')" 200
check "Linux hesabı ve görevler geri geldi" "$(getent shadow $O | cut -d: -f8 | sed 's/^$/suresiz/') $(grep -c 'echo sinir' /etc/cron.d/pixpanel-$O)" "suresiz 1"

say "Silinen hesap"
OLD_ID=$(user_id $O)
for name in sinir-b.test kendi.sinir-b.test vekil-dis.test; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
check "hesabı sil" "$(code DELETE /users/$OLD_ID)" 200
check "Linux hesabı ve ev klasörü kalktı" "$(getent passwd $O | wc -l) $(test -e /home/$O && echo var || echo yok)" "0 yok"
check "zamanlı görevleri kalktı" "$(test -e /etc/cron.d/pixpanel-$O && echo var || echo yok) $(sqlite3 $DB "SELECT count(*) FROM cron_jobs WHERE run_as='$O'")" "yok 0"
check "aynı adla yeni hesap temiz başlar" "$(code POST /users -d "{\"username\":\"$O\",\"password\":\"$PASS\",\"role\":\"user\"}") $(login $JB $O; call $JB GET /cron | jget 'len(d)' 2>/dev/null)" "200 0"
# Dosyaları saklanarak silinen sitenin hesabı: Linux hesabı silinmez (dosyalar durur), ad yeniden kullanılamaz.
api DELETE "/sites/$(site_id alt.sinir-a.test)?delete_files=true" >/dev/null 2>&1
for zone in $(api GET /dns | jget '" ".join(str(z["id"]) for z in d["zones"] if "sinir-" in z["domain"])'); do api DELETE /dns/zones/$zone >/dev/null; done
api DELETE "/sites/$(site_id sinir-a.test)?delete_files=false" >/dev/null
check "dosyaları saklanan hesabı sil" "$(code DELETE /users/$(user_id $A))" 200
check "Linux hesabı kilitli olarak kalır, dosyalar durur" "$(getent shadow $A | cut -d: -f8 | sed 's/^$/suresiz/') $(test -d /home/$A/sites/sinir-a.test && echo var || echo yok)" "1 var"
has "o ad yeniden kullanılamaz" "$(api POST /users -d "{\"username\":\"$A\",\"password\":\"$PASS\",\"role\":\"user\"}")" "sunucuda kullanılıyor"

say "Temizlik"
for name in vekil-yonetici.test; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
check "ikinci hesabı sil" "$(code DELETE /users/$(user_id $O))" 200
pkill -KILL -u $A 2>/dev/null; userdel $A 2>/dev/null; rm -rf /home/$A /etc/cron.d/pixpanel-$A
rm -f $J $JA $JB
echo "bitti"
