#!/usr/bin/env bash
# Birden çok IP sınaması: adres ekleme, hesaba ayırma, siteyi adrese bağlama, DNS kaydı.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash ips.sh
# Eklenen adres yalnızca bu makinede anlamlıdır (/32, yerel ağda duyurulmaz).
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-ips
JC=/tmp/pxjar-ips-customer
EXTRA=10.77.0.5
SECOND=10.77.0.6
SITE=ozel-adres.test
SHARED=ortak-adres.test
CUSTOMER=ipmusteri
PASS='Ip-Sinama-Parolasi-1'
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
# at <adres> <alan adı>: sitenin o adresten verdiği yanıt.
at() { curl -s -m 5 -H "Host: $2" "http://$1/"; }
entry() { api GET /system/ips | jget "next(($2) for a in d['addresses'] if a['address']=='$1')"; }
on_host() { ip -o addr show | grep -c " $1/"; }
q() { dig +short +time=2 +tries=1 @127.0.0.1 "$@" | tr '\n' ' ' | sed 's/ $//'; }
MAIN=$(ip -4 -o addr show scope global | awk '$2 !~ /^(docker|br-|veth)/ {print $4}' | head -1 | cut -d/ -f1)

rm -f $J $JC
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
# Önceki koşudan kalanlar.
for zone in $(api GET /dns | jget '" ".join(str(z["id"]) for z in d["zones"] if z["domain"] in ("'$SITE'",))'); do api DELETE /dns/zones/$zone >/dev/null; done
for name in $SITE $SHARED musteri-$SITE; do id=$(site_id $name); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null; done
OLD=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$CUSTOMER'), '')")
[ -n "$OLD" ] && api DELETE "/users/$OLD" >/dev/null
for address in $EXTRA $SECOND; do api DELETE /system/ips/$address >/dev/null; ip addr del $address/32 dev eth0 2>/dev/null; done

say "Adres listesi"
check "sunucunun adresi listede" "$(entry $MAIN '(a["present"], a["managed"], a["family"])')" "(True, False, 'ipv4')"
check "konteyner köprüleri listede yok" "$(api GET /system/ips | jget 'len([a for a in d["addresses"] if a["interface"].startswith(("docker","br-","lo"))])')" 0
check "arayüz seçenekleri" "$(api GET /system/ips | jget '"eth0" in d["interfaces"]')" True

say "Adres ekleme"
check "geri döngü adresi" "$(code POST /system/ips -d '{"address":"127.0.0.2","prefix":8}')" 400
check "adres olmayan metin" "$(code POST /system/ips -d '{"address":"10.0.0.1; id","prefix":24}')" 400
check "geçersiz önek" "$(code POST /system/ips -d "{\"address\":\"$EXTRA\",\"prefix\":33}")" 400
check "olmayan arayüz" "$(code POST /system/ips -d "{\"address\":\"$EXTRA\",\"prefix\":32,\"interface\":\"eth9\"}")" 400
check "zaten tanımlı adres" "$(code POST /system/ips -d "{\"address\":\"$MAIN\",\"prefix\":24}")" 409
check "reddedilen istek sunucuya adres eklemedi" "$(on_host $EXTRA)" 0
check "ekle" "$(code POST /system/ips -d "{\"address\":\"$EXTRA\",\"prefix\":32,\"label\":\"Sınama adresi\"}")" 200
check "adres sunucuda" "$(on_host $EXTRA)" 1
check "listede panelin eklediği adres olarak görünür" "$(entry $EXTRA '(a["present"], a["managed"], a["label"], a["interface"], a["prefix"])')" "(True, True, 'Sınama adresi', 'eth0', 32)"
check "açılış birimi kuruldu" "$(systemctl is-enabled pixpanel-addresses.service 2>/dev/null) $(grep -c 'Before=nginx.service' /etc/systemd/system/pixpanel-addresses.service)" "enabled 1"

say "Siteyi adrese bağlama"
S=$(api POST /sites -d "{\"domain\":\"$SITE\",\"kind\":\"static\"}" | jget 'd["id"]')
O=$(api POST /sites -d "{\"domain\":\"$SHARED\",\"kind\":\"static\"}" | jget 'd["id"]')
echo "özel" > /home/$PX_USER/sites/$SITE/public/index.html; echo "ortak" > /home/$PX_USER/sites/$SHARED/public/index.html
chown $PX_USER:www-data /home/$PX_USER/sites/$SITE/public/index.html /home/$PX_USER/sites/$SHARED/public/index.html
check "yeni site bütün adreslerden yayınlanır" "$(api GET /sites/$S | jget 'd["ip"]')|$(at 127.0.0.1 $SITE)|$(at $EXTRA $SITE)" "|özel|özel"
check "seçenekler adresleri taşır" "$(api GET /sites/options | jget "{'$MAIN','$EXTRA'} <= {i['address'] for i in d['ips']}")" True
check "sunucuda olmayan adres" "$(code PATCH /sites/$S -d '{"ip":"10.77.0.99"}')" 400
check "adres olmayan metin" "$(code PATCH /sites/$S -d '{"ip":"*; return 200"}')" 400
check "adrese bağla" "$(api PATCH /sites/$S -d "{\"ip\":\"$EXTRA\"}" | jget 'd["ip"]')" $EXTRA
check "web sunucusu yalnız o adresi dinler" "$(grep -c "listen $EXTRA:80;" /etc/nginx/pixpanel/sites/$SITE.conf) $(grep -c 'listen 80;' /etc/nginx/pixpanel/sites/$SITE.conf)" "1 0"
check "site kendi adresinden yanıt verir" "$(at $EXTRA $SITE)" özel
check "site başka adresten yanıt vermez" "$([ "$(at 127.0.0.1 $SITE)" = özel ] && echo verdi || echo vermedi)" vermedi
check "özel adrese gelen bilinmeyen ad o siteye düşer" "$(at $EXTRA bilinmeyen.test)" özel
check "ortak site özel adresten yanıt vermez" "$([ "$(at $EXTRA $SHARED)" = ortak ] && echo verdi || echo vermedi)" vermedi
check "ortak site öteki adreslerde çalışmayı sürdürür" "$(at 127.0.0.1 $SHARED)" ortak
check "listede adresin site sayısı" "$(entry $EXTRA 'a["site_count"]')" 1
check "site bağlıyken adres kaldırılamaz" "$(code DELETE /system/ips/$EXTRA)" 409
check "panelin eklemediği adres kaldırılamaz" "$(code DELETE /system/ips/$MAIN)" 404
api POST /health/check >/dev/null
check "sağlık yoklaması siteyi kendi adresinden bulur" "$(api GET /sites/$S/uptime | jget 'd["last_status"]')" 200

say "DNS"
check "dış adres başka türden olamaz" "$(code PUT /system/ips/$EXTRA -d '{"label":"x","public_address":"2001:db8::1"}')" 400
Z=$(api POST /dns/zones -d "{\"domain\":\"$SITE\",\"provider\":\"local\"}" | jget 'd.get("zone", d)["id"]')
check "bölge sitenin adresiyle açılır" "$(q A $SITE) $(q A www.$SITE)" "$EXTRA $EXTRA"
check "ad sunucusu kayıtları sunucunun genel adresinde kalır" "$(q A ns1.$SITE)" "$(api GET /system/ips | jget 'd["dns_address"]')"
check "dış adres tanımla" "$(code PUT /system/ips/$EXTRA -d '{"label":"Sınama adresi","public_address":"203.0.113.9"}')" 200
check "dış adres tanımlanınca kayıtlar ona çevrilir" "$(q A $SITE) $(q A www.$SITE)" "203.0.113.9 203.0.113.9"
check "site ortak adrese alınır" "$(api PATCH /sites/$S -d '{"ip":""}' | jget 'd["ip"]')" ""
check "kayıtlar sunucunun genel adresine döner" "$(q A $SITE)" "$(api GET /system/ips | jget 'd["dns_address"]')"
api PATCH /sites/$S -d "{\"ip\":\"$EXTRA\"}" >/dev/null
check "yeniden bağlanınca kayıt dış adresi gösterir" "$(q A $SITE)" 203.0.113.9
api POST /dns/zones/$Z/records/delete -d "{\"record\":{\"id\":\"\",\"type\":\"A\",\"name\":\"$SITE\",\"content\":\"203.0.113.9\",\"ttl\":3600,\"priority\":null,\"proxied\":null}}" >/dev/null
api POST /dns/zones/$Z/records -d "{\"record\":{\"id\":\"\",\"type\":\"A\",\"name\":\"$SITE\",\"content\":\"198.51.100.4\",\"ttl\":3600,\"priority\":null,\"proxied\":null}}" >/dev/null
api PATCH /sites/$S -d '{"ip":""}' >/dev/null
check "elle başka yere yönlendirilmiş kayda dokunulmaz" "$(q A $SITE)" 198.51.100.4
api PATCH /sites/$S -d "{\"ip\":\"$EXTRA\"}" >/dev/null
api DELETE /dns/zones/$Z >/dev/null

say "Hesaba ayırma"
check "müşteri hesabı" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
C=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
curl -sk -o /dev/null -c $JC -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\"}"
check "müşteri adres listesini göremez" "$(status $JC GET /system/ips)" 403
check "müşteri adres ekleyemez" "$(status $JC POST /system/ips -d "{\"address\":\"$SECOND\",\"prefix\":32}")" 403
check "ayrılmamışken müşterinin seçeneği yok" "$(call $JC GET /sites/options | jget 'len(d["ips"])')" 0
CS=$(call $JC POST /sites -d "{\"domain\":\"musteri-$SITE\",\"kind\":\"static\"}" | jget 'd["id"]')
check "müşteri ayrılmamış adrese bağlanamaz" "$(status $JC PATCH /sites/$CS -d "{\"ip\":\"$EXTRA\"}")" 400
check "müşteri sunucunun ana adresine de bağlanamaz" "$(status $JC PATCH /sites/$CS -d "{\"ip\":\"$MAIN\"}")" 400
check "ikinci adresi ekle" "$(code POST /system/ips -d "{\"address\":\"$SECOND\",\"prefix\":32}")" 200
check "olmayan hesaba ayırma" "$(code PUT /system/ips/$SECOND -d '{"label":"","user_id":999999}')" 400
check "sunucuda olmayan adresi tanımlama" "$(code PUT /system/ips/10.77.0.99 -d '{"label":"x"}')" 404
check "müşteriye ayır" "$(code PUT /system/ips/$SECOND -d "{\"label\":\"Müşteri adresi\",\"user_id\":$C}")" 200
check "ayrılan adrese bağlı başka hesabın sitesi ortak adrese döner" "$(api PATCH /sites/$O -d "{\"ip\":\"$SECOND\"}" | jget 'd["ip"]') $(code PUT /system/ips/$SECOND -d "{\"label\":\"Müşteri adresi\",\"user_id\":$C}") $(api GET /sites/$O | jget 'd["ip"]')|$(at 127.0.0.1 $SHARED)" "$SECOND 200 |ortak"
check "listede sahibi görünür" "$(entry $SECOND '(a["owner"], a["label"])')" "('$CUSTOMER', 'Müşteri adresi')"
check "müşterinin seçeneği yalnız kendi adresi" "$(call $JC GET /sites/options | jget '[i["address"] for i in d["ips"]]')" "['$SECOND']"
check "müşteri kendi adresine bağlanır" "$(call $JC PATCH /sites/$CS -d "{\"ip\":\"$SECOND\"}" | jget 'd["ip"]')" $SECOND
check "müşteri başkasının adresine bağlanamaz" "$(status $JC PATCH /sites/$CS -d "{\"ip\":\"$EXTRA\"}")" 400
call $JC DELETE "/sites/$CS?delete_files=true" >/dev/null
CS=$(call $JC POST /sites -d "{\"domain\":\"musteri-$SITE\",\"kind\":\"static\"}" | jget 'd["id"]')
check "ayrılmış adresi olan hesabın yeni sitesi o adresle açılır" "$(call $JC GET /sites/$CS | jget 'd["ip"]') $(grep -c "listen $SECOND:80;" /etc/nginx/pixpanel/sites/musteri-$SITE.conf)" "$SECOND 1"

say "Yeniden başlatma"
ip addr del $EXTRA/32 dev eth0; ip addr del $SECOND/32 dev eth0
check "adresi kaybolan site düzenlenince bütün adreslere döner" "$(code PATCH /sites/$S -d '{"hsts":false}') $(at 127.0.0.1 $SITE)" "200 özel"
check "listede eksik adres görünür" "$(entry $EXTRA '(a["present"], a["managed"])')" "(False, True)"
check "açılış komutu adresleri yeniden kurar" "$(pixpanel apply-addresses) $(on_host $EXTRA) $(on_host $SECOND)" "2 adres kuruldu. 1 1"
check "ikinci çalıştırma bir şey yapmaz" "$(pixpanel apply-addresses)" "0 adres kuruldu."
check "site yeniden kendi adresine döner" "$(code PATCH /sites/$S -d '{"hsts":false}') $(at $EXTRA $SITE) $([ "$(at 127.0.0.1 $SITE)" = özel ] && echo verdi || echo vermedi)" "200 özel vermedi"

say "Temizlik"
check "siteleri sil" "$(code DELETE "/sites/$S?delete_files=true") $(code DELETE "/sites/$O?delete_files=true") $(code DELETE "/sites/$CS?delete_files=true")" "200 200 200"
check "hesabı sil" "$(code DELETE /users/$C)" 200
check "hesabı silinen adres boşa çıkar" "$(entry $SECOND 'a["user_id"]')" None
check "adresleri kaldır" "$(code DELETE /system/ips/$EXTRA) $(code DELETE /system/ips/$SECOND)" "200 200"
check "adresler sunucudan kalktı" "$(on_host $EXTRA) $(on_host $SECOND)" "0 0"
check "listeden de kalktı" "$(api GET /system/ips | jget "len([a for a in d['addresses'] if a['address'] in ('$EXTRA','$SECOND')])")" 0
rm -f $J $JC
echo "bitti"
