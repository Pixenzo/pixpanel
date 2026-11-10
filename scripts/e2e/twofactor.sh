#!/usr/bin/env bash
# İki adımlı doğrulamanın sınaması. Test makinesinde çalışır; geçici bir hesap
# açar ve iş bitince siler (yönetici hesabının ayarlarına dokunmaz).
# Kullanım: PX_USER=... PX_PASS=... bash twofactor.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
ADMIN=/tmp/pxjar-2fa-admin
J=/tmp/pxjar-2fa-user
U=ikiadim
UP='Iki-Adim-Parola-2468'
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b "$jar" -c "$jar" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b "$jar" -c "$jar" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
# Gizli anahtardan o anki (ya da verilen kaydırmayla) kodu hesaplar.
totp() { python3 -c "
import base64,hmac,hashlib,struct,sys,time
key=base64.b32decode(sys.argv[1]); step=int(time.time())//30+int(sys.argv[2])
d=hmac.new(key,struct.pack('>Q',step),hashlib.sha1).digest(); o=d[19]&15
print('%06d' % ((struct.unpack('>I',d[o:o+4])[0]&0x7fffffff)%1000000))" "$1" "${2:-0}"; }
login() { rm -f $J; code $J POST /auth/login -d "{\"username\":\"$U\",\"password\":\"$UP\"${1:+,\"code\":\"$1\"}}"; }

rm -f $ADMIN $J
call $ADMIN POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
OLD=$(call $ADMIN GET /users | jget "next((u['id'] for u in d if u['username']=='$U'), '')")
[ -n "$OLD" ] && call $ADMIN DELETE /users/$OLD >/dev/null
call $ADMIN POST /users -d "{\"username\":\"$U\",\"password\":\"$UP\",\"role\":\"user\",\"email\":\"\"}" >/dev/null
UID_=$(call $ADMIN GET /users | jget "next(u['id'] for u in d if u['username']=='$U')")

say "Kurulum"
check "ikinci adım yokken giriş" "$(login)" 200
check "başlangıçta kapalı" "$(call $J GET /auth/2fa | jget 'd["enabled"]')" False
check "yanlış parolayla kurulum" "$(code $J POST /auth/2fa/setup -d '{"password":"yanlis-parola-00"}')" 400
S=$(call $J POST /auth/2fa/setup -d "{\"password\":\"$UP\"}")
SECRET=$(echo "$S" | jget 'd["secret"]')
echo "$S" | jget '("adres:", d["uri"][:44], "QR:", d["qr_svg"].startswith("<?xml") or d["qr_svg"].startswith("<svg"))'
check "kod doğrulanmadan açılmaz" "$(call $J GET /auth/2fa | jget 'd["enabled"]')" False
check "yanlış kodla açma" "$(code $J POST /auth/2fa/enable -d '{"code":"000000"}')" 400
R=$(call $J POST /auth/2fa/enable -d "{\"code\":\"$(totp "$SECRET")\"}")
RC1=$(echo "$R" | jget 'd["recovery_codes"][0]'); RC2=$(echo "$R" | jget 'd["recovery_codes"][1]')
check "kurtarma kodları verildi" "$(echo "$R" | jget 'len(d["recovery_codes"])')" 10
check "açıldı" "$(call $J GET /auth/2fa | jget '(d["enabled"], d["recovery_codes_left"])')" "(True, 10)"
check "kullanıcı listesinde görünüyor" "$(call $ADMIN GET /users | jget "next(u['two_factor'] for u in d if u['username']=='$U')")" True
check "kurtarma kodları veritabanında açık durmuyor" "$(python3 -c "
import sqlite3; r=sqlite3.connect('/var/lib/pixpanel/pixpanel.db').execute(\"SELECT recovery_codes FROM users WHERE username='$U'\").fetchone()[0]; print('$RC1' in r)")" False

say "Giriş"
check "kodsuz giriş reddedilir" "$(login)" 401
check "kod istendiği bildirilir" "$(rm -f $J; call $J POST /auth/login -d "{\"username\":\"$U\",\"password\":\"$UP\"}" | jget 'd["error"]["code"]')" two_factor_required
check "yanlış kod" "$(login 123456)" 400
check "yanlış parola + doğru kod" "$(rm -f $J; code $J POST /auth/login -d "{\"username\":\"$U\",\"password\":\"yanlis-parola-00\",\"code\":\"$(totp "$SECRET" 1)\"}")" 400
C=$(totp "$SECRET" 1)
check "doğru kodla giriş" "$(login "$C")" 200
check "aynı kod ikinci kez kullanılamaz" "$(login "$C")" 400
check "kurtarma koduyla giriş" "$(login "$RC1")" 200
check "kurtarma kodu tek kullanımlık" "$(login "$RC1")" 400
check "kalan kurtarma kodu sayısı" "$(login "$RC2" >/dev/null; call $J GET /auth/2fa | jget 'd["recovery_codes_left"]')" 8
TOKEN=$(call $J POST /auth/tokens -d '{"name":"deneme"}' | jget 'd["token"]')
check "API anahtarı ikinci adımdan etkilenmez" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $TOKEN" $B/auth/me)" 200
check "API anahtarı korumayı kapatamaz" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' $B/auth/2fa/disable -d "{\"password\":\"$UP\"}")" 403

say "Kapatma ve sıfırlama"
check "yanlış parolayla kapatma" "$(code $J POST /auth/2fa/disable -d '{"password":"yanlis-parola-00"}')" 400
check "yönetici sıfırlar" "$(code $ADMIN POST /users/$UID_/2fa/reset)" 200
check "sıfırlama oturumları kapatır" "$(code $J GET /auth/me)" 401
check "sıfırlandıktan sonra kodsuz giriş" "$(login)" 200
S=$(call $J POST /auth/2fa/setup -d "{\"password\":\"$UP\"}"); SECRET=$(echo "$S" | jget 'd["secret"]')
call $J POST /auth/2fa/enable -d "{\"code\":\"$(totp "$SECRET")\"}" >/dev/null
check "komut satırından kapatma" "$(pixpanel disable-two-factor $U >/dev/null 2>&1; login)" 200
check "olmayan kullanıcıda komut hata verir" "$(pixpanel disable-two-factor yok-boyle-biri >/dev/null 2>&1; echo $?)" 1

call $ADMIN DELETE /users/$UID_ >/dev/null
echo "bitti"
