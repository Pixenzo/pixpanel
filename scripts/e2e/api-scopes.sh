#!/usr/bin/env bash
# API anahtarı kapsamlarının sınaması: alan kapsamlı anahtar yalnızca seçtiği alanların uçlarını
# çağırabilir, yönetim uçlarına ve anahtar üretimine giremez; kapsam alt hesabın yetkisini
# genişletmez; kapsamsız anahtar ve komut satırı (geçici anahtar) eskisi gibi çalışır.
# Test makinesinde root olarak çalışır. Kullanım: PX_USER=... PX_PASS=... bash api-scopes.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-scope
JO=/tmp/pxjar-scope-own
JM=/tmp/pxjar-scope-mem
OWNER=kapsamsahip
MEMBER=kapsamalt
PASS='Kapsam-Sinama-Parolasi-1'
SITE=kapsam-sinama.test
KEY_PREFIX=kapsam-sinama
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 200)]" ;; esac; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }

# Oturumla (çerez + CSRF başlığı) istek.
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
# API anahtarıyla istek (çerez yok).
tcall() { local tok=$1 m=$2 p=$3; shift 3; curl -sk -H "Authorization: Bearer $tok" -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
tcode() { local tok=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $tok" -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
# Bir uç 403 dışında bir şey dönüyorsa "geçti" sayılır (bileşen kurulu değilse 400 olabilir).
not403() { [ "$1" != 403 ] && [ "$1" != 401 ] && echo gecti || echo "reddedildi($1)"; }
# Ham anahtar üretir: mk_token AD 'EK JSON ALANLARI' (ör. ',"scope":["sites"]').
mk_token() { api POST /auth/tokens -d "{\"name\":\"$KEY_PREFIX-$1\"$2}" | jget 'd["token"]'; }
login() { curl -sk -o /dev/null -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$3\"}"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }

cleanup() {
  local id
  for id in $(api GET /auth/tokens | jget '" ".join(str(t["id"]) for t in d if t["name"].startswith("'$KEY_PREFIX'"))'); do api DELETE "/auth/tokens/$id" >/dev/null; done
  id=$(site_id $SITE); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null
  for id in $(api GET /cron | jget '" ".join(str(c["id"]) for c in d if "kapsam sınaması" in c.get("comment",""))'); do api DELETE "/cron/$id" >/dev/null; done
  for name in $MEMBER $OWNER; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done
  rm -f $JO $JM
}

rm -f $J $JO $JM
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
cleanup

say "Hazırlık"
T_ALL=$(mk_token tumu '')
T_SITES=$(mk_token siteler ',"scope":["sites"]')
T_SITES_RO=$(mk_token siteler-okuma ',"scope":["sites"],"read_only":true')
T_MULTI=$(mk_token coklu ',"scope":["dns","sites","dns"]')
T_CRON=$(mk_token zamanli ',"scope":["cron"]')
T_EMPTY=$(mk_token bos ',"scope":[]')
check "anahtarlar üretildi" "$(for t in "$T_ALL" "$T_SITES" "$T_SITES_RO" "$T_MULTI" "$T_CRON" "$T_EMPTY"; do case "$t" in pix_*) printf y ;; *) printf n ;; esac; done)" yyyyyy

say "Anahtar listesinde kapsam"
LIST=$(api GET /auth/tokens)
check "kapsamsız anahtar: null" "$(jget "next(t['scope'] for t in d if t['name']=='$KEY_PREFIX-tumu')" <<<"$LIST")" None
check "boş liste kapsamsız sayılır" "$(jget "next(t['scope'] for t in d if t['name']=='$KEY_PREFIX-bos')" <<<"$LIST")" None
check "kapsamlı anahtar: alan listesi" "$(jget "next(t['scope'] for t in d if t['name']=='$KEY_PREFIX-siteler')" <<<"$LIST")" "['sites']"
check "yinelenen alan teke iner, sıra tanım sırasıdır" "$(jget "next(t['scope'] for t in d if t['name']=='$KEY_PREFIX-coklu')" <<<"$LIST")" "['sites', 'dns']"
check "salt okunur bayrağı korunur" "$(jget "next(t['read_only'] for t in d if t['name']=='$KEY_PREFIX-siteler-okuma')" <<<"$LIST")" True

say "Kapsamsız anahtar bugünkü gibi çalışır"
check "site listesi" "$(tcode "$T_ALL" GET /sites)" 200
check "kullanıcı listesi (yönetici ucu)" "$(tcode "$T_ALL" GET /users)" 200
check "servisler (yönetici ucu)" "$(tcode "$T_ALL" GET /system/services)" 200
check "veritabanı özeti" "$(tcode "$T_ALL" GET /databases)" 200
check "kimlikte kapsam görünmez" "$(tcall "$T_ALL" GET /auth/me | jget "d.get('token_scope')")" None
check "boş kapsamla üretilen anahtar da tam yetkili" "$(tcode "$T_EMPTY" GET /users)" 200
check "kapsamsız anahtarla anahtar üretilemez (eski kural)" "$(tcode "$T_ALL" POST /auth/tokens -d '{"name":"kapsam-sinama-yasak"}')" 403

say "Oturumsuz ve geçersiz anahtar"
check "anahtarsız istek" "$(curl -sk -o /dev/null -w '%{http_code}' $B/sites)" 401
check "geçersiz anahtar" "$(tcode pix_yok GET /sites)" 401
check "geçersiz anahtar kimlik ucunda da" "$(tcode pix_yok GET /auth/me)" 401

say "Alan kapsamlı anahtar: [sites]"
check "kimlikte kapsam görünür" "$(tcall "$T_SITES" GET /auth/me | jget "d['token_scope']")" "['sites']"
check "site listesi" "$(tcode "$T_SITES" GET /sites)" 200
check "site seçenekleri" "$(tcode "$T_SITES" GET /sites/options)" 200
check "site ekleme" "$(tcode "$T_SITES" POST /sites -d "{\"domain\":\"$SITE\",\"kind\":\"static\"}")" 200
SID=$(site_id $SITE)
check "site ayrıntısı" "$(tcode "$T_SITES" GET /sites/$SID)" 200
check "site güncelleme" "$(tcode "$T_SITES" PATCH /sites/$SID -d '{"template":"spa"}')" 200
check "veritabanı özeti 403" "$(tcode "$T_SITES" GET /databases)" 403
check "veritabanı oluşturma 403" "$(tcode "$T_SITES" POST /databases -d '{"engine":"mariadb","name":"kapsam_sinama"}')" 403
check "veritabanı kullanıcısı 403" "$(tcode "$T_SITES" POST /databases/users -d '{}')" 403
check "kendi Redis'i 403" "$(tcode "$T_SITES" GET /redis/mine)" 403
check "posta özeti 403" "$(tcode "$T_SITES" GET /mail)" 403
check "posta kutusu açma 403" "$(tcode "$T_SITES" POST /mail/domains/1/mailboxes -d '{"local":"x","password":"UzunBirParola123"}')" 403
check "DNS özeti 403" "$(tcode "$T_SITES" GET /dns)" 403
check "DNS kaydı ekleme 403" "$(tcode "$T_SITES" POST /dns/zones/1/records -d '{"record":{"type":"A","name":"a.ornek.test","content":"203.0.113.9","ttl":300}}')" 403
check "SSL ayrı alandır: 403" "$(tcode "$T_SITES" GET /sites/$SID/ssl)" 403
check "SSL alma 403" "$(tcode "$T_SITES" POST /sites/$SID/ssl/letsencrypt -d '{}')" 403
check "FTP ayrı alandır: 403" "$(tcode "$T_SITES" GET /sites/$SID/ftp)" 403
check "dosyalar 403" "$(tcode "$T_SITES" GET /files/roots)" 403
check "zamanlı görevler 403" "$(tcode "$T_SITES" GET /cron)" 403
check "günlükler 403" "$(tcode "$T_SITES" GET /logs/sources)" 403
check "hesabın yedekleri 403" "$(tcode "$T_SITES" GET /backup/mine)" 403
check "Docker 403" "$(tcode "$T_SITES" GET /docker)" 403

say "Alan kapsamlı yönetici anahtarı yönetim uçlarına giremez"
for target in "GET /users" "GET /users/1/usage" "GET /system/overview" "GET /system/services" "GET /system/audit" "GET /system/processes" "GET /firewall" "GET /fail2ban" "GET /components" "GET /health" "GET /php" "GET /packages" "GET /branding/resellers/1" "GET /notifications" "GET /notifications/smtp" "GET /webhooks" "GET /backup" "GET /redis" "GET /redis/accounts" "GET /events" "GET /auth/sessions" "GET /auth/tokens" "GET /auth/2fa" "GET /terminal/ws" "GET /users/1/access"; do
  set -- $target
  check "$1 $2" "$(tcode "$T_SITES" $1 $2)" 403
done
check "kullanıcı ekleme" "$(tcode "$T_SITES" POST /users -d '{"username":"kapsamyasak","password":"UzunBirParola123","role":"user"}')" 403
check "kullanıcı ekleme gerçekten yapılmadı" "$(user_id kapsamyasak)" ""
check "hizmet yeniden başlatma" "$(tcode "$T_SITES" POST /system/services/nginx/restart)" 403
check "güvenlik duvarı uygulama" "$(tcode "$T_SITES" PUT /firewall -d '{}')" 403
check "bileşen kurma" "$(tcode "$T_SITES" POST /components/redis/install)" 403
check "parola değiştirme" "$(tcode "$T_SITES" POST /auth/password -d '{"current_password":"x","new_password":"yeniparola12345"}')" 403
check "kendi kapsamını genişletecek anahtar üretme" "$(tcode "$T_SITES" POST /auth/tokens -d '{"name":"kapsam-sinama-genis"}')" 403
check "kapsamlı anahtarla kapsamlı anahtar da üretilemez" "$(tcode "$T_SITES" POST /auth/tokens -d '{"name":"kapsam-sinama-genis2","scope":["sites"]}')" 403
check "anahtar silme" "$(tcode "$T_SITES" DELETE /auth/tokens/1)" 403
check "kapsamlı anahtar üretilmedi" "$(api GET /auth/tokens | jget "len([t for t in d if t['name'].startswith('$KEY_PREFIX-genis')])")" 0
check "olmayan uç 404 verir (kapı yol tablosuyla değil, eşleşen uçla çalışır)" "$(tcode "$T_SITES" GET /users/1/olmayan)" 404

say "Görevler: kapsamdaki alanın görevleri görünür"
CRON=$(api POST /cron -d '{"schedule":"0 4 1 1 *","command":"echo kapsam-gorev","comment":"kapsam sınaması"}' | jget 'd["id"]')
JOB=$(api POST /cron/$CRON/run | jget 'd["job_id"]')
check "cron kapsamlı anahtar görevi görür" "$(tcode "$T_CRON" GET /jobs/$JOB)" 200
check "cron kapsamlı anahtar görev listesinde" "$(tcall "$T_CRON" GET /jobs | jget "len([j for j in d if j['id']==$JOB])")" 1
check "sites kapsamlı anahtar cron görevini göremez" "$(tcode "$T_SITES" GET /jobs/$JOB)" 404
check "sites kapsamlı anahtarın görev listesinde yok" "$(tcall "$T_SITES" GET /jobs | jget "len([j for j in d if j['id']==$JOB])")" 0
check "görev akışı da süzülür" "$(tcode "$T_SITES" GET /jobs/$JOB/stream)" 404
api DELETE /cron/$CRON >/dev/null

say "Salt okunur + kapsamlı anahtar"
check "okuma kapsamda" "$(tcode "$T_SITES_RO" GET /sites)" 200
check "yazma reddedilir" "$(tcode "$T_SITES_RO" POST /sites -d '{"domain":"kapsam-okuma.test","kind":"static"}')" 403
check "site oluşmadı" "$(site_id kapsam-okuma.test)" ""
check "silme reddedilir" "$(tcode "$T_SITES_RO" DELETE /sites/$SID)" 403
check "kapsam dışı okuma de reddedilir" "$(tcode "$T_SITES_RO" GET /databases)" 403

say "Çok alanlı kapsam: [sites, dns]"
check "siteler" "$(tcode "$T_MULTI" GET /sites)" 200
check "DNS kapsamda (403 değil)" "$(not403 $(tcode "$T_MULTI" GET /dns))" gecti
check "veritabanı kapsam dışı" "$(tcode "$T_MULTI" GET /databases)" 403
check "posta kapsam dışı" "$(tcode "$T_MULTI" GET /mail)" 403
check "DNS sunucusu yönetimi: kapsamdaki alanın yönetici ucu" "$(not403 $(tcode "$T_MULTI" GET /dns/settings))" gecti
check "yine de kullanıcı yönetimi kapalı" "$(tcode "$T_MULTI" GET /users)" 403

say "Geçersiz alan adı ve biçim"
check "bilinmeyen alan" "$(code POST /auth/tokens -d '{"name":"kapsam-sinama-x","scope":["bilinmeyen"]}')" 400
check "yönetim alanı kapsama alınamaz (users)" "$(code POST /auth/tokens -d '{"name":"kapsam-sinama-x","scope":["users"]}')" 400
check "boş ad" "$(code POST /auth/tokens -d '{"name":"kapsam-sinama-x","scope":["sites",""]}')" 400
check "büyük harf farklı alandır" "$(code POST /auth/tokens -d '{"name":"kapsam-sinama-x","scope":["Sites"]}')" 400
has "hata iletisi alanı söyler" "$(api POST /auth/tokens -d '{"name":"kapsam-sinama-x","scope":["bilinmeyen"]}')" "bilinmeyen"
check "reddedilen istek anahtar bırakmaz" "$(api GET /auth/tokens | jget "len([t for t in d if t['name']=='kapsam-sinama-x'])")" 0

say "Alt hesap kendi yetkisinde olmayan alanı kapsama alamaz"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$OWNER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
login $JO $OWNER "$PASS"
check "müşteri alt hesap açar (yetki: siteler)" "$(status $JO POST /users -d "{\"username\":\"$MEMBER\",\"password\":\"$PASS\",\"role\":\"member\",\"permissions\":[\"sites\"]}")" 200
login $JM $MEMBER "$PASS"
check "alt hesap oturumu çalışır" "$(status $JM GET /sites)" 200
check "yetkisi olmayan alan: 400" "$(status $JM POST /auth/tokens -d '{"name":"kapsam-sinama-alt1","scope":["dns"]}')" 400
check "yetkili + yetkisiz karışık: 400" "$(status $JM POST /auth/tokens -d '{"name":"kapsam-sinama-alt2","scope":["sites","dns"]}')" 400
check "geçersiz alan: 400" "$(status $JM POST /auth/tokens -d '{"name":"kapsam-sinama-alt3","scope":["xyz"]}')" 400
check "yetkisi içindeki alan: 200" "$(status $JM POST /auth/tokens -d '{"name":"kapsam-sinama-alt4","scope":["sites"]}')" 200
T_MEM=$(call $JM POST /auth/tokens -d '{"name":"kapsam-sinama-alt5","scope":["sites"]}' | jget 'd["token"]')
T_MEM_ALL=$(call $JM POST /auth/tokens -d '{"name":"kapsam-sinama-alt6"}' | jget 'd["token"]')
check "alt hesabın kapsamlı anahtarı kendi alanında çalışır" "$(tcode "$T_MEM" GET /sites)" 200
check "kapsam dışı alan reddedilir" "$(tcode "$T_MEM" GET /databases)" 403
check "alt hesabın kapsamsız anahtarı da yetkisini aşamaz" "$(tcode "$T_MEM_ALL" GET /dns)" 403
check "alt hesabın anahtarı yönetim ucuna giremez" "$(tcode "$T_MEM" GET /users)" 403
# Üst hesap sonradan alt hesabın yetkisini daraltırsa kapsam kendiliğinden daralır (kesişim anlık hesaplanır).
MID=$(call $JO GET /users | jget "next(u['id'] for u in d if u['username']=='$MEMBER')")
check "alt hesabın siteler yetkisi alınır" "$(status $JO PATCH /users/$MID -d '{"permissions":["files"]}')" 200
check "kapsamda olsa da artık yetkisiz: 403" "$(tcode "$T_MEM" GET /sites)" 403

say "Komut satırı (geçici anahtar) çalışmaya devam eder"
check "pixpanel api GET /auth/me (varsayılan: yönetici)" "$(pixpanel api GET /auth/me | jget "d['role']")" admin
check "kimlikte kapsam yok" "$(pixpanel api GET /auth/me | jget "d.get('token_scope')")" None
check "pixpanel site list" "$(pixpanel site list >/dev/null 2>&1; echo $?)" 0
check "yönetici ucu komut satırından" "$(pixpanel api GET /system/overview >/dev/null 2>&1; echo $?)" 0
check "komut satırı kullanıcı listesi" "$(pixpanel user list >/dev/null 2>&1; echo $?)" 0
check "geçici anahtar işi bitince silinir" "$(sqlite3 /var/lib/pixpanel/pixpanel.db "SELECT count(*) FROM api_tokens WHERE name = 'pixpanel-cli'")" 0

say "Denetim kaydı"
check "anahtar üretimi kayda geçer" "$(api GET '/system/audit?limit=500' | jget "len([e for e in d if e['action']=='token.create'])>0")" True

say "Temizlik"
cleanup
check "sınama anahtarları silindi" "$(api GET /auth/tokens | jget "len([t for t in d if t['name'].startswith('$KEY_PREFIX')])")" 0
check "sınama sitesi silindi" "$(site_id $SITE)" ""
check "sınama hesapları silindi" "$(user_id $OWNER)$(user_id $MEMBER)" ""
rm -f $J
