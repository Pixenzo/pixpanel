#!/usr/bin/env bash
# "Müşteri olarak giriş" sınaması: yönetici ya da bayi, yönettiği hesabın panelini onun
# gözünden görür ve kendi hesabına döner.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash impersonate.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-imp
JR=/tmp/pxjar-imp-r
JC=/tmp/pxjar-imp-c
CUSTOMER=girismusteri
RESELLER=girisbayi
OTHER=girisbaska
PASS='Giris-Sinama-Parolasi-1'
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
user_id() { call $1 GET /users | jget "next((u['id'] for u in d if u['username']=='$2'), '')"; }
login() { curl -sk -o /dev/null -w '%{http_code}' -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
whoami() { call $1 GET /auth/me | jget '(d["username"], d["role"], d["impersonated_by"])'; }
admin_login() { rm -f $J; login $J "$PX_USER" "$PX_PASS" >/dev/null; }

admin_login
for name in $CUSTOMER $OTHER $RESELLER; do id=$(user_id $J $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done

say "Hazırlık"
check "müşteri" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
check "bayi" "$(code POST /users -d "{\"username\":\"$RESELLER\",\"password\":\"$PASS\",\"role\":\"reseller\"}")" 200
login $JR $RESELLER "$PASS" >/dev/null
check "bayinin müşterisi" "$(status $JR POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
C=$(user_id $J $CUSTOMER); R=$(user_id $J $RESELLER); O=$(user_id $J $OTHER)
check "normal oturumda başkası adına değil" "$(whoami $J)" "('$PX_USER', 'admin', None)"

say "Yönetici müşteri olarak girer"
check "olmayan hesap" "$(code POST /users/999999/impersonate)" 404
check "kendi hesabı" "$(code POST /users/$(user_id $J "$PX_USER")/impersonate)" 400
OLD=$(grep pixpanel $J | awk '{print $NF}')
check "müşteri hesabına gir" "$(code POST /users/$C/impersonate)" 200
check "artık müşterinin gözünden" "$(whoami $J)" "('$CUSTOMER', 'user', '$PX_USER')"
check "yöneticinin eski oturumu kapandı" "$(curl -sk -o /dev/null -w '%{http_code}' -H "cookie: $(grep pixpanel $J | awk '{print $(NF-1)}')=$OLD" $B/auth/me)" 401
check "yönetici uçları kapalı" "$(code GET /system/overview) $(code GET /health)" "403 403"
check "müşterinin kaynaklarını müşteri gibi açar" "$(api POST /sites -d '{"domain":"giris-sinamasi.test","kind":"static"}' | jget 'd["owner"]')" $CUSTOMER
has "denetim kaydında kimin yaptığı da yazar" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT username FROM audit_log WHERE action='site.create' AND target='giris-sinamasi.test' ORDER BY id DESC LIMIT 1")" "$PX_USER → $CUSTOMER"
has "bu oturumdan API anahtarı üretilemez" "$(api POST /auth/tokens -d '{"name":"arka-kapi","read_only":false,"allowed_ips":""}')" "API anahtarı üretilemez"
check "parolayı bilmeden parola değiştirilemez" "$(code POST /auth/password -d '{"current_password":"bilmiyorum","new_password":"Yeni-Parola-123456"}')" 400
check "iç içe geçiş yok" "$(code POST /users/$O/impersonate)" 400
check "müşterinin son giriş zamanı değişmedi" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT last_login_at IS NULL FROM users WHERE id=$C")" 1

say "Geri dönüş"
check "kendi hesabına dön" "$(code POST /auth/impersonate/stop)" 200
check "yeniden yönetici" "$(whoami $J)" "('$PX_USER', 'admin', None)"
check "yönetici uçları yeniden açık" "$(code GET /system/overview)" 200
check "normal oturumda geri dönüş ucu anlamsız" "$(code POST /auth/impersonate/stop)" 400
check "geçiş ve dönüş denetim günlüğünde" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT group_concat(action, ' ') FROM (SELECT action FROM audit_log WHERE action LIKE 'user.impersonate%' ORDER BY id DESC LIMIT 2)")" "user.impersonate.stop user.impersonate"

say "Yetki sınırları"
login $JC $CUSTOMER "$PASS" >/dev/null
check "müşteri başkasının hesabına giremez" "$(status $JC POST /users/$O/impersonate)" 404
check "müşteri yöneticinin hesabına giremez" "$(status $JC POST /users/$(user_id $J "$PX_USER")/impersonate)" 404
check "bayi kendi müşterisine girer" "$(status $JR POST /users/$O/impersonate) $(whoami $JR)" "200 ('$OTHER', 'user', '$RESELLER')"
check "bayi geri döner" "$(status $JR POST /auth/impersonate/stop) $(whoami $JR)" "200 ('$RESELLER', 'reseller', None)"
check "bayi başkasının müşterisine giremez" "$(status $JR POST /users/$C/impersonate)" 404
api POST /users -d "{\"username\":\"${CUSTOMER}yon\",\"password\":\"$PASS\",\"role\":\"admin\"}" >/dev/null
A2=$(user_id $J ${CUSTOMER}yon)
check "başka yöneticinin hesabına girilemez" "$(code POST /users/$A2/impersonate)" 400
api DELETE /users/$A2 >/dev/null
api PATCH /users/$C -d '{"status":"suspended"}' >/dev/null
check "askıdaki hesaba girilemez" "$(code POST /users/$C/impersonate)" 400
api PATCH /users/$C -d '{"status":"active"}' >/dev/null
TOKEN=$(api POST /auth/tokens -d '{"name":"giris-sinamasi","read_only":false,"allowed_ips":""}' | jget 'd["token"]')
check "API anahtarıyla başka hesaba girilemez" "$(curl -sk -o /dev/null -w '%{http_code}' -H "authorization: Bearer $TOKEN" -X POST $B/users/$C/impersonate)" 403
api DELETE "/auth/tokens/$(api GET /auth/tokens | jget "next(t['id'] for t in d if t['name']=='giris-sinamasi')")" >/dev/null

say "Oturumu açan hesap askıya alınırsa"
status $JR POST /users/$O/impersonate >/dev/null
check "bayi müşterisinin hesabında" "$(whoami $JR)" "('$OTHER', 'user', '$RESELLER')"
api PATCH /users/$R -d '{"status":"suspended"}' >/dev/null
check "bayi askıya alınınca müşteri adına oturumu da düşer" "$(status $JR GET /auth/me)" 401
api PATCH /users/$R -d '{"status":"active"}' >/dev/null

say "Temizlik"
SITE=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='giris-sinamasi.test'), '')")
check "siteyi sil" "$(code DELETE "/sites/$SITE?delete_files=true")" 200
check "hesapları sil" "$(code DELETE /users/$C) $(code DELETE /users/$O) $(code DELETE /users/$R)" "200 200 200"
rm -f $J $JR $JC
echo "bitti"
