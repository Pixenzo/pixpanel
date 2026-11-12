#!/usr/bin/env bash
# Sistem güncellemeleri, yeniden başlatma ve bileşen kaldırma sınaması. Test makinesinde
# (Debian 13, yetkisiz LXC) root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash sys-updates.sh
#   PX_UPGRADE_ALL=1  bekleyen paket varsa "Hepsini kur" görevi de koşulur (varsayılan: yalnızca
#                     bekleyen paket yoksa koşulur; böylece test makinesinin paketleri değişmez).
# Sunucu yeniden başlatması ASLA gerçekten yapılmaz: zamanlanır ve hemen iptal edilir.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-sysupd
JC=/tmp/pxjar-sysupd-customer
CUSTOMER=sysupdmusteri
PASS='Guncelleme-Parola-2468'
SITE=e2e-sysupd.test
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
wait_job() { for _ in $(seq 1 1800); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
job_log() { api GET "/jobs/$1" | jget 'd["log"]'; }
# job_ok <açıklama> <görev no>: görev başarıyla bitmeli; bitmezse günlüğün sonu yazılır.
job_ok() {
  local result; result=$(wait_job "$2")
  check "$1" "$result" success
  [ "$result" = success ] || job_log "$2" | tail -15 | sed 's/^/        /'
}
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
component() { api GET /components | jget "next(c['$2'] for c in d if c['id']=='$1')"; }

# Sınama sırasında dokunulan dosyalar ve zamanlanmış yeniden başlatma, ne olursa olsun geri alınır.
BACKUP=/tmp/sysupd-backup
restore() {
  shutdown -c >/dev/null 2>&1
  rm -f /var/run/reboot-required /var/run/reboot-required.pkgs
  [ -f $BACKUP/reboot-required ] && cp -p $BACKUP/reboot-required /var/run/reboot-required
  [ -f $BACKUP/reboot-required.pkgs ] && cp -p $BACKUP/reboot-required.pkgs /var/run/reboot-required.pkgs
  return 0
}
mkdir -p $BACKUP
[ -f /var/run/reboot-required ] && cp -p /var/run/reboot-required $BACKUP/
[ -f /var/run/reboot-required.pkgs ] && cp -p /var/run/reboot-required.pkgs $BACKUP/
trap restore EXIT

cleanup() {
  local id
  for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['domain']=='$SITE')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
  id=$(user_id $CUSTOMER)
  [ -n "$id" ] && api DELETE /users/$id >/dev/null
}

rm -f $J $JC
login $J "$PX_USER" "$PX_PASS" >/dev/null
cleanup

say "Yetki"
for endpoint in "GET /system/updates" "POST /system/updates/refresh" "POST /system/updates/install" "POST /system/reboot" "POST /system/reboot/cancel" "POST /components/adminer/remove"; do
  # shellcheck disable=SC2086
  check "oturumsuz: $endpoint" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X ${endpoint% *} "$B${endpoint#* }" -d '{"scope":"all"}')" 401
done
check "müşteri hesabı açılır" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
check "müşteri girişi" "$(login $JC $CUSTOMER "$PASS")" 200
for endpoint in "GET /system/updates" "POST /system/updates/refresh" "POST /system/updates/install" "POST /system/reboot" "POST /system/reboot/cancel" "POST /components/adminer/remove"; do
  # shellcheck disable=SC2086
  check "müşteri: $endpoint" "$(status $JC ${endpoint% *} "${endpoint#* }" -d '{"scope":"all"}')" 403
done
check "müşteri hesabı silinir" "$(code DELETE /users/$(user_id $CUSTOMER))" 200

say "Liste biçimi"
V=$(api GET /system/updates)
check "alanlar" "$(jget "sorted(k for k in d.keys() if k not in ('refresh_error', 'services_to_restart'))" <<<"$V")" "['busy', 'checked_at', 'kernel', 'packages', 'reboot', 'security', 'total', 'unattended']"
check "toplam, liste uzunluğuna eşit" "$(jget 'd["total"] == len(d["packages"])' <<<"$V")" True
check "güvenlik sayısı, işaretli paketlere eşit" "$(jget 'd["security"] == sum(1 for p in d["packages"] if p["security"])' <<<"$V")" True
check "paket alanları" "$(jget 'all(sorted(p.keys()) == ["candidate", "installed", "name", "security"] and p["name"] and p["candidate"] for p in d["packages"])' <<<"$V")" True
check "güvenlik paketleri önce gelir" "$(jget '(lambda flags: flags == sorted(flags, reverse=True))([p["security"] for p in d["packages"]])' <<<"$V")" True
check "çalışan çekirdek bilinir" "$(jget 'bool(d["kernel"]["running"])' <<<"$V")" True
check "çekirdek alanları" "$(jget 'sorted(d["kernel"].keys())' <<<"$V")" "['latest', 'outdated', 'running']"
check "otomatik güncelleme alanları" "$(jget 'sorted(d["unattended"].keys())' <<<"$V")" "['configured', 'installed', 'last_install', 'last_run']"
check "yeniden başlatma alanları" "$(jget 'sorted(d["reboot"].keys())' <<<"$V")" "['packages', 'required', 'scheduled_at', 'scheduled_mode']"
check "görev çalışmıyor" "$(jget 'd["busy"]' <<<"$V")" False
check "son denetim zamanı" "$(jget 'isinstance(d["checked_at"], int) and d["checked_at"] > 0' <<<"$V")" True
PENDING=$(jget 'd["total"]' <<<"$V")
SECURITY=$(jget 'd["security"]' <<<"$V")
echo "  (bekleyen: $PENDING, güvenlik: $SECURITY)"

say "Listeyi yenile"
T1=$(jget 'd["checked_at"]' <<<"$V")
sleep 2
R=$(api POST /system/updates/refresh)
check "yenileme hatasız" "$(jget 'd["refresh_error"]' <<<"$R")" None
T2=$(jget 'd["checked_at"]' <<<"$R")
check "son denetim zamanı ilerledi" "$([ "$T2" -gt "$T1" ] 2>/dev/null && echo evet)" evet
check "yenilemeden sonra liste gelir" "$(jget 'isinstance(d["packages"], list)' <<<"$R")" True
H=$(api POST /health/check)
check "Genel bakış aynı sayıyı gösterir" "$(jget 'next((f["title"] for f in d["report"]["findings"] if f["key"]=="updates.pending:server"), "yok") != "yok"' <<<"$H")" "$([ "$(jget 'd["total"]' <<<"$R")" -gt 0 ] && echo True || echo False)"
if [ "$(jget 'd["total"]' <<<"$R")" -gt 0 ]; then
  check "bekleyen güncelleme bulgusu bu sayfaya bağlanır" "$(jget 'next(f["link"] for f in d["report"]["findings"] if f["key"]=="updates.pending:server")' <<<"$H")" /updates
fi

say "Geçersiz kurulum istekleri"
check "bilinmeyen kapsam" "$(code POST /system/updates/install -d '{"scope":"everything"}')" 400
check "boş kapsam" "$(code POST /system/updates/install -d '{"scope":""}')" 400
check "komut sokma denemesi" "$(code POST /system/updates/install -d '{"scope":"all; touch /tmp/sysupd-pwned"}')" 400
check "büyük harfli kapsam" "$(code POST /system/updates/install -d '{"scope":"ALL"}')" 400
check "kapsamsız gövde" "$(code POST /system/updates/install -d '{}')" 422
check "kimse bir şey çalıştırmadı" "$([ -e /tmp/sysupd-pwned ] && echo var || echo yok)" yok

say "Güvenlik güncellemelerini kur"
R=$(api POST /system/updates/install -d '{"scope":"security"}')
JOB=$(jget 'd["job_id"]' <<<"$R")
check "görev başladı" "$([[ "$JOB" =~ ^[0-9]+$ ]] && echo evet)" evet
R2=$(api POST /system/updates/install -d '{"scope":"security"}')
if [ "$(jget 'd.get("job_id", "")' <<<"$R2")" = "" ]; then
  check "çalışan görev varken ikinci kurulum reddedilir" "$(jget 'd["error"]["code"]' <<<"$R2")" conflict
else
  echo "  (ilk görev ikinci istekten önce bitti; çakışma denetimi atlandı)"
  job_ok "ikinci görev de başarıyla bitti" "$(jget 'd["job_id"]' <<<"$R2")"
fi
job_ok "güvenlik güncelleme görevi başarıyla bitti" "$JOB"
LOG=$(job_log "$JOB")
has "günlükte apt çıktısı" "$LOG" "apt-get update"
has "günlükte kalan sayısı" "$LOG" "Kalan bekleyen güncelleme"
has "görev tamamlandı" "$LOG" "Güncelleme görevi tamamlandı"
check "görev türü" "$(api GET "/jobs/$JOB" | jget 'd["job"]["kind"]')" system.upgrade
V=$(api GET /system/updates)
check "görev bitince çalışıyor görünmez" "$(jget 'd["busy"]' <<<"$V")" False
check "güvenlik güncellemesi sayısı artmadı" "$([ "$(jget 'd["security"]' <<<"$V")" -le "$SECURITY" ] && echo evet)" evet
check "denetim günlüğünde kurulum" "$(api GET '/system/audit?limit=20' | jget 'any(e["action"]=="system.updates.install" and e["target"]=="security" for e in d)')" True

say "Hepsini kur"
TOTAL=$(jget 'd["total"]' <<<"$V")
if [ "$TOTAL" -eq 0 ] || [ "${PX_UPGRADE_ALL:-0}" = 1 ]; then
  R=$(api POST /system/updates/install -d '{"scope":"all"}')
  JOB=$(jget 'd["job_id"]' <<<"$R")
  job_ok "tam güncelleme görevi başarıyla bitti" "$JOB"
  LOG=$(job_log "$JOB")
  if [ "$TOTAL" -eq 0 ]; then has "bekleyen yoksa sistem güncel denir" "$LOG" "Sistem güncel"; fi
  V=$(api GET /system/updates)
  check "bitince bekleyen paket kalmadı" "$(jget 'd["total"]' <<<"$V")" 0
  H=$(api POST /health/check)
  check "Genel bakıştaki güncelleme bulgusu kalktı" "$(jget 'next((f["key"] for f in d["report"]["findings"] if f["key"]=="updates.pending:server"), "yok")' <<<"$H")" yok
else
  echo "  (atlandı: $TOTAL paket bekliyor; koşmak için PX_UPGRADE_ALL=1)"
fi

say "Yeniden başlatma gerekli uyarısı"
rm -f /var/run/reboot-required /var/run/reboot-required.pkgs
V=$(api GET /system/updates)
check "dosya yokken uyarı yok" "$(jget 'd["reboot"]["required"]' <<<"$V")" False
touch /var/run/reboot-required
printf 'linux-image-test\nnginx\nnginx\n' > /var/run/reboot-required.pkgs
V=$(api GET /system/updates)
check "dosya varken uyarı görünür" "$(jget 'd["reboot"]["required"]' <<<"$V")" True
check "paket adları tekilleştirilip sıralanır" "$(jget 'd["reboot"]["packages"]' <<<"$V")" "['linux-image-test', 'nginx']"
H=$(api POST /health/check)
check "Genel bakış bulgusu bu sayfaya bağlanır" "$(jget 'next((f["link"] for f in d["report"]["findings"] if f["key"]=="system.reboot:server"), "yok")' <<<"$H")" /updates
printf 'bozuk paket; touch /tmp/sysupd-pwned\nnginx\n' > /var/run/reboot-required.pkgs
V=$(api GET /system/updates)
check "geçersiz paket adı listeye girmez" "$(jget 'd["reboot"]["packages"]' <<<"$V")" "['nginx']"
rm -f /var/run/reboot-required /var/run/reboot-required.pkgs
V=$(api GET /system/updates)
check "dosya silinince uyarı kalkar" "$(jget 'd["reboot"]["required"]' <<<"$V")" False
H=$(api POST /health/check)
check "bulgu da kalkar" "$(jget 'next((f["key"] for f in d["report"]["findings"] if f["key"]=="system.reboot:server"), "yok")' <<<"$H")" yok
restore

say "Yeniden başlatma (zamanlanır ve hemen iptal edilir)"
if ! command -v shutdown >/dev/null 2>&1; then
  echo "  (atlandı: shutdown komutu yok)"
else
  RES=$(api POST /system/reboot)
  if [ "$(jget 'd.get("ok", False)' <<<"$RES")" = True ]; then
    # Önce iptal güvence altına alınır; denetimler ondan sonra yapılır.
    SCHEDULED=no
    [ -f /run/systemd/shutdown/scheduled ] && SCHEDULED=yes
    V=$(api GET /system/updates)
    CANCEL=$(code POST /system/reboot/cancel)
    check "zamanlama kaydı oluştu" "$SCHEDULED" yes
    check "iptal" "$CANCEL" 200
    NOW=$(date +%s)
    SCHED_AT=$(jget 'd["reboot"]["scheduled_at"]' <<<"$V")
    check "sayfada zamanlanmış yeniden başlatma görünür" "$(jget 'd["reboot"]["scheduled_mode"]' <<<"$V")" reboot
    check "zaman bir dakika kadar sonrasında" "$([ "$SCHED_AT" != None ] && [ "$((SCHED_AT - NOW))" -ge -5 ] && [ "$((SCHED_AT - NOW))" -le 75 ] && echo evet)" evet
    check "iptal sonrası zamanlama kaydı yok" "$([ -f /run/systemd/shutdown/scheduled ] && echo var || echo yok)" yok
    V=$(api GET /system/updates)
    check "iptal sonrası sayfada zamanlama yok" "$(jget 'd["reboot"]["scheduled_at"]' <<<"$V")" None
    check "denetim günlüğünde yeniden başlatma ve iptal" "$(api GET '/system/audit?limit=20' | jget 'sorted(set(e["action"] for e in d if e["action"] in ("system.reboot", "system.reboot.cancel")))')" "['system.reboot', 'system.reboot.cancel']"
  else
    # Yetkisiz LXC'de zamanlama çalışmayabilir; hata iletisi olduğu gibi gösterilir.
    MSG=$(jget 'd["error"]["message"]' <<<"$RES")
    echo "  (atlandı: bu makinede yeniden başlatma zamanlanamıyor: $MSG)"
    has "hata iletisi nedeni söyler" "$MSG" "zamanlanamadı"
  fi
  # Her durumda hiçbir şey zamanlanmış kalmamalı.
  shutdown -c >/dev/null 2>&1
  check "zamanlanmış yeniden başlatma kalmadı" "$([ -f /run/systemd/shutdown/scheduled ] && echo var || echo yok)" yok
fi

say "Bileşen kaldırma: yetki ve geçersiz istekler"
LIST=$(api GET /components)
check "kaldırılabilirlik alanı" "$(jget 'all("removable" in c and "in_use" in c for c in d)' <<<"$LIST")" True
for id in nginx mariadb postgresql mail dns restic python; do
  check "$id kaldırılabilir değil" "$(jget "next(c['removable'] for c in d if c['id']=='$id')" <<<"$LIST")" False
done
check "adminer kaldırılabilir" "$(jget 'next(c["removable"] for c in d if c["id"]=="adminer")' <<<"$LIST")" True
check "nginx kaldırma isteği" "$(code POST /components/nginx/remove)" 400
check "mariadb kaldırma isteği" "$(code POST /components/mariadb/remove)" 400
check "mail kaldırma isteği" "$(code POST /components/mail/remove)" 400
check "olmayan bileşen" "$(code POST /components/yok/remove)" 404
check "seçenek gibi görünen ad" "$(code POST /components/--purge/remove)" 404
check "yol gezintisi" "$(code POST "/components/..%2F..%2Fetc/remove")" 404
NOT_INSTALLED=$(jget 'next((c["id"] for c in d if c["removable"] and not c["installed"]), "")' <<<"$LIST")
if [ -n "$NOT_INSTALLED" ]; then
  check "kurulu olmayan bileşen ($NOT_INSTALLED) kaldırılamaz" "$(code POST /components/$NOT_INSTALLED/remove)" 409
fi

say "Bileşen kaldırma: kullanımdaki PHP sürümü"
PHPV=$(api GET /php | jget 'd[0]["version"] if d else ""')
if [ -z "$PHPV" ]; then
  echo "  (atlandı: kurulu PHP sürümü yok)"
else
  S=$(api POST /sites -d "{\"domain\":\"$SITE\",\"kind\":\"php\",\"php_version\":\"$PHPV\"}" | jget 'd["id"]')
  check "PHP sitesi açıldı" "$([[ "$S" =~ ^[0-9]+$ ]] && echo evet)" evet
  check "listede kullanım nedeni görünür" "$(component php-$PHPV in_use | grep -c "$SITE")" 1
  RES=$(api POST /components/php-$PHPV/remove)
  check "kullanımdaki PHP sürümü 409 verir" "$(jget 'd["error"]["code"]' <<<"$RES")" conflict
  has "neden site adını söyler" "$RES" "$SITE"
  check "PHP hâlâ kurulu" "$(component php-$PHPV installed)" True
  check "PHP hâlâ listede" "$(api GET /php | jget "any(v['version']=='$PHPV' for v in d)")" True
  api DELETE "/sites/$S?delete_files=true" >/dev/null
  check "site silinince kullanım nedeni kalkar" "$(component php-$PHPV in_use)" None
fi

say "Bileşen kaldırma: Adminer'i kaldır ve yeniden kur"
WAS_INSTALLED=$(component adminer installed)
if [ "$WAS_INSTALLED" != True ]; then
  R=$(api POST /components/adminer/install)
  job_ok "Adminer önce kuruldu (sınama öncesi kurulu değildi)" "$(jget 'd["job_id"]' <<<"$R")"
fi
check "Adminer kurulu" "$(component adminer installed)" True
check "Adminer kullanımda değil" "$(component adminer in_use)" None
R=$(api POST /components/adminer/remove)
JOB=$(jget 'd["job_id"]' <<<"$R")
check "kaldırma görevi başladı" "$([[ "$JOB" =~ ^[0-9]+$ ]] && echo evet)" evet
job_ok "kaldırma görevi başarıyla bitti" "$JOB"
LOG=$(job_log "$JOB")
has "günlükte apt-get remove" "$LOG" "apt-get remove"
has "yapılandırmanın korunduğu söylenir" "$LOG" "yapılandırma dosyaları korundu"
check "purge kullanılmadı" "$(grep -c 'apt-get purge' <<<"$LOG")" 0
check "Adminer kaldırıldı" "$(component adminer installed)" False
check "Adminer betiği silindi" "$([ -f /usr/share/adminer/adminer.php ] && echo var || echo yok)" yok
check "panel bağlantısı temizlendi" "$([ -e /var/lib/pixpanel/tools/root/tools/adminer ] && echo var || echo yok)" yok
check "nginx yapılandırması geçerli" "$(nginx -t >/dev/null 2>&1 && echo evet)" evet
check "nginx çalışıyor" "$(systemctl is-active nginx)" active
check "panel yanıt veriyor" "$(code GET /components)" 200
check "denetim günlüğünde kaldırma" "$(api GET '/system/audit?limit=20' | jget 'any(e["action"]=="component.remove" and e["target"]=="adminer" for e in d)')" True
check "kaldırılmışı yeniden kaldırma" "$(code POST /components/adminer/remove)" 409
R=$(api POST /components/adminer/install)
job_ok "Adminer yeniden kuruldu" "$(jget 'd["job_id"]' <<<"$R")"
check "Adminer kurulu (yeniden)" "$(component adminer installed)" True
check "Adminer betiği geri geldi" "$([ -f /usr/share/adminer/adminer.php ] && echo var || echo yok)" var
check "panel bağlantısı geri geldi" "$([ -L /var/lib/pixpanel/tools/root/tools/adminer ] && echo var || echo yok)" var
check "nginx yapılandırması geçerli (yeniden kurulumdan sonra)" "$(nginx -t >/dev/null 2>&1 && echo evet)" evet
if [ "$WAS_INSTALLED" != True ]; then
  # Makine sınama öncesi hâline döndürülür.
  R=$(api POST /components/adminer/remove)
  job_ok "Adminer sınama öncesi hâline döndürüldü" "$(jget 'd["job_id"]' <<<"$R")"
  check "Adminer kurulu değil (başlangıç hâli)" "$(component adminer installed)" False
fi

cleanup
# Başlangıçtaki yeniden başlatma bayrakları geri konur, sonra yedek silinir (çıkış tuzağı artık gereksiz).
restore
trap - EXIT
rm -rf $BACKUP
echo
echo bitti
