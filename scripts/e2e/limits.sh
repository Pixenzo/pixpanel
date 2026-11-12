#!/usr/bin/env bash
# Pakete bağlı işlemci ve bellek sınırının sınaması: paket alanları, hesabın systemd dilimi,
# Redis/uygulama servislerinin dilimi, PHP havuzu sınırları, gerçek bellek sınırı, temizlik.
# Test makinesinde root olarak çalışır (Debian 13 yetkisiz LXC olabilir).
# Kullanım: PX_USER=... PX_PASS=... bash limits.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-lim
JC=/tmp/pxjar-lim-customer
JR=/tmp/pxjar-lim-reseller
CUSTOMER=limmusteri
RESELLER=limbayi
PASS='Limit-Parola-13579'
PHP_DOMAIN=lim-php.test
PY_DOMAIN=lim-py.test
HOLD=px-lim-hold
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
skip() { echo "  atlandı: $*"; }
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
# Komut beklenen çıktıyı verene kadar (en çok ~10 sn) yeniden denenir; son çıktıyı yazar.
until_eq() { local want=$1 got=; shift; for _ in $(seq 1 20); do got=$("$@" 2>/dev/null); [ "$got" = "$want" ] && break; sleep 0.5; done; echo "$got"; }
slice_prop() { systemctl show -p "$2" --value -- "user-$1.slice" 2>/dev/null; }
dropin() { echo "/etc/systemd/system/user-$1.slice.d/50-pixpanel.conf"; }
unit_slice() { systemctl show -p Slice --value -- "$1" 2>/dev/null; }

# Önceki koşudan kalanları temizler: tutucu servis, hesabın siteleri, hesap, sınama paketleri.
cleanup() {
  systemctl stop $HOLD.service >/dev/null 2>&1
  for name in $CUSTOMER $RESELLER; do
    local id; id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')")
    [ -z "$id" ] && continue
    for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['owner']=='$name')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
    call $JC POST /redis/mine/disable >/dev/null 2>&1
    api DELETE /users/$id >/dev/null
  done
  for package in $(api GET /packages | jget "' '.join(str(p['id']) for p in d if p['name'].startswith('e2e-lim'))"); do api DELETE /packages/$package >/dev/null; done
}

rm -f $J $JC $JR
login $J "$PX_USER" "$PX_PASS" >/dev/null
login $JC $CUSTOMER "$PASS" >/dev/null
cleanup
rm -f $JC

say "Sunucu desteği"
SUPPORT=$(api GET /packages/limits-support)
check "destek ucu yöneticiye yanıt verir" "$(jget 'type(d["enforced"]).__name__' <<<"$SUPPORT")" bool
echo "  bilgi: sınırlar bu sunucuda uygulanabilir mi: $(jget 'd["enforced"]' <<<"$SUPPORT") ($(jget 'd["warning"]' <<<"$SUPPORT"))"
# Gerçek uygulama sınamaları için cgroup v2 ve cpu/memory denetleyicileri gerekir.
ENFORCED=$(jget 'd["enforced"]' <<<"$SUPPORT")

say "Paket alanlarının doğrulaması"
for body in \
  '{"cpu_percent":9}' '{"cpu_percent":3201}' '{"cpu_percent":-5}' \
  '{"memory_mb":63}' '{"memory_mb":262145}' \
  '{"php_workers":0}' '{"php_workers":201}' \
  '{"processes":19}' '{"processes":10001}'; do
  check "geçersiz $body" "$(code POST /packages -d "{\"name\":\"e2e-lim-bozuk\",\"limits\":$body}")" 400
done
check "aralığın uçları geçerli" "$(code POST /packages -d '{"name":"e2e-lim-uc","limits":{"cpu_percent":10,"memory_mb":64,"php_workers":1,"processes":20}}')" 200
check "üst uçlar geçerli" "$(code PATCH /packages/$(api GET /packages | jget "next(p['id'] for p in d if p['name']=='e2e-lim-uc')") -d '{"name":"e2e-lim-uc","limits":{"cpu_percent":3200,"memory_mb":262144,"php_workers":200,"processes":10000}}')" 200
LIM=$(api POST /packages -d '{"name":"e2e-lim","limits":{"sites":5,"cpu_percent":150,"memory_mb":128,"php_workers":3,"processes":200}}' | jget 'd["id"]')
LIM_MEM=134217728
check "paket alanları kaydedildi" "$(api GET /packages | jget "next((p['limits']['cpu_percent'], p['limits']['memory_mb'], p['limits']['php_workers'], p['limits']['processes']) for p in d if p['id']==$LIM)")" "(150, 128, 3, 200)"
check "güncellemede de doğrulanır" "$(code PATCH /packages/$LIM -d '{"name":"e2e-lim","limits":{"cpu_percent":5}}')" 400

say "Bayi kendi sınırını aşamaz"
BAYI=$(api POST /packages -d '{"name":"e2e-lim-bayi","limits":{"cpu_percent":200,"memory_mb":1024}}' | jget 'd["id"]')
check "paketli bayi" "$(code POST /users -d "{\"username\":\"$RESELLER\",\"password\":\"$PASS\",\"role\":\"reseller\",\"package_id\":$BAYI}")" 200
check "bayi girişi" "$(login $JR $RESELLER "$PASS")" 200
has "işlemci sınırını aşan paket" "$(call $JR POST /packages -d '{"name":"e2e-lim-asan","limits":{"cpu_percent":300,"memory_mb":512}}')" "işlemci"
has "belleği sınırsız bırakan paket" "$(call $JR POST /packages -d '{"name":"e2e-lim-asan","limits":{"cpu_percent":100}}')" "bellek"
has "bellek sınırını aşan paket" "$(call $JR POST /packages -d '{"name":"e2e-lim-asan","limits":{"cpu_percent":100,"memory_mb":2048}}')" "bellek"
check "sınırı içinde paket" "$(status $JR POST /packages -d '{"name":"e2e-lim-bayi-alt","limits":{"cpu_percent":100,"memory_mb":512,"processes":100}}')" 200
check "bayi destek ucunu görür" "$(status $JR GET /packages/limits-support)" 200

say "Hesaba paket atanması (Linux kullanıcısı henüz yok)"
check "paketli müşteri" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\",\"package_id\":$LIM}")" 200
CID=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
check "müşteri girişi" "$(login $JC $CUSTOMER "$PASS")" 200
check "istenen sınırlar kayıt dosyasında" "$(jget '(d["cpu_percent"], d["memory_mb"], d["php_workers"], d["processes"])' < /etc/pixpanel/limits/$CUSTOMER.json)" "(150, 128, 3, 200)"
check "kullanıcı yokken dilim dosyası yok" "$(id -u $CUSTOMER >/dev/null 2>&1 && echo var || echo yok)" yok

say "İlk site: kullanıcı oluşur, dilim sınırı yazılır"
PHPV=$(api GET /sites/options | jget 'd["php_versions"][0] if d["php_versions"] else ""')
if [ -n "$PHPV" ]; then
  PS=$(call $JC POST /sites -d "{\"domain\":\"$PHP_DOMAIN\",\"kind\":\"php\",\"php_version\":\"$PHPV\"}" | jget 'd["id"]')
else
  skip "PHP kurulu değil; site statik açılır, PHP havuzu denetimleri atlanır"
  PS=$(call $JC POST /sites -d "{\"domain\":\"$PHP_DOMAIN\",\"kind\":\"static\"}" | jget 'd["id"]')
fi
check "site açıldı" "$([[ "$PS" =~ ^[0-9]+$ ]] && echo evet)" evet
UID_C=$(id -u $CUSTOMER)
check "dilim dosyası yazıldı" "$([ -f "$(dropin $UID_C)" ] && echo var || echo yok)" var
D=$(cat "$(dropin $UID_C)" 2>/dev/null)
has "CPUQuota" "$D" "CPUQuota=150%"
has "MemoryMax" "$D" "MemoryMax=128M"
has "MemoryHigh yüzde 90" "$D" "MemoryHigh=115M"
has "TasksMax" "$D" "TasksMax=200"
check "PHP işçisi dilime yazılmaz" "$(grep -c php_workers "$(dropin $UID_C)")" 0
check "dilim dosyası root'a ait, yalnız root yazar" "$(stat -c '%U %a' "$(dropin $UID_C)")" "root 644"
check "müşteri kayıt dosyasını okuyamaz" "$(runuser -u $CUSTOMER -- cat /etc/pixpanel/limits/$CUSTOMER.json >/dev/null 2>&1; echo $?)" 1
check "CPUQuotaPerSecUSec" "$(slice_prop $UID_C CPUQuotaPerSecUSec)" 1.5s
check "MemoryMax" "$(slice_prop $UID_C MemoryMax)" 134217728
check "MemoryHigh" "$(slice_prop $UID_C MemoryHigh)" 120586240
check "TasksMax" "$(slice_prop $UID_C TasksMax)" 200

say "Müşteri sınırı değiştiremez"
check "paketini kaldıramaz" "$(status $JC PATCH /users/$CID -d '{"package_id":0}')" 404
check "paket tanımlayamaz" "$(status $JC POST /packages -d '{"name":"e2e-lim-kacak","limits":{}}')" 403
check "paketi değiştiremez" "$(status $JC PATCH /packages/$LIM -d '{"name":"e2e-lim","limits":{}}')" 403
check "destek ucunu göremez" "$(status $JC GET /packages/limits-support)" 403
check "dilim dosyasına yazamaz" "$(timeout 10 runuser -u $CUSTOMER -- sh -c "echo '[Slice]' > $(dropin $UID_C)" >/dev/null 2>&1; echo $?)" 1
check "systemctl ile dilimi gevşetemez" "$(timeout 15 runuser -u $CUSTOMER -- systemctl set-property --runtime user-$UID_C.slice MemoryMax=infinity >/dev/null 2>&1; [ $? -ne 0 ] && echo reddedildi || echo uygulandi)" reddedildi
check "dilim değeri değişmedi" "$(slice_prop $UID_C MemoryMax)" 134217728

say "Kullanım görünümü"
U=$(call $JC GET /packages/usage)
check "sınırlar görünür" "$(jget '(d["limits"]["cpu_percent"], d["limits"]["memory_mb"], d["limits"]["php_workers"], d["limits"]["processes"])' <<<"$U")" "(150, 128, 3, 200)"
check "çalışma durumu bildirilir" "$(jget 'type(d["runtime"]["enforced"]).__name__' <<<"$U")" bool
check "yönetici müşterinin durumunu görür" "$(api GET /users/$CID/usage | jget 'd["runtime"]["enforced"]')" "$ENFORCED"
check "yöneticide çalışma durumu yok" "$(api GET /packages/usage | jget 'd["runtime"]')" None

say "PHP havuzu"
if [ -n "$PHPV" ]; then
  POOL=/etc/php/$PHPV/fpm/pool.d/pixpanel-$PS.conf
  pool_value() { grep -F -- "$1" $POOL | head -1 | sed 's/^[^=]*= *//'; }
  check "işçi sayısı paket sınırına iner (varsayılan 8)" "$(pool_value 'pm.max_children')" 3
  check "genel memory_limit paketin belleğine kırpılır" "$(pool_value 'php_admin_value[memory_limit]')" 128M
  check "havuz FPM sınamasından geçer" "$(php-fpm$PHPV --test >/dev/null 2>&1; echo $?)" 0
  check "site küçük bir değer isteyince korunur" "$(code PATCH /sites/$PS -d '{"php_settings":{"memory_limit":"64M"}}')" 200
  check "  havuzda 64M" "$(pool_value 'php_admin_value[memory_limit]')" 64M
  check "site büyük bir değer isteyince kırpılır" "$(code PATCH /sites/$PS -d '{"php_settings":{"memory_limit":"1G","max_children":20}}')" 200
  check "  havuzda 128M" "$(pool_value 'php_admin_value[memory_limit]')" 128M
  check "  işçi sayısı yine 3" "$(pool_value 'pm.max_children')" 3
  check "site işçiyi sınırın altına indirebilir" "$(code PATCH /sites/$PS -d '{"php_settings":{"max_children":2}}')" 200
  check "  havuzda 2" "$(pool_value 'pm.max_children')" 2
  check "ayar sıfırlanır" "$(code PATCH /sites/$PS -d '{"php_settings":{"memory_limit":"1G"}}')" 200
  check "  yine 3 işçi, 128M" "$(pool_value 'pm.max_children') $(pool_value 'php_admin_value[memory_limit]')" "3 128M"
  say "Paket düzenlenince havuz yenilenir"
  check "php_workers 5, bellek 192" "$(code PATCH /packages/$LIM -d '{"name":"e2e-lim","limits":{"sites":5,"cpu_percent":150,"memory_mb":192,"php_workers":5,"processes":200}}')" 200
  check "  havuzda 5 işçi (site ayarı 8)" "$(pool_value 'pm.max_children')" 5
  check "  havuzda 192M" "$(pool_value 'php_admin_value[memory_limit]')" 192M
  LIM_MEM=201326592
  check "  dilim değeri de yenilendi" "$(until_eq 201326592 slice_prop $UID_C MemoryMax)" 201326592
else
  skip "PHP kurulu değil"
fi

say "Redis ve uygulama servisleri hesabın diliminde"
# Port koruması çekirdekte çalışıyorsa uygulama servisleri dilime alınmaz (portguard.rs ile çakışma; resource_limits.rs başında açıklı).
cat > /tmp/px-lim-bindprobe.py <<'XEOF'
import socket, sys
s = socket.socket()
try:
    s.bind(("127.0.0.1", 19999))
except PermissionError:
    sys.exit(0)
sys.exit(1)
XEOF
if systemd-run --wait --quiet --collect -p SocketBindDeny=tcp:19999 python3 /tmp/px-lim-bindprobe.py >/dev/null 2>&1; then GUARD=evet; else GUARD=hayir; fi
echo "  bilgi: port koruması çekirdekte çalışıyor mu: $GUARD"
REDIS_AVAILABLE=$(call $JC GET /redis/mine | jget 'd["available"]')
if [ "$REDIS_AVAILABLE" = True ]; then
  check "Redis açılır" "$(status $JC POST /redis/mine/enable)" 200
  check "Redis servisi hesabın diliminde" "$(unit_slice pixpanel-redis-$CUSTOMER.service)" "user-$UID_C.slice"
  check "Redis birim dosyasında dilim satırı" "$(grep -c "^Slice=user-$UID_C.slice" /etc/systemd/system/pixpanel-redis-$CUSTOMER.service)" 1
else
  skip "Redis kurulu değil"
fi
PYV=$(api GET /sites/options | jget 'd["python_versions"][0] if d["python_versions"] else ""')
PYS=
if [ -n "$PYV" ]; then
  PYS=$(call $JC POST /sites -d "{\"domain\":\"$PY_DOMAIN\",\"kind\":\"python\"}" | jget 'd["id"]')
  check "Python sitesi açıldı" "$([[ "$PYS" =~ ^[0-9]+$ ]] && echo evet)" evet
  if [ "$GUARD" = evet ]; then
    check "port koruması çalışırken uygulama dilime alınmaz" "$(grep -c '^Slice=' /etc/systemd/system/pixpanel-node-$PYS.service)" 0
  else
    check "Python servisi hesabın diliminde" "$(unit_slice pixpanel-node-$PYS.service)" "user-$UID_C.slice"
  fi
  check "uygulama dilimdeyken de çalışır" "$(until_eq active systemctl is-active pixpanel-node-$PYS.service)" active
else
  skip "Python kurulu değil"
fi

say "Gerçek sınır: bellek tüketen komut öldürülür"
HOLD_OK=
if [ "$ENFORCED" = True ]; then
  # Dilimi etkin tutan geçici servis (dilim yalnız içinde süreç varken bulunur).
  systemd-run --quiet --collect --unit=$HOLD --uid=$CUSTOMER --slice=user-$UID_C.slice sleep 600 >/dev/null 2>&1
  CG=/sys/fs/cgroup/user.slice/user-$UID_C.slice
  if [ "$(until_eq 134217728 cat $CG/memory.max)" = 134217728 ]; then HOLD_OK=1; fi
fi
if [ -n "$HOLD_OK" ]; then
  check "cgroup memory.max dilimde" "$(cat $CG/memory.max)" 134217728
  check "cgroup cpu.max dilimde (150% = 150000/100000)" "$(cat $CG/cpu.max)" "150000 100000"
  if [ -f $CG/pids.max ]; then check "cgroup pids.max dilimde" "$(cat $CG/pids.max)" 200; else skip "pids denetleyicisi yok"; fi
  OUT=$(timeout 60 systemd-run --uid=$CUSTOMER --slice=user-$UID_C.slice --wait --pipe --quiet --collect -- python3 -c "x = b'a' * (16 * 1024 * 1024); print('survived')" 2>&1)
  has "küçük bellek kullanımı sınırın altında çalışır" "$OUT" survived
  OUT=$(timeout 90 systemd-run --uid=$CUSTOMER --slice=user-$UID_C.slice --wait --pipe --quiet --collect -- python3 -c "x = b'a' * (400 * 1024 * 1024); print('survived')" 2>&1; echo "rc=$?")
  check "sınırı aşan komut öldürüldü" "$(grep -c survived <<<"$OUT") $(grep -c 'rc=0' <<<"$OUT")" "0 0"
  U=$(call $JC GET /packages/usage)
  check "anlık bellek kullanımı görünür" "$(jget 'type(d["runtime"]["memory_bytes"]).__name__' <<<"$U")" int
  check "süreç sayısı görünür" "$(jget 'type(d["runtime"]["tasks"]).__name__' <<<"$U")" int
  check "işlemci süresi görünür" "$(jget 'type(d["runtime"]["cpu_seconds"]).__name__' <<<"$U")" float

  say "Çalışan dilimde sınır değişimi (daemon-reload yeter mi?)"
  check "bellek 256 MB" "$(code PATCH /packages/$LIM -d '{"name":"e2e-lim","limits":{"sites":5,"cpu_percent":150,"memory_mb":256,"php_workers":5,"processes":200}}')" 200
  LIM_MEM=268435456
  check "  çalışan dilimin memory.max değeri hemen güncellendi" "$(until_eq 268435456 cat $CG/memory.max)" 268435456
  check "  MemoryHigh da" "$(until_eq 241172480 cat $CG/memory.high)" 241172480
  check "işlemci sınırı 50%" "$(code PATCH /packages/$LIM -d '{"name":"e2e-lim","limits":{"sites":5,"cpu_percent":50,"memory_mb":256,"php_workers":5,"processes":200}}')" 200
  check "  cpu.max güncellendi" "$(until_eq '50000 100000' cat $CG/cpu.max)" "50000 100000"
else
  skip "cgroup v2 ya da cpu/memory denetleyicisi yok; gerçek bellek sınırı sınanamadı"
fi

say "Paket kaldırılınca"
check "paketi kaldır" "$(code PATCH /users/$CID -d '{"package_id":0}')" 200
check "dilim dosyası silindi" "$([ -e "$(dropin $UID_C)" ] && echo var || echo yok)" yok
check "dilim klasörü de gitti" "$([ -d "$(dirname "$(dropin $UID_C)")" ] && echo var || echo yok)" yok
check "kayıt dosyası silindi" "$([ -e /etc/pixpanel/limits/$CUSTOMER.json ] && echo var || echo yok)" yok
check "CPUQuotaPerSecUSec infinity" "$(until_eq infinity slice_prop $UID_C CPUQuotaPerSecUSec)" infinity
check "MemoryMax infinity" "$(until_eq infinity slice_prop $UID_C MemoryMax)" infinity
check "MemoryHigh infinity" "$(slice_prop $UID_C MemoryHigh)" infinity
# TasksMax varsayılanı sistemin kendi değeridir (user-.slice için genellikle %33); "sınırsız" beklenmez, 200 olmamalı.
check "TasksMax paket değerine dönmedi" "$([ "$(slice_prop $UID_C TasksMax)" != 200 ] && echo evet || echo hayir)" evet
if [ -n "$HOLD_OK" ]; then
  check "çalışan dilimin memory.max değeri kalktı" "$(until_eq max cat $CG/memory.max)" max
  check "çalışan dilimin cpu.max değeri kalktı" "$(until_eq 'max 100000' cat $CG/cpu.max)" "max 100000"
fi
if [ -n "$PHPV" ]; then
  check "PHP havuzu site ayarına döndü (8 işçi, 1G)" "$(pool_value 'pm.max_children') $(pool_value 'php_admin_value[memory_limit]')" "8 1G"
fi
if [ -n "$PYS" ]; then
  check "Python birimindeki dilim satırı kalktı" "$(grep -c '^Slice=' /etc/systemd/system/pixpanel-node-$PYS.service)" 0
fi
if [ "$REDIS_AVAILABLE" = True ]; then
  check "Redis birimindeki dilim satırı kalktı" "$(grep -c '^Slice=' /etc/systemd/system/pixpanel-redis-$CUSTOMER.service)" 0
fi
check "kullanım görünümünde kaynak durumu yok" "$(call $JC GET /packages/usage | jget 'd["runtime"]')" None

say "Paketi geri verince"
check "paketi geri ver" "$(code PATCH /users/$CID -d "{\"package_id\":$LIM}")" 200
check "dilim dosyası yeniden yazıldı" "$(until_eq var sh -c "[ -f $(dropin $UID_C) ] && echo var")" var
check "MemoryMax paketteki değere döndü" "$(until_eq $LIM_MEM slice_prop $UID_C MemoryMax)" $LIM_MEM
systemctl stop $HOLD.service >/dev/null 2>&1

say "Fazlalıkların silinmesi (panel yeniden başlayınca)"
# Bu bölüm main.rs'e açılış eşitlemesi (resource_limits::sync) eklenmişse çalışır.
STRAY=/etc/systemd/system/user-59999.slice.d/50-pixpanel.conf
mkdir -p "$(dirname $STRAY)"
printf '[Slice]\nMemoryMax=64M\n' > $STRAY
printf '{"cpu_percent":10}' > /etc/pixpanel/limits/olmayanhesap.json
rm -f "$(dropin $UID_C)"
systemctl daemon-reload
systemctl restart pixpanel
for _ in $(seq 1 40); do curl -sk -o /dev/null "$B/auth/login" && break; sleep 1; done
check "açılışta eksik dilim dosyası yeniden yazıldı" "$(until_eq var sh -c "[ -f $(dropin $UID_C) ] && echo var")" var
check "açılışta fazla dilim dosyası silindi" "$(until_eq yok sh -c "[ -e $STRAY ] && echo var || echo yok")" yok
check "açılışta fazla kayıt dosyası silindi" "$(until_eq yok sh -c "[ -e /etc/pixpanel/limits/olmayanhesap.json ] && echo var || echo yok")" yok
check "yeniden yazılan dilim değeri doğru" "$(until_eq $LIM_MEM slice_prop $UID_C MemoryMax)" $LIM_MEM
rm -f $J; login $J "$PX_USER" "$PX_PASS" >/dev/null

say "Hesap silinince iz kalmaz"
for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['owner']=='$CUSTOMER')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
[ "$REDIS_AVAILABLE" = True ] && call $JC POST /redis/mine/disable >/dev/null
check "hesap silinir" "$(code DELETE /users/$CID)" 200
check "dilim dosyası yok" "$([ -e "$(dropin $UID_C)" ] && echo var || echo yok)" yok
check "dilim klasörü yok" "$([ -d "$(dirname "$(dropin $UID_C)")" ] && echo var || echo yok)" yok
check "kayıt dosyası yok" "$([ -e /etc/pixpanel/limits/$CUSTOMER.json ] && echo var || echo yok)" yok
check "dilim değerleri sınırsıza döndü" "$(until_eq infinity slice_prop $UID_C MemoryMax)" infinity
check "Linux kullanıcısı kalktı" "$(id -u $CUSTOMER >/dev/null 2>&1 && echo var || echo yok)" yok
check "Python birimi kalktı" "$([ -e /etc/systemd/system/pixpanel-node-${PYS:-0}.service ] && echo var || echo yok)" yok

say "Temizlik"
cleanup
rm -f /tmp/px-lim-bindprobe.py
check "hesaplar ve paketler silindi" "$(api GET /users | jget "[u['username'] for u in d if u['username'] in ('$CUSTOMER', '$RESELLER')]") $(api GET /packages | jget "[p['name'] for p in d if p['name'].startswith('e2e-lim')]")" "[] []"
echo "bitti"
