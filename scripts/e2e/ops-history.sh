#!/usr/bin/env bash
# İşletim geçmişi paketinin sınaması: servis ölçüm geçmişi, haftalık özet, varsayılan yedek planı,
# veritabanı çöp kutusu (7 gün) ve kara liste denetimi. Test makinesinde root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash ops-history.sh
#
# Zamana bağlı işler için ayrı "şimdi çalıştır" uçları vardır (haftalık özeti şimdi gönder, kara liste
# sorgusunu şimdi yap); sınama bunları kullanır. Servis geçmişinin sızıntı ve sık yeniden başlama
# bulguları için veritabanına geçici örnek satır yazılır ve sonunda eski hâli geri konur.
# Kalıcı örnek verilere (deneme.test vb.) dokunulmaz; oluşturulan her şey sonunda silinir.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-ops
JC=/tmp/pxjar-ops-cust
DB=/var/lib/pixpanel/pixpanel.db
PASS='Ops-Gecmisi-Parola-1'
CUST=ohcust
REPO=/var/backups/pixpanel-e2e-ops
SITES="ohist-a.test ohist-b.test ohist-c.test"
DBNAME=ohtrash_db
DBUSER=ohtrash_u
TRASH_DIR=/var/lib/pixpanel/trash/databases
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
ccall() { local m=$1 p=$2; shift 2; curl -sk -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
ccode() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $JC -c $JC -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
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
sql() { python3 -c 'import sqlite3,sys; c=sqlite3.connect("'$DB'", timeout=20); r=c.execute(sys.argv[1]).fetchone(); print("" if r is None else r[0]); c.commit()' "$1"; }
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
db_id() { api GET /databases | jget "next((x['id'] for x in d['databases'] if x['name']=='$1'), '')"; }
dbuser_id() { api GET /databases | jget "next((x['id'] for x in d['users'] if x['name']=='$1'), '')"; }
plan_id() { api GET /backup | jget "next((p['id'] for p in d['plans'] if p['name']=='$1'), '')"; }
target_id() { api GET /backup | jget "next((t['id'] for t in d['targets'] if t['name']=='$1'), '')"; }
trash_field() { api GET /databases/trash | jget "next((str(x['$2']) for x in d if x['name']=='$1'), '')"; }
finding() { api GET /health | jget "next((f['$2'] for f in d['report']['findings'] if f['key']=='$1'), 'yok')"; }
recheck_finding() { api POST /health/check | jget "next((f['$2'] for f in d['report']['findings'] if f['key']=='$1'), 'yok')"; }
mq() { mariadb -N -B -e "$1" 2>&1; }

# Servis geçmişi tablosuna geçici satırlar: save (kaydet+sil), flapping, leak, restore (geri koy).
fixture() { python3 - "$@" <<'PY'
import sqlite3, sys, json, time, os
action, unit = sys.argv[1], sys.argv[2]
db = sqlite3.connect("/var/lib/pixpanel/pixpanel.db", timeout=20)
snap = "/tmp/ops-history-fixture.json"
hour = int(time.time()) // 3600 * 3600
INSERT = "INSERT OR REPLACE INTO service_hours (unit, ts, memory, memory_max, cpu, tasks, failed_minutes, samples, restarts) VALUES (?,?,?,?,?,?,?,?,?)"
if action == "save":
    rows = db.execute("SELECT * FROM service_hours WHERE unit=? AND ts < ?", (unit, hour)).fetchall()
    json.dump(rows, open(snap, "w"))
    db.execute("DELETE FROM service_hours WHERE unit=? AND ts < ?", (unit, hour))
elif action == "clear":
    db.execute("DELETE FROM service_hours WHERE unit=? AND ts < ?", (unit, hour))
elif action == "flapping":
    db.execute(INSERT, (unit, hour - 3600, 1e6, 1000000, 0.0, 1.0, 0, 60, 5))
elif action == "leak":
    mib = 1048576
    for i in range(1, 169):
        memory = 100 * mib + (168 - i) * (300 * mib / 167)
        db.execute(INSERT, (unit, hour - i * 3600, memory, int(memory), 0.0, 1.0, 0, 60, 0))
elif action == "restore" and os.path.exists(snap):
    db.execute("DELETE FROM service_hours WHERE unit=? AND ts < ?", (unit, hour))
    for row in json.load(open(snap)):
        db.execute(INSERT, row)
    os.remove(snap)
db.commit()
PY
}

# Var olan yedek planlarının açık/kapalı durumunu saklayıp kapatır (yeni sitenin kendiliğinden bir
# "tüm siteler" planına girmesi sınamayı bozmasın); restore eski hâli döner.
plans() { python3 - "$1" <<'PY'
import sqlite3, sys, json
db = sqlite3.connect("/var/lib/pixpanel/pixpanel.db", timeout=20)
snap = "/tmp/ops-history-plans.json"
if sys.argv[1] == "disable":
    rows = db.execute("SELECT id FROM backup_plans WHERE enabled = 1").fetchall()
    json.dump([r[0] for r in rows], open(snap, "w"))
    db.execute("UPDATE backup_plans SET enabled = 0 WHERE enabled = 1")
else:
    try:
        ids = json.load(open(snap))
    except Exception:
        ids = []
    for i in ids:
        db.execute("UPDATE backup_plans SET enabled = 1 WHERE id = ?", (i,))
db.commit()
PY
}

START=$(date +%s)
# audited <eylem> [hedef]: bu çalıştırmadan beri denetim günlüğüne düşen kayıt sayısı.
audited() { sql "SELECT COUNT(*) FROM audit_log WHERE action = '$1' AND created_at >= $START${2:+ AND target = '$2'}"; }
# trash_files: ohtrash_ veritabanlarının çöp kutusu dosyalarını siler.
trash_files() {
  for f in $(python3 -c 'import sqlite3; print(" ".join(r[0] for r in sqlite3.connect("/var/lib/pixpanel/pixpanel.db", timeout=20).execute("SELECT file FROM database_trash WHERE name LIKE '"'"'ohtrash_%'"'"'")))'); do rm -f "$TRASH_DIR/$f"; done
}
STARTED_MOCK=""
cleanup() {
  fixture restore ssh.service 2>/dev/null
  for name in $SITES; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
  id=$(plan_id "Yeni siteler (varsayılan)"); [ -n "$id" ] && api DELETE /backup/plans/$id >/dev/null
  id=$(target_id e2e-ops); [ -n "$id" ] && api DELETE /backup/targets/$id >/dev/null
  rm -rf $REPO
  plans restore
  api PUT /backup/default-plan -d '{"enabled":false,"target_id":null,"frequency":"daily","at_minute":180,"weekday":1,"month_day":1,"keep_daily":7,"keep_weekly":4,"keep_monthly":6}' >/dev/null
  for id in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["databases"] if x["name"].startswith("ohtrash_"))'); do api DELETE /databases/$id >/dev/null; done
  for id in $(api GET /databases | jget '" ".join(str(x["id"]) for x in d["users"] if x["name"].startswith("ohtrash_"))'); do api DELETE /databases/users/$id >/dev/null; done
  for id in $(api GET /databases/trash | jget '" ".join(str(x["id"]) for x in d if x["name"].startswith("ohtrash_"))' 2>/dev/null); do api DELETE /databases/trash/$id >/dev/null; done
  trash_files
  sql "DELETE FROM database_trash WHERE name LIKE 'ohtrash_%'" >/dev/null
  id=$(user_id $CUST); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
  api PUT /health/weekly-report -d "$WEEKLY_BEFORE" >/dev/null 2>&1
  [ -n "$STARTED_MOCK" ] && kill $STARTED_MOCK 2>/dev/null
  rm -f /tmp/ops-history-fixture.json /tmp/ops-history-plans.json
}

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
WEEKLY_BEFORE=$(api GET /health/weekly-report | jget 'json.dumps(d["settings"])')
[ "$WEEKLY_BEFORE" = "" ] && WEEKLY_BEFORE='{"enabled":true,"weekday":1,"at_minute":480,"email_to":""}'
cleanup
trap cleanup EXIT

say "Hazırlık: müşteri hesabı"
check "müşteri oluştur" "$(code POST /users -d "{\"username\":\"$CUST\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUST\",\"password\":\"$PASS\"}"

say "Erişim: oturumsuz 401, müşteri 403"
for path in /system/services/nginx.service/history /health/weekly-report /health/blacklist /backup/default-plan /databases/trash; do
  check "oturumsuz $path" "$(curl -sk -o /dev/null -w '%{http_code}' $B$path)" 401
done
for path in /system/services/nginx.service/history /health/weekly-report /health/blacklist /backup/default-plan; do
  check "müşteri $path" "$(ccode GET $path)" 403
done
check "müşteri haftalık özeti gönderemez" "$(ccode POST /health/weekly-report/send -d '{"preview":true}')" 403
check "müşteri kara listeyi sorgulayamaz" "$(ccode POST /health/blacklist/check)" 403
check "müşteri varsayılan planı değiştiremez" "$(ccode PUT /backup/default-plan -d '{"enabled":false}')" 403
check "müşterinin çöp kutusu boş" "$(ccall GET /databases/trash | jget 'len(d)')" 0

# ------------------------------------------------------------------ 1. Servis ölçüm geçmişi
say "Servis ölçüm geçmişi: biçim ve doğrulama"
check "geçersiz aralık" "$(code GET '/system/services/nginx.service/history?range=year')" 400
check "geçersiz servis adı" "$(code GET '/system/services/bad%20name/history')" 400
check "seçenek gibi servis adı" "$(code GET '/system/services/-x.service/history')" 400
# Panel dakikada bir ölçer; ilk örnek için en çok 100 saniye beklenir.
for _ in $(seq 1 20); do
  [ "$(sql "SELECT COUNT(*) FROM service_minutes WHERE ts >= strftime('%s','now') - 180")" -gt 0 ] && break
  sleep 5
done
check "dakikalık servis kayıtları yazılıyor" "$(sql "SELECT COUNT(*) > 0 FROM service_minutes WHERE ts >= strftime('%s','now') - 180")" 1
check "içinde bulunulan saatin özeti de var" "$(sql "SELECT COUNT(*) > 0 FROM service_hours WHERE ts = strftime('%s','now') / 3600 * 3600")" 1
check "panel de izleniyor" "$(sql "SELECT COUNT(*) > 0 FROM service_minutes WHERE unit = 'pixpanel.service'")" 1
for range in day week month; do
  check "$range: nginx geçmişi biçimi" "$(api GET "/system/services/nginx.service/history?range=$range" | jget '(d["unit"], d["monitored"], d["range"], len(d["points"]) <= 400, [p["ts"] for p in d["points"]] == sorted(p["ts"] for p in d["points"]))')" "('nginx.service', True, '$range', True, True)"
done
check "özet alanları" "$(api GET '/system/services/nginx.service/history?range=day' | jget '(isinstance(d["summary"]["restarts_24h"], int), isinstance(d["summary"]["restarts_7d"], int), isinstance(d["summary"]["leak_suspect"], bool))')" "(True, True, True)"
check "varsayılan aralık gün" "$(api GET '/system/services/nginx.service/history' | jget 'd["range"]')" day
check "dakikalık kayıt makul değerler taşır" "$(sql "SELECT COUNT(*) FROM service_minutes WHERE (memory IS NOT NULL AND memory < 0) OR (cpu IS NOT NULL AND cpu < 0) OR (tasks IS NOT NULL AND tasks < 0) OR state NOT IN (0,1,2)")" 0
check "izlenmeyen servis boş geçmişle yanıtlanır" "$(api GET '/system/services/getty@tty1.service/history?range=day' | jget '(d["monitored"], d["summary"], d["points"])')" "(False, None, [])"
if [ "$(sql "SELECT COUNT(*) FROM service_minutes WHERE memory IS NOT NULL AND memory > 0")" -gt 0 ]; then echo "  OK   bellek ölçümü alınıyor"; else echo "  BİLGİ bellek muhasebesi kapalı (kapsayıcı); bellek grafiği boş kalır"; fi

say "Servis ölçüm geçmişi: yeniden başlama"
BEFORE_RESTARTS=$(api GET '/system/services/cron.service/history?range=day' | jget 'd["summary"]["restarts_24h"]')
systemctl restart cron
# İki ardışık dakikalık örnek arasında etkinleşme anı değişince sayılır; en çok 150 saniye beklenir.
GOT=$BEFORE_RESTARTS
for _ in $(seq 1 30); do
  GOT=$(api GET '/system/services/cron.service/history?range=day' | jget 'd["summary"]["restarts_24h"]')
  [ "$GOT" -gt "$BEFORE_RESTARTS" ] 2>/dev/null && break
  sleep 5
done
check "elle yeniden başlatma sayıldı" "$([ "$GOT" -gt "$BEFORE_RESTARTS" ] 2>/dev/null && echo evet || echo hayir)" evet
check "7 günlük sayı 24 saatlikten az değil" "$(api GET '/system/services/cron.service/history?range=day' | jget 'd["summary"]["restarts_7d"] >= d["summary"]["restarts_24h"]')" True
check "grafik noktasında yeniden başlama işareti" "$(api GET '/system/services/cron.service/history?range=day' | jget 'sum(p["restarts"] for p in d["points"]) >= 1')" True

say "Sağlık denetimi: sık yeniden başlama ve bellek sızıntısı"
fixture save ssh.service
check "başlangıçta bulgu yok (sık yeniden başlama)" "$(recheck_finding service.flapping:ssh.service severity)" yok
fixture flapping ssh.service
check "son 24 saatte 3'ten çok yeniden başlayan servis uyarı olur" "$(recheck_finding service.flapping:ssh.service severity)" warning
has "başlıkta sayı var" "$(finding service.flapping:ssh.service title)" "yeniden başladı"
check "özet 24 saatlik sayıyı gösterir" "$(api GET '/system/services/ssh.service/history?range=week' | jget 'd["summary"]["restarts_24h"] >= 5')" True
fixture clear ssh.service
check "satır silinince bulgu kalkar" "$(recheck_finding service.flapping:ssh.service severity)" yok
check "başlangıçta sızıntı bulgusu yok" "$(recheck_finding service.memory_leak:ssh.service severity)" yok
fixture leak ssh.service
check "yedi gündür büyüyen bellek uyarı olur" "$(recheck_finding service.memory_leak:ssh.service severity)" warning
check "sızıntıda yeniden başlatma önerilir" "$(finding service.memory_leak:ssh.service fix)" "Yeniden başlat"
check "özet sızıntı şüphesini bildirir" "$(api GET '/system/services/ssh.service/history?range=week' | jget '(d["summary"]["leak_suspect"], d["summary"]["trend"]["growth_ratio"] > 0.3)')" "(True, True)"
fixture clear ssh.service
check "veri silinince bulgu kalkar" "$(recheck_finding service.memory_leak:ssh.service severity)" yok
fixture restore ssh.service

# ------------------------------------------------------------------ 2. Haftalık özet
say "Haftalık özet: ayar"
check "varsayılanlar ya da kayıtlı ayar biçimi" "$(api GET /health/weekly-report | jget 'sorted(d["settings"].keys())')" "['at_minute', 'email_to', 'enabled', 'weekday']"
check "geçersiz gün" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":0,"at_minute":480,"email_to":""}')" 400
check "geçersiz gün (8)" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":8,"at_minute":480,"email_to":""}')" 400
check "geçersiz saat" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":1,"at_minute":1440,"email_to":""}')" 400
check "geçersiz alıcı" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":1,"at_minute":480,"email_to":"bozuk adres"}')" 400
check "satır sonlu alıcı" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":1,"at_minute":480,"email_to":"a@b.test\nBcc: x@y.test"}')" 400
check "yanlış türde alan" "$(code PUT /health/weekly-report -d '{"enabled":"evet"}')" 422
check "çarşamba 10:00 kaydı" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":3,"at_minute":600,"email_to":""}')" 200
check "sıradaki gönderim gelecekte" "$(api GET /health/weekly-report | jget 'd["next_run_at"] > __import__("time").time()')" True
check "sıradaki gönderim çarşamba 10:00 (sunucu saati)" "$(api GET /health/weekly-report | jget '__import__("time").strftime("%u %H:%M", __import__("time").localtime(d["next_run_at"]))')" "3 10:00"
check "kapatınca sıradaki gönderim yok" "$(api PUT /health/weekly-report -d '{"enabled":false,"weekday":3,"at_minute":600,"email_to":""}' | jget 'd["next_run_at"]')" None
check "yeniden aç" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":3,"at_minute":600,"email_to":""}')" 200

say "Haftalık özet: önizleme ve gönderim"
P=$(api POST /health/weekly-report/send -d '{"preview":true}')
has "önizleme konusu" "$P" "PixPanel haftalık özet"
check "önizleme bölümleri" "$(jget 'all(x in d["text"] for x in ["Denetimler ve düzeltmeler", "Bekleyen sorunlar", "Sertifikalar ve yedekler", "Disk", "En çok ziyaret alan siteler", "Bekleyen güncellemeler"])' <<<"$P")" True
check "HTML gövde" "$(jget '(d["html"].startswith("<!doctype html>"), "<li" in d["html"])' <<<"$P")" "(True, True)"
check "önizleme hiçbir yere göndermez" "$(jget '(d["emailed"], d["telegram"])' <<<"$P")" "([], 0)"
# Gönderim: sahte SMTP (2525). Çalışmıyorsa sınama kendisi başlatır.
if ! (exec 3<>/dev/tcp/127.0.0.1/2525) 2>/dev/null; then
  python3 "$(dirname "$0")/mocks.py" >/dev/null 2>&1 &
  STARTED_MOCK=$!
  sleep 2
fi
: > /tmp/px-mock.log
check "smtp kaydı" "$(code PUT /notifications/smtp -d '{"host":"127.0.0.1","port":2525,"security":"none","username":"","from":"PixPanel <panel@pix.test>"}')" 200
check "ek alıcıyla kaydet" "$(code PUT /health/weekly-report -d '{"enabled":true,"weekday":3,"at_minute":600,"email_to":"haftalik@pix.test"}')" 200
S=$(api POST /health/weekly-report/send -d '{"preview":false}')
check "ek alıcıya gönderildi" "$(jget '"haftalik@pix.test" in d["emailed"]' <<<"$S")" True
check "ek alıcı için hata yok" "$(jget 'all("haftalik@pix.test" not in e for e in d["errors"])' <<<"$S")" True
sleep 1
has "sahte SMTP iletiyi aldı" "$(cat /tmp/px-mock.log)" "haftalik@pix.test"
has "ileti konusu" "$(cat /tmp/px-mock.log)" "PixPanel haftalık özet"
check "son gönderim kaydedildi" "$(api GET /health/weekly-report | jget '(d["last"]["at"] > 0, d["last"]["delivered"] >= 1)')" "(True, True)"

# ------------------------------------------------------------------ 5. Kara liste
say "Kara liste denetimi"
check "durum biçimi" "$(api GET /health/blacklist | jget '(sorted(d.keys()), d["lists"])')" "(['checked_at', 'entries', 'lists', 'mail_installed', 'skipped'], ['zen.spamhaus.org', 'bl.spamcop.net', 'b.barracudacentral.org'])"
R=$(api POST /health/blacklist/check)
check "şimdi sorgula" "$(jget 'isinstance(d["entries"], list)' <<<"$R")" True
if [ "$(jget 'd["mail_installed"]' <<<"$R")" = True ]; then
  check "sorgu zamanı kaydedildi" "$(jget 'd["checked_at"] > 0' <<<"$R")" True
  # Test makinesi özel ağdadır: özel adresler sorgulanmaz, listeye gitmez.
  check "özel adresler sorgulanmadı" "$(jget 'all(not (e["ip"].startswith("192.168.") or e["ip"].startswith("10.")) for e in d["entries"])' <<<"$R")" True
  check "her sorgu bir karar taşır" "$(jget 'all(e["verdict"]["kind"] in ("listed","clean","refused","unknown") for e in d["entries"])' <<<"$R")" True
  LISTED=$(jget 'len(set(e["ip"] for e in d["entries"] if e["verdict"]["kind"]=="listed"))' <<<"$R")
  check "listelenen her adres için bir bulgu (listelenmemişse hiç)" "$(api GET /health | jget 'sum(1 for f in d["report"]["findings"] if f["rule"]=="mail.blacklisted")')" "$LISTED"
else
  check "posta kurulu değilse sorgu atlanır" "$(jget '(d["mail_installed"], d["entries"])' <<<"$R")" "(False, [])"
fi
check "denetim kaydı" "$([ "$(audited health.blacklist.check)" -gt 0 ] && echo 1 || echo 0)" "$([ "$(jget 'd["mail_installed"]' <<<"$R")" = True ] && echo 1 || echo 0)"

# ------------------------------------------------------------------ 3. Varsayılan yedek planı
say "Varsayılan yedek planı: doğrulama"
check "varsayılan ayar biçimi" "$(api GET /backup/default-plan | jget 'sorted(d.keys())')" "['at_minute', 'enabled', 'frequency', 'keep_daily', 'keep_monthly', 'keep_weekly', 'month_day', 'plan_id', 'target_id', 'weekday']"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="restic")')" != True ]; then
  check "restic kurulumu" "$(wait_job "$(api POST /components/restic/install | jget 'd["job_id"]')")" success
fi
tbody() { python3 -c 'import json,sys; print(json.dumps({"name":sys.argv[1],"kind":"local","config":{"path":sys.argv[2]},"secret_key":"","password":""}))' "$@"; }
check "yedek yeri oluştur" "$(code POST /backup/targets -d "$(tbody e2e-ops $REPO)")" 200
T=$(target_id e2e-ops)
api POST /backup/targets/$T/check >/dev/null
body() { python3 -c 'import json,sys; print(json.dumps({"enabled":sys.argv[1]=="1","target_id":(int(sys.argv[2]) if sys.argv[2] else None),"frequency":sys.argv[3],"at_minute":int(sys.argv[4]),"weekday":int(sys.argv[5]),"month_day":int(sys.argv[6]),"keep_daily":7,"keep_weekly":4,"keep_monthly":6}))' "$@"; }
check "yedek yeri seçilmeden açılamaz" "$(code PUT /backup/default-plan -d "$(body 1 '' daily 180 1 1)")" 400
check "elle sıklık plan olamaz" "$(code PUT /backup/default-plan -d "$(body 1 $T manual 180 1 1)")" 400
check "geçersiz saat" "$(code PUT /backup/default-plan -d "$(body 1 $T daily 5000 1 1)")" 400
check "olmayan yedek yeri" "$(code PUT /backup/default-plan -d "$(body 1 99999 daily 180 1 1)")" 400
check "geçersiz ayın günü" "$(code PUT /backup/default-plan -d "$(body 1 $T monthly 180 1 31)")" 400
check "saklama sınırı" "$(code PUT /backup/default-plan -d "$(echo "$(body 1 $T daily 180 1 1)" | sed 's/"keep_daily": 7/"keep_daily": 999/')")" 400

say "Varsayılan yedek planı: yeni site kendiliğinden plana girer"
plans disable
check "ayarı aç (haftalık çarşamba 05:00)" "$(code PUT /backup/default-plan -d "$(body 1 $T weekly 300 3 1)")" 200
check "plan henüz yok (ilk site gelince kurulur)" "$(plan_id 'Yeni siteler (varsayılan)')" ""
check "ilk site" "$(code POST /sites -d '{"domain":"ohist-a.test","kind":"static"}')" 200
A=$(site_id ohist-a.test)
PID=$(plan_id 'Yeni siteler (varsayılan)')
check "varsayılan plan kuruldu" "$([ -n "$PID" ] && echo var)" var
check "plan siteyi kapsıyor" "$(api GET /backup | jget "next(p['scope']['site_ids'] for p in d['plans'] if p['id']==$PID)")" "[$A]"
check "plan ayarlardaki zamanlamayla" "$(api GET /backup | jget "next((p['frequency'], p['at_minute'], p['weekday'], p['enabled'], p['target_id']==$T) for p in d['plans'] if p['id']==$PID)")" "('weekly', 300, 3, True, True)"
check "ayarda plan kimliği tutulur" "$(api GET /backup/default-plan | jget "d['plan_id']==$PID")" True
check "plan zamanlanmış" "$(api GET /backup | jget "next(p['next_run_at'] is not None for p in d['plans'] if p['id']==$PID)")" True
check "denetim kaydı (ekleme)" "$(audited backup.default_plan.attach ohist-a.test)" 1
check "ikinci site" "$(code POST /sites -d '{"domain":"ohist-b.test","kind":"static"}')" 200
B_ID=$(site_id ohist-b.test)
check "ikinci site de plana eklendi (tek plan)" "$(api GET /backup | jget "(sorted(next(p['scope']['site_ids'] for p in d['plans'] if p['id']==$PID)), len([p for p in d['plans'] if p['name']=='Yeni siteler (varsayılan)']))")" "($(python3 -c "print(sorted([$A,$B_ID]))"), 1)"
check "ayar değişince plan da uyar (aylık, 5'i)" "$(code PUT /backup/default-plan -d "$(body 1 $T monthly 240 1 5)")" 200
check "plan güncellendi" "$(api GET /backup | jget "next((p['frequency'], p['at_minute'], p['month_day'], len(p['scope']['site_ids'])) for p in d['plans'] if p['id']==$PID)")" "('monthly', 240, 5, 2)"

say "Varsayılan plan kapalıyken: Sağlık denetimi plansız siteyi bulur ve tek tıkla ekler"
check "ayarı kapat" "$(code PUT /backup/default-plan -d "$(body 0 $T monthly 240 1 5)")" 200
check "üçüncü site" "$(code POST /sites -d '{"domain":"ohist-c.test","kind":"static"}')" 200
C_ID=$(site_id ohist-c.test)
check "kapalıyken kendiliğinden eklenmez" "$(api GET /backup | jget "$C_ID in next(p['scope']['site_ids'] for p in d['plans'] if p['id']==$PID)")" False
check "plansız site bulgusu" "$(recheck_finding backup.uncovered:server severity)" warning
has "bulgu plansız siteleri sayar" "$(finding backup.uncovered:server title)" "site hiçbir yedek planına girmiyor"
check "tek tıkla düzeltme etiketi" "$(finding backup.uncovered:server fix)" "Yedek planına ekle"
has "düzelt" "$(api POST /health/fix -d '{"key":"backup.uncovered:server"}' | jget 'd["message"]')" "planına eklendi"
check "site plana girdi" "$(api GET /backup | jget "$C_ID in next(p['scope']['site_ids'] for p in d['plans'] if p['id']==$PID)")" True
sleep 4
check "bulgu kalktı" "$(recheck_finding backup.uncovered:server severity)" yok
check "kapsanacak site kalmadıysa düzeltme reddedilir" "$(code POST /health/fix -d '{"key":"backup.uncovered:server"}')" 404

say "Yeni siteler: plan silinirse yeniden kurulur"
check "planı sil" "$(code DELETE /backup/plans/$PID)" 200
check "ayarı aç" "$(code PUT /backup/default-plan -d "$(body 1 $T daily 180 1 1)")" 200
for name in $SITES; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
check "siteler silindi" "$(site_id ohist-a.test)$(site_id ohist-b.test)$(site_id ohist-c.test)" ""
check "ilk siteyle plan yeniden kurulur" "$(code POST /sites -d '{"domain":"ohist-a.test","kind":"static"}')" 200
A=$(site_id ohist-a.test)
PID2=$(plan_id 'Yeni siteler (varsayılan)')
check "yeni plan kuruldu ve siteyi kapsıyor" "$(api GET /backup | jget "next(p['scope']['site_ids'] for p in d['plans'] if p['id']==$PID2)")" "[$A]"
api PUT /backup/default-plan -d "$(body 0 $T daily 180 1 1)" >/dev/null
plans restore

# ------------------------------------------------------------------ 4. Veritabanı silmeden önce yedek
say "Veritabanı çöp kutusu: hazırlık"
if [ "$(api GET /databases | jget 'next((e["installed"] for e in d["engines"] if e["id"]=="mariadb"), False)')" != True ]; then
  echo "  BİLGİ MariaDB kurulu değil; çöp kutusu sınaması atlandı"
else
  check "veritabanı ve kullanıcısı" "$(code POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DBNAME\",\"user\":{\"name\":\"$DBUSER\",\"password\":\"Ohtrash-Parola-1234\"}}")" 200
  mq "CREATE TABLE $DBNAME.t (a INT); INSERT INTO $DBNAME.t VALUES (42), (43);" >/dev/null
  D=$(db_id $DBNAME)
  check "geçersiz yedek değeri" "$(code DELETE "/databases/$D?backup=belki")" 400
  check "olmayan veritabanı" "$(code DELETE '/databases/999999?backup=true')" 404
  check "müşteri başkasının veritabanını silemez" "$(ccode DELETE "/databases/$D?backup=true")" 404
  check "silinmedi" "$(mq "SELECT COUNT(*) FROM $DBNAME.t")" 2

  say "Veritabanı çöp kutusu: yedekle sil"
  R=$(api DELETE "/databases/$D?backup=true")
  check "silindi ve çöp kutusuna kaydedildi" "$(jget '(d["ok"], isinstance(d["trash_id"], int), 6.9*86400 < d["expires_at"]-__import__("time").time() < 7.1*86400)' <<<"$R")" "(True, True, True)"
  TID=$(jget 'd["trash_id"]' <<<"$R")
  check "veritabanı sunucudan gitti" "$(mq "SHOW DATABASES LIKE '$DBNAME'")" ""
  check "listeden de kalktı" "$(db_id $DBNAME)" ""
  check "çöp kutusunda görünür" "$(api GET /databases/trash | jget "next((x['engine'], x['expires_at'] > x['deleted_at'], x['size'] > 0, x['owner']) for x in d if x['id']==$TID)")" "('mariadb', True, True, '$PX_USER')"
  FILE=$(ls -t $TRASH_DIR/*.sql.gz 2>/dev/null | head -1)
  check "döküm dosyası var" "$([ -f "$FILE" ] && echo var)" var
  check "dosya yalnız root'a açık (600)" "$(stat -c %a "$FILE")" 600
  check "çöp klasörü yalnız root'a açık (700)" "$(stat -c %a $TRASH_DIR)" 700
  check "dosya adı veritabanı adını sızdırmaz" "$(basename "$FILE" | grep -c "$DBNAME")" 0
  check "döküm içeriği" "$(zcat "$FILE" | grep -c 'INSERT INTO `t` VALUES (42),(43)')" 1
  check "müşteri çöp kutusunu göremez" "$(ccall GET /databases/trash | jget 'len(d)')" 0
  check "müşteri başkasının dökümünü geri getiremez" "$(ccode POST /databases/trash/$TID/restore)" 404
  check "müşteri başkasının dökümünü silemez" "$(ccode DELETE /databases/trash/$TID)" 404
  check "olmayan kayıt" "$(code POST /databases/trash/999999/restore)" 404
  check "denetim kaydı (silme)" "$(sql "SELECT COUNT(*) > 0 FROM audit_log WHERE action = 'database.delete' AND target = '$DBNAME' AND detail LIKE '%7 gün%' AND created_at >= $START")" 1

  say "Veritabanı çöp kutusu: geri getir"
  RJ=$(api POST /databases/trash/$TID/restore | jget 'd["job_id"]')
  check "geri getirme görevi" "$(wait_job $RJ)" success
  check "veri geri geldi" "$(mq "SELECT GROUP_CONCAT(a ORDER BY a) FROM $DBNAME.t")" "42,43"
  check "listede yeniden var" "$([ -n "$(db_id $DBNAME)" ] && echo var)" var
  check "kullanıcının yetkisi geri verildi" "$(mq "SELECT COUNT(*) FROM information_schema.schema_privileges WHERE grantee LIKE \"'$DBUSER'@%\" AND table_schema = '$DBNAME'")" 1
  check "panel kaydında yetki" "$(sql "SELECT COUNT(*) FROM database_grants g JOIN databases d ON d.id = g.database_id WHERE d.name = '$DBNAME' AND g.access = 'full'")" 1
  check "çöp kutusundan kalktı" "$(trash_field $DBNAME id)" ""
  check "döküm dosyası silindi" "$([ -f "$FILE" ] && echo var || echo yok)" yok
  check "ikinci geri getirme" "$(code POST /databases/trash/$TID/restore)" 404
  check "denetim kaydı (geri getirme)" "$(audited database.trash.restore $DBNAME)" 1

  say "Veritabanı çöp kutusu: ad çakışması ve elle silme"
  D=$(db_id $DBNAME)
  R=$(api DELETE "/databases/$D?backup=true"); TID2=$(jget 'd["trash_id"]' <<<"$R")
  check "aynı adla yeni veritabanı" "$(code POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DBNAME\"}")" 200
  check "ad doluyken geri getirilemez" "$(code POST /databases/trash/$TID2/restore)" 409
  check "dökümü hemen sil" "$(code DELETE /databases/trash/$TID2)" 200
  check "ikinci silme" "$(code DELETE /databases/trash/$TID2)" 404
  check "veritabanı (yeni) dokunulmadı" "$([ -n "$(db_id $DBNAME)" ] && echo var)" var

  say "Veritabanı çöp kutusu: yedeksiz silme ve süre dolumu"
  D=$(db_id $DBNAME)
  R=$(api DELETE "/databases/$D")
  check "yedeksiz silme kaydı bırakmaz" "$(jget '(d["ok"], d["trash_id"])' <<<"$R") $(trash_field $DBNAME id)" "(True, None) "
  check "yedeksiz sürüm de çalışır (backup=false)" "$(code POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DBNAME\"}") $(code DELETE "/databases/$(db_id $DBNAME)?backup=false")" "200 200"
  check "yeniden oluştur" "$(code POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DBNAME\"}")" 200
  R=$(api DELETE "/databases/$(db_id $DBNAME)?backup=true"); TID3=$(jget 'd["trash_id"]' <<<"$R")
  FILE3=$(ls -t $TRASH_DIR/*.sql.gz | head -1)
  sql "UPDATE database_trash SET expires_at = strftime('%s','now') - 10 WHERE id = $TID3" >/dev/null
  check "süresi dolan döküm listelenmez" "$(trash_field $DBNAME id)" ""
  check "süresi dolan döküm geri getirilemez" "$(code POST /databases/trash/$TID3/restore)" 400
  check "süresi dolan kayıt temizleyiciye aday" "$(sql "SELECT COUNT(*) FROM database_trash WHERE id = $TID3 AND expires_at <= strftime('%s','now')")" 1
  sql "DELETE FROM database_trash WHERE id = $TID3" >/dev/null
  rm -f "$FILE3"
fi

say "Denetim kayıtları"
for action in backup.default_plan.update health.weekly_report.settings health.weekly_report.send database.trash.restore; do
  check "$action" "$([ "$(audited $action)" -gt 0 ] && echo 1)" 1
done
echo "bitti"
