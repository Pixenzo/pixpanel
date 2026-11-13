#!/usr/bin/env bash
# Sunucu metinlerinin çevirisi: dil hesabın kendi ayarından gelir; oturum yokken tarayıcının
# diline bakılır; yeni hesabın dili ilk girişte belirlenir. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash i18n.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-i18n
JN=/tmp/pxjar-i18n-new
DB=i18ne2e
NEWUSER=dildeneme
NEWPASS='Dil-Deneme-24680'
# api <başlık dili> <yöntem> <yol> [curl seçenekleri]; dil boşsa Accept-Language gönderilmez.
api() { local l=$1 m=$2 p=$3; shift 3; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' ${l:+-H "accept-language: $l"} -X "$m" "$B$p" "$@"; }
# anon: oturumsuz istek.
anon() { local l=$1 m=$2 p=$3; shift 3; curl -sk -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' ${l:+-H "accept-language: $l"} -X "$m" "$B$p" "$@"; }
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
message() { jget 'd["error"]["message"]'; }

rm -f $J $JN
api "" POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
ME=$(api "" GET /auth/me | jget 'd.get("id") or d["user"]["id"]')
# lang <dil>: oturumdaki hesabın dilini değiştirir.
lang() { api "" PATCH "/users/$ME" -d "{\"language\":\"$1\"}" >/dev/null; }

say "Oturum yokken tarayıcının dili geçerli"
check "başlık yoksa Türkçe" "$(anon "" GET /sites | message)" "Oturum açmanız gerekiyor"
check "en başlığıyla İngilizce" "$(anon en GET /sites | message | turkish_lines)" 0
check "tarayıcı biçimli başlıkta ilk dil geçerli" "$(anon 'en-US,en;q=0.9,tr;q=0.8' GET /sites | message | turkish_lines)" 0
check "tr-TR başlığıyla Türkçe" "$(anon 'tr-TR,en;q=0.9' GET /sites | message)" "Oturum açmanız gerekiyor"
has "hata kodu değişmez" "$(anon en GET /sites)" '"unauthorized"'
check "hatalı girişin iletisi İngilizce" "$(anon en POST /auth/login -d '{"username":"yok-boyle-biri","password":"yanlis"}' | message | turkish_lines)" 0
has "hatalı girişin iletisi Türkçe" "$(anon tr POST /auth/login -d '{"username":"yok-boyle-biri","password":"yanlis"}')" "Kullanıcı adı ya da parola hatalı"

say "Oturum varken hesabın dili geçerli, başlık değil"
lang tr
check "hesap Türkçe, başlık en → Türkçe" "$(api en GET /sites/99999999 | message)" "Kayıt bulunamadı"
lang en
check "hesap İngilizce, başlık yok → İngilizce" "$(api "" GET /sites/99999999 | message)" "Record not found"
check "hesap İngilizce, başlık tr → İngilizce" "$(api tr GET /sites/99999999 | message)" "Record not found"
check "hata kodu değişmez" "$(api "" GET /sites/99999999 | jget 'd["error"]["code"]')" "not_found"
check "durum kodu değişmez" "$(api "" GET /sites/99999999 -o /dev/null -w '%{http_code}')" 404
BAD=$(api "" POST /databases -d '{"engine":"mariadb","name":"x y"}')
has "doğrulama hatası İngilizce" "$BAD" "Database name"
check "doğrulama hatasında Türkçe harf kalmaz" "$(message <<<"$BAD" | turkish_lines)" 0
COMP=$(api "" GET /components)
has "bileşen adı çevrilir" "$COMP" "Roundcube webmail"
lacks "bileşen adı Türkçe kalmaz" "$COMP" "web postası"
check "bileşen açıklamalarında Türkçe kalmaz" "$(jget '"\n".join(str(c.get("description","")) for c in (d if isinstance(d, list) else d.get("components", [])))' <<<"$COMP" | turkish_lines)" 0
HEALTH=$(api "" GET /health)
check "bulgu başlık ve ayrıntılarında Türkçe kalmaz" "$(jget '"\n".join(f["title"] + "\n" + f["detail"] for f in d["report"]["findings"])' <<<"$HEALTH" | turkish_lines)" 0
EN_KEYS=$(jget 'sorted(f["key"] for f in d["report"]["findings"])' <<<"$HEALTH")
EN_SITES=$(api "" GET /sites | jget 'sorted(s["domain"] for s in d)')
check "çevrilen yanıt geçerli JSON" "$(jget 'type(d).__name__' <<<"$HEALTH")" dict
check "çevrilen yanıtın türü JSON" "$(api "" GET /health -o /dev/null -w '%{content_type}')" "application/json"
check "olay akışı akış olarak kalır" "$(curl -sk -o /dev/null -m 2 -w '%{content_type}' -b $J $B/events)" "text/event-stream"
WEEKLY=$(api "" POST /health/weekly-report/send -d '{"preview":true}')
has "haftalık özetin konusu İngilizce" "$(jget 'd["subject"]' <<<"$WEEKLY")" "PixPanel weekly summary"
has "bölüm başlıkları İngilizce" "$(jget 'd["text"]' <<<"$WEEKLY")" "Checks and fixes"
has "alt bilgi İngilizce" "$(jget 'd["text"]' <<<"$WEEKLY")" "sent by PixPanel every week"
has "HTML gövde de İngilizce" "$(jget 'd["html"]' <<<"$WEEKLY")" "Certificates and backups"
lang tr
has "hesap Türkçeyken bileşen adı Türkçe" "$(api en GET /components)" "Roundcube web postası"
has "hesap Türkçeyken doğrulama hatası Türkçe" "$(api en POST /databases -d '{"engine":"mariadb","name":"x y"}')" "Veritabanı adı"
check "bulgu anahtarları dile göre değişmez" "$EN_KEYS" "$(api "" GET /health | jget 'sorted(f["key"] for f in d["report"]["findings"])')"
check "kullanıcı verisi olan alanlar çevrilmez (site listesi aynı)" "$EN_SITES" "$(api "" GET /sites | jget 'sorted(s["domain"] for s in d)')"
has "hesap Türkçeyken haftalık özet Türkçe" "$(api en POST /health/weekly-report/send -d '{"preview":true}' | jget 'd["text"]')" "Denetimler ve düzeltmeler"

say "Yeni hesabın dili ilk girişte tarayıcıdan belirlenir"
newuser() {
  local old
  old=$(api "" GET /users | jget "next((u['id'] for u in d if u['username']=='$NEWUSER'), '')")
  [ -n "$old" ] && api "" DELETE "/users/$old" >/dev/null
  api "" POST /users -d "{\"username\":\"$NEWUSER\",\"password\":\"$NEWPASS\",\"role\":\"user\",\"email\":\"\"}" >/dev/null
}
userlang() { api "" GET /users | jget "next(u['language'] for u in d if u['username']=='$NEWUSER')"; }
newuser
check "yeni hesabın dili boş (henüz belirlenmedi)" "[$(userlang)]" "[]"
anon "" POST /auth/login -d "{\"username\":\"$NEWUSER\",\"password\":\"yanlis-parola\",\"language\":\"en\"}" >/dev/null
check "hatalı giriş dili belirlemez" "[$(userlang)]" "[]"
anon "" POST /auth/login -c $JN -d "{\"username\":\"$NEWUSER\",\"password\":\"$NEWPASS\",\"language\":\"en\"}" >/dev/null
check "ilk girişte tarayıcı dili kaydedilir" "$(userlang)" en
check "yeni hesap yanıtları kendi dilinde alır" "$(curl -sk -b $JN $B/sites/99999999 | message)" "Record not found"
check "kimlik ucunda dil görünür" "$(curl -sk -b $JN $B/auth/me | jget 'd.get("language") or d["user"]["language"]')" en
anon "" POST /auth/login -d "{\"username\":\"$NEWUSER\",\"password\":\"$NEWPASS\",\"language\":\"tr\"}" >/dev/null
check "sonraki girişler dili değiştirmez" "$(userlang)" en
newuser
anon "tr-TR,tr;q=0.9" POST /auth/login -d "{\"username\":\"$NEWUSER\",\"password\":\"$NEWPASS\"}" >/dev/null
check "dil bildirilmezse başlıktan alınır (tr)" "$(userlang)" tr
newuser
anon "en-GB" POST /auth/login -d "{\"username\":\"$NEWUSER\",\"password\":\"$NEWPASS\",\"language\":\"xx\"}" >/dev/null
check "geçersiz dil değeri yok sayılır, başlık geçerli (en)" "$(userlang)" en
api "" DELETE "/users/$(api "" GET /users | jget "next(u['id'] for u in d if u['username']=='$NEWUSER')")" >/dev/null
rm -f $JN
check "var olan hesabın dili değişmedi" "$(api "" GET /auth/me | jget 'd.get("language") or d["user"]["language"]')" tr

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
check "görev başlığı Türkçe" "$(api en GET "/jobs/$JOB" | jget 'd["job"]["title"]')" "$DB veritabanı denetleniyor"
has "görev günlüğü Türkçe" "$(api en GET "/jobs/$JOB" | jget 'd["log"]')" "$DB veritabanı denetleniyor"
has "hesap dili Türkçeyken akış Türkçe" "$(timeout 4 curl -skN -b $J "$B/jobs/$JOB/stream")" "$DB veritabanı denetleniyor"
lang en
check "görev başlığı İngilizce" "$(api "" GET "/jobs/$JOB" | jget 'd["job"]["title"]')" "Checking database $DB"
has "görev günlüğü İngilizce" "$(api "" GET "/jobs/$JOB" | jget 'd["log"]')" "Checking database $DB"
has "görev listesinde başlık İngilizce" "$(api "" GET '/jobs?limit=20')" "Checking database $DB"
STREAM=$(timeout 4 curl -skN -b $J "$B/jobs/$JOB/stream")
lang tr
has "hesap dili İngilizceyken akış İngilizce" "$STREAM" "Checking database $DB"
has "akış bitiş olayını taşır" "$STREAM" '"type":"done"'
check "hesap dili geri alındı" "$(api "" GET /auth/me | jget 'd.get("language") or d["user"]["language"]')" tr
api "" DELETE "/databases/$ID" >/dev/null
rm -f $J
