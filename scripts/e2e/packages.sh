#!/usr/bin/env bash
# Hizmet paketleri ve kotaların sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash packages.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-pkg
JC=/tmp/pxjar-pkg-customer
JR=/tmp/pxjar-pkg-reseller
CUSTOMER=pkgmusteri
RESELLER=pkgbayi
PASS='Paket-Parola-13579'
DB=/var/lib/pixpanel/pixpanel.db
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
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
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
sql() { python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1], timeout=10); c.execute(sys.argv[2]); c.commit()' $DB "$1"; }

# Önceki koşudan kalanları temizler: hesabın kaynakları, hesap ve sınama paketleri.
cleanup() {
  for name in $CUSTOMER $RESELLER; do
    local id; id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')")
    [ -z "$id" ] && continue
    for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['owner']=='$name')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
    for database in $(api GET /databases | jget "' '.join(str(x['id']) for x in d['databases'] if x['owner']=='$name')"); do api DELETE /databases/$database >/dev/null; done
    for zone in $(api GET /dns | jget "' '.join(str(z['id']) for z in d['zones'] if z['domain'].startswith('paket-'))"); do api DELETE /dns/zones/$zone >/dev/null; done
    api DELETE /users/$id >/dev/null
  done
  for package in $(api GET /packages | jget "' '.join(str(p['id']) for p in d if p['name'].startswith('e2e-'))"); do api DELETE /packages/$package >/dev/null; done
}

rm -f $J $JC $JR
login $J "$PX_USER" "$PX_PASS" >/dev/null
cleanup

say "Paket tanımlama"
check "negatif sınır" "$(code POST /packages -d '{"name":"e2e-bozuk","limits":{"sites":-1}}')" 400
check "adsız paket" "$(code POST /packages -d '{"name":"  ","limits":{}}')" 400
MINI=$(api POST /packages -d '{"name":"e2e-mini","limits":{"sites":1,"databases":1,"mail_domains":0,"dns_zones":1,"disk_mb":1000}}' | jget 'd["id"]')
check "paket oluştu" "$(api GET /packages | jget "next((p['limits']['sites'], p['limits']['mailboxes'], p['accounts']) for p in d if p['id']==$MINI)")" "(1, None, 0)"
check "aynı adla ikinci paket" "$(code POST /packages -d '{"name":"e2e-mini","limits":{}}')" 409
check "güncelle" "$(code PATCH /packages/$MINI -d '{"name":"e2e-mini","limits":{"sites":1,"databases":1,"mail_domains":0,"dns_zones":1,"disk_mb":2000}}')" 200
check "olmayan paket" "$(code PATCH /packages/999999 -d '{"name":"x","limits":{}}')" 404

say "Hesaba paket atama"
check "olmayan paketle hesap" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":999999}")" 400
check "paketli müşteri" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":$MINI}")" 200
CID=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
check "hesap listesinde paket adı" "$(api GET /users | jget "next(u['package_name'] for u in d if u['id']==$CID)")" e2e-mini
check "kullanılan paket silinemez" "$(code DELETE /packages/$MINI)" 409
check "yöneticiye paket atanmaz" "$(code PATCH /users/1 -d "{\"package_id\":$MINI}")" 403
check "müşteri girişi" "$(login $JC $CUSTOMER "$PASS")" 200

say "Sınırların uygulanması"
check "müşteri paket tanımlayamaz" "$(status $JC POST /packages -d '{"name":"e2e-kacak","limits":{}}')" 403
check "müşteri paketleri listeleyemez" "$(status $JC GET /packages)" 403
check "müşteri kendi paketini değiştiremez" "$(status $JC PATCH /users/$CID -d '{"package_id":0}')" 404
S1=$(call $JC POST /sites -d '{"domain":"paket-bir.test","kind":"static"}' | jget 'd["id"]')
check "ilk site açılır" "$([[ "$S1" =~ ^[0-9]+$ ]] && echo evet)" evet
R=$(call $JC POST /sites -d '{"domain":"paket-iki.test","kind":"static"}')
has "ikinci site reddedilir, nedeni söylenir" "$R" "en çok 1 site"
check "yönetici de müşteri adına sınırı aşamaz" "$(code POST /sites -d "{\"domain\":\"paket-uc.test\",\"kind\":\"static\",\"user_id\":$CID}")" 400
check "ilk veritabanı" "$(status $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_bir\"}")" 200
check "ikinci veritabanı reddedilir" "$(status $JC POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"${CUSTOMER}_iki\"}")" 400
has "pakette olmayan kaynak" "$(call $JC POST /mail/domains -d '{"domain":"paket-bir.test"}')" "posta alan adı içermiyor"
check "sınırsız bırakılan alan serbest" "$(status $JC POST /sites/$S1/ftp -d "{\"name\":\"paketftp\",\"password\":\"$PASS\"}" | sed 's/200/200/')" 200
U=$(call $JC GET /packages/usage)
check "müşteri kendi kullanımını görür" "$(jget '(d["package"], d["sites"], d["limits"]["sites"], d["databases"], d["ftp_accounts"], d["limits"]["mailboxes"])' <<<"$U")" "('e2e-mini', 1, 1, 1, 1, None)"
check "yönetici müşterinin kullanımını görür" "$(api GET /users/$CID/usage | jget '(d["package"], d["sites"])')" "('e2e-mini', 1)"
check "yöneticinin sınırı yok" "$(api GET /packages/usage | jget '(d["package"], d["limits"]["sites"])')" "(None, None)"

say "Disk kotası"
# Ölçüm saatte bir yapılır; sınama için ölçüm kaydı doğrudan yazılır.
sql "INSERT INTO account_usage (user_id, disk_bytes, measured_at) VALUES ($CID, 2097152000, strftime('%s','now')) ON CONFLICT(user_id) DO UPDATE SET disk_bytes = excluded.disk_bytes"
check "kota dolunca kullanım görünür" "$(call $JC GET /packages/usage | jget 'd["disk_bytes"]')" 2097152000
has "kota dolunca yeni kaynak açılamaz" "$(call $JC POST /dns/zones -d '{"domain":"paket-bir.test","provider":"local"}')" "Disk kotası doldu"
sql "UPDATE account_usage SET disk_bytes = 1048576 WHERE user_id = $CID"
check "yer açılınca yeniden açılabilir" "$(status $JC POST /dns/zones -d '{"domain":"paket-bir.test","provider":"local"}')" 200

say "Paketi değiştirme"
check "paketi kaldır" "$(code PATCH /users/$CID -d '{"package_id":0}')" 200
check "paketsiz hesap sınırsız" "$(status $JC POST /sites -d '{"domain":"paket-iki.test","kind":"static"}')" 200
check "paketi geri ver" "$(code PATCH /users/$CID -d "{\"package_id\":$MINI}")" 200
check "sınırın üstündeki var olan kaynaklar durur, yenisi açılamaz" "$(call $JC GET /packages/usage | jget 'd["sites"]') $(status $JC POST /sites -d '{"domain":"paket-dort.test","kind":"static"}')" "2 400"

say "Bayi"
BIG=$(api POST /packages -d '{"name":"e2e-bayi","limits":{"sites":5,"databases":5,"disk_mb":5000}}' | jget 'd["id"]')
check "paketli bayi" "$(code POST /users -d "{\"username\":\"$RESELLER\",\"password\":\"$PASS\",\"role\":\"reseller\",\"package_id\":$BIG}")" 200
check "bayi girişi" "$(login $JR $RESELLER "$PASS")" 200
check "bayi yöneticinin paketlerini görmez" "$(call $JR GET /packages | jget 'len(d)')" 0
has "bayi kendi sınırını aşan paket tanımlayamaz" "$(call $JR POST /packages -d '{"name":"e2e-genis","limits":{"sites":10,"databases":1,"disk_mb":100}}')" "site sınırı sizin paketinizin sınırını aşıyor"
has "sınırsız alan da aşım sayılır" "$(call $JR POST /packages -d '{"name":"e2e-genis","limits":{"sites":1,"databases":1}}')" "disk sınırı"
RP=$(call $JR POST /packages -d '{"name":"e2e-kucuk","limits":{"sites":2,"databases":1,"disk_mb":500}}' | jget 'd["id"]')
check "bayi kendi sınırları içinde paket tanımlar" "$([[ "$RP" =~ ^[0-9]+$ ]] && echo evet)" evet
check "bayi yöneticinin paketini atayamaz" "$(status $JR POST /users -d "{\"username\":\"pkgalt\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":$MINI}")" 400
check "bayi başkasının paketini değiştiremez" "$(status $JR PATCH /packages/$MINI -d '{"name":"e2e-mini","limits":{}}')" 404
check "yönetici bayinin paketini görür" "$(api GET /packages | jget "any(p['id']==$RP for p in d)")" True

say "Temizlik"
cleanup
check "hesaplar ve paketler silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$CUSTOMER', '$RESELLER')]") $(api GET /packages | jget "[p['name'] for p in d if p['name'].startswith('e2e-')]")" "[] []"
echo "bitti"
