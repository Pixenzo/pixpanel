#!/usr/bin/env bash
# Posta modülünün uçtan uca sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash mail.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-mail
HERE="$(cd "$(dirname "$0")" && pwd)"
export MAIL_PASS='Kutu-Parola-12345'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if printf '%s' "$2" | grep -qF -- "$3"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(printf '%s' "$2" | head -c 200)]"; fi; }
hasnt() { if printf '%s' "$2" | grep -qF -- "$3"; then echo "  HATA $1: [$3] olmamalıydı"; else echo "  OK   $1"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
mf() { python3 "$HERE/mailflow.py" "$@"; }

api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
T=$(date +%s)

say "Kurulum"
check "temel yapılandırma" "$(code POST /mail/setup)" 200
api GET /mail | jget '[(s["label"].split()[0], s["running"]) for s in d["services"]], d["hostname"]'
OLD=$(api GET /mail | jget 'next((x["id"] for x in d["domains"] if x["domain"]=="posta.test"), "")')
[ -n "$OLD" ] && api DELETE "/mail/domains/$OLD?delete_mail=true" >/dev/null

say "Alan adı, kutular, takma ad"
check "geçersiz alan adı" "$(code POST /mail/domains -d '{"domain":"../etc"}')" 400
D=$(api POST /mail/domains -d '{"domain":"posta.test"}' | jget 'd["id"]')
check "aynı alan adı ikinci kez" "$(code POST /mail/domains -d '{"domain":"posta.test"}')" 409
check "yol kaçışlı kutu adı" "$(code POST /mail/domains/$D/mailboxes -d "{\"local\":\"../x\",\"password\":\"$MAIL_PASS\"}")" 400
check "kısa parola" "$(code POST /mail/domains/$D/mailboxes -d '{"local":"ali","password":"kisa"}')" 400
ALI=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"ali\",\"password\":\"$MAIL_PASS\",\"quota_mb\":50}" | jget 'd["id"]')
VELI=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"Veli\",\"password\":\"$MAIL_PASS\",\"quota_mb\":10}" | jget 'd["id"]')
check "aynı adres ikinci kez" "$(code POST /mail/domains/$D/mailboxes -d "{\"local\":\"ali\",\"password\":\"$MAIL_PASS\"}")" 409
check "takma ad" "$(code POST /mail/domains/$D/aliases -d '{"local":"bilgi","destinations":["ali@posta.test","veli@posta.test"]}')" 200
check "kutuyla çakışan takma ad" "$(code POST /mail/domains/$D/aliases -d '{"local":"ali","destinations":["veli@posta.test"]}')" 409
check "parola özeti dışarı verilmiyor" "$(api GET /mail/domains/$D | grep -c argon2)" 0

say "Gönderim ve teslimat"
check "oturum" "$(mf login ali@posta.test)" ok
check "yanlış parola" "$(MAIL_PASS=yanlis-parola-123 mf login ali@posta.test)" fail
check "kimlikli gönderim (587)" "$(mf send ali@posta.test ali@posta.test veli@posta.test tek-$T)" sent
check "takma ada gönderim (465)" "$(mf send ali@posta.test ali@posta.test bilgi@posta.test grup-$T 465)" sent
check "takma ad adresinden gönderim" "$(mf send ali@posta.test bilgi@posta.test veli@posta.test takmadan-$T)" sent
has "başkası adına gönderim reddedilir" "$(mf send ali@posta.test veli@posta.test ali@posta.test sahte-$T)" "rejected"
has "kimliksiz gönderim reddedilir" "$(mf send - ali@posta.test veli@posta.test kimliksiz-$T)" "rejected"
# Yerel adres güvenilir ağ sayılır; dışarıdan gelen bağlantıyı sınamak için sunucunun ağ adresi kullanılır.
has "açık aktarma reddedilir (25)" "$(MAIL_HOST=$(hostname -I | awk '{print $1}') mf send - ali@posta.test biri@disarisi.example aktarma-$T 25)" "rejected"
has "dışarıdan yerel kutuya teslim (25)" "$(MAIL_HOST=$(hostname -I | awk '{print $1}') mf send - biri@disarisi.example veli@posta.test disaridan-$T 25)" "sent"
sleep 4
V=$(mf list veli@posta.test); A=$(mf list ali@posta.test)
has "veli tek iletiyi aldı" "$V" "tek-$T"
has "veli grup iletisini aldı" "$V" "grup-$T"
has "ali grup iletisini aldı" "$A" "grup-$T"
hasnt "sahte ileti teslim edilmedi" "$A" "sahte-$T"
has "dışarıdan gelen ileti teslim edildi" "$V" "disaridan-$T"

say "Filtre, otomatik yanıt, yönlendirme"
cat > /tmp/px-filters.json <<'XEOF'
{"filters":[
 {"name":"Faturalar","enabled":true,"match_all":true,"stop":true,
  "conditions":[{"field":"subject","header":"","op":"contains","value":"fatura"}],
  "actions":[{"kind":"move","value":"Faturalar"},{"kind":"flag","value":""}]},
 {"name":"Kacis denemesi","enabled":true,"match_all":false,"stop":false,
  "conditions":[{"field":"from","header":"","op":"is","value":"x\"; discard; #@y.z"},{"field":"size_over","header":"","op":"","value":"900"}],
  "actions":[{"kind":"discard","value":""}]}
]}
XEOF
check "filtre kaydı" "$(code PATCH /mail/mailboxes/$ALI -d @/tmp/px-filters.json)" 200
check "satır sonlu koşul" "$(code PATCH /mail/mailboxes/$ALI -d '{"filters":[{"name":"x","conditions":[{"field":"subject","op":"contains","value":"a\nb"}],"actions":[{"kind":"discard","value":""}]}]}')" 400
check "kök dışı klasör" "$(code PATCH /mail/mailboxes/$ALI -d '{"filters":[{"name":"x","conditions":[{"field":"subject","op":"contains","value":"a"}],"actions":[{"kind":"move","value":"../../x"}]}]}')" 400
check "kendine yönlendirme" "$(code PATCH /mail/mailboxes/$ALI -d '{"forward_to":["ali@posta.test"]}')" 400
check "otomatik yanıt" "$(code PATCH /mail/mailboxes/$VELI -d '{"auto_reply":{"enabled":true,"subject":"Tatildeyim","body":"Yakinda donerim.\nIyi gunler","start_date":"","end_date":""}}')" 200
mf send ali@posta.test ali@posta.test ali@posta.test "fatura-$T" >/dev/null
mf send ali@posta.test ali@posta.test ali@posta.test "siradan-$T" >/dev/null
mf send ali@posta.test ali@posta.test veli@posta.test "soru-$T" >/dev/null
sleep 5
has "fatura klasöre taşındı" "$(mf list ali@posta.test Faturalar)" "fatura-$T"
A=$(mf list ali@posta.test)
hasnt "fatura gelen kutusunda değil" "$A" "fatura-$T"
has "sıradan ileti gelen kutusunda (kaçış denemeli kural silmedi)" "$A" "siradan-$T"
has "otomatik yanıt geldi" "$A" "Tatildeyim"
check "kopyasız yönlendirme" "$(code PATCH /mail/mailboxes/$VELI -d '{"forward_to":["ali@posta.test"],"forward_keep":false,"auto_reply":{"enabled":false,"subject":"","body":"","start_date":"","end_date":""}}')" 200
mf send ali@posta.test ali@posta.test veli@posta.test "ilet-$T" >/dev/null; sleep 4
has "yönlendirilen ileti hedefe ulaştı" "$(mf list ali@posta.test)" "ilet-$T"
hasnt "kaynakta kopya kalmadı" "$(mf list veli@posta.test)" "ilet-$T"
check "kurallar kaldırılınca betik silinir" "$(code PATCH /mail/mailboxes/$VELI -d '{"forward_to":[]}')" 200
check "etkin betik yok" "$(ls -a /var/vmail/posta.test/veli | grep -c dovecot.sieve)" 0

say "Hepsini yakala, kota, kapatma"
check "hepsini yakala" "$(code PATCH /mail/domains/$D -d '{"catch_all":"ali@posta.test"}')" 200
mf send ali@posta.test ali@posta.test olmayan@posta.test "yakala-$T" >/dev/null
mf send ali@posta.test ali@posta.test veli@posta.test "dogrudan-$T" >/dev/null; sleep 4
has "olmayan adres hedefe düştü" "$(mf list ali@posta.test)" "yakala-$T"
has "var olan kutu kendi postasını alıyor" "$(mf list veli@posta.test)" "dogrudan-$T"
api GET /mail/domains/$D | jget '[(m["address"], m["quota_mb"], m["used_bytes"]) for m in d["mailboxes"]]'
check "kutuyu kapat" "$(code PATCH /mail/mailboxes/$VELI -d '{"enabled":false}')" 200
check "kapalı kutu oturum açamaz" "$(mf login veli@posta.test)" fail
check "parola değiştir" "$(code PATCH /mail/mailboxes/$ALI -d '{"password":"Yeni-Parola-98765"}')" 200
check "eski parola geçersiz" "$(mf login ali@posta.test)" fail
check "yeni parola geçerli" "$(MAIL_PASS=Yeni-Parola-98765 mf login ali@posta.test)" ok

say "DNS, kuyruk, ayarlar"
api GET /mail/domains/$D | jget '[(r["kind"], r["name"], r["value"][:28]) for r in d["dns"]]'
api GET /mail/domains/$D/dns | jget '[(r["name"], r["status"]) for r in d]'
api GET /mail/queue | jget '"kuyrukta", len(d), "ileti"'
check "geçersiz kuyruk kimliği" "$(code DELETE /mail/queue/ALL)" 400
S='{"hostname":"'$(hostname -f)'","spam":{"reject_score":14,"junk_score":5.5,"greylisting":false,"greylist_score":4,"whitelist":["@guvenilir.test","dost@ornek.test"],"blacklist":["spam@kotu.test"]}}'
check "spam ayarları" "$(code PUT /mail/settings -d "$S")" 200
check "eşik dosyaya yazıldı" "$(grep -c 'add_header = 5.50' /etc/rspamd/local.d/actions.conf)" 1
check "beyaz liste dosyası" "$(cat /etc/pixpanel/mail/rspamd/whitelist_domains.map)" "guvenilir.test"
check "ters eşikler" "$(code PUT /mail/settings -d "${S/\"junk_score\":5.5/\"junk_score\":50}")" 400
check "bozuk liste kaydı" "$(code PUT /mail/settings -d "${S/dost@ornek.test/a b; c}")" 400
sleep 3
api GET /mail/activity | jget '"taranan:", d["scanned"], "son ileti sayısı:", len(d["messages"])'
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/mail/settings)" 401

say "Temizlik"
check "alan adını postalarıyla sil" "$(code DELETE "/mail/domains/$D?delete_mail=true")" 200
check "posta klasörü silindi" "$(ls /var/vmail | grep -c posta.test)" 0
check "eşleme dosyaları boşaldı" "$(cat /etc/pixpanel/mail/domains /etc/pixpanel/mail/dovecot-users | grep -c posta.test)" 0
