#!/usr/bin/env bash
# FTP yedek hedefinin sınaması: bu makinedeki FTP sunucusuna (pure-ftpd) yedek alınır,
# listelenir ve geri yüklenir. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash backup-ftp.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-bftp
STORE=ftp-depo-e2e.test
SOURCE=ftp-kaynak-e2e.test
FTP_USER=yedekdepo
FTP_PASS='Ftp-Yedek-Parola-97531'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 300); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
target_id() { api GET /backup | jget "next((t['id'] for t in d['targets'] if t['name']=='$1'), '')"; }
tbody() { python3 -c 'import json,sys; print(json.dumps({"name":sys.argv[1],"kind":"ftp","config":json.loads(sys.argv[2]),"secret_key":sys.argv[3]}))' "$@"; }

cleanup() {
  for name in e2e-ftp e2e-ftp-bozuk; do id=$(target_id $name); [ -n "$id" ] && api DELETE /backup/targets/$id >/dev/null; done
  for name in $STORE $SOURCE; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
}

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup
if ! ss -ltn | grep -q ':21 '; then echo "  (FTP sunucusu çalışmıyor; sınama atlandı)"; echo "bitti"; exit 0; fi

say "Hazırlık"
# Yedeklerin gideceği FTP hesabı: ayrı bir sitenin klasörüne hapsedilmiş hesap.
D=$(api POST /sites -d "{\"domain\":\"$STORE\",\"kind\":\"static\"}" | jget 'd["id"]')
check "depo için FTP hesabı" "$(code POST /sites/$D/ftp -d "{\"name\":\"$FTP_USER\",\"password\":\"$FTP_PASS\"}")" 200
S=$(api POST /sites -d "{\"domain\":\"$SOURCE\",\"kind\":\"static\"}" | jget 'd["id"]')
ROOT=/home/$PX_USER/sites/$SOURCE/public
echo "ftp yedeği öncesi" > $ROOT/index.html; chown $PX_USER:www-data $ROOT/index.html

say "Hedef doğrulaması"
check "seçenek gibi sunucu adı" "$(code POST /backup/targets -d "$(tbody x '{"host":"-oProxyCommand=id","user":"u","path":"y"}' "$FTP_PASS")")" 400
check "boşluklu kullanıcı adı" "$(code POST /backup/targets -d "$(tbody x '{"host":"127.0.0.1","user":"u --config x","path":"y"}' "$FTP_PASS")")" 400
check "üst klasöre çıkan yol" "$(code POST /backup/targets -d "$(tbody x '{"host":"127.0.0.1","user":"u","path":"../../etc"}' "$FTP_PASS")")" 400
check "parolasız hedef" "$(code POST /backup/targets -d "$(tbody x '{"host":"127.0.0.1","user":"u","path":"y"}' '')")" 400

say "FTP'ye yedek"
R=$(api POST /backup/targets -d "$(tbody e2e-ftp "{\"host\":\"127.0.0.1\",\"port\":21,\"user\":\"$FTP_USER\",\"path\":\"depo\",\"tls\":true,\"tls_skip_verify\":true}" "$FTP_PASS")")
T=$(target_id e2e-ftp)
check "hedef oluştu" "$([[ "$T" =~ ^[0-9]+$ ]] && echo evet)" evet
check "rclone kuruldu" "$(command -v rclone >/dev/null && echo var)" var
check "parola yanıtta ve genel görünümde yok" "$(api GET /backup | grep -c "$FTP_PASS")" 0
check "ayarlar kaydedildi" "$(api GET /backup | jget "next((t['kind'], t['config']['port'], t['config']['tls'], t['has_secret_key']) for t in d['targets'] if t['id']==$T)")" "('ftp', 21, True, True)"
check "bağlantı denemesi depoyu kurar" "$(api POST /backup/targets/$T/check | jget "next((t['last_check_ok'], t['last_check_error']) for t in d['targets'] if t['id']==$T)")" "(True, '')"
check "depo FTP hesabının klasöründe" "$([ -f /home/$PX_USER/sites/$STORE/depo/config ] && echo var)" var
JOB=$(api POST /backup/run -d "{\"target_id\":$T,\"scope\":{\"site_ids\":[$S]}}" | jget 'd["job_id"]')
RESULT=$(wait_job "$JOB")
check "yedekleme görevi" "$RESULT" success
[ "$RESULT" = success ] || api GET "/jobs/$JOB" | jget 'd["log"][-800:]' | sed 's/^/        /'
check "FTP parolası görev günlüğünde yok" "$(api GET "/jobs/$JOB" | jget 'd["log"]' | grep -c "$FTP_PASS")" 0
SNAP=$(api GET "/backup/targets/$T/snapshots" | jget "next((s['id'] for s in d['snapshots'] if s['kind']=='site' and s['name']=='$SOURCE'), '')")
check "yedek listelenir" "$([ -n "$SNAP" ] && echo var)" var
check "yedekler depoda şifreli durur" "$(grep -rl 'ftp yedeği öncesi' /home/$PX_USER/sites/$STORE/depo 2>/dev/null | wc -l)" 0

say "FTP'den geri yükleme"
echo "bozuldu" > $ROOT/index.html
check "geri yükleme görevi" "$(wait_job "$(api POST /backup/restore -d "{\"target_id\":$T,\"snapshot\":\"$SNAP\"}" | jget 'd["job_id"]')")" success
check "dosya geri geldi" "$(cat $ROOT/index.html)" "ftp yedeği öncesi"

say "Yanlış parola"
api POST /backup/targets -d "$(tbody e2e-ftp-bozuk "{\"host\":\"127.0.0.1\",\"user\":\"$FTP_USER\",\"path\":\"depo2\",\"tls\":true,\"tls_skip_verify\":true}" 'yanlis-parola-123456')" >/dev/null
BAD=$(target_id e2e-ftp-bozuk)
check "yanlış parolayla bağlantı başarısız, nedeni kaydedilir" "$(api POST /backup/targets/$BAD/check | jget "next((t['last_check_ok'], t['last_check_error'] != '') for t in d['targets'] if t['id']==$BAD)")" "(False, True)"
check "parolayı düzeltince bağlanır" "$(api PUT /backup/targets/$BAD -d "$(tbody e2e-ftp-bozuk "{\"host\":\"127.0.0.1\",\"user\":\"$FTP_USER\",\"path\":\"depo2\",\"tls\":true,\"tls_skip_verify\":true}" "$FTP_PASS")" >/dev/null; api POST /backup/targets/$BAD/check | jget "next(t['last_check_ok'] for t in d['targets'] if t['id']==$BAD)")" True

say "Temizlik"
cleanup
check "hedefler silindi" "$(target_id e2e-ftp)$(target_id e2e-ftp-bozuk)" ""
echo "bitti"
