#!/usr/bin/env bash
# DNS kümesi sınaması: bölgelerin ikincil ad sunucusuna eşitlenmesi. Geliştirme makinesinde çalışır.
# Birincil: test makinesi (192.168.1.25). İkincil: temiz konteyner (varsayılan 190 → 192.168.1.190);
# konteyner önceden `scripts/e2e/ct.sh create 190` ile kurulmuş olmalıdır. Betik, geliştirme
# derlemesini ikincile kendisi yükler.
#
# Kullanım: scripts/e2e/cluster.sh [konteyner no]
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ID="${1:-190}"
PRIMARY=root@192.168.1.25
PRIMARY_IP=192.168.1.25
SECONDARY_IP=192.168.1.$ID
PVE=proxmox
BINARY="$ROOT/server/target/x86_64-unknown-linux-musl/debug/pixpanel"
ZONE=kume-sinamasi.test
OTHER=kume-ikinci.test
CLASH=kume-cakisma.test
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 300)]" ;; esac; }
# Birincilde ve ikincilde komut çalıştırır.
p() { ssh -o BatchMode=yes "$PRIMARY" "$@"; }
s() { ssh -o BatchMode=yes "$PVE" "pct exec $ID -- env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin $*"; }
# Panel API'si: komut satırı aracı üzerinden. Gövde yerelde dosyaya yazılıp makineye taşınır.
papi() { local m=$1 path=$2 body=${3:-}; if [ -n "$body" ]; then printf %s "$body" | p "cat > /tmp/cluster-body.json; pixpanel api $m '$path' --data @/tmp/cluster-body.json" 2>/dev/null; else p "pixpanel api $m '$path'" 2>/dev/null; fi; }
sapi() { local m=$1 path=$2 body=${3:-}; if [ -n "$body" ]; then printf %s "$body" | ssh -o BatchMode=yes "$PVE" "cat > /tmp/cluster-body-$ID.json; pct push $ID /tmp/cluster-body-$ID.json /tmp/cluster-body.json; pct exec $ID -- /usr/local/bin/pixpanel api $m '$path' --data @/tmp/cluster-body.json" 2>/dev/null; else s "pixpanel api $m '$path'" 2>/dev/null; fi; }
jget() { python -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
# Ad sunucusuna sorar (sorgu birincilin içinden atılır; iki sunucu da aynı ağda).
q() { local server=$1; shift; p "dig +short +time=2 +tries=1 @$server $*" | tr '\n' ' ' | sed 's/ $//'; }
# wait_for <saniye> <beklenen> <komut...>: çıktı beklenen olana kadar yeniden dener.
wait_for() { local limit=$1 want=$2 got=; shift 2; for _ in $(seq 1 "$limit"); do got=$("$@"); [ "$got" = "$want" ] && break; sleep 1; done; echo "$got"; }
zone_id() { papi GET /dns | jget "next((z['id'] for z in d['zones'] if z['domain']=='$1'), '')"; }
rec() { python -c 'import json,sys; a=sys.argv; print(json.dumps({"record":{"id":"","type":a[1],"name":a[2],"content":a[3],"ttl":300,"priority":None,"proxied":None}}))' "$@"; }
cluster() { papi PUT /dns/cluster "{\"secondaries\":$1}"; }

test -f "$BINARY" || { echo "derleme yok: $BINARY (önce scripts/deploy-dev.sh)"; exit 1; }

say "İkincil makinenin hazırlanması"
if [ "$(s 'test -x /usr/local/bin/pixpanel && echo var' 2>/dev/null)" != var ] || [ "${CLUSTER_REINSTALL:-}" = 1 ]; then
  scp -q "$BINARY" "$PVE:/tmp/pixpanel-cluster-$ID" && scp -q "$ROOT/install/pixpanel.service" "$PVE:/tmp/pixpanel-cluster-$ID.service"
  ssh -o BatchMode=yes "$PVE" "pct push $ID /tmp/pixpanel-cluster-$ID /usr/local/bin/pixpanel --perms 755 && pct push $ID /tmp/pixpanel-cluster-$ID.service /etc/systemd/system/pixpanel.service && rm -f /tmp/pixpanel-cluster-$ID /tmp/pixpanel-cluster-$ID.service"
  s "sh -c 'mkdir -p /etc/pixpanel /var/lib/pixpanel && apt-get -qq update >/dev/null 2>&1; apt-get -qq install -y curl ca-certificates >/dev/null 2>&1; systemctl daemon-reload && systemctl enable --quiet pixpanel && systemctl restart pixpanel'"
  s "sh -c 'for i in \$(seq 1 30); do curl -sk -o /dev/null https://127.0.0.1:8443/ && break; sleep 1; done'"
fi
check "ikincil panel çalışıyor" "$(s 'systemctl is-active pixpanel')" active
# Yönetici hesabı ve parolası: yalnızca bu geçici makinede kullanılır, betik dışına yazılmaz.
ADMIN_PASSWORD=$(s 'pixpanel reset-password admin' 2>/dev/null | sed -n 's/^.*: //p' | tail -1)
[ -n "$ADMIN_PASSWORD" ] || ADMIN_PASSWORD=$(s 'pixpanel create-admin' 2>/dev/null | sed -n 's/^.*[Pp]arola *: //p' | tail -1)
check "ikincil panelde yönetici hesabı" "$([ -n "$ADMIN_PASSWORD" ] && echo var)" var
if [ "$(sapi GET /components | jget 'next(c["installed"] for c in d if c["id"]=="dns")')" != True ]; then
  check "ikincilde DNS sunucusu kurulumu" "$(s 'pixpanel api POST /components/dns/install --wait' >/dev/null 2>&1; echo $?)" 0
fi
check "ikincilde DNS sunucusu çalışıyor" "$(s 'systemctl is-active pdns')" active
# API anahtarı yalnızca oturumdan üretilebilir; ikincil panelde oturum açılır.
TOKEN=$(s "sh -c 'rm -f /tmp/jar; curl -sk -c /tmp/jar -o /dev/null -H \"content-type: application/json\" -X POST https://127.0.0.1:8443/api/v1/auth/login -d \"{\\\"username\\\":\\\"admin\\\",\\\"password\\\":\\\"$ADMIN_PASSWORD\\\"}\"; curl -sk -b /tmp/jar -H \"x-pixpanel-csrf: 1\" -H \"content-type: application/json\" -X POST https://127.0.0.1:8443/api/v1/auth/tokens -d \"{\\\"name\\\":\\\"kume\\\",\\\"read_only\\\":false,\\\"allowed_ips\\\":\\\"$PRIMARY_IP\\\"}\"; rm -f /tmp/jar'" | jget 'd["token"]')
has "ikincil panelde API anahtarı üretildi" "$TOKEN" pix_
READONLY=$(s "sh -c 'rm -f /tmp/jar; curl -sk -c /tmp/jar -o /dev/null -H \"content-type: application/json\" -X POST https://127.0.0.1:8443/api/v1/auth/login -d \"{\\\"username\\\":\\\"admin\\\",\\\"password\\\":\\\"$ADMIN_PASSWORD\\\"}\"; curl -sk -b /tmp/jar -H \"x-pixpanel-csrf: 1\" -H \"content-type: application/json\" -X POST https://127.0.0.1:8443/api/v1/auth/tokens -d \"{\\\"name\\\":\\\"kume-salt-okunur\\\",\\\"read_only\\\":true,\\\"allowed_ips\\\":\\\"\\\"}\"; rm -f /tmp/jar'" | jget 'd["token"]')

say "Temizlik (önceki koşudan kalanlar)"
cluster '[]' >/dev/null
for name in $ZONE $OTHER $CLASH; do id=$(zone_id $name); [ -n "$id" ] && papi DELETE /dns/zones/$id >/dev/null; done
sapi DELETE /dns/replica/$PRIMARY_IP >/dev/null
for id in $(sapi GET /dns | jget '" ".join(str(z["id"]) for z in d["zones"])'); do sapi DELETE /dns/zones/$id >/dev/null; done

say "Geçersiz tanımlar"
has "adres olmayan ikincil" "$(cluster '[{"address":"ns2.ornek.com"}]')" "IP adresi"
has "sunucunun kendi adresi" "$(cluster "[{\"address\":\"$PRIMARY_IP\"}]")" "kendi adresi"
has "şifresiz panel adresi" "$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"http://$SECONDARY_IP:8443\",\"token\":\"x\"}]")" "https"
has "anahtarsız panel adresi" "$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:8443\"}]")" "API anahtarını girin"
has "ulaşılamayan panel" "$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:9\",\"token\":\"x\"}]")" "bağlanılamadı"
check "reddedilen tanımlar kaydedilmedi" "$(papi GET /dns/cluster | jget 'len(d["secondaries"])')" 0

say "Yanlış anahtarlar"
R=$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:8443\",\"token\":\"pix_yanlis\"}]")
has "yanlış anahtar: neden açıkça yazılır" "$(jget 'd["secondaries"][0]["last_error"]' <<<"$R")" "kabul etmedi"
R=$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:8443\",\"token\":\"$READONLY\"}]")
has "salt okunur anahtar" "$(jget 'd["secondaries"][0]["last_error"]' <<<"$R")" "salt okunur olmamalı"

say "Kümenin kurulması"
Z=$(papi POST /dns/zones "{\"domain\":\"$ZONE\",\"provider\":\"local\"}" | jget 'd["zone"]["id"]')
papi POST /dns/zones/$Z/records "$(rec A www2 203.0.113.50)" >/dev/null
R=$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:8443\",\"token\":\"$TOKEN\"}]")
check "ikincil kaydedildi ve eşitlendi" "$(jget '(d["secondaries"][0]["address"], d["secondaries"][0]["last_error"], d["secondaries"][0]["token_set"], len(d["secondaries"][0]["fingerprint"]))' <<<"$R")" "('$SECONDARY_IP', '', True, 64)"
check "yanıt anahtarı taşımaz" "$(grep -c "$TOKEN" <<<"$R")" 0
check "parmak izi ikincilin sertifikasıyla aynı" "$(jget 'd["secondaries"][0]["fingerprint"]' <<<"$R")" "$(s "sh -c 'echo | openssl s_client -connect 127.0.0.1:8443 2>/dev/null | openssl x509 -outform der | sha256sum | cut -d\" \" -f1'")"
has "birincil yapılandırması" "$(p 'cat /etc/powerdns/pdns.d/pixpanel.conf')" "also-notify=$SECONDARY_IP"
has "ikincil yapılandırması" "$(s 'cat /etc/powerdns/pdns.d/pixpanel.conf')" "allow-notify-from=$PRIMARY_IP"
check "ikincil kendini ikincil olarak görür" "$(sapi GET /dns/cluster | jget '[(r["primary"], r["zones"]) for r in d["replica_of"]]')" "[('$PRIMARY_IP', $(papi GET /dns | jget "len([z for z in d['zones'] if z['provider']=='local'])"))]"
check "var olan bölge ikincile aktarıldı" "$(wait_for 30 203.0.113.50 q $SECONDARY_IP A www2.$ZONE)" 203.0.113.50
check "ikincil aynı seri numarasını yayınlar" "$(q $SECONDARY_IP SOA $ZONE | awk '{print $3}')" "$(q $PRIMARY_IP SOA $ZONE | awk '{print $3}')"
check "ns2 kaydı ikincili gösterir" "$(q $PRIMARY_IP A ns2.$ZONE)" "$SECONDARY_IP"
check "ns1 kaydı birincilde kalır" "$(q $PRIMARY_IP A ns1.$ZONE)" "$PRIMARY_IP"
has "başkası bölge çekemez (yalnız ikincile izin)" "$(p "dig +time=2 +tries=1 @$PRIMARY_IP AXFR $ZONE" | tail -5)" "Transfer failed"

say "Değişikliklerin yayılması"
papi POST /dns/zones/$Z/records "$(rec A yeni 203.0.113.51)" >/dev/null
check "eklenen kayıt ikincile ulaşır" "$(wait_for 30 203.0.113.51 q $SECONDARY_IP A yeni.$ZONE)" 203.0.113.51
papi POST /dns/zones/$Z/records/delete "$(rec A yeni 203.0.113.51)" >/dev/null
check "silinen kayıt ikincilden de kalkar" "$(wait_for 30 "" q $SECONDARY_IP A yeni.$ZONE)" ""
Z2=$(papi POST /dns/zones "{\"domain\":\"$OTHER\",\"provider\":\"local\"}" | jget 'd["zone"]["id"]')
check "yeni bölge ikincilde kendiliğinden açılır" "$(wait_for 40 "$SECONDARY_IP" q $SECONDARY_IP A ns2.$OTHER)" "$SECONDARY_IP"
check "yeni bölgenin ns2 kaydı doğrudan ikincili gösterir" "$(q $PRIMARY_IP A ns2.$OTHER)" "$SECONDARY_IP"
papi DELETE /dns/zones/$Z2 >/dev/null
check "silinen bölge ikincilden de kalkar" "$(wait_for 40 "" q $SECONDARY_IP SOA $OTHER)" ""
check "öteki bölge yerinde" "$(q $SECONDARY_IP A www2.$ZONE)" 203.0.113.50

say "Çakışma ve yetki"
sapi POST /dns/zones "{\"domain\":\"$CLASH\",\"provider\":\"local\"}" >/dev/null
Z3=$(papi POST /dns/zones "{\"domain\":\"$CLASH\",\"provider\":\"local\"}" | jget 'd["zone"]["id"]')
R=$(papi POST /dns/cluster/sync)
check "ikincilin kendi bölgesiyle çakışan ad bildirilir" "$(jget 'd["secondaries"][0]["conflicts"]' <<<"$R")" "['$CLASH']"
check "ikincilin kendi bölgesine dokunulmaz" "$(sapi GET /dns | jget "len([z for z in d['zones'] if z['domain']=='$CLASH'])")" 1
papi DELETE /dns/zones/$Z3 >/dev/null
check "anahtar başka adresten kullanılamaz (izinli adres)" "$(s "curl -sk -o /dev/null -w %{http_code} -H 'authorization: Bearer $TOKEN' -H 'content-type: application/json' -X PUT https://127.0.0.1:8443/api/v1/dns/replica -d '{\"zones\":[]}'")" 401
check "ikincil kendisinin ikincili olamaz" "$(sapi PUT /dns/replica "{\"zones\":[],\"primary\":\"$SECONDARY_IP\"}" | jget 'd["error"]["code"]')" bad_request
check "geçersiz alan adı listeyi bozmaz" "$(sapi PUT /dns/replica "{\"zones\":[\"../etc\"],\"primary\":\"$PRIMARY_IP\"}" | jget 'd["error"]["code"]') $(q $SECONDARY_IP A www2.$ZONE)" "bad_request 203.0.113.50"

say "İkincil kapalıyken"
s 'systemctl stop pixpanel'
R=$(papi POST /dns/cluster/sync)
has "eşitleme hatası kaydedilir" "$(jget 'd["secondaries"][0]["last_error"]' <<<"$R")" "bağlanılamadı"
papi POST /dns/zones/$Z/records "$(rec A kapaliyken 203.0.113.52)" >/dev/null
check "panel kapalıyken de kayıtlar aktarılır (DNS sunucusu çalışıyor)" "$(wait_for 30 203.0.113.52 q $SECONDARY_IP A kapaliyken.$ZONE)" 203.0.113.52
s 'systemctl start pixpanel'
s "sh -c 'for i in \$(seq 1 30); do curl -sk -o /dev/null https://127.0.0.1:8443/ && break; sleep 1; done'"
check "panel dönünce eşitleme düzelir" "$(papi POST /dns/cluster/sync | jget 'd["secondaries"][0]["last_error"]')" ""

say "İkincilin sertifikası değişirse"
FP=$(papi GET /dns/cluster | jget 'd["secondaries"][0]["fingerprint"]')
s 'rm -f /var/lib/pixpanel/tls/panel.crt /var/lib/pixpanel/tls/panel.key'
s 'systemctl restart pixpanel'
s "sh -c 'for i in \$(seq 1 30); do curl -sk -o /dev/null https://127.0.0.1:8443/ && break; sleep 1; done'"
has "değişen sertifikaya anahtar gönderilmez" "$(papi POST /dns/cluster/sync | jget 'd["secondaries"][0]["last_error"]')" "sertifikası değişmiş"
R=$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:8443\"}]")
check "yeniden kaydetmek sabiti sessizce yenilemez" "$(jget '(d["secondaries"][0]["fingerprint"] == "'$FP'", d["secondaries"][0]["last_error"] != "")' <<<"$R")" "(True, True)"
R=$(cluster "[{\"address\":\"$SECONDARY_IP\",\"panel_url\":\"https://$SECONDARY_IP:8443\",\"repin\":true}]")
check "yönetici onaylayınca yeni sertifika sabitlenir ve eşitleme düzelir" "$(jget '(d["secondaries"][0]["fingerprint"] != "'$FP'", d["secondaries"][0]["last_error"])' <<<"$R")" "(True, '')"

say "Kümenin dağıtılması"
check "ikincili kaldır" "$(cluster '[]' | jget 'len(d["secondaries"])')" 0
check "birincil artık bildirim göndermez" "$(p 'grep -c also-notify /etc/powerdns/pdns.d/pixpanel.conf')" 0
check "ns2 kaydı birincile döner" "$(q $PRIMARY_IP A ns2.$ZONE)" "$PRIMARY_IP"
check "ikincilde bağlantıyı kes" "$(sapi DELETE /dns/replica/$PRIMARY_IP | jget 'len(d["replica_of"])')" 0
check "ikincildeki bölgeler silindi" "$(q $SECONDARY_IP SOA $ZONE)" ""
check "ikincil artık bildirim kabul etmez" "$(s 'grep -c allow-notify-from /etc/powerdns/pdns.d/pixpanel.conf')" 0
check "ikincilin kendi bölgesi duruyor" "$(sapi GET /dns | jget "len([z for z in d['zones'] if z['domain']=='$CLASH'])")" 1
papi DELETE /dns/zones/$Z >/dev/null
for id in $(sapi GET /dns | jget '" ".join(str(z["id"]) for z in d["zones"])'); do sapi DELETE /dns/zones/$id >/dev/null; done
p 'rm -f /tmp/cluster-body.json'
echo "bitti"
