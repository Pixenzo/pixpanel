#!/usr/bin/env bash
# Posta kutusunu IMAP ile içe aktarma sınaması. "Eski sunucu" olarak bu makinenin Dovecot'u
# kullanılır (ağ adresinden; yönetici özel ağ adresine bağlanabilir): kaynak kutu doldurulur,
# hedef kutuya panelin ucuyla aktarılır.
# Posta sunucusu kurulu makinede çalışır. Kullanım: PX_USER=... PX_PASS=... bash mail-import.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-mimp
JC=/tmp/pxjar-mimp-c
DOM=aktarma.test
C=aktaran
PASS='Aktarma-Sinama-Parolasi-1'
SRCPASS='Kaynak-Kutu-Parolasi-7'
DSTPASS='Hedef-Kutu-Parolasi-7'
ADDR=$(hostname -I | awk '{print $1}')
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
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
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
joblog() { api GET "/jobs/$1" | jget 'd["log"]'; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
domain_id() { api GET /mail | jget "next((x['id'] for x in d['domains'] if x['domain']=='$1'), '')"; }
# imap <kutu> <parola> <python ifadesi>: yerel Dovecot'a bağlanıp `c` bağlantısıyla ifadeyi çalıştırır.
imap() { python3 - "$1" "$2" "$3" <<'PY'
import imaplib, ssl, sys, time
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
c = imaplib.IMAP4_SSL("127.0.0.1", 993, ssl_context=ctx)
c.login(sys.argv[1], sys.argv[2])
def count(folder):
    typ, data = c.select('"%s"' % folder, readonly=True)
    return int(data[0]) if typ == "OK" else -1
def folders():
    return sorted(line.decode().rsplit(' ', 1)[-1].strip('"') for line in c.list()[1])
def msg(subject, body="govde"):
    return ("From: gonderen@ornek.com\r\nTo: alici@ornek.com\r\nSubject: %s\r\nMessage-ID: <%s@sinama>\r\n\r\n%s\r\n" % (subject, subject.replace(' ', '-'), body)).encode()
def flags(folder, subject):
    c.select('"%s"' % folder, readonly=True)
    typ, found = c.search(None, 'SUBJECT', '"%s"' % subject)
    typ, data = c.fetch(found[0].split()[0], '(FLAGS INTERNALDATE)')
    return data[0].decode()
exec(sys.argv[3])
PY
}
body() { python3 -c "
import json, sys
d = {'host': '$ADDR', 'username': 'kaynak@$DOM', 'password': '$SRCPASS', 'verify_tls': False}
d.update(eval(sys.argv[1]) if len(sys.argv) > 1 else {})
print(json.dumps(d))" "$@"; }
cleanup() {
  id=$(domain_id $DOM); [ -n "$id" ] && api DELETE "/mail/domains/$id?delete_mail=true" >/dev/null
  id=$(user_id $C); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
  rm -rf /var/vmail/$DOM
}

rm -f $J $JC
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' "$B/mail/mailboxes/1/import" -d '{}')" 401
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Hazırlık"
D=$(api POST /mail/domains -d "{\"domain\":\"$DOM\"}" | jget 'd["id"]')
SRC=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"kaynak\",\"password\":\"$SRCPASS\",\"quota_mb\":100}" | jget 'd["id"]')
DST=$(api POST /mail/domains/$D/mailboxes -d "{\"local\":\"hedef\",\"password\":\"$DSTPASS\",\"quota_mb\":100}" | jget 'd["id"]')
check "kutular açıldı" "$([ "$SRC" -gt 0 ] 2>/dev/null && [ "$DST" -gt 0 ] 2>/dev/null && echo var)" var
sleep 1
imap kaynak@$DOM "$SRCPASS" '
old = imaplib.Time2Internaldate(time.mktime((2024, 3, 15, 10, 30, 0, 0, 0, -1)))
c.append("INBOX", "(\\Seen)", old, msg("okunmus eski ileti"))
c.append("INBOX", "", imaplib.Time2Internaldate(time.time()), msg("okunmamis ileti", "satir bir\r\nsatir iki"))
c.append("INBOX", "(\\Flagged \\Answered)", imaplib.Time2Internaldate(time.time()), msg("yildizli yanitlanmis"))
c.append("INBOX", "(\\Deleted)", imaplib.Time2Internaldate(time.time()), msg("silinmis isaretli"))
c.append("Sent", "(\\Seen)", imaplib.Time2Internaldate(time.time()), msg("gonderilmis ileti"))
c.create("Projeler"); c.create("Projeler.2026")
c.append("Projeler.2026", "", imaplib.Time2Internaldate(time.time()), msg("proje iletisi"))
print("hazir")' >/dev/null
check "kaynak kutu dolduruldu" "$(imap kaynak@$DOM "$SRCPASS" 'print(count("INBOX"), count("Sent"), count("Projeler.2026"))')" "4 1 1"

say "Girdi denetimi"
check "eksik bilgiler" "$(code POST /mail/mailboxes/$DST/import -d '{"host":"imap.ornek.com"}')" 422
check "seçenek gibi sunucu adı" "$(code POST /mail/mailboxes/$DST/import -d "$(body "{'host': '-oX'}")")" 400
check "kabuk karakterli sunucu adı" "$(code POST /mail/mailboxes/$DST/import -d "$(body "{'host': 'a;id'}")")" 400
check "şifresiz bağlantı türü" "$(code POST /mail/mailboxes/$DST/import -d "$(body "{'security': 'none'}")")" 400
check "yerel adres (yöneticiye de kapalı)" "$(code POST /mail/mailboxes/$DST/import -d "$(body "{'host': '127.0.0.1'}")")" 400
check "yerel adrese çözülen ad" "$(code POST /mail/mailboxes/$DST/import -d "$(body "{'host': 'localhost'}")")" 400
check "olmayan kutu" "$(code POST /mail/mailboxes/999999/import -d "$(body)")" 404
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$C\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$C\",\"password\":\"$PASS\"}"
check "başkasının kutusuna aktarılamaz" "$(status $JC POST /mail/mailboxes/$DST/import -d "$(body)")" 404

say "Sertifika doğrulaması"
JOB=$(api POST /mail/mailboxes/$DST/import -d "$(body "{'verify_tls': True}")" | jget 'd["job_id"]')
check "kendinden imzalı sertifikada görev durur" "$(wait_job "$JOB")" failed
has "nedeni açıklanır" "$(joblog "$JOB")" "sertifikası doğrulanamadı"
JOB=$(api POST /mail/mailboxes/$DST/import -d "$(body "{'password': 'yanlis-parola'}")" | jget 'd["job_id"]')
check "yanlış parolada görev durur" "$(wait_job "$JOB")" failed
has "giriş reddi açıklanır" "$(joblog "$JOB")" "girişi reddetti"
check "hedef kutu boş kaldı" "$(imap hedef@$DOM "$DSTPASS" 'print(count("INBOX"))')" 0

say "İçe aktarma"
JOB=$(api POST /mail/mailboxes/$DST/import -d "$(body)" | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "aktarma görevi" "$RESULT" success
[ "$RESULT" = success ] || joblog "$JOB" | tail -20 | sed 's/^/        /'
check "ileti sayıları (silinmiş işaretli alınmaz)" "$(imap hedef@$DOM "$DSTPASS" 'print(count("INBOX"), count("Sent"), count("Projeler.2026"))')" "3 1 1"
has "iç içe klasör açıldı" "$(imap hedef@$DOM "$DSTPASS" 'print(folders())')" "Projeler.2026"
has "okundu bayrağı taşındı" "$(imap hedef@$DOM "$DSTPASS" 'print(flags("INBOX", "okunmus eski ileti"))')" '\Seen'
has "iletinin tarihi korundu" "$(imap hedef@$DOM "$DSTPASS" 'print(flags("INBOX", "okunmus eski ileti"))')" '15-Mar-2024'
check "okunmamış ileti okunmamış kaldı" "$(imap hedef@$DOM "$DSTPASS" 'print("Seen" in flags("INBOX", "okunmamis ileti"))')" False
has "yıldız ve yanıtlandı bayrakları" "$(imap hedef@$DOM "$DSTPASS" 'f = flags("INBOX", "yildizli yanitlanmis"); print("Flagged" in f and "Answered" in f)')" True
has "gövde bozulmadan geldi" "$(imap hedef@$DOM "$DSTPASS" 'c.select("INBOX", readonly=True); n = c.search(None, "SUBJECT", "\"okunmamis ileti\"")[1][0].split()[0]; print(c.fetch(n, "(BODY.PEEK[TEXT])")[1][0][1])')" 'satir bir\r\nsatir iki'
check "dosyaların tamamı vmail kullanıcısının" "$(find /var/vmail/$DOM/hedef ! -user vmail | wc -l)" 0
check "kaynak kutuya dokunulmadı" "$(imap kaynak@$DOM "$SRCPASS" 'print(count("INBOX"), "Seen" in flags("INBOX", "okunmamis ileti"))')" "4 False"
LOG=$(joblog "$JOB")
check "parola görev günlüğünde yok" "$(grep -cF "$SRCPASS" <<<"$LOG")" 0
check "parola süreç listesinde ve diskte yok" "$(grep -rlF "$SRCPASS" /var/vmail/$DOM/hedef /usr/local/lib/pixpanel 2>/dev/null | wc -l)" 0
check "denetim günlüğüne yazıldı" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) > 0 FROM audit_log WHERE action='mail.mailbox.import' AND target='hedef@$DOM'")" 1

say "Yeniden çalıştırma"
imap kaynak@$DOM "$SRCPASS" 'c.append("INBOX", "", imaplib.Time2Internaldate(time.time()), msg("sonradan gelen"))' >/dev/null
JOB=$(api POST /mail/mailboxes/$DST/import -d "$(body)" | jget 'd["job_id"]')
check "ikinci aktarma görevi" "$(wait_job "$JOB")" success
check "yalnızca yeni ileti geldi, yinelenen yok" "$(imap hedef@$DOM "$DSTPASS" 'print(count("INBOX"), count("Sent"), count("Projeler.2026"))')" "4 1 1"
has "günlük yeni ileti olmadığını söyler" "$(joblog "$JOB")" "yeni ileti yok"

say "Temizlik"
cleanup
check "alan adı ve hesap silindi" "$(domain_id $DOM)$(user_id $C)" ""
rm -f $J $JC
echo "bitti"
