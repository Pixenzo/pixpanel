#!/usr/bin/env bash
# Bayi bazında marka sınaması: bayi kendi adını/logosunu/giriş metnini/destek bağlantısını ve panel
# alan adını tanımlar; müşterisi ve alt hesabı bu markayı görür, başkaları sunucu markasını görür.
# Oturumsuz istekte marka Host başlığındaki alan adına göre seçilir.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash reseller-brand.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
DB=/var/lib/pixpanel/pixpanel.db
J=/tmp/pxjar-rb
JR1=/tmp/pxjar-rb-r1
JR2=/tmp/pxjar-rb-r2
JC1=/tmp/pxjar-rb-c1
JC2=/tmp/pxjar-rb-c2
JM1=/tmp/pxjar-rb-m1
JI=/tmp/pxjar-rb-i
RES1=rbayibir
RES2=rbayiiki
RES3=rbayiuc
CUS1=rmusteribir
CUS2=rmusteriiki
MEM1=raltbir
PASS='Bayi-Marka-Sinama-1'
DOMAIN=panel.bayi.test
DOMAIN2=panel.bayiiki.test

call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
user_id() { call $J GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
# Oturumlu görünen marka adı ve oturumsuz (isteğe bağlı Host başlıklı) görünen marka adı.
seen() { call $1 GET /branding | jget 'd["name"]'; }
anon() { curl -sk "$@" $B/branding | jget 'd["name"]'; }
PNG='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=='
SVG="data:image/svg+xml;base64,$(printf '<svg xmlns="http://www.w3.org/2000/svg"/>' | base64 -w0)"
# Sunucu geneli marka gövdesi: ad, logo, giriş metni, destek bağlantısı.
sbody() { python3 -c 'import json,sys; print(json.dumps(dict(zip(["name","logo","login_message","support_url"], sys.argv[1:]))))' "$@"; }
# Bayi marka gövdesi: ad, logo, giriş metni, destek bağlantısı, panel alan adı.
rbody() { python3 -c 'import json,sys; print(json.dumps(dict(zip(["name","logo","login_message","support_url","panel_domain"], sys.argv[1:]))))' "$@"; }
sql() { sqlite3 $DB "$1"; }

cleanup() {
  for name in $MEM1 $CUS1 $CUS2 $RES1 $RES2 $RES3; do
    id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
  done
}

rm -f $J; login $J "$PX_USER" "$PX_PASS" >/dev/null
BEFORE=$(api GET /branding)
cleanup

say "Hazırlık"
check "sunucu markasını belirle" "$(code PUT /branding -d "$(sbody 'Sunucu Markasi' "$PNG" 'Sunucu metni' '')")" 200
check "bayi 1" "$(code POST /users -d "{\"username\":\"$RES1\",\"password\":\"$PASS\",\"role\":\"reseller\"}")" 200
check "bayi 2" "$(code POST /users -d "{\"username\":\"$RES2\",\"password\":\"$PASS\",\"role\":\"reseller\"}")" 200
check "bayi 3" "$(code POST /users -d "{\"username\":\"$RES3\",\"password\":\"$PASS\",\"role\":\"reseller\"}")" 200
login $JR1 $RES1 "$PASS" >/dev/null
login $JR2 $RES2 "$PASS" >/dev/null
check "bayi 1'in müşterisi" "$(status $JR1 POST /users -d "{\"username\":\"$CUS1\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
check "bayi 2'nin müşterisi" "$(status $JR2 POST /users -d "{\"username\":\"$CUS2\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
login $JC1 $CUS1 "$PASS" >/dev/null
login $JC2 $CUS2 "$PASS" >/dev/null
check "müşterinin alt hesabı" "$(status $JC1 POST /users -d "{\"username\":\"$MEM1\",\"password\":\"$PASS\",\"role\":\"member\",\"permissions\":[\"sites\"]}")" 200
login $JM1 $MEM1 "$PASS" >/dev/null
R1=$(user_id $RES1); R2=$(user_id $RES2); R3=$(user_id $RES3); C1=$(user_id $CUS1); C2=$(user_id $CUS2)

say "Marka tanımlanmadan"
check "bayinin müşterisi sunucu markasını görür" "$(seen $JC1)" "Sunucu Markasi"
check "bayinin kendisi sunucu markasını görür" "$(seen $JR1)" "Sunucu Markasi"
check "bayinin kaydı yok" "$(call $JR1 GET /branding/resellers/$R1 | jget '(d["defined"], d["name"], d["panel_domain"])')" "(False, '', '')"

say "Bayi markasını kaydeder"
check "kaydet (alan adı küçük harfe çevrilir)" "$(call $JR1 PUT /branding/resellers/$R1 -d "$(rbody ' Bayi Bir ' "$SVG" 'Bayi destek metni' 'https://destek.bayi.test/yardim' 'Panel.Bayi.Test')" | jget '(d["name"], d["logo"][:26], d["login_message"], d["support_url"], d["panel_domain"], d["defined"])')" "('Bayi Bir', 'data:image/svg+xml;base64,', 'Bayi destek metni', 'https://destek.bayi.test/yardim', '$DOMAIN', True)"
check "okuma aynı kaydı verir" "$(call $JR1 GET /branding/resellers/$R1 | jget '(d["name"], d["panel_domain"])')" "('Bayi Bir', '$DOMAIN')"
check "denetim günlüğüne yazıldı" "$(sql "SELECT count(*) > 0 FROM audit_log WHERE action='branding.reseller.update' AND target='$RES1'")" 1

say "Kim hangi markayı görür"
check "bayi kendi markasını görür" "$(seen $JR1)" "Bayi Bir"
check "müşteri bayinin markasını görür" "$(seen $JC1)" "Bayi Bir"
check "alt hesap bayinin markasını görür" "$(seen $JM1)" "Bayi Bir"
check "destek bağlantısı müşteriye de gelir" "$(call $JC1 GET /branding | jget 'd["support_url"]')" "https://destek.bayi.test/yardim"
check "başka bayi sunucu markasını görür" "$(seen $JR2)" "Sunucu Markasi"
check "başka bayinin müşterisi sunucu markasını görür" "$(seen $JC2)" "Sunucu Markasi"
check "yönetici sunucu markasını görür" "$(seen $J)" "Sunucu Markasi"
check "sunucu markası değişmedi" "$(api GET /branding | jget 'd["login_message"]')" "Sunucu metni"

say "Oturumsuz görünüm (Host başlığı yalnız görünümü seçer)"
check "oturumsuz: sunucu markası" "$(anon)" "Sunucu Markasi"
check "oturumsuz + bayinin alan adı: bayi markası" "$(anon -H "Host: $DOMAIN")" "Bayi Bir"
check "oturumsuz + büyük harf ve port" "$(anon -H "Host: PANEL.Bayi.Test:8443")" "Bayi Bir"
check "oturumsuz + bilinmeyen alan adı: sunucu markası" "$(anon -H 'Host: baska.bayi.test')" "Sunucu Markasi"
check "oturumlu istekte Host başlığı markayı değiştirmez (oturum önce gelir)" "$(curl -sk -H "Cookie: $(awk 'NF>=7 {print $6"="$7}' $JC2 | paste -sd';')" -H "Host: $DOMAIN" $B/branding | jget 'd["name"]')" "Sunucu Markasi"
check "Host başlığı yetki vermez: bayi uçları yine oturum ister" "$(curl -sk -o /dev/null -w '%{http_code}' -H "Host: $DOMAIN" $B/branding/resellers/$R1)" 401
check "Host ile oturumsuz yazma yine 401" "$(curl -sk -o /dev/null -w '%{http_code}' -H "Host: $DOMAIN" -H 'content-type: application/json' -X PUT $B/branding -d "$(sbody X '' '' '')")" 401

say "Yetki"
check "oturumsuz bayi markası yazılamaz" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -X PUT $B/branding/resellers/$R1 -d "$(rbody X '' '' '' '')")" 401
check "müşteri bayi markasını değiştiremez" "$(status $JC1 PUT /branding/resellers/$R1 -d "$(rbody Hile '' '' '' '')")" 403
check "müşteri kendi markasını da tanımlayamaz" "$(status $JC1 PUT /branding/resellers/$C1 -d "$(rbody Hile '' '' '' '')")" 403
check "müşteri bayi markasını okuyamaz" "$(status $JC1 GET /branding/resellers/$R1)" 403
check "alt hesap değiştiremez" "$(status $JM1 PUT /branding/resellers/$R1 -d "$(rbody Hile '' '' '' '')")" 403
check "müşteri sunucu markasını değiştiremez" "$(status $JC1 PUT /branding -d "$(sbody Hile '' '' '')")" 403
check "bayi sunucu markasını değiştiremez" "$(status $JR1 PUT /branding -d "$(sbody Hile '' '' '')")" 403
check "başka bayi bu bayinin markasını değiştiremez" "$(status $JR2 PUT /branding/resellers/$R1 -d "$(rbody Hile '' '' '' '')")" 404
check "başka bayi bu bayinin markasını okuyamaz" "$(status $JR2 GET /branding/resellers/$R1)" 404
check "başka bayi bu bayinin markasını silemez" "$(status $JR2 DELETE /branding/resellers/$R1)" 404
check "bayi olmayan hesap için marka yok" "$(code PUT /branding/resellers/$C1 -d "$(rbody X '' '' '' '')")" 404
check "olmayan hesap" "$(code GET /branding/resellers/999999)" 404
check "saldırı sonrası marka bozulmadı" "$(seen $JC1)" "Bayi Bir"
check "yönetici bayinin markasını görür" "$(api GET /branding/resellers/$R1 | jget '(d["name"], d["defined"])')" "('Bayi Bir', True)"
check "yönetici bayi adına değiştirir" "$(code PUT /branding/resellers/$R1 -d "$(rbody 'Bayi Bir' "$SVG" 'Bayi destek metni' 'mailto:destek@bayi.test' "$DOMAIN")")" 200
check "değişiklik müşteriye yansır" "$(call $JC1 GET /branding | jget 'd["support_url"]')" "mailto:destek@bayi.test"
check "alan adı korundu" "$(api GET /branding/resellers/$R1 | jget 'd["panel_domain"]')" "$DOMAIN"

say "Panel alan adı tek bayiye bağlanır"
check "ikinci bayi aynı adı alamaz" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' '' '' '' "$DOMAIN")")" 409
check "büyük harf ve nokta ile de alamaz" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' '' '' '' 'PANEL.Bayi.Test.')")" 409
check "yönetici de ikinci bayiye aynı adı bağlayamaz" "$(code PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' '' '' '' "$DOMAIN")")" 409
check "reddedilen istek ikinci bayiye kayıt açmadı" "$(call $JR2 GET /branding/resellers/$R2 | jget 'd["defined"]')" "False"
check "bayi kendi adını yeniden kaydedebilir" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody 'Bayi Bir' "$SVG" 'Bayi destek metni' 'mailto:destek@bayi.test' "$DOMAIN")")" 200
check "ikinci bayi kendi adını alır" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' "$PNG" '' '' "$DOMAIN2")")" 200
check "ikinci bayinin adıyla oturumsuz istek" "$(anon -H "Host: $DOMAIN2")" "Bayi Iki"
check "birinci bayinin adı hâlâ birincide" "$(anon -H "Host: $DOMAIN")" "Bayi Bir"
check "ikinci bayi birincinin adına geçemez" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' "$PNG" '' '' "$DOMAIN")")" 409
check "ikinci bayinin müşterisi artık ikinci markayı görür" "$(seen $JC2)" "Bayi Iki"
PANEL_HOST=$(hostname -f 2>/dev/null || hostname)
check "panelin kendi adı bayiye bağlanamaz" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' "$PNG" '' '' "$PANEL_HOST")")" 400

say "Geçersiz değerler (400)"
big=/tmp/rb-big.json
check "görsel olmayan logo" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad 'data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==' '' '' '')")" 400
check "javascript: logo" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad 'javascript:alert(1)' '' '' '')")" 400
check "dış adres logo olamaz" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad 'https://bayi.test/logo.png' '' '' '')")" 400
check "bozuk base64 logo" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad 'data:image/png;base64,***' '' '' '')")" 400
python3 -c 'import json; print(json.dumps({"name":"Ad","logo":"data:image/png;base64,"+"A"*210000,"login_message":"","support_url":"","panel_domain":""}))' > $big
check "aşırı büyük logo" "$(status $JR1 PUT /branding/resellers/$R1 -d @$big)" 400
rm -f $big
check "uzun ad" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody "$(printf 'a%.0s' $(seq 1 41))" '' '' '' '')")" 400
check "uzun giriş metni" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' "$(printf 'a%.0s' $(seq 1 301))" '' '')")" 400
check "denetim karakterli ad" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody "$(printf 'a\007b')" '' '' '' '')")" 400
check "javascript: destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'javascript:alert(1)' '')")" 400
check "data: destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'data:text/html,x' '')")" 400
check "http: destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'http://destek.bayi.test' '')")" 400
check "satır sonlu destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' "$(printf 'https://destek.bayi.test/\nx')" '')")" 400
check "boşluklu destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'https://destek.bayi.test/a b' '')")" 400
check "tırnaklı destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'https://destek.bayi.test/"onclick=1' '')")" 400
check "kullanıcı bilgili destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'https://kullanici:parola@destek.bayi.test' '')")" 400
check "adressiz mailto" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' 'mailto:destek' '')")" 400
check "uzun destek bağlantısı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' "https://bayi.test/$(printf 'a%.0s' $(seq 1 200))" '')")" 400
check "boşluklu alan adı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' '' 'bad domain.test')")" 400
check "satır sonlu alan adı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' '' "$(printf 'a.b\nserver{}')")")" 400
check "IP adresi alan adı olamaz" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' '' '1.2.3.4')")" 400
check "tek etiketli ad" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' '' 'localhost')")" 400
check "yol içeren alan adı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' '' 'bayi.test/../etc')")" 400
check "joker alan adı" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody Ad '' '' '' '*.bayi.test')")" 400
check "reddedilen istekler markayı bozmadı" "$(call $JR1 GET /branding/resellers/$R1 | jget '(d["name"], d["support_url"], d["panel_domain"])')" "('Bayi Bir', 'mailto:destek@bayi.test', '$DOMAIN')"

say "Müşteri olarak giriş"
rm -f $JI; login $JI "$PX_USER" "$PX_PASS" >/dev/null
check "yönetici müşteriye girer" "$(status $JI POST /users/$C1/impersonate)" 200
check "bayinin müşterisinin gözünden bayi markası" "$(seen $JI)" "Bayi Bir"
check "dönüş" "$(status $JI POST /auth/impersonate/stop)" 200
check "yönetici yine sunucu markasını görür" "$(seen $JI)" "Sunucu Markasi"
rm -f $JI

say "Marka kaldırılır"
check "bayi markasını siler" "$(status $JR1 DELETE /branding/resellers/$R1)" 200
check "kayıt kalktı" "$(call $JR1 GET /branding/resellers/$R1 | jget '(d["defined"], d["panel_domain"])')" "(False, '')"
check "müşteri sunucu markasını görür" "$(seen $JC1)" "Sunucu Markasi"
check "alt hesap sunucu markasını görür" "$(seen $JM1)" "Sunucu Markasi"
check "alan adı eşlemesi de kalktı" "$(anon -H "Host: $DOMAIN")" "Sunucu Markasi"
check "silme denetim günlüğünde" "$(sql "SELECT count(*) > 0 FROM audit_log WHERE action='branding.reseller.delete' AND target='$RES1'")" 1
check "silinmiş markayı silmek zararsız" "$(status $JR1 DELETE /branding/resellers/$R1)" 200
check "bayi markası yeniden tanımlar, sonra hepsini boşaltarak kaldırır" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody 'Bayi Bir' '' '' '' '')") $(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody '' '' '' '' '')") $(call $JR1 GET /branding/resellers/$R1 | jget 'd["defined"]')" "200 200 False"
check "adı serbest kalan ikinci bayi alabilir" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' "$PNG" '' '' "$DOMAIN")")" 200
check "ikinci bayi eski adını bırakır" "$(status $JR2 PUT /branding/resellers/$R2 -d "$(rbody 'Bayi Iki' "$PNG" '' '' '')")" 200

say "Bayi silinince marka ve alan adı eşlemesi kalkar"
check "üçüncü bayiye marka ve alan adı" "$(code PUT /branding/resellers/$R3 -d "$(rbody 'Bayi Uc' "$PNG" '' '' "$DOMAIN")")" 200
check "oturumsuz Host ile bayi 3" "$(anon -H "Host: $DOMAIN")" "Bayi Uc"
check "bayi 3'ü sil" "$(code DELETE /users/$R3)" 200
check "marka satırı kalmadı" "$(sql "SELECT count(*) FROM reseller_brands WHERE reseller_id=$R3 OR panel_domain='$DOMAIN'")" 0
check "oturumsuz Host artık sunucu markası" "$(anon -H "Host: $DOMAIN")" "Sunucu Markasi"
check "alan adı başka bayiye bağlanabilir" "$(status $JR1 PUT /branding/resellers/$R1 -d "$(rbody 'Bayi Bir' '' '' '' "$DOMAIN")")" 200

say "Temizlik"
cleanup
check "hesaplar silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$MEM1','$CUS1','$CUS2','$RES1','$RES2','$RES3')]")" "[]"
check "marka satırları silindi" "$(sql "SELECT count(*) FROM reseller_brands")" 0
check "sunucu markasını eski hâline döndür" "$(code PUT /branding -d "$BEFORE")" 200
check "sunucu markası eski" "$(api GET /branding | jget 'd["name"]')" "$(printf %s "$BEFORE" | jget 'd["name"]')"
rm -f $J $JR1 $JR2 $JC1 $JC2 $JM1 $JI
echo "bitti"
