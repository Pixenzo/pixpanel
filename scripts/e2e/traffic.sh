#!/usr/bin/env bash
# Aylık trafik sınırı ve bayinin toplam kullanımı sınaması. Test makinesinde root olarak çalışır.
# Kapsam: paket alanlarının doğrulaması, %80 / %100 eşikleri (bildirimin ayda bir kez gitmesi),
# yalnız bildirim ve askıya alma davranışı, ay değişince / sınır yükselince / paket değişince
# kendiliğinden geri açılma, yöneticinin elle askıya aldığı sitenin asla açılmaması, bayinin
# müşterilerinin toplam kullanımı. Kendi oluşturduğu her şeyi siler.
# Kullanım: PX_USER=... PX_PASS=... bash traffic.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-trf
JC=/tmp/pxjar-trf-customer
JR=/tmp/pxjar-trf-reseller
CUSTOMER=trfmusteri
CUSTOMER2=trfmusteri2
RESELLER=trfbayi
PASS='Trafik-Parola-13579'
DB=/var/lib/pixpanel/pixpanel.db
GB=1073741824
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
PASSED=0
FAILED=0
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; PASSED=$((PASSED + 1)); else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; FAILED=$((FAILED + 1)); fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; PASSED=$((PASSED + 1)); else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 300)]"; FAILED=$((FAILED + 1)); fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
sql() { python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1], timeout=10); c.execute(sys.argv[2]); c.commit()' $DB "$1"; }
# Tek değerli sorgu.
query() { python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1], timeout=10); r=c.execute(sys.argv[2]).fetchone(); print("" if r is None else r[0])' $DB "$1"; }
# Komut beklenen çıktıyı verene kadar (en çok ~20 sn) yeniden denenir; son çıktıyı yazar.
until_eq() { local want=$1 got=; shift; for _ in $(seq 1 40); do got=$("$@" 2>/dev/null); [ "$got" = "$want" ] && break; sleep 0.5; done; echo "$got"; }

TODAY=$(date -u +%Y%m%d)
THIS_MONTH=$(date -u +%Y%m)

# Siteye bu ay için trafik yazar (bayt); eskisini ezer.
set_traffic() { sql "INSERT OR REPLACE INTO site_stats_daily (site_id, day, bytes) VALUES ($1, $TODAY, $2)"; }
# Paketi aynı gövdeyle güncelleyip denetim turunu tetikler (güncelleme turu arka planda başlatır).
body() { echo "{\"name\":\"e2e-trf\",\"limits\":{\"sites\":10,\"traffic_gb\":$1,\"traffic_action\":\"$2\"}}"; }
state_level() { query "SELECT warned_level FROM traffic_state WHERE user_id = $CID"; }
site_status() { api GET /sites | jget "next(s['status'] for s in d if s['id']==$1)"; }
two_status() { echo "$(site_status $1) $(site_status $2)"; }
mock_count() { grep -c -- "$1" /tmp/px-mock.log 2>/dev/null || true; }

cleanup() {
  for name in $CUSTOMER $CUSTOMER2 $RESELLER; do
    local id; id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')")
    [ -z "$id" ] && continue
    for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['owner']=='$name')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
    api DELETE /users/$id >/dev/null
  done
  for package in $(api GET /packages | jget "' '.join(str(p['id']) for p in d if p['name'].startswith('e2e-trf'))"); do api DELETE /packages/$package >/dev/null; done
}

rm -f $J $JC $JR
login $J "$PX_USER" "$PX_PASS" >/dev/null
cleanup

say "Paket alanlarının doğrulaması"
for limits in '{"traffic_gb":0}' '{"traffic_gb":-5}' '{"traffic_gb":1000001}' '{"traffic_gb":10,"traffic_action":"delete"}'; do
  check "geçersiz $limits" "$(code POST /packages -d "{\"name\":\"e2e-trf-bozuk\",\"limits\":$limits}")" 400
done
check "yalnız davranış alanı geçerli değerlerle kabul edilir" "$(code POST /packages -d '{"name":"e2e-trf-uc","limits":{"traffic_gb":1,"traffic_action":"notify"}}')" 200
check "üst uç geçerli" "$(code POST /packages -d '{"name":"e2e-trf-uc2","limits":{"traffic_gb":1000000,"traffic_action":"suspend"}}')" 200
PK=$(api POST /packages -d "$(body 10 suspend)" | jget 'd["id"]')
check "paket alanları kaydedildi" "$(api GET /packages | jget "next((p['limits']['traffic_gb'], p['limits']['traffic_action']) for p in d if p['id']==$PK)")" "(10, 'suspend')"

say "Hesap ve siteler"
check "müşteri" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":$PK}")" 200
CID=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
check "müşteri girişi" "$(login $JC $CUSTOMER "$PASS")" 200
S1=$(api POST /sites -d "{\"domain\":\"trf-bir.test\",\"kind\":\"static\",\"user_id\":$CID}" | jget 'd["id"]')
S2=$(api POST /sites -d "{\"domain\":\"trf-iki.test\",\"kind\":\"static\",\"user_id\":$CID}" | jget 'd["id"]')
S3=$(api POST /sites -d "{\"domain\":\"trf-manuel.test\",\"kind\":\"static\",\"user_id\":$CID}" | jget 'd["id"]')
check "üç site açıldı" "$([[ "$S1" =~ ^[0-9]+$ && "$S2" =~ ^[0-9]+$ && "$S3" =~ ^[0-9]+$ ]] && echo evet)" evet
check "yönetici bir siteyi elle askıya alır" "$(code PATCH /sites/$S3 -d '{"status":"suspended"}')" 200

# Müşterinin bildirim kanalı (sahte Telegram sunucusu yalnız sınama ortamında vardır).
MOCKED=$(grep -c 'telegram_api = "http://127.0.0.1:9911"' /etc/pixpanel/config.toml 2>/dev/null)
call $JC PUT /notifications -d '{"email_enabled":false,"email_to":"","telegram_enabled":true,"telegram_token":"123456:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi","telegram_chat_id":"-100123","events":["traffic_warning"]}' >/dev/null
check "bildirim olayı tanınır ve kaydedilir" "$(call $JC GET /notifications | jget "('traffic_warning' in d['events'], any(e['id']=='traffic_warning' for e in d['available_events']))")" "(True, True)"
: > /tmp/px-mock.log 2>/dev/null

say "Kullanım görünümü"
set_traffic $S1 $((2 * GB))
U=$(call $JC GET /packages/usage)
check "müşteri bu ayki trafiği ve sınırı görür" "$(jget '(d["traffic_bytes"], d["limits"]["traffic_gb"], d["limits"]["traffic_action"], d["traffic_suspended_sites"])' <<<"$U")" "($((2 * GB)), 10, 'suspend', 0)"
check "yönetici de görür" "$(api GET /users/$CID/usage | jget 'd["traffic_bytes"]')" $((2 * GB))
check "müşteri bayi özeti görmez" "$(jget 'd["customers"]' <<<"$U")" None

say "%80 eşiği"
set_traffic $S1 $((8 * GB))
api PATCH /packages/$PK -d "$(body 10 suspend)" >/dev/null
check "düzey 1 kaydedilir" "$(until_eq "1" state_level)" 1
check "%80'de site askıya alınmaz" "$(site_status $S1) $(site_status $S2)" "active active"
if [ "$MOCKED" = 1 ]; then
  check "%80 bildirimi bir kez gitti" "$(sleep 2; mock_count 'aylık trafik sınırının %80')" 1
fi
api PATCH /packages/$PK -d "$(body 10 suspend)" >/dev/null
sleep 3
check "yeniden denetimde düzey değişmez" "$(state_level)" 1
if [ "$MOCKED" = 1 ]; then
  check "%80 bildirimi yinelenmez" "$(mock_count 'aylık trafik sınırının %80')" 1
fi

say "%100 eşiği: askıya alma"
set_traffic $S2 $((3 * GB))
api PATCH /packages/$PK -d "$(body 10 suspend)" >/dev/null
check "düzey 2 kaydedilir" "$(until_eq "2" state_level)" 2
check "etkin siteler askıya alındı" "$(until_eq "suspended suspended" two_status $S1 $S2)" "suspended suspended"
check "askıya alınan siteler işaretlendi" "$(query "SELECT COUNT(*) FROM traffic_suspensions WHERE user_id = $CID")" 2
check "elle askıya alınan site işaretlenmedi" "$(query "SELECT COUNT(*) FROM traffic_suspensions WHERE site_id = $S3")" 0
check "kullanım görünümü askıdaki siteleri söyler" "$(call $JC GET /packages/usage | jget 'd["traffic_suspended_sites"]')" 2
check "nginx askıdaki siteye bakım yanıtı verir" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Host: trf-bir.test' http://127.0.0.1/)" 503
check "denetim günlüğüne yazıldı" "$(query "SELECT COUNT(*) > 0 FROM audit_log WHERE action = 'account.traffic_suspend' AND target = '$CUSTOMER'")" 1
if [ "$MOCKED" = 1 ]; then
  check "%100 bildirimi bir kez gitti" "$(sleep 2; mock_count 'aylık trafik sınırı doldu')" 1
fi
api PATCH /packages/$PK -d "$(body 10 suspend)" >/dev/null
sleep 3
if [ "$MOCKED" = 1 ]; then
  check "%100 bildirimi yinelenmez" "$(mock_count 'aylık trafik sınırı doldu')" 1
fi
check "müşteri kendi askıdaki sitesini açamaz" "$(status $JC PATCH /sites/$S1 -d '{"status":"active"}')" 403

say "Sınır yükselince geri açılma"
api PATCH /packages/$PK -d "$(body 100 suspend)" >/dev/null
check "siteler açıldı" "$(until_eq "active active" two_status $S1 $S2)" "active active"
check "işaretler temizlendi" "$(query "SELECT COUNT(*) FROM traffic_suspensions WHERE user_id = $CID")" 0
check "elle askıya alınan site açılmadı" "$(site_status $S3)" suspended
check "geri açma denetim günlüğüne yazıldı" "$(query "SELECT COUNT(*) > 0 FROM audit_log WHERE action = 'account.traffic_resume' AND target = '$CUSTOMER'")" 1

say "Yalnız bildirim davranışı"
set_traffic $S1 $((120 * GB))
api PATCH /packages/$PK -d "$(body 100 notify)" >/dev/null
check "düzey 2 kaydedilir" "$(until_eq "2" state_level)" 2
sleep 3
check "yalnız bildirimde site askıya alınmaz" "$(site_status $S1) $(site_status $S2)" "active active"

say "Paket değişince geri açılma"
api PATCH /packages/$PK -d "$(body 100 suspend)" >/dev/null
check "askıya alma davranışında siteler askıya alınır" "$(until_eq "suspended" site_status $S1)" suspended
check "paketi kaldır" "$(code PATCH /users/$CID -d '{"package_id":0}')" 200
check "paket kalkınca site açılır" "$(until_eq "active" site_status $S1)" active
check "elle askıya alınan site yine açılmadı" "$(site_status $S3)" suspended
check "paketi geri ver" "$(code PATCH /users/$CID -d "{\"package_id\":$PK}")" 200
check "paket geri gelince (sınır hâlâ aşık) yeniden askıya alınır" "$(until_eq "suspended" site_status $S1)" suspended

say "Ay değişimi"
# Sayıları geçen aya taşıyarak ay başını taklit eder: bu ayın trafiği sıfırlanır.
sql "UPDATE site_stats_daily SET day = day - 100 WHERE site_id IN ($S1, $S2)"
# Geçen ay sayıları bu ayda sayılmaz; kayıtlı düzey de eski aya ait gibi davranır.
sql "UPDATE traffic_state SET month = month - 1 WHERE user_id = $CID"
api PATCH /packages/$PK -d "$(body 100 suspend)" >/dev/null
check "ay başında siteler açılır" "$(until_eq "active" site_status $S1)" active
check "ikinci site de açılır" "$(site_status $S2)" active
check "bu ayın trafiği sıfırlandı" "$(call $JC GET /packages/usage | jget 'd["traffic_bytes"]')" 0
check "kayıtlı düzey yeni aya geçti" "$(query "SELECT month || ':' || warned_level FROM traffic_state WHERE user_id = $CID")" "$THIS_MONTH:0"
check "elle askıya alınan site ay başında da açılmadı" "$(site_status $S3)" suspended
if [ "$MOCKED" = 1 ]; then
  set_traffic $S1 $((85 * GB))
  api PATCH /packages/$PK -d "$(body 100 suspend)" >/dev/null
  until_eq "1" state_level >/dev/null
  check "yeni ayda %80 bildirimi yeniden gider" "$(sleep 2; mock_count 'aylık trafik sınırının %80')" 2
fi
sql "DELETE FROM site_stats_daily WHERE site_id IN ($S1, $S2, $S3)"

say "Elle askıya alma kaydı geçersiz kılar"
set_traffic $S1 $((150 * GB))
api PATCH /packages/$PK -d "$(body 100 suspend)" >/dev/null
check "site trafik yüzünden askıya alındı" "$(until_eq "suspended" site_status $S1)" suspended
sleep 2
check "işaretli" "$(query "SELECT COUNT(*) FROM traffic_suspensions WHERE site_id = $S1")" 1
check "yönetici aynı siteyi elle askıya alır" "$(code PATCH /sites/$S1 -d '{"status":"suspended"}')" 200
check "işaret silindi" "$(query "SELECT COUNT(*) FROM traffic_suspensions WHERE site_id = $S1")" 0
api PATCH /packages/$PK -d "$(body 1000 suspend)" >/dev/null
sleep 4
check "sınır yükselse de elle askıya alınan site açılmaz" "$(site_status $S1)" suspended
sql "DELETE FROM site_stats_daily WHERE site_id IN ($S1, $S2, $S3)"

say "Bayinin toplam kullanımı"
BIG=$(api POST /packages -d '{"name":"e2e-trf-bayi","limits":{"sites":20,"databases":10,"mailboxes":50,"disk_mb":10000,"traffic_gb":500}}' | jget 'd["id"]')
check "bayi" "$(code POST /users -d "{\"username\":\"$RESELLER\",\"password\":\"$PASS\",\"role\":\"reseller\",\"package_id\":$BIG}")" 200
RID=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$RESELLER')")
check "bayi girişi" "$(login $JR $RESELLER "$PASS")" 200
# Bayinin müşterisi: bayi açar (iki müşteri), her birine site ve veritabanı.
# Paketi olan bayi müşterisini paketsiz açamaz (paketsiz hesap sınırsızdır); kendi paketinden pay verir.
check "bayi müşteriyi paketsiz açamaz" "$(status $JR POST /users -d "{\"username\":\"$CUSTOMER2\",\"password\":\"$PASS\",\"role\":\"user\"}")" 400
RPK=$(call $JR POST /packages -d '{"name":"e2e-trf-bayi-musteri","limits":{"sites":10,"databases":5,"mailboxes":20,"disk_mb":5000,"traffic_gb":100}}' | jget 'd["id"]')
check "bayi müşteri açar" "$(status $JR POST /users -d "{\"username\":\"$CUSTOMER2\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":$RPK}")" 200
C2=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER2')")
R1=$(api POST /sites -d "{\"domain\":\"trf-bayi1.test\",\"kind\":\"static\",\"user_id\":$C2}" | jget 'd["id"]')
R2=$(api POST /sites -d "{\"domain\":\"trf-bayi2.test\",\"kind\":\"static\",\"user_id\":$C2}" | jget 'd["id"]')
# Gerçek veritabanı açmaya gerek yok: sayım kayıt satırından yapılır.
sql "INSERT INTO databases (user_id, engine, name, created_at) VALUES ($C2, 'mariadb', '${CUSTOMER2}_trf', strftime('%s','now'))"
set_traffic $R1 $((3 * GB))
set_traffic $R2 $((1 * GB))
sql "INSERT INTO account_usage (user_id, disk_bytes, measured_at) VALUES ($C2, 5368709120, strftime('%s','now')) ON CONFLICT(user_id) DO UPDATE SET disk_bytes = excluded.disk_bytes"
# $CUSTOMER bayinin müşterisi değil (yönetici açtı); toplama girmemeli.
check "bayi kendi toplamını görür" "$(call $JR GET /packages/usage | jget '(d["customers"]["accounts"], d["customers"]["sites"], d["customers"]["databases"], d["customers"]["disk_bytes"], d["customers"]["traffic_bytes"], d["limits"]["traffic_gb"])')" "(1, 2, 1, 5368709120, $((4 * GB)), 500)"
check "yönetici bayinin toplamını kullanım ucundan görür" "$(api GET /users/$RID/usage | jget '(d["customers"]["sites"], d["customers"]["traffic_bytes"])')" "(2, $((4 * GB)))"
check "yönetici listesinde bayi satırında özet" "$(api GET /users | jget "next((u['customer_totals']['accounts'], u['customer_totals']['sites'], u['customer_totals']['mailboxes']) for u in d if u['id']==$RID)")" "(1, 2, 0)"
check "müşteri satırında özet yok" "$(api GET /users | jget "next(u.get('customer_totals') for u in d if u['id']==$C2)")" None
check "bayi listesinde yalnız müşteriler, özet yok" "$(call $JR GET /users | jget "[u.get('customer_totals') for u in d]")" "[None]"
check "yönetici hesabında bayi özeti yok" "$(api GET /packages/usage | jget 'd["customers"]')" None
# Bayinin kendi sitesinin trafiği müşteri toplamına karışmaz.
RS=$(call $JR POST /sites -d '{"domain":"trf-bayi-kendi.test","kind":"static"}' | jget 'd["id"]')
set_traffic $RS $((7 * GB))
U=$(call $JR GET /packages/usage)
check "bayinin kendi trafiği müşteri toplamında değil" "$(jget '(d["traffic_bytes"], d["customers"]["traffic_bytes"])' <<<"$U")" "($((7 * GB)), $((4 * GB)))"
check "başka bayinin müşterisi bu toplama girmez" "$(jget 'd["customers"]["accounts"]' <<<"$U")" 1

say "Temizlik"
sql "DELETE FROM site_stats_daily WHERE site_id IN ($S1, $S2, $S3, $R1, $R2, $RS)"
sql "DELETE FROM databases WHERE user_id = $C2"
cleanup
check "hesaplar ve paketler silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$CUSTOMER', '$CUSTOMER2', '$RESELLER')]") $(api GET /packages | jget "[p['name'] for p in d if p['name'].startswith('e2e-trf')]")" "[] []"
check "trafik kayıtları kalmadı" "$(query "SELECT COUNT(*) FROM traffic_state WHERE user_id IN ($CID, $C2, $RID)") $(query "SELECT COUNT(*) FROM traffic_suspensions WHERE user_id IN ($CID, $C2)")" "0 0"
echo "bitti: $PASSED geçti, $FAILED kaldı"
