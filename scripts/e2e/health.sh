#!/usr/bin/env bash
# Kendi kendine bakımın (sağlık merkezi) sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash health.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-health
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
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
# recheck: denetimleri hemen çalıştırır ve raporu döner.
recheck() { api POST /health/check; }
# finding <rapor> <anahtar> <alan>: bulgunun bir alanı; bulgu yoksa "yok".
finding() { jget "next((f['$3'] for f in d['report']['findings'] if f['key']=='$2'), 'yok')" <<<"$1"; }
# unreachable <rapor> <site no> <alan adı> <alan>: sertifikasız site bulgusu; çok sayıda site varsa
# bulgular tek satırda toplandığı için alan adı ayrıntıda aranır.
unreachable() { jget "next((f['$4'] for f in d['report']['findings'] if f['key']=='ssl.unreachable:$2' or (f['rule']=='ssl.unreachable' and '$3' in f['detail'])), 'yok')" <<<"$1"; }
SETTINGS_ON='{"restart_services":true,"issue_certificates":true,"check_sites":true,"notify":false}'

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
# Bildirimler sınama boyunca kapalı; her adım bilinen bir ayarla başlar.
api PUT /health/settings -d "$SETTINGS_ON" >/dev/null

say "Erişim ve rapor"
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' $B/health)" 401
R=$(recheck)
check "denetim yapıldı" "$(jget 'd["report"]["checks"] > 10 and d["report"]["checked_at"] > 0' <<<"$R")" True
check "ayarlar raporla gelir" "$(jget 'sorted(d["settings"].keys())' <<<"$R")" "['check_sites', 'issue_certificates', 'notify', 'restart_services']"
check "bulgular önem sırasıyla" "$(jget '(lambda order: order == sorted(order, reverse=True))([{"info":0,"warning":1,"critical":2}[f["severity"]] for f in d["report"]["findings"]])' <<<"$R")" True
check "geçersiz ayar gövdesi" "$(code PUT /health/settings -d '{"restart_services":"evet"}')" 422

say "Durdurulmuş servis"
systemctl stop cron
R=$(recheck)
check "durdurulan servis bulgu olur" "$(finding "$R" service.stopped:cron.service severity)" warning
check "tek tıkla düzeltilebilir" "$(finding "$R" service.stopped:cron.service fix)" "Başlat"
check "kendiliğinden başlatılmaz (bilerek durdurulmuş olabilir)" "$(finding "$R" service.stopped:cron.service auto)" False
check "servis hâlâ duruyor" "$(systemctl is-active cron)" inactive
has "düzelt" "$(api POST /health/fix -d '{"key":"service.stopped:cron.service"}' | jget 'd["message"]')" "yeniden başlatıldı"
check "servis çalışıyor" "$(systemctl is-active cron)" active
R=$(recheck)
check "bulgu kendiliğinden kalktı" "$(finding "$R" service.stopped:cron.service severity)" yok
check "elle düzeltme günlüğe düştü" "$(jget '(d["actions"][0]["rule"], d["actions"][0]["automatic"], d["actions"][0]["success"])' <<<"$R")" "('service.stopped', False, True)"

say "Geçersiz düzeltme istekleri"
check "olmayan bulgu" "$(code POST /health/fix -d '{"key":"service.failed:nginx.service"}')" 404
check "izlenmeyen servis adı" "$(code POST /health/fix -d '{"key":"service.failed:../../etc/passwd"}')" 404
INFO=$(jget 'next((f["key"] for f in d["report"]["findings"] if f["fix"] is None), "")' <<<"$R")
[ -n "$INFO" ] && check "düzeltmesi olmayan bulgu" "$(code POST /health/fix -d "{\"key\":\"$INFO\"}")" 400

say "Çöken servis kendiliğinden kalkar"
if systemctl is-active --quiet nginx; then
  systemctl kill -s KILL nginx; sleep 1
  if [ "$(systemctl is-active nginx)" = failed ]; then
    R=$(recheck)
    check "çöken servis kritik bulgu" "$(finding "$R" service.failed:nginx.service severity)" critical
    check "panel kendisi düzeltecek" "$(finding "$R" service.failed:nginx.service auto)" True
    sleep 1
    check "servis yeniden başlatıldı" "$(systemctl is-active nginx)" active
    R=$(recheck)
    check "bulgu kalktı" "$(finding "$R" service.failed:nginx.service severity)" yok
    check "kendiliğinden düzeltme günlüğe düştü" "$(jget '(d["actions"][0]["rule"], d["actions"][0]["target"], d["actions"][0]["automatic"], d["actions"][0]["success"])' <<<"$R")" "('service.failed', 'nginx.service', True, True)"

    api PUT /health/settings -d '{"restart_services":false,"issue_certificates":true,"check_sites":true,"notify":false}' >/dev/null
    systemctl kill -s KILL nginx; sleep 1
    R=$(recheck); sleep 1
    check "ayar kapalıyken kendiliğinden düzeltilmez" "$(finding "$R" service.failed:nginx.service auto) $(systemctl is-active nginx)" "False failed"
    check "elle düzeltme" "$(code POST /health/fix -d '{"key":"service.failed:nginx.service"}')" 200
    check "servis çalışıyor" "$(systemctl is-active nginx)" active
    api PUT /health/settings -d "$SETTINGS_ON" >/dev/null
  else
    echo "  (nginx öldürülünce 'failed' durumuna geçmedi; bölüm atlandı)"
    systemctl start nginx
  fi
else
  echo "  (nginx çalışmıyor; bölüm atlandı)"
fi

say "Yanıt vermeyen site"
OLD=$(api GET /sites | jget 'next((s["id"] for s in d if s["domain"]=="saglik-e2e.test"), "")')
[ -n "$OLD" ] && api DELETE "/sites/$OLD?delete_files=true" >/dev/null
S=$(api POST /sites -d '{"domain":"saglik-e2e.test","kind":"proxy","proxy_target":"http://127.0.0.1:9"}' | jget 'd["id"]')
R=$(recheck)
check "tek başarısız yoklama bulgu sayılmaz" "$(finding "$R" site.down:$S severity)" yok
check "sertifikasız site bildirilir" "$(unreachable "$R" $S saglik-e2e.test severity)" info
check "DNS'i bu sunucuyu göstermeyen siteye sertifika alınmaz" "$(unreachable "$R" $S saglik-e2e.test auto)" False
R=$(recheck)
check "ikinci başarısız yoklamada kritik bulgu" "$(finding "$R" site.down:$S severity)" critical
has "nedeni başlıkta" "$(finding "$R" site.down:$S title)" "HTTP 502"
api PUT /health/settings -d '{"restart_services":true,"issue_certificates":true,"check_sites":false,"notify":false}' >/dev/null
R=$(recheck)
check "site yoklaması kapatılabilir" "$(finding "$R" site.down:$S severity)" yok
api PUT /health/settings -d "$SETTINGS_ON" >/dev/null
check "site sil" "$(code DELETE "/sites/$S?delete_files=true")" 200
R=$(recheck)
check "silinen sitenin bulguları kalkar" "$(unreachable "$R" $S saglik-e2e.test severity)" yok

say "Kalıcılık"
SINCE=$(jget 'd["report"]["findings"][0]["since"] if d["report"]["findings"] else 0' <<<"$R")
sleep 1
R=$(recheck)
check "süren bulgunun ilk görülme anı değişmez" "$(jget 'd["report"]["findings"][0]["since"] if d["report"]["findings"] else 0' <<<"$R")" "$SINCE"
check "bulgu kayıtları güncel bulgularla aynı sayıda" "$(sqlite3 /var/lib/pixpanel/pixpanel.db 'SELECT COUNT(*) FROM health_findings' 2>/dev/null || python3 -c 'import sqlite3; print(sqlite3.connect("/var/lib/pixpanel/pixpanel.db").execute("SELECT COUNT(*) FROM health_findings").fetchone()[0])')" "$(jget 'len(d["report"]["findings"])' <<<"$R")"
api PUT /health/settings -d '{"restart_services":true,"issue_certificates":true,"check_sites":true,"notify":true}' >/dev/null
echo "bitti"
