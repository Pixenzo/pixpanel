#!/usr/bin/env bash
# Hesap başına Redis sınaması: her hesabın ayrı örneği, yalıtım, parola, bellek sınırı.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash redis-account.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-ra
JA=/tmp/pxjar-ra-a
JB=/tmp/pxjar-ra-b
A=redisbir
O=redisiki
PASS='Redis-Sinama-Parolasi-1'
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
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
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
login() { curl -sk -o /dev/null -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$PASS\"}"; }
# rc <port> <parola> <komut...>: örneğe redis-cli ile bağlanır.
rc() { local port=$1 pass=$2; shift 2; redis-cli -p "$port" -a "$pass" --no-auth-warning "$@" 2>&1; }
cmd() { call "$1" POST /redis/command -d "$(python3 -c 'import json,sys; print(json.dumps({"db":0,"command":sys.argv[1]}))' "$2")"; }
field() { call "$1" GET /redis/mine | jget "d['instance']['$2']"; }
# Komut beklenen çıktıyı verene kadar (en çok ~10 sn) yeniden denenir; son çıktıyı yazar.
until_eq() { local want=$1 got=; shift; for _ in $(seq 1 20); do got=$("$@" 2>/dev/null); [ "$got" = "$want" ] && break; sleep 0.5; done; echo "$got"; }
# Hesap örneğinin maxmemory değeri (bayt).
maxmem() { rc $PA "$WA" info memory | tr -d '\r' | sed -n 's/^maxmemory://p'; }
# Önceki koşudan kalan sınama paketleri silinir (hesaplar önce silinmiş olmalı).
drop_packages() { for package in $(api GET /packages | jget "' '.join(str(p['id']) for p in d if p['name'].startswith('e2e-redis'))"); do api DELETE /packages/$package >/dev/null; done; }

rm -f $J $JA $JB
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in $A $O; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done
drop_packages

say "Hesaplar"
check "birinci hesap" "$(code POST /users -d "{\"username\":\"$A\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
check "ikinci hesap" "$(code POST /users -d "{\"username\":\"$O\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
login $JA $A; login $JB $O
check "başlangıçta örnek yok" "$(call $JA GET /redis/mine | jget '(d["available"], d["instance"])')" "(True, None)"
has "örnek yokken tarayıcı çalışmaz" "$(call $JA GET '/redis/keys?db=0')" "Redis açık değil"
check "müşteri ortak Redis'in ayarını göremez" "$(status $JA GET /redis)" 403
check "müşteri öteki hesapların listesini göremez" "$(status $JA GET /redis/accounts)" 403

say "Örneği açma"
check "aç" "$(call $JA POST /redis/mine/enable | jget '(d["instance"]["running"], d["instance"]["host"], d["instance"]["memory_mb"], len(d["instance"]["password"]))')" "(True, '127.0.0.1', 64, 48)"
PA=$(field $JA port); WA=$(field $JA password); SA=$(field $JA socket)
check "ikinci kez açmak aynı örneği verir" "$(call $JA POST /redis/mine/enable | jget '(d["instance"]["port"], d["instance"]["password"])')" "($PA, '$WA')"
check "parolayla bağlanılır" "$(rc $PA "$WA" ping)" PONG
has "parolasız bağlanılamaz" "$(redis-cli -p $PA ping 2>&1)" NOAUTH
has "yanlış parola" "$(rc $PA yanlis ping)" WRONGPASS
check "süreç hesabın kendi kullanıcısıyla çalışır" "$(ps -o user= -p "$(systemctl show -p MainPID --value pixpanel-redis-$A.service)" | tr -d ' ')" $A
check "yalnız yerel adresi dinler" "$(ss -ltnH "sport = :$PA" | awk '{print $4}' | sort -u | tr '\n' ' ' | sed 's/ $//')" "127.0.0.1:$PA"
check "açılışta kendiliğinden başlar" "$(systemctl is-enabled pixpanel-redis-$A.service)" enabled
check "bellek sınırı uygulanmış" "$(rc $PA "$WA" info memory | tr -d '\r' | sed -n 's/^maxmemory://p')" 67108864
has "çalışma anında ayar değiştirilemez" "$(rc $PA "$WA" config set bind 0.0.0.0)" "unknown command"
has "sunucu kapatılamaz" "$(rc $PA "$WA" shutdown nosave)" "unknown command"
check "yapılandırmayı başka hesap okuyamaz" "$(stat -c '%a %U:%G' /etc/pixpanel/redis/$A.conf)" "640 root:$A"
check "soketten bağlanılır (hesabın kendisi)" "$(runuser -u $A -- redis-cli -s $SA -a "$WA" --no-auth-warning ping 2>&1)" PONG

say "Yalıtım"
check "ikinci hesap açar" "$(call $JB POST /redis/mine/enable | jget 'd["instance"]["running"]')" True
PB=$(field $JB port); WB=$(field $JB password)
check "portlar ve parolalar ayrı" "$([ "$PA" != "$PB" ] && [ "$WA" != "$WB" ] && echo ayrı)" ayrı
rc $PA "$WA" set gizli birinci-hesabin-verisi >/dev/null
check "ikinci hesabın parolası birincide geçmez" "$(rc $PA "$WB" get gizli | grep -c WRONGPASS)" 1
check "ikinci hesap birincinin soketine ulaşamaz" "$(runuser -u $O -- redis-cli -s $SA -a "$WA" --no-auth-warning ping 2>&1 | grep -ci 'permission denied')" 1
check "ikinci hesap birincinin verisini okuyamaz (dosya)" "$(runuser -u $O -- ls /var/lib/pixpanel/redis/$A 2>&1 | grep -ci 'permission denied')" 1
check "tarayıcı: birinci hesap kendi anahtarını görür" "$(call $JA GET '/redis/keys?db=0' | jget '[k["key"] for k in d["keys"]]')" "['gizli']"
check "tarayıcı: ikinci hesap görmez" "$(call $JB GET '/redis/keys?db=0' | jget '[k["key"] for k in d["keys"]]')" "[]"
check "konsol hesabın kendi örneğinde çalışır" "$(cmd $JB 'SET benim ikinci' >/dev/null; rc $PB "$WB" get benim) $(rc $PA "$WA" exists benim)" "ikinci 0"
has "konsolda ayar komutu kapalı" "$(cmd $JA 'CONFIG GET requirepass')" "error"
check "özet anahtar sayısını gösterir" "$(field $JA keys)" 1
check "yönetici tarayıcısı ortak Redis'e bakar (hesabın anahtarı yok)" "$(api GET '/redis/keys?db=0&pattern=gizli' | jget '[k["key"] for k in d["keys"]]')" "[]"

say "Parola yenileme"
NEW=$(call $JA POST /redis/mine/password | jget 'd["instance"]["password"]')
check "yeni parola farklı" "$([ "$NEW" != "$WA" ] && [ ${#NEW} = 48 ] && echo evet)" evet
check "eski parola artık geçmez" "$(rc $PA "$WA" ping | grep -c WRONGPASS)" 1
check "yeni parola geçer, port aynı" "$(rc "$(field $JA port)" "$NEW" ping)" PONG
check "veri yeniden başlatmada korunur" "$(rc $PA "$NEW" get gizli)" birinci-hesabin-verisi
WA=$NEW

say "Yönetici"
check "liste parolaları taşımaz" "$(api GET /redis/accounts | jget '(sorted(a["username"] for a in d["accounts"] if a["username"] in ("'$A'","'$O'")), any("password" in a for a in d["accounts"]))')" "(['$A', '$O'], False)"
IDA=$(user_id $A); IDB=$(user_id $O)
check "geçersiz bellek sınırı" "$(code PUT /redis/accounts/$IDA -d '{"memory_mb":8}')" 400
check "bellek sınırını 16 MB yap" "$(api PUT /redis/accounts/$IDA -d '{"memory_mb":16}' | jget 'd["memory_mb"]')" 16
check "sınır örneğe işlendi" "$(rc $PA "$WA" info memory | tr -d '\r' | sed -n 's/^maxmemory://p')" 16777216
python3 - "$PA" "$WA" <<'PY'
import socket, sys
port, password = int(sys.argv[1]), sys.argv[2].encode()
s = socket.create_connection(("127.0.0.1", port))
def send(*args):
    out = b"*%d\r\n" % len(args)
    for a in args:
        out += b"$%d\r\n%s\r\n" % (len(a), a)
    s.sendall(out)
    return s.recv(4096)
send(b"AUTH", password)
blob = b"x" * (1024 * 1024)
for i in range(40):
    send(b"SET", b"dolgu:%d" % i, blob)
PY
check "sınır dolunca eski anahtarlar atılır, örnek çalışmayı sürdürür" "$(rc $PA "$WA" ping) $([ "$(rc $PA "$WA" info stats | tr -d '\r' | sed -n 's/^evicted_keys://p')" -gt 0 ] && echo atıldı) $([ "$(rc $PA "$WA" info memory | tr -d '\r' | sed -n 's/^used_memory://p')" -lt 25000000 ] && echo sınırda)" "PONG atıldı sınırda"
check "dolan hesap ötekini etkilemez" "$(rc $PB "$WB" get benim)" ikinci
check "olmayan hesabın örneği" "$(code PUT /redis/accounts/999999 -d '{"memory_mb":64}')" 400

say "Bellek sınırı pakete bağlı"
# Paketteki redis_memory_mb hesabın örneğinin maxmemory değeridir; paket atanınca/değişince eşitlenir.
RP=$(api POST /packages -d '{"name":"e2e-redis-32","limits":{"redis_memory_mb":32}}' | jget 'd["id"]')
check "paket oluşturuldu" "$([[ "$RP" =~ ^[0-9]+$ ]] && echo evet)" evet
check "pakete bağlanınca örneğin bellek sınırı paketten gelir" "$(code PATCH /users/$IDA -d "{\"package_id\":$RP}") $(rc $PA "$WA" info memory | tr -d '\r' | sed -n 's/^maxmemory://p')" "200 33554432"
check "hesabın görünümü paketten geldiğini söyler" "$(call $JA GET /redis/mine | jget '(d["allowed"], d["package"], d["package_memory_mb"], d["instance"]["memory_mb"])')" "(True, 'e2e-redis-32', 32, 32)"
check "yönetici listesi bellek sınırının paketten geldiğini söyler" "$(api GET /redis/accounts | jget "next((a['memory_from_package'], a['package'], a['memory_mb']) for a in d['accounts'] if a['username']=='$A')")" "(True, 'e2e-redis-32', 32)"
has "yönetici örnek başına değiştiremez" "$(api PUT /redis/accounts/$IDA -d '{"memory_mb":64}')" "paketinden geliyor"
check "örnek çalışıyor, parola ve port aynı" "$(rc $PA "$WA" ping)" PONG
check "birim dosyasında MemoryMax = 2 x bellek + 64" "$(grep -c '^MemoryMax=128M' /etc/systemd/system/pixpanel-redis-$A.service)" 1
check "paket düzenlenince örnek yeni sınıra çekilir" "$(code PATCH /packages/$RP -d '{"name":"e2e-redis-32","limits":{"redis_memory_mb":48}}') $(until_eq 50331648 maxmem)" "200 50331648"
check "öteki hesabın örneği etkilenmedi (paketsiz, 64 MB)" "$(rc $PB "$WB" info memory | tr -d '\r' | sed -n 's/^maxmemory://p')" 67108864

say "Paket Redis içermiyor (redis_memory_mb = 0)"
RZ=$(api POST /packages -d '{"name":"e2e-redis-yok","limits":{"redis_memory_mb":0}}' | jget 'd["id"]')
check "pakete bağlanınca açık örnek durdurulur" "$(code PATCH /users/$IDA -d "{\"package_id\":$RZ}") $(until_eq inactive systemctl is-active pixpanel-redis-$A.service)" "200 inactive"
check "açılışta başlamaz" "$(systemctl is-enabled pixpanel-redis-$A.service 2>/dev/null)" disabled
check "port kapandı" "$(ss -ltnH "sport = :$PA" | wc -l)" 0
check "veri ve kayıt silinmedi" "$(test -d /var/lib/pixpanel/redis/$A && echo var || echo yok) $(call $JA GET /redis/mine | jget 'd["instance"]["username"]')" "var $A"
check "hesabın görünümü izin yok der" "$(call $JA GET /redis/mine | jget '(d["allowed"], d["package"], d["instance"]["running"])')" "(False, 'e2e-redis-yok', False)"
check "yönetici listesi izin yok der" "$(api GET /redis/accounts | jget "next((a['allowed'], a['running']) for a in d['accounts'] if a['username']=='$A')")" "(False, False)"
has "tarayıcı çalışmaz" "$(call $JA GET '/redis/keys?db=0')" "Redis içermiyor"
has "konsol çalışmaz" "$(cmd $JA 'PING')" "Redis içermiyor"
has "parola yenilenemez (örnek yeniden başlamaz)" "$(call $JA POST /redis/mine/password)" "Redis içermiyor"
check "örnek hâlâ durmuş" "$(systemctl is-active pixpanel-redis-$A.service)" inactive
has "bellek sınırı elle değiştirilemez" "$(api PUT /redis/accounts/$IDA -d '{"memory_mb":64}')" "paketinden geliyor"
check "Redis'siz paketli ikinci hesap açamaz" "$(code PATCH /users/$IDB -d "{\"package_id\":$RZ}") $(call $JB POST /redis/mine/disable | jget 'd["instance"]')" "200 None"
has "açma denemesi nedenini söyler" "$(call $JB POST /redis/mine/enable)" "Redis içermiyor"
check "ikinci hesapta örnek oluşmadı" "$(call $JB GET /redis/mine | jget '(d["allowed"], d["instance"])')" "(False, None)"
check "paket izin verince durdurulan örnek kendiliğinden başlar" "$(code PATCH /users/$IDA -d "{\"package_id\":$RP}") $(until_eq PONG rc $PA "$WA" ping)" "200 PONG"
check "  bellek sınırı yeniden paketten (48 MB)" "$(rc $PA "$WA" info memory | tr -d '\r' | sed -n 's/^maxmemory://p')" 50331648
check "  birim yeniden etkin" "$(systemctl is-enabled pixpanel-redis-$A.service)" enabled
check "paket kaldırılınca bellek olduğu gibi kalır, yönetici yeniden değiştirebilir" "$(code PATCH /users/$IDA -d '{"package_id":0}') $(api PUT /redis/accounts/$IDA -d '{"memory_mb":16}' | jget 'd["memory_mb"]')" "200 16"
check "ikinci hesabı paketsiz bırak, açabilir" "$(code PATCH /users/$IDB -d '{"package_id":0}') $(call $JB POST /redis/mine/enable | jget 'd["instance"]["running"]')" "200 True"

say "Kapatma"
check "kapat" "$(call $JA POST /redis/mine/disable | jget 'd["instance"]')" None
check "süreç ve birim kalktı" "$(systemctl is-active pixpanel-redis-$A.service) $(test -e /etc/systemd/system/pixpanel-redis-$A.service && echo var || echo yok)" "inactive yok"
check "veri ve yapılandırma silindi" "$(test -e /var/lib/pixpanel/redis/$A && echo var || echo yok) $(test -e /etc/pixpanel/redis/$A.conf && echo var || echo yok)" "yok yok"
check "port kapandı" "$(ss -ltnH "sport = :$PA" | wc -l)" 0
check "yeniden açınca boş ve yeni parolalı gelir" "$(call $JA POST /redis/mine/enable | jget '(d["instance"]["keys"], d["instance"]["password"] != "'$WA'")')" "(0, True)"
check "yönetici bir hesabın örneğini kapatır" "$(code DELETE /redis/accounts/$IDA) $(systemctl is-active pixpanel-redis-$A.service)" "200 inactive"
check "hesap silinince örneği de kalkar" "$(code DELETE /users/$IDB) $(systemctl is-active pixpanel-redis-$O.service) $(test -e /var/lib/pixpanel/redis/$O && echo var || echo yok)" "200 inactive yok"
check "ortak Redis etkilenmedi" "$(systemctl is-active redis-server.service)" active
check "birinci hesabı sil" "$(code DELETE /users/$IDA)" 200
drop_packages
check "sınama paketleri silindi" "$(api GET /packages | jget "[p['name'] for p in d if p['name'].startswith('e2e-redis')]")" "[]"
rm -f $J $JA $JB
echo "bitti"
