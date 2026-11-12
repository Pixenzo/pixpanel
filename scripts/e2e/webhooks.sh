#!/usr/bin/env bash
# Olay kancaları (web kancaları) ve Git web kancası imzasının sınaması.
# Test makinesinde root olarak çalışır. Kullanım: PX_USER=... PX_PASS=... bash webhooks.sh
#
# Alıcı, makinenin LAN adresinde dinleyen yerel bir Python sunucusudur. Yerel/özel adresler
# müşteriye yasak olduğundan kancalar yönetici hesabıyla tanımlanır; müşteri hesabıyla aynı
# adresin reddedildiği ayrıca sınanır.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-wh
JC=/tmp/pxjar-wh-customer
CUSTOMER=kancamusteri
CUSTOMER_PASS='Kanca-Parola-24680'
RECV_PORT=9931
RECV_LOG=/tmp/px-wh-recv.jsonl
QUERY=/tmp/px-wh-query.py
DB=/var/lib/pixpanel/pixpanel.db
LAN=$(hostname -I | awk '{print $1}')

api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
capi() { local m=$1 p=$2; shift 2; curl -sk -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
ccode() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
# Oturumsuz istek (çerez yok).
anon() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 300)]"; fi; }
lacks() { if grep -qF -- "$3" <<<"$2"; then echo "  HATA $1: [$3] görünmemeliydi"; else echo "  OK   $1"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_until() { local t=$1; shift; for _ in $(seq 1 "$t"); do "$@" >/dev/null 2>&1 && return 0; sleep 1; done; return 1; }

# ---- Alıcı sunucu ve sorgu yardımcısı ----
cat > /tmp/px-wh-recv.py <<'PYEOF'
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = "/tmp/px-wh-recv.jsonl"


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", 0))
        body = self.rfile.read(length)
        record = {"path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()}, "body": body.decode("utf-8", "replace")}
        with open(LOG, "a") as handle:
            handle.write(json.dumps(record) + "\n")
        if self.path.startswith("/redir"):
            self.send_response(302)
            self.send_header("Location", "http://127.0.0.1:9931/redirected")
            self.send_header("content-length", "0")
            self.end_headers()
            return
        code = 500 if self.path.startswith("/fail") else 200
        self.send_response(code)
        self.send_header("content-length", "2")
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("0.0.0.0", 9931), Handler).serve_forever()
PYEOF

cat > $QUERY <<'PYEOF'
import hashlib
import hmac
import json
import sys

LOG = "/tmp/px-wh-recv.jsonl"


def records(path, event):
    out = []
    try:
        lines = open(LOG).read().splitlines()
    except FileNotFoundError:
        return out
    for line in lines:
        rec = json.loads(line)
        if rec["path"].split("?")[0] != path:
            continue
        if event and rec["headers"].get("x-pixpanel-event") != event:
            continue
        out.append(rec)
    return out


def sign(secret, body):
    return "sha256=" + hmac.new(secret.encode(), body.encode(), hashlib.sha256).hexdigest()


cmd = sys.argv[1]
path, event = sys.argv[2], sys.argv[3]
found = records(path, event)
if cmd == "count":
    print(len(found))
elif cmd == "verify":
    secret, account, target = sys.argv[4], sys.argv[5], sys.argv[6]
    if not found:
        print("kayit yok")
        sys.exit()
    rec = found[-1]
    problems = []
    headers = rec["headers"]
    if headers.get("x-pixpanel-signature") != sign(secret, rec["body"]):
        problems.append("imza yanlis")
    if headers.get("user-agent") != "PixPanel-Webhook":
        problems.append("user-agent=" + str(headers.get("user-agent")))
    if "json" not in headers.get("content-type", ""):
        problems.append("content-type")
    try:
        data = json.loads(rec["body"])
        for key in ("event", "at", "account", "target", "detail"):
            if key not in data:
                problems.append("alan yok: " + key)
        if data.get("event") != event:
            problems.append("govde olay adi: " + str(data.get("event")))
        if not isinstance(data.get("at"), int):
            problems.append("at tam sayi degil")
        if account and data.get("account") != account:
            problems.append("hesap: " + str(data.get("account")))
        if target and data.get("target") != target:
            problems.append("hedef: " + str(data.get("target")))
    except ValueError:
        problems.append("govde JSON degil")
    print("ok" if not problems else ", ".join(problems))
elif cmd == "signed-with":
    # Son kayit verilen gizli anahtarla imzalanmis mi? (evet/hayir)
    rec = found[-1] if found else None
    print("evet" if rec and rec["headers"].get("x-pixpanel-signature") == sign(sys.argv[4], rec["body"]) else "hayir")
PYEOF

# ---- Yardımcılar ----
cnt() { python3 $QUERY count "$1" "$2"; }
cnt_ge() { [ "$(cnt "$1" "$2")" -ge "$3" ]; }
mk() { api POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT$1\",\"events\":$2,\"enabled\":${3:-true}}"; }
hmac_hex() { python3 -c 'import hmac,hashlib,sys; print(hmac.new(sys.argv[1].encode(), sys.argv[2].encode(), hashlib.sha256).hexdigest())' "$1" "$2"; }
cleanup_old() {
  for id in $(api GET /webhooks | jget '" ".join(str(h["id"]) for h in d["hooks"])'); do api DELETE "/webhooks/$id" >/dev/null; done
  for name in wh-e2e.test wh-git.test; do
    id=$(api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$name'), '')")
    [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null
  done
  id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$CUSTOMER'), '')")
  [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
}

rm -f $J $JC $RECV_LOG
pkill -f /tmp/px-wh-recv.py 2>/dev/null
python3 /tmp/px-wh-recv.py &
RECV_PID=$!
sleep 1
check "alıcı sunucu ayakta" "$(curl -s -o /dev/null -w '%{http_code}' -X POST http://$LAN:$RECV_PORT/ping -d x)" 200
: > $RECV_LOG

api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
say "Hazırlık"
cleanup_old
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CUSTOMER_PASS\",\"role\":\"user\",\"email\":\"\"}")" 200
check "müşteri girişi" "$(curl -sk -o /dev/null -w '%{http_code}' -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$CUSTOMER_PASS\"}")" 200

say "Oturumsuz erişim"
check "liste" "$(anon GET /webhooks)" 401
check "oluşturma" "$(anon POST /webhooks -d '{"url":"https://8.8.8.8/x","events":["*"]}')" 401
check "güncelleme" "$(anon PUT /webhooks/1 -d '{"url":"https://8.8.8.8/x","events":["*"],"enabled":true}')" 401
check "silme" "$(anon DELETE /webhooks/1)" 401
check "sınama" "$(anon POST /webhooks/1/test)" 401
check "gizli anahtar yenileme" "$(anon POST /webhooks/1/secret)" 401
check "geçmiş" "$(anon GET /webhooks/1/deliveries)" 401

say "Adres doğrulaması (yönetici)"
BAD=(
  'http://127.0.0.1:9931/x' 'http://127.0.0.1/' 'http://[::1]/' 'http://[::1]:9931/' 'http://localhost/' 'http://localhost:9931/x'
  'http://LOCALHOST/' 'http://a.localhost/' 'file:///etc/passwd' 'ftp://example.com/x' 'gopher://127.0.0.1/' 'javascript:alert(1)'
  'http://169.254.169.254/latest/meta-data/' 'http://0.0.0.0/' 'http://0/' 'http://2130706433/' 'http://0x7f.1/' 'http://127.1/'
  'http://[::ffff:127.0.0.1]/' 'http://[fe80::1]/' 'http://224.0.0.1/' 'https://user:pass@8.8.8.8/' 'example.com/x' ''
)
for url in "${BAD[@]}"; do
  body=$(python3 -c 'import json,sys; print(json.dumps({"url": sys.argv[1], "events": ["*"], "enabled": True}))' "$url")
  check "yönetici için reddedilir: ${url:-boş}" "$(code POST /webhooks -d "$body")" 400
done
check "bilinmeyen olay türü" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x\",\"events\":[\"yok.olay\"]}")" 400
check "boş olay listesi" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x\",\"events\":[]}")" 400
check "satır sonlu olay adı" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x\",\"events\":[\"deploy_failed\\nX: y\"]}")" 400
check "hiç kanca oluşmadı" "$(api GET /webhooks | jget 'len(d["hooks"])')" 0
api GET /webhooks | jget '"olay türleri:", len(d["available_events"]), "üst sınır:", d["max_hooks"], "özel ağ:", d["private_allowed"]'

say "Kancaların oluşturulması"
A=$(mk /a '["audit.site"]');       A_ID=$(jget 'd["hook"]["id"]' <<<"$A");  A_SEC=$(jget 'd["secret"]' <<<"$A")
Bk=$(mk /b '["*"]');               B_ID=$(jget 'd["hook"]["id"]' <<<"$Bk"); B_SEC=$(jget 'd["secret"]' <<<"$Bk")
C=$(mk /c '["*"]' false);          C_ID=$(jget 'd["hook"]["id"]' <<<"$C");  C_SEC=$(jget 'd["secret"]' <<<"$C")
D=$(mk /d '["deploy_failed"]');    D_ID=$(jget 'd["hook"]["id"]' <<<"$D")
E=$(mk /e '["audit.webhook"]');    E_ID=$(jget 'd["hook"]["id"]' <<<"$E");  E_SEC=$(jget 'd["secret"]' <<<"$E")
F=$(mk /fail '["audit.webhook.secret.rotate"]'); F_ID=$(jget 'd["hook"]["id"]' <<<"$F")
G=$(api POST /webhooks -d "{\"url\":\"http://$LAN:9/kapali\",\"events\":[\"deploy_failed\"]}"); G_ID=$(jget 'd["hook"]["id"]' <<<"$G")
R=$(mk /redir '["deploy_failed"]'); R_ID=$(jget 'd["hook"]["id"]' <<<"$R")
has "gizli anahtar önekli ve yeterince uzun" "$A_SEC" "whsec_"
check "gizli anahtar uzunluğu" "$(printf %s "$A_SEC" | awk '{print (length($0) >= 30) ? "evet" : "hayir"}')" evet
check "iki kancanın anahtarı farklı" "$([ "$A_SEC" != "$B_SEC" ] && echo evet || echo hayir)" evet
LIST=$(api GET /webhooks)
check "sekiz kanca" "$(jget 'len(d["hooks"])' <<<"$LIST")" 8
lacks "liste gizli anahtarı göstermez" "$LIST" "$A_SEC"
lacks "liste gizli anahtar alanı içermez" "$LIST" '"secret"'
check "kapalı kanca kapalı kaydedildi" "$(api GET /webhooks | jget "next(h['enabled'] for h in d['hooks'] if h['id']==$C_ID)")" False
check "olay süzgeci saklandı" "$(api GET /webhooks | jget "next(h['events'] for h in d['hooks'] if h['id']==$A_ID)")" "['audit.site']"
check "'*' diğer olayları ezer" "$(api POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x\",\"events\":[\"deploy_failed\",\"*\"]}" | jget 'd["hook"]["events"]')" "['*']"
# Fazlalığı hemen sil (sekiz kanca kalsın).
EXTRA=$(api GET /webhooks | jget '[h["id"] for h in d["hooks"]][-1]'); api DELETE "/webhooks/$EXTRA" >/dev/null

say "Olay iletimi ve süzgeç"
sleep 3
: > $RECV_LOG
# Güvenli bir tetikleyici: kapalı kancayı aynı değerlerle güncellemek "audit.webhook.update" üretir.
check "tetikleyici güncelleme" "$(code PUT /webhooks/$C_ID -d "{\"url\":\"http://$LAN:$RECV_PORT/c\",\"events\":[\"*\"],\"enabled\":false}")" 200
wait_until 10 cnt_ge /b audit.webhook.update 1
sleep 2
check "'*' kancası olayı aldı" "$(cnt /b audit.webhook.update)" 1
check "alan süzgeci (audit.webhook) olayı aldı" "$(cnt /e audit.webhook.update)" 1
check "başka alan süzgeci almadı (audit.site)" "$(cnt /a '')" 0
check "başka olay süzgeci almadı (deploy_failed)" "$(cnt /d '')" 0
check "kapalı kanca almadı" "$(cnt /c '')" 0
check "imza, başlıklar ve gövde doğru (*)" "$(python3 $QUERY verify /b audit.webhook.update "$B_SEC" "$PX_USER" '')" ok
check "imza, başlıklar ve gövde doğru (alan süzgeci)" "$(python3 $QUERY verify /e audit.webhook.update "$E_SEC" "$PX_USER" '')" ok
check "başka kancanın anahtarıyla doğrulanmaz" "$(python3 $QUERY signed-with /b audit.webhook.update "$A_SEC")" hayir

: > $RECV_LOG
SITE=$(api POST /sites -d '{"domain":"wh-e2e.test","kind":"static"}' | jget 'd["id"]')
wait_until 10 cnt_ge /a audit.site.create 1
wait_until 10 cnt_ge /b audit.site.create 1
sleep 2
check "site olayı: audit.site süzgeci aldı" "$(cnt /a audit.site.create)" 1
check "site olayı: '*' kancası aldı" "$(cnt /b audit.site.create)" 1
check "site olayı: audit.webhook süzgeci almadı" "$(cnt /e audit.site.create)" 0
check "site olayı: kapalı kanca almadı" "$(cnt /c '')" 0
check "site olayı: deploy_failed süzgeci almadı" "$(cnt /d '')" 0
check "site olayı yükü (hedef = alan adı)" "$(python3 $QUERY verify /a audit.site.create "$A_SEC" "$PX_USER" wh-e2e.test)" ok
check "yükte at şu anın yakınında" "$(python3 -c "
import json,time
rec=[json.loads(l) for l in open('$RECV_LOG')]
rec=[r for r in rec if r['path']=='/a'][-1]
print('evet' if abs(json.loads(rec['body'])['at']-time.time()) < 60 else 'hayir')")" evet

say "Kapatılan kanca"
check "kancayı kapat" "$(code PUT /webhooks/$B_ID -d "{\"url\":\"http://$LAN:$RECV_PORT/b\",\"events\":[\"*\"],\"enabled\":false}")" 200
sleep 3
: > $RECV_LOG
code PUT /webhooks/$C_ID -d "{\"url\":\"http://$LAN:$RECV_PORT/c\",\"events\":[\"*\"],\"enabled\":false}" >/dev/null
wait_until 10 cnt_ge /e audit.webhook.update 1
sleep 2
check "açık kanca hâlâ alıyor" "$(cnt /e audit.webhook.update)" 1
check "kapatılan kanca artık almıyor" "$(cnt /b '')" 0

say "Başarısız alıcıda yeniden deneme (5 sn + 20 sn aralıkla en çok 3 deneme)"
: > $RECV_LOG
# Anahtar yenileme "audit.webhook.secret.rotate" üretir ve /fail kancası bunu 500 ile yanıtlar.
ROT=$(api POST /webhooks/$C_ID/secret)
C_SEC_NEW=$(jget 'd["secret"]' <<<"$ROT")
check "yeni gizli anahtar eskisinden farklı" "$([ -n "$C_SEC_NEW" ] && [ "$C_SEC_NEW" != "$C_SEC" ] && echo evet || echo hayir)" evet
wait_until 15 cnt_ge /fail audit.webhook.secret.rotate 2
check "en az iki deneme geldi" "$([ "$(cnt /fail audit.webhook.secret.rotate)" -ge 2 ] && echo evet || echo hayir)" evet
HIST=$(api GET /webhooks/$F_ID/deliveries)
check "geçmişte 500 durum kodu var" "$(jget 'sorted(set(x["status"] for x in d))' <<<"$HIST")" "[500]"
has "geçmişte hata iletisi var" "$(jget '[x["error"] for x in d][0]' <<<"$HIST")" 500
check "ikinci deneme kaydı var" "$(jget '2 in [x["attempt"] for x in d]' <<<"$HIST")" True
wait_until 35 cnt_ge /fail audit.webhook.secret.rotate 3
sleep 8
check "tam olarak 3 deneme yapıldı" "$(cnt /fail audit.webhook.secret.rotate)" 3
HIST=$(api GET /webhooks/$F_ID/deliveries)
check "geçmişte üçüncü deneme var, dördüncüsü yok" "$(jget 'sorted(x["attempt"] for x in d if x["event"]=="audit.webhook.secret.rotate")' <<<"$HIST")" "[1, 2, 3]"
check "başarısız kanca son gönderimi hata olarak gösterir" "$(api GET /webhooks | jget "next(h['last_delivery']['status'] for h in d['hooks'] if h['id']==$F_ID)")" 500

say "Eski anahtar bir kez gösterildi; yenilenince yeni anahtar geçerli"
check "sınama gönderimi (kapalı kancada da çalışır)" "$(api POST /webhooks/$C_ID/test | jget 'd["status"]')" 200
check "sınama olayı alıcıya ulaştı" "$(cnt /c webhook.test)" 1
check "sınama yükü yeni anahtarla imzalı" "$(python3 $QUERY verify /c webhook.test "$C_SEC_NEW" "$PX_USER" '')" ok
check "eski anahtarla doğrulanmaz" "$(python3 $QUERY signed-with /c webhook.test "$C_SEC")" hayir
TEST_RESULT=$(api POST /webhooks/$C_ID/test)
has "sınama sonucu süre içerir" "$TEST_RESULT" '"duration_ms"'
check "alıcı 500 verirse sınama bunu bildirir" "$(api POST /webhooks/$F_ID/test | jget 'd["status"]')" 500
CLOSED=$(api POST /webhooks/$G_ID/test)
check "kapalı porta sınama: durum kodu yok" "$(jget 'd["status"]' <<<"$CLOSED")" None
check "kapalı porta sınama: hata yazıldı" "$(jget 'len(d["error"]) > 0' <<<"$CLOSED")" True
lacks "hata iletisi adresi içermez" "$CLOSED" "kapali"
REDIR=$(api POST /webhooks/$R_ID/test)
check "yönlendirme izlenmez: 302 hata sayılır" "$(jget 'd["status"]' <<<"$REDIR")" 302
check "yönlendirilen adrese istek gitmedi" "$(cnt /redirected '')" 0

say "Geçmiş sınırı (son 20)"
for _ in $(seq 1 22); do api POST /webhooks/$C_ID/test >/dev/null; done
check "geçmiş 20 kayıtla sınırlı" "$(api GET /webhooks/$C_ID/deliveries | jget 'len(d)')" 20

say "Kanca sayısı sınırı (hesap başına 10)"
check "dokuzuncu" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x1\",\"events\":[\"deploy_failed\"]}")" 200
check "onuncu" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x2\",\"events\":[\"deploy_failed\"]}")" 200
check "on birinci reddedilir" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x3\",\"events\":[\"deploy_failed\"]}")" 409
check "listede tam 10 kanca" "$(api GET /webhooks | jget 'len(d["hooks"])')" 10
X1=$(api GET /webhooks | jget '[h["id"] for h in d["hooks"]][-1]')
check "silme" "$(code DELETE /webhooks/$X1)" 200
check "silinen kancanın geçmişi 404" "$(code GET /webhooks/$X1/deliveries)" 404
check "silinen kancayı yeniden silme 404" "$(code DELETE /webhooks/$X1)" 404
check "silince yer açılır" "$(code POST /webhooks -d "{\"url\":\"http://$LAN:$RECV_PORT/x3\",\"events\":[\"deploy_failed\"]}")" 200
check "güncellemede adres yeniden doğrulanır" "$(code PUT /webhooks/$A_ID -d '{"url":"http://127.0.0.1:9931/a","events":["audit.site"],"enabled":true}')" 400
check "güncellemede geçersiz olay" "$(code PUT /webhooks/$A_ID -d "{\"url\":\"http://$LAN:$RECV_PORT/a\",\"events\":[\"yok\"],\"enabled\":true}")" 400

say "Müşteri hesabı"
CAN_LOCAL=$(capi GET /webhooks | jget 'd["private_allowed"]')
check "müşteri özel ağa kanca tanımlayamayacağını görür" "$CAN_LOCAL" False
for url in "http://$LAN:$RECV_PORT/musteri" "http://127.0.0.1:$RECV_PORT/x" "http://[::1]/x" "http://localhost/x" "http://10.0.0.5/x" "http://192.168.1.1/x" "http://172.16.0.1/x" "http://100.64.0.1/x" "http://169.254.169.254/x" "file:///etc/passwd" "http://[fd00::1]/x" "http://[::ffff:192.168.1.25]/x"; do
  check "müşteri için reddedilir: $url" "$(ccode POST /webhooks -d "{\"url\":\"$url\",\"events\":[\"*\"]}")" 400
done
# Dışarıdan erişilebilir bir adres (IP ile; DNS gerektirmez). Gönderim ulaşmasa da kayıt olur.
CH=$(capi POST /webhooks -d '{"url":"http://8.8.8.8:9/kanca","events":["*"]}')
CH_ID=$(jget 'd["hook"]["id"]' <<<"$CH")
check "müşteri genel adrese kanca tanımlayabilir" "$(printf %s "$CH_ID" | grep -qE '^[0-9]+$' && echo evet || echo hayir)" evet
check "müşteri yalnızca kendi kancasını listeler" "$(capi GET /webhooks | jget 'len(d["hooks"])')" 1
check "başka hesabın kancası: geçmiş" "$(ccode GET /webhooks/$A_ID/deliveries)" 404
check "başka hesabın kancası: güncelleme" "$(ccode PUT /webhooks/$A_ID -d '{"url":"http://8.8.8.8/x","events":["*"],"enabled":true}')" 404
check "başka hesabın kancası: silme" "$(ccode DELETE /webhooks/$A_ID)" 404
check "başka hesabın kancası: sınama" "$(ccode POST /webhooks/$A_ID/test)" 404
check "başka hesabın kancası: anahtar yenileme" "$(ccode POST /webhooks/$A_ID/secret)" 404
check "yönetici de müşterinin kancasına erişemez (kendi hesabı gibi 404)" "$(code GET /webhooks/$CH_ID/deliveries)" 404
check "yöneticinin kancası yerinde" "$(code GET /webhooks/$A_ID/deliveries)" 200
# Müşteri kendi bir eylemini yapar; yalnızca kendi olayları kancasına gitmeye çalışılır.
capi PUT /webhooks/$CH_ID -d '{"url":"http://8.8.8.8:9/kanca","events":["*"],"enabled":true}' >/dev/null
cust_has_history() { [ "$(capi GET /webhooks/$CH_ID/deliveries | jget 'len(d)')" -ge 1 ]; }
wait_until 30 cust_has_history
CH_HIST=$(capi GET /webhooks/$CH_ID/deliveries)
has "müşterinin kendi olayı kancasına gitti" "$CH_HIST" "audit.webhook"
lacks "müşteri yöneticinin site olayını almadı" "$CH_HIST" "audit.site.create"
lacks "müşteri yöneticinin anahtar yenileme olayını almadı" "$CH_HIST" "secret.rotate"
# Müşterinin kendi gönderimi 8.8.8.8:9'a ulaşamaz ama sınama sonucu hata ile döner (çökmez).
check "müşteri sınama gönderimi sonuç döner" "$(ccode POST /webhooks/$CH_ID/test)" 200
check "müşteri kancasını siler" "$(ccode DELETE /webhooks/$CH_ID)" 200

say "Gizli anahtar ve günlükler"
AUDIT_HITS=$(python3 - "$DB" "$A_SEC" "$B_SEC" "$C_SEC_NEW" <<'PYEOF'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1])
hits = 0
for secret in sys.argv[2:]:
    hits += conn.execute("SELECT COUNT(*) FROM audit_log WHERE target LIKE ? OR detail LIKE ?", (f"%{secret}%", f"%{secret}%")).fetchone()[0]
print(hits)
PYEOF
)
check "denetim günlüğü gizli anahtar içermez" "$AUDIT_HITS" 0
if command -v journalctl >/dev/null; then
  check "panel günlüğü gizli anahtar içermez" "$(journalctl -u pixpanel --no-pager -n 3000 2>/dev/null | grep -cF -e "$A_SEC" -e "$B_SEC" -e "$C_SEC_NEW")" 0
fi
check "denetim günlüğü yalnızca sunucu adını yazar" "$(python3 - "$DB" <<'PYEOF'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1])
rows = conn.execute("SELECT target FROM audit_log WHERE action = 'webhook.create'").fetchall()
print("evet" if rows and all("/" not in r[0].split("://", 1)[-1] for r in rows) else "hayir")
PYEOF
)" evet

say "Git web kancası imzası"
GSITE=$(api POST /sites -d '{"domain":"wh-git.test","kind":"static"}' | jget 'd["id"]')
GSECRET='Imza-Gizli-Anahtari-12345'
GV=$(api PUT /sites/$GSITE/git -d '{"repo_url":"https://127.0.0.1:1/yok.git","branch":"main"}')
GHOOK=$(jget 'd["webhook_path"]' <<<"$GV")
check "imza tanımsızken alan kapalı" "$(jget 'd["signature_secret_set"]' <<<"$GV")" False
BODY='{"ref":"refs/heads/baska-dal"}'
post_hook() { curl -sk -o /tmp/px-wh-hook.out -w '%{http_code}' -X POST "https://127.0.0.1:8443$GHOOK" -H 'content-type: application/json' "$@"; }
check "imza tanımsız: imzasız istek eski gibi kabul" "$(post_hook -d "$BODY")" 200
has "imza tanımsız: sonuç atlandı (başka dal)" "$(cat /tmp/px-wh-hook.out)" skipped

check "çok kısa imza anahtarı reddedilir" "$(code PUT /sites/$GSITE/git -d '{"repo_url":"https://127.0.0.1:1/yok.git","branch":"main","signature_secret":"kisa"}')" 400
check "boşluklu imza anahtarı reddedilir" "$(code PUT /sites/$GSITE/git -d '{"repo_url":"https://127.0.0.1:1/yok.git","branch":"main","signature_secret":"icinde bosluk var burada"}')" 400
GV=$(api PUT /sites/$GSITE/git -d "{\"repo_url\":\"https://127.0.0.1:1/yok.git\",\"branch\":\"main\",\"signature_secret\":\"$GSECRET\"}")
check "imza anahtarı tanımlandı" "$(jget 'd["signature_secret_set"]' <<<"$GV")" True
lacks "imza anahtarı yanıtta görünmez" "$GV" "$GSECRET"
lacks "imza anahtarı GET yanıtında görünmez" "$(api GET /sites/$GSITE/git)" "$GSECRET"

SIG="sha256=$(hmac_hex "$GSECRET" "$BODY")"
WRONG="sha256=$(hmac_hex "baska-gizli-anahtar" "$BODY")"
OTHER_BODY_SIG="sha256=$(hmac_hex "$GSECRET" '{"ref":"refs/heads/main"}')"
check "imzasız istek 401" "$(post_hook -d "$BODY")" 401
has "401 yanıtı imza hatasını söyler" "$(cat /tmp/px-wh-hook.out)" invalid_signature
check "yanlış anahtarla imzalı istek 401" "$(post_hook -H "X-Hub-Signature-256: $WRONG" -d "$BODY")" 401
check "başka gövdenin imzası 401" "$(post_hook -H "X-Hub-Signature-256: $OTHER_BODY_SIG" -d "$BODY")" 401
check "sha256= öneksiz imza 401" "$(post_hook -H "X-Hub-Signature-256: ${SIG#sha256=}" -d "$BODY")" 401
check "bozuk onaltılık imza 401" "$(post_hook -H "X-Hub-Signature-256: sha256=zzzz" -d "$BODY")" 401
check "boş imza başlığı 401" "$(post_hook -H "X-Hub-Signature-256;" -d "$BODY")" 401
check "yanlış GitLab anahtarı 401" "$(post_hook -H "X-Gitlab-Token: yanlis" -d "$BODY")" 401
check "boş GitLab anahtarı 401" "$(post_hook -H "X-Gitlab-Token;" -d "$BODY")" 401
check "GitHub başlığı varken GitLab anahtarı kurtarmaz" "$(post_hook -H "X-Hub-Signature-256: $WRONG" -H "X-Gitlab-Token: $GSECRET" -d "$BODY")" 401
check "imzasız ping 401" "$(post_hook -H 'X-GitHub-Event: ping' -d '{}')" 401
check "doğru GitHub imzası kabul" "$(post_hook -H "X-Hub-Signature-256: $SIG" -d "$BODY")" 200
has "doğru imza: istek işlendi (başka dal atlandı)" "$(cat /tmp/px-wh-hook.out)" skipped
check "doğru GitLab anahtarı kabul" "$(post_hook -H "X-Gitlab-Token: $GSECRET" -d "$BODY")" 200
PING_SIG="sha256=$(hmac_hex "$GSECRET" '{}')"
check "imzalı ping kabul" "$(post_hook -H 'X-GitHub-Event: ping' -H "X-Hub-Signature-256: $PING_SIG" -d '{}')" 200
has "imzalı ping yanıtı" "$(cat /tmp/px-wh-hook.out)" pong
check "yanlış adres anahtarı yine 404" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST $B/hooks/deploy/000000000000000000000000000000000000000000000000 -H "X-Hub-Signature-256: $SIG" -d "$BODY")" 404

GV=$(api PUT /sites/$GSITE/git -d '{"repo_url":"https://127.0.0.1:1/yok.git","branch":"main"}')
check "alan verilmezse imza anahtarı korunur" "$(jget 'd["signature_secret_set"]' <<<"$GV")" True
check "korunan anahtar: imzasız istek hâlâ 401" "$(post_hook -d "$BODY")" 401
check "anahtar başka sitenin kancasında geçmez" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST "https://127.0.0.1:8443/api/v1/hooks/deploy/$(printf 'a%.0s' $(seq 1 24))" -H "X-Hub-Signature-256: $SIG" -d "$BODY")" 404
GV=$(api PUT /sites/$GSITE/git -d '{"repo_url":"https://127.0.0.1:1/yok.git","branch":"main","signature_secret":""}')
check "boş metin imza anahtarını kaldırır" "$(jget 'd["signature_secret_set"]' <<<"$GV")" False
check "kaldırınca imzasız istek yine kabul" "$(post_hook -d "$BODY")" 200
check "başka hesap git ayarını göremez" "$(ccode GET /sites/$GSITE/git)" 404

say "Temizlik"
check "git sitesini sil" "$(code DELETE "/sites/$GSITE?delete_files=true")" 200
check "sınama sitesini sil" "$(code DELETE "/sites/$SITE?delete_files=true")" 200
cleanup_old
check "kancalar silindi" "$(api GET /webhooks | jget 'len(d["hooks"])')" 0
kill $RECV_PID 2>/dev/null
rm -f /tmp/px-wh-recv.py $QUERY /tmp/px-wh-hook.out $RECV_LOG
echo "bitti"
