#!/usr/bin/env bash
# Güncelleme sınamasının test makinesinde koşan yarısı; update.sh tarafından çağrılır.
# /root/px-release altında hazır sürümler bekler: good, broken, old, badsig, badarchive.
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-update
CFG=/etc/pixpanel/config.toml
REL=/root/px-release
api() { local m=$1 p=$2; shift 2; curl -sk -m 60 -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -m 60 -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 240)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 120); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
publish() { ln -sfn "$REL/$1" "$REL/current"; }
restart_panel() { systemctl restart pixpanel; for _ in $(seq 1 30); do [ "$(code GET /auth/me)" != 000 ] && return; sleep 1; done; }
# Bekçinin sonucu yazmasını bekler ve durumunu döner.
wait_outcome() {
  for _ in $(seq 1 120); do
    s=$(api GET /system/update | jget '(d["last"] or {}).get("status", "")' 2>/dev/null)
    case "$s" in success|rolled_back) echo "$s"; return ;; esac
    sleep 1
  done
  echo timeout
}

cp $CFG $CFG.e2e-bak
restore() {
  cp $CFG.e2e-bak $CFG; rm -f $CFG.e2e-bak
  systemctl stop pxrelease 2>/dev/null
  # Sınamanın bıraktığı sürüm bilgisi ve sonuç kaydı temizlenir.
  rm -rf /var/lib/pixpanel/update
  sqlite3 /var/lib/pixpanel/pixpanel.db "DELETE FROM settings WHERE key = 'update_state'"
  systemctl restart pixpanel
}
trap restore EXIT

publish good
systemctl stop pxrelease 2>/dev/null
systemd-run --quiet --unit pxrelease python3 -m http.server 9920 --bind 127.0.0.1 --directory "$REL/current"
sed -i '/^update_/d' $CFG
sed -i '1i update_url = "http://127.0.0.1:9920"' $CFG
rm -f /var/lib/pixpanel/update/result.json
restart_panel
rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
START=$(/usr/local/bin/pixpanel --version)

say "Denetim"
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/system/update)" 401
check "oturumsuz güncelleme" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST -H 'x-pixpanel-csrf: 1' $B/system/update/apply)" 401
check "yeni sürüm görülür" "$(api POST /system/update/check | jget '(d["latest"], d["available"], d["check_error"])')" "('9.0.1', True, '')"
publish old
check "eski sürüm güncelleme sayılmaz" "$(api POST /system/update/check | jget '(d["latest"], d["available"])')" "('0.0.1', False)"
check "eski sürüme inilmez" "$(wait_job "$(api POST /system/update/apply | jget 'd["job_id"]')")" failed

say "İmza ve özet doğrulaması"
publish badsig
has "değiştirilmiş özet dosyası reddedilir" "$(api POST /system/update/check | jget 'd["check_error"]')" "imza"
check "imzasız sürüm kurulmaz" "$(wait_job "$(api POST /system/update/apply | jget 'd["job_id"]')")" failed
publish badarchive
JOB=$(api POST /system/update/apply | jget 'd["job_id"]')
check "değiştirilmiş arşiv kurulmaz" "$(wait_job "$JOB")" failed
has "neden günlükte" "$(api GET /jobs/$JOB | jget 'd["log"]')" "özet uyuşmuyor"
publish good
sed -i '1i update_public_key = "0000000000000000000000000000000000000000000000000000000000000001"' $CFG
restart_panel
has "başka anahtarla imzalanmış sürüm reddedilir" "$(api POST /system/update/check | jget 'd["check_error"]')" "imza"
sed -i '/^update_public_key/d' $CFG
restart_panel
check "reddedilenler ikiliyi değiştirmedi" "$(/usr/local/bin/pixpanel --version)" "$START"
check "yarım dosya kalmadı" "$([ -e /usr/local/bin/pixpanel.new ] || echo yok)" yok

say "Geçerli güncelleme"
SITES=$(api GET /sites | jget 'len(d)')
JOB=$(api POST /system/update/apply | jget 'd["job_id"]')
check "indirme ve doğrulama" "$(wait_job "$JOB")" success
check "panel yeni sürümle ayağa kalktı" "$(wait_outcome)" success
check "ikili yeni sürüm" "$(/usr/local/bin/pixpanel --version)" "pixpanel 9.0.1"
check "panel yeni sürümü bildiriyor" "$(api GET /system/update | jget '(d["current"], d["available"], d["last"]["from"] != d["last"]["to"])')" "('9.0.1', False, True)"
check "oturum sürüyor" "$(code GET /auth/me)" 200
check "veriler yerinde" "$(api GET /sites | jget 'len(d)')" "$SITES"
check "eski ikili saklandı" "$(/usr/local/bin/pixpanel.previous --version)" "$START"
check "bekçi birimi kapandı" "$(systemctl list-units --all 'pixpanel-update-*' --no-legend | wc -l)" 0

say "Başlamayan sürüm"
publish broken
check "bozuk sürüm görülür" "$(api POST /system/update/check | jget '(d["latest"], d["available"])')" "('9.0.2', True)"
rm -f /var/lib/pixpanel/update/result.json
JOB=$(api POST /system/update/apply | jget 'd["job_id"]')
check "indirme ve doğrulama" "$(wait_job "$JOB")" success
check "eski sürüme dönüldü" "$(wait_outcome)" rolled_back
check "ikili eski sürüm" "$(/usr/local/bin/pixpanel --version)" "pixpanel 9.0.1"
check "panel çalışıyor" "$(systemctl is-active pixpanel)" active
check "oturum ve veriler yerinde" "$(api GET /sites | jget 'len(d)')" "$SITES"
has "sonuç açıklaması" "$(api GET /system/update | jget 'd["last"]["detail"]')" "eski sürüme dönüldü"
echo "bitti"
