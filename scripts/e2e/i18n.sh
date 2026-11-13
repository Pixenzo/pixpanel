#!/usr/bin/env bash
# Sunucu metinlerinin çevirisi (Accept-Language, akışlar, haftalık özet). Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash i18n.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-i18n
DB=i18ne2e
# api <dil> <yöntem> <yol> [curl seçenekleri]; dil boşsa başlık gönderilmez.
api() { local l=$1 m=$2 p=$3; shift 3; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' ${l:+-H "accept-language: $l"} -X "$m" "$B$p" "$@"; }
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
# Metinde Türkçeye özgü harf geçen satır sayısı.
turkish_lines() { python3 -c "
import sys,re
print(sum(1 for line in sys.stdin.read().split('\n') if re.search('[çğıöşüÇĞİÖŞÜ]', line)))"; }

rm -f $J
api "" POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
ME=$(api "" GET /auth/me | jget 'd.get("id") or d["user"]["id"]')

say "Hata iletileri isteğin diline göre"
check "başlık yoksa Türkçe" "$(api "" GET /sites/99999999 | jget 'd["error"]["message"]')" "Kayıt bulunamadı"
check "en → İngilizce" "$(api en GET /sites/99999999 | jget 'd["error"]["message"]')" "Record not found"
check "tarayıcı biçimli başlıkta ilk dil geçerli" "$(api 'en-US,en;q=0.9,tr;q=0.8' GET /sites/99999999 | jget 'd["error"]["message"]')" "Record not found"
check "tr-TR → Türkçe" "$(api 'tr-TR,en;q=0.9' GET /sites/99999999 | jget 'd["error"]["message"]')" "Kayıt bulunamadı"
check "desteklenmeyen dil Türkçe kalır" "$(api de GET /sites/99999999 | jget 'd["error"]["message"]')" "Kayıt bulunamadı"
check "hata kodu değişmez" "$(api en GET /sites/99999999 | jget 'd["error"]["code"]')" "not_found"
check "durum kodu değişmez" "$(api en GET /sites/99999999 -o /dev/null -w '%{http_code}')" 404
BAD=$(api en POST /databases -d '{"engine":"mariadb","name":"x y"}')
has "doğrulama hatası İngilizce" "$BAD" "Database name"
check "doğrulama hatasında Türkçe harf kalmaz" "$(jget 'd["error"]["message"]' <<<"$BAD" | turkish_lines)" 0
has "aynı hata Türkçe" "$(api tr POST /databases -d '{"engine":"mariadb","name":"x y"}')" "Veritabanı adı"
ANON=$(curl -sk -H 'accept-language: en' $B/sites)
check "oturumsuz istekte de çevrilir" "$(jget 'd["error"]["message"]' <<<"$ANON" | turkish_lines)" 0
has "oturumsuz hata kodu" "$ANON" '"unauthorized"'
check "hatalı girişin iletisi İngilizce" "$(curl -sk -H 'accept-language: en' -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d '{"username":"yok-boyle-biri","password":"yanlis"}' | jget 'd["error"]["message"]' | turkish_lines)" 0

say "Listelerdeki sunucu metinleri"
COMP=$(api en GET /components)
has "bileşen adı çevrilir" "$COMP" "Roundcube webmail"
lacks "bileşen adı Türkçe kalmaz" "$COMP" "web postası"
check "bileşen açıklamalarında Türkçe kalmaz" "$(jget '"\n".join(str(c.get("description","")) for c in (d if isinstance(d, list) else d.get("components", [])))' <<<"$COMP" | turkish_lines)" 0
has "Türkçe istekte bileşen adı Türkçe" "$(api tr GET /components)" "Roundcube web postası"
HEALTH=$(api en GET /health)
check "bulgu başlık ve ayrıntılarında Türkçe kalmaz" "$(jget '"\n".join(f["title"] + "\n" + f["detail"] for f in d["report"]["findings"])' <<<"$HEALTH" | turkish_lines)" 0
check "bulgu anahtarları değişmez" "$(jget 'sorted(f["key"] for f in d["report"]["findings"])' <<<"$HEALTH")" "$(api tr GET /health | jget 'sorted(f["key"] for f in d["report"]["findings"])')"
check "kullanıcı verisi olan alanlar çevrilmez (site listesi aynı)" "$(api en GET /sites | jget 'sorted(s["domain"] for s in d)')" "$(api tr GET /sites | jget 'sorted(s["domain"] for s in d)')"

say "Görev başlığı, günlüğü ve canlı akışı"
OLD=$(api "" GET /databases | jget "next((x['id'] for x in d.get('databases', d) if x['name']=='$DB'), '')")
[ -n "$OLD" ] && api "" DELETE "/databases/$OLD" >/dev/null
CREATED=$(api "" POST /databases -d "{\"engine\":\"mariadb\",\"name\":\"$DB\"}")
ID=$(jget 'd.get("id") or d["database"]["id"]' <<<"$CREATED")
JOB=$(api "" POST "/databases/$ID/maintenance" -d '{"action":"check"}' | jget 'd["job_id"]')
for _ in $(seq 1 30); do
  [ "$(api "" GET "/jobs/$JOB" | jget 'd["job"]["status"]')" != running ] && break
  sleep 0.5
done
check "görev başlığı Türkçe" "$(api tr GET "/jobs/$JOB" | jget 'd["job"]["title"]')" "$DB veritabanı denetleniyor"
check "görev başlığı İngilizce" "$(api en GET "/jobs/$JOB" | jget 'd["job"]["title"]')" "Checking database $DB"
has "görev günlüğü İngilizce" "$(api en GET "/jobs/$JOB" | jget 'd["log"]')" "Checking database $DB"
has "görev günlüğü Türkçe" "$(api tr GET "/jobs/$JOB" | jget 'd["log"]')" "$DB veritabanı denetleniyor"
has "görev listesinde başlık İngilizce" "$(api en GET '/jobs?limit=20')" "Checking database $DB"
# Akışlarda dil başlıktan değil hesaptan alınır (EventSource başlık gönderemez).
has "hesap dili Türkçeyken akış Türkçe" "$(timeout 4 curl -skN -b $J "$B/jobs/$JOB/stream")" "$DB veritabanı denetleniyor"
api "" PATCH "/users/$ME" -d '{"language":"en"}' >/dev/null
STREAM=$(timeout 4 curl -skN -b $J "$B/jobs/$JOB/stream")
api "" PATCH "/users/$ME" -d '{"language":"tr"}' >/dev/null
has "hesap dili İngilizceyken akış İngilizce" "$STREAM" "Checking database $DB"
has "akış bitiş olayını taşır" "$STREAM" '"type":"done"'
check "hesap dili geri alındı" "$(api "" GET /auth/me | jget 'd.get("language") or d["user"]["language"]')" tr
api "" DELETE "/databases/$ID" >/dev/null

say "Haftalık özet"
WEEKLY=$(api en POST /health/weekly-report/send -d '{"preview":true}')
has "konu İngilizce" "$(jget 'd["subject"]' <<<"$WEEKLY")" "PixPanel weekly summary"
has "bölüm başlıkları İngilizce" "$(jget 'd["text"]' <<<"$WEEKLY")" "Checks and fixes"
has "alt bilgi İngilizce" "$(jget 'd["text"]' <<<"$WEEKLY")" "sent by PixPanel every week"
has "HTML gövde de İngilizce" "$(jget 'd["html"]' <<<"$WEEKLY")" "Certificates and backups"
has "Türkçe istekte özet Türkçe" "$(api tr POST /health/weekly-report/send -d '{"preview":true}' | jget 'd["text"]')" "Denetimler ve düzeltmeler"

say "Çeviri katmanı başka yanıtlara dokunmaz"
check "olay akışı akış olarak kalır" "$(curl -sk -o /dev/null -m 2 -w '%{content_type}' -H 'accept-language: en' -b $J $B/events)" "text/event-stream"
check "JSON yanıtın türü değişmez" "$(api en GET /sites -o /dev/null -w '%{content_type}')" "application/json"
check "çevrilen yanıt geçerli JSON" "$(api en GET /health | jget 'type(d).__name__')" dict
rm -f $J
