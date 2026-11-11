#!/usr/bin/env bash
# Komut satırının sınaması: komutlar çalışan panelin API'sini çağırır, iz bırakmaz.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash cli.sh
set -uo pipefail
DB=/var/lib/pixpanel/pixpanel.db
SITE=komut-satiri.test
ACCOUNT=clideneme
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
# Komutu çalıştırır, çıkış kodunu yazar.
rc() { "$@" >/dev/null 2>&1; echo $?; }
tokens() { sqlite3 "$DB" "SELECT count(*) FROM api_tokens WHERE name = 'pixpanel-cli'"; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }

# Önceki koşudan kalanlar.
pixpanel site remove $SITE --delete-files >/dev/null 2>&1
OLD=$(pixpanel api GET /users 2>/dev/null | jget "next((u['id'] for u in d if u['username']=='$ACCOUNT'), '')")
[ -n "$OLD" ] && pixpanel api DELETE "/users/$OLD" >/dev/null 2>&1

say "Siteler"
check "site listesi başlıkla gelir" "$(pixpanel site list | head -1 | awk '{print $1, $2}')" "ALAN ADI"
check "site ekleme" "$(pixpanel site add $SITE | grep -c "$SITE eklendi")" 1
check "eklenen site listede" "$(pixpanel site list | awk -v d=$SITE '$1==d {print $2}')" static
check "site gerçekten yayında" "$(echo cli > /home/$PX_USER/sites/$SITE/public/index.html; curl -s -H "Host: $SITE" http://127.0.0.1/)" cli
check "aynı site ikinci kez eklenemez (çıkış kodu)" "$(rc pixpanel site add $SITE)" 1
check "hata iletisi panelden gelir" "$(pixpanel site add $SITE 2>&1 | grep -c 'HTTP 409')" 1
check "geçersiz alan adı" "$(rc pixpanel site add 'a;b')" 1
check "vekil sitede hedef zorunlu" "$(pixpanel site add vekil-$SITE --kind proxy 2>&1 | grep -c -- '--target')" 1
check "olmayan site kaldırılamaz" "$(rc pixpanel site remove yok-$SITE)" 1

say "Doğrudan API"
check "GET" "$(pixpanel api GET /sites | jget "len([s for s in d if s['domain']=='$SITE'])")" 1
check "başında bölü olmayan yol" "$(pixpanel api get sites | jget "len([s for s in d if s['domain']=='$SITE'])")" 1
ID=$(pixpanel api GET /sites | jget "next(s['id'] for s in d if s['domain']=='$SITE')")
check "gövde komut satırından" "$(pixpanel api PATCH /sites/$ID --data '{"template":"spa"}' | jget 'd["template"]')" spa
echo '{"template":"clean_urls"}' > /tmp/cli-body.json
check "gövde dosyadan" "$(pixpanel api PATCH /sites/$ID --data @/tmp/cli-body.json | jget 'd["template"]')" clean_urls
check "gövde standart girdiden" "$(echo '{"template":"default"}' | pixpanel api PATCH /sites/$ID --data - | jget 'd["template"]')" default
check "bozuk gövde" "$(pixpanel api PATCH /sites/$ID --data '{bozuk' 2>&1 | grep -c 'JSON')" 1
check "hata yanıtında çıkış kodu" "$(rc pixpanel api GET /sites/999999)" 1
check "hata yanıtının gövdesi yine yazılır" "$(pixpanel api GET /sites/999999 2>/dev/null | jget 'd["error"]["code"]')" not_found
check "geçersiz yöntem" "$(rc pixpanel api 'GE T' /sites)" 1
CRON=$(pixpanel api POST /cron --data '{"schedule":"0 4 1 1 *","command":"echo cli-gorev-ciktisi","comment":"cli sınaması"}' | jget 'd["id"]')
check "görev bekleme: günlük yazılır" "$(pixpanel api POST /cron/$CRON/run --wait 2>&1 | grep -c '^cli-gorev-ciktisi')" 1
pixpanel api PATCH /cron/$CRON --data '{"command":"exit 3"}' >/dev/null
check "görev bekleme: başarısız görev çıkış kodu" "$(rc pixpanel api POST /cron/$CRON/run --wait)" 1
check "beklemeden: görev başlar, komut hemen döner" "$(pixpanel api POST /cron/$CRON/run | jget "'job_id' in d")" True
pixpanel api DELETE /cron/$CRON >/dev/null

say "Hesaplar"
OUT=$(pixpanel user add $ACCOUNT)
PASSWORD=$(sed -n 's/^Parola: //p' <<<"$OUT")
check "hesap ekleme parola üretir" "${#PASSWORD}" 20
check "hesap listede" "$(pixpanel user list | awk -v u=$ACCOUNT '$1==u {print $2}')" user
check "üretilen parolayla giriş yapılır" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -X POST https://127.0.0.1:8443/api/v1/auth/login -d "{\"username\":\"$ACCOUNT\",\"password\":\"$PASSWORD\"}")" 200
check "verilen parola yazdırılmaz" "$(pixpanel user add ${ACCOUNT}2 --password 'Deneme-Parola-123' | grep -c Parola)" 0
check "başka hesap adına: yalnız kendi siteleri" "$(pixpanel api GET /sites --as $ACCOUNT | jget 'len(d)')" 0
check "başka hesap adına: yönetici ucu kapalı" "$(pixpanel api GET /health --as $ACCOUNT 2>/dev/null | jget 'd["error"]["code"]')" forbidden
check "olmayan hesap adına" "$(pixpanel api GET /sites --as yok-boyle-biri 2>&1 | grep -c 'bulunamadı')" 1

say "Sağlık"
pixpanel health >/tmp/cli-health.txt 2>&1; HEALTH=$?
check "sağlık çıktısı" "$([ -s /tmp/cli-health.txt ] && echo dolu)" dolu
check "çıkış kodu kritik bulguyla uyumlu" "$HEALTH" "$(grep -q '^KRİTİK' /tmp/cli-health.txt && echo 1 || echo 0)"

say "İz bırakmama"
check "geçici anahtar kalmadı (hatalı komutlardan sonra da)" "$(tokens)" 0
check "işlemler denetim günlüğünde" "$(sqlite3 "$DB" "SELECT count(*) FROM audit_log WHERE action LIKE 'site.%' AND target LIKE '%$SITE%'" 2>/dev/null | awk '{print ($1>0)?"var":"yok"}')" var

say "Panel kapalıyken"
systemctl stop pixpanel
check "anlaşılır hata" "$(pixpanel site list 2>&1 | grep -c 'bağlanılamadı')" 1
check "anahtar yine temizlenir" "$(tokens)" 0
systemctl start pixpanel
for _ in $(seq 1 30); do curl -sk -o /dev/null https://127.0.0.1:8443/ && break; sleep 1; done

say "Temizlik"
check "siteyi kaldır" "$(pixpanel site remove $SITE --delete-files | grep -c 'dosyalarıyla')" 1
for name in $ACCOUNT ${ACCOUNT}2; do
  id=$(pixpanel api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')")
  check "hesabı sil: $name" "$(rc pixpanel api DELETE "/users/$id")" 0
done
rm -f /tmp/cli-body.json /tmp/cli-wait.txt /tmp/cli-health.txt
echo "bitti"
