#!/usr/bin/env bash
# DNS derinliği sınaması: DNSSEC (yerel PowerDNS), BIND bölge dosyası dışa/içe aktarma ve
# Cloudflare'de SRV/CAA yazma (sahte API'ye karşı). Test makinesinde root olarak çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash dns-depth.sh
# Önkoşul: DNS sunucusu kurulu (dns.sh kurar); Cloudflare bölümü için /etc/pixpanel/config.toml
# içinde cloudflare_api = "http://127.0.0.1:9912" (yoksa o bölüm atlanır).
# Kapsam dışı: DNSSEC'li bölgenin ikincil ad sunucusuna aktarımı iki makine ister (cluster.sh'ye bakın).
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-depth
JA=/tmp/pxjar-depth-a
HERE="$(cd "$(dirname "$0")" && pwd)"
CF_TOKEN=cf_test_token_0123456789abcdefghij
ZONE=dnsdepth.test
OTHER=dnsdepa
OTHER_ZONE=dnsdepth-a.test
PASS='Derinlik-Sinama-Parolasi-1'
WORK=/tmp/px-depth
mkdir -p "$WORK"

call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b "$jar" -c "$jar" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b "$jar" -c "$jar" -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 260)]"; fi; }
hasnt() { if grep -qF -- "$3" <<<"$2"; then echo "  HATA $1: [$3] olmamalıydı: [$(tr -d '\n' <<<"$2" | head -c 260)]"; else echo "  OK   $1"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
q() { dig +short +time=2 +tries=1 @127.0.0.1 "$@" | tr '\n' ' ' | sed 's/ $//'; }
# Çıktı beklenene eşit olana dek (en çok 20 sn) dener; PowerDNS önbelleği değişikliği geciktirebilir.
poll() { local want=$1 out; shift; for _ in $(seq 1 20); do out=$("$@" 2>/dev/null); [ "$out" = "$want" ] && break; sleep 1; done; printf '%s' "$out"; }
# İmzalı yanıt (RRSIG) var mı?
signed() { dig +dnssec +noall +answer +time=2 +tries=1 @127.0.0.1 "$@" | grep -q RRSIG && echo evet || echo hayir; }
# Yok adın yanıtında NSEC (kanıt) var mı?
denial() { dig +dnssec +noall +authority +time=2 +tries=1 @127.0.0.1 "$@" | grep -qE '[[:space:]]NSEC[[:space:]]' && echo evet || echo hayir; }
zone_id() { api GET /dns | jget "next((str(z['id']) for z in d['zones'] if z['domain']=='$1'), '')"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
site_id() { api GET /sites | jget "next((s['id'] for s in d if s['domain']=='$1'), '')"; }
rec() { python3 -c 'import json,sys; a=sys.argv; print(json.dumps({"record":{"id":"","type":a[1],"name":a[2],"content":a[3],"ttl":int(a[4]) if len(a)>4 else 3600,"priority":int(a[5]) if len(a)>5 and a[5] else None,"proxied":None}}))' "$@"; }
# imp <bölge kimliği> <dosya> <değiştir true|false> <uygula true|false> [jar]: içe aktarma isteği, yanıt gövdesi yazılır
imp() {
  python3 -c 'import json,sys; print(json.dumps({"text": open(sys.argv[1], encoding="utf-8", newline="").read(), "replace": sys.argv[2]=="true", "apply": sys.argv[3]=="true"}))' "$2" "$3" "$4" >"$WORK/body.json"
  call ${5:-$J} POST "/dns/zones/$1/import" -d @"$WORK/body.json"
}
imp_code() {
  python3 -c 'import json,sys; print(json.dumps({"text": open(sys.argv[1], encoding="utf-8", newline="").read(), "replace": False, "apply": False}))' "$2" >"$WORK/body.json"
  status ${3:-$J} POST "/dns/zones/$1/import" -d @"$WORK/body.json"
}
login() { curl -sk -o /dev/null -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$PASS\"}"; }

rm -f $J $JA
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Hazırlık"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="dns")')" != True ]; then
  echo "  HATA DNS sunucusu kurulu değil; önce dns.sh çalıştırın"; exit 1
fi
# Önceki koşudan kalanlar temizlenir.
for name in $ZONE bulut.test $OTHER_ZONE; do id=$(zone_id $name); [ -n "$id" ] && api DELETE /dns/zones/$id >/dev/null; done
id=$(site_id $OTHER_ZONE); [ -n "$id" ] && api DELETE "/sites/$id?delete_files=true" >/dev/null
id=$(user_id $OTHER); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null
pkill -f "python3 $HERE/[m]ock_cloudflare" 2>/dev/null; api DELETE /dns/cloudflare >/dev/null

Z=$(api POST /dns/zones -d "{\"domain\":\"$ZONE\",\"provider\":\"local\"}" | jget 'd["zone"]["id"]')
check "bölge açıldı" "$([[ "$Z" =~ ^[0-9]+$ ]] && echo evet)" evet
ZD=/dns/zones/$Z
check "başka hesap kuruldu" "$(code POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\",\"role\":\"user\"}")" 200
login $JA $OTHER
check "başka hesabın sitesi" "$(status $JA POST /sites -d "{\"domain\":\"$OTHER_ZONE\",\"kind\":\"static\"}")" 200
OZ=$(call $JA POST /dns/zones -d "{\"domain\":\"$OTHER_ZONE\",\"provider\":\"local\"}" | jget 'd["zone"]["id"]')
check "başka hesabın bölgesi açıldı" "$([[ "$OZ" =~ ^[0-9]+$ ]] && echo evet)" evet

# ---------------------------------------------------------------------------
say "DNSSEC: açmadan önce"
D=$(api GET $ZD)
check "DNSSEC kapalı görünüyor" "$(echo "$D" | jget 'd["dnssec"]["enabled"]')" False
check "imza yok" "$(signed A $ZONE)" hayir
S0=$(q SOA $ZONE | awk '{print $3}')
echo "  (SOA seri numarası: $S0)"

say "DNSSEC: açma"
check "aç" "$(code POST $ZD/dnssec -d '{"enabled":true}')" 200
check "ikinci kez açmak" "$(code POST $ZD/dnssec -d '{"enabled":true}')" 409
D=$(api GET $ZD)
check "panel açık gösteriyor" "$(echo "$D" | jget 'd["dnssec"]["enabled"]')" True
check "tek anahtar (CSK)" "$(echo "$D" | jget '[k["keytype"] for k in d["dnssec"]["keys"]]')" "['csk']"
check "DS algoritması 13 (ECDSA P-256)" "$(echo "$D" | jget 'sorted(set(x["algorithm"] for x in d["dnssec"]["keys"][0]["ds"]))')" "[13]"
check "SHA-256 özetli DS var" "$(echo "$D" | jget 'any(x["digest_type"]==2 for x in d["dnssec"]["keys"][0]["ds"])')" True
check "SHA-1 özeti gösterilmiyor" "$(echo "$D" | jget 'any(x["digest_type"]==1 for x in d["dnssec"]["keys"][0]["ds"])')" False
check "yanıt imzalı (RRSIG)" "$(poll evet signed A $ZONE)" evet
check "ikinci tür de imzalı" "$(poll evet signed SOA $ZONE)" evet
check "yok ad için kanıt (NSEC) dönüyor" "$(poll evet denial A yok-boyle-bir-ad.$ZONE)" evet
has "DNSKEY yayınlanıyor (bayrak 257, algoritma 13)" "$(q DNSKEY $ZONE)" "257 3 13 "

# Panelin gösterdiği DNSKEY ve DS, çözücünün gördüğü anahtardan bağımsız olarak yeniden hesaplanır.
DIG_KEY=$(dig +short +time=2 +tries=1 @127.0.0.1 DNSKEY $ZONE | grep '^257 ' | head -1)
PANEL_KEY=$(echo "$D" | jget 'd["dnssec"]["keys"][0]["dnskey"]')
check "DNSKEY panelle aynı" "$(tr -d ' ()' <<<"$DIG_KEY")" "$(tr -d ' ()' <<<"$PANEL_KEY")"
COMPUTED=$(python3 - "$ZONE" "$DIG_KEY" <<'PY'
import base64, hashlib, struct, sys
owner, text = sys.argv[1], sys.argv[2].split()
flags, proto, alg = int(text[0]), int(text[1]), int(text[2])
rdata = struct.pack("!HBB", flags, proto, alg) + base64.b64decode("".join(text[3:]))
acc = 0
for i, b in enumerate(rdata):
    acc += b if i & 1 else b << 8
acc += (acc >> 16) & 0xFFFF
tag = acc & 0xFFFF
wire = b"".join(bytes([len(label)]) + label.encode() for label in owner.lower().rstrip(".").split(".")) + b"\0"
print(tag, hashlib.sha256(wire + rdata).hexdigest().upper())
PY
)
check "DS (anahtar etiketi ve SHA-256 özeti) DNSKEY'den hesaplananla aynı" "$(echo "$D" | jget 'next((str(x["key_tag"]) + " " + x["digest"] for x in d["dnssec"]["keys"][0]["ds"] if x["digest_type"]==2), "")')" "$COMPUTED"
if command -v dnssec-dsfromkey >/dev/null 2>&1; then
  TOOL=$(dig +noall +answer +time=2 +tries=1 @127.0.0.1 DNSKEY $ZONE | dnssec-dsfromkey -2 -f - $ZONE 2>/dev/null | awk '{print $4, $7}')
  check "DS dnssec-dsfromkey çıktısıyla aynı" "$TOOL" "$COMPUTED"
else echo "  (dnssec-dsfromkey yok; bu karşılaştırma atlandı)"; fi
if command -v pdnsutil >/dev/null 2>&1; then
  has "pdnsutil show-zone aynı DS özetini gösteriyor" "$(pdnsutil show-zone $ZONE 2>/dev/null | tr 'a-f' 'A-F')" "$(awk '{print $2}' <<<"$COMPUTED")"
else echo "  (pdnsutil yok; show-zone karşılaştırması atlandı)"; fi

S1=""
for _ in $(seq 1 15); do S1=$(q SOA $ZONE | awk '{print $3}'); [ -n "$S1" ] && [ "$S1" != "$S0" ] && break; sleep 1; done
check "açınca SOA seri numarası arttı (ikincil yeniden çeksin diye)" "$([ -n "$S1" ] && [ -n "$S0" ] && [ "$S1" -gt "$S0" ] && echo evet || echo "hayir ($S0 -> $S1)")" evet

say "DNSSEC: açıkken yeni kayıt"
check "A kaydı ekle" "$(code POST $ZD/records -d "$(rec A yeni 203.0.113.77 300)")" 200
check "yeni kayıt yayınlandı" "$(poll 203.0.113.77 q A yeni.$ZONE)" 203.0.113.77
check "yeni kayıt da imzalı" "$(poll evet signed A yeni.$ZONE)" evet
check "TXT ekle" "$(code POST $ZD/records -d "$(rec TXT imzali 'imza deneme' 300)")" 200
check "yeni TXT imzalı" "$(poll evet signed TXT imzali.$ZONE)" evet
check "kayıt silinince imzalı bölge sağlam" "$(code POST $ZD/records/delete -d "$(rec A yeni 203.0.113.77 300)")" 200
check "silinen ad için kanıt (NSEC)" "$(poll evet denial A yeni.$ZONE)" evet

say "DNSSEC: yetki"
check "oturumsuz: durum değiştirme" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -H 'x-pixpanel-csrf: 1' -d '{"enabled":false}' $B$ZD/dnssec)" 401
check "başka hesap: bölge ayrıntısı" "$(status $JA GET $ZD)" 404
check "başka hesap: DNSSEC kapatma" "$(status $JA POST $ZD/dnssec -d '{"enabled":false}')" 404
check "başka hesap: DNSSEC açma" "$(status $JA POST $ZD/dnssec -d '{"enabled":true}')" 404
check "DNSSEC hâlâ açık" "$(api GET $ZD | jget 'd["dnssec"]["enabled"]')" True
check "geçersiz gövde" "$(code POST $ZD/dnssec -d '{"enabled":"evet"}')" 422
check "başka hesap kendi bölgesinde DNSSEC açabilir" "$(status $JA POST /dns/zones/$OZ/dnssec -d '{"enabled":true}')" 200
check "başka hesabın bölgesi imzalı" "$(poll evet signed SOA $OTHER_ZONE)" evet
check "başka hesap kendi bölgesinde kapatabilir" "$(status $JA POST /dns/zones/$OZ/dnssec -d '{"enabled":false}')" 200

# ---------------------------------------------------------------------------
say "Bölge dosyası: dışa aktarma"
X=$(api GET $ZD/export)
check "dosya adı" "$(echo "$X" | jget 'd["filename"]')" "$ZONE.zone"
has "başlıkta \$ORIGIN" "$(echo "$X" | jget 'd["content"]')" "\$ORIGIN $ZONE."
has "SOA satırı" "$(echo "$X" | jget 'd["content"]')" "SOA"
has "kök A kaydı @ olarak yazıldı" "$(echo "$X" | jget 'repr(d["content"])')" '@\t3600\tIN\tA\t'
check "başka hesap dışa aktaramaz" "$(status $JA GET $ZD/export)" 404
check "oturumsuz dışa aktarma" "$(curl -sk -o /dev/null -w '%{http_code}' $B$ZD/export)" 401

say "Bölge dosyası: içe aktarma önizlemesi"
cat >"$WORK/sample.zone" <<'EOF'
; örnek bölge dosyası (yorum)
$ORIGIN dnsdepth.test.
$TTL 1h
@ IN SOA ns1.dnsdepth.test. hostmaster.dnsdepth.test. (
        2026111201 ; seri
        3h 1h 1w 1h )
@ IN NS ns1.dnsdepth.test.
@ 300 IN A 198.51.100.10
imp1 IN A 203.0.113.21
imp2 IN AAAA 2001:DB8:0:0:0:0:0:2
impc IN CNAME imp1
@ IN MX 10 mail.dnsdepth.test.
@ IN TXT "v=spf1 ip4:198.51.100.10" " ~all"
txtq IN TXT "has \"quote\" and ; semicolon \\ backslash"
_imp._tcp IN SRV 5 10 5060 imp1
@ IN CAA 0 issue "letsencrypt.org"
ns3 IN A 203.0.113.30
sub IN NS ns.example.net.
other.example.org. IN A 192.0.2.1
@ IN LOC 52 22 23.000 N 4 53 32.000 E -2.00m 0.00m 10000m 10m 10m
broken IN A 999.1.1.1
EOF
P=$(imp $Z "$WORK/sample.zone" false false)
check "eklenecek sayısı" "$(echo "$P" | jget 'len(d["add"])')" 11
check "zaten var sayısı (kök NS)" "$(echo "$P" | jget 'd["existing_count"]')" 1
check "atlanacak sayısı (SOA, bölge dışı, LOC, bozuk)" "$(echo "$P" | jget 'd["skipped_total"]')" 4
check "silinecek yok" "$(echo "$P" | jget 'len(d["remove"])')" 0
has "SOA atlama nedeni" "$(echo "$P" | jget '[s["reason"] for s in d["skipped"]]')" "SOA"
has "bölge dışı atlama nedeni" "$(echo "$P" | jget '[s["reason"] for s in d["skipped"]]')" "dışında"
has "desteklenmeyen tür nedeni" "$(echo "$P" | jget '[s["reason"] for s in d["skipped"]]')" "LOC"
check "atlanan satır numaraları doğru" "$(echo "$P" | jget 'sorted(s["line"] for s in d["skipped"])')" "[4, 19, 20, 21]"
check "önizleme hiçbir şey yazmadı" "$(q A imp1.$ZONE)" ""
check "başka hesap önizleyemez" "$(imp_code $Z "$WORK/sample.zone" $JA)" 404
check "oturumsuz önizleme" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -H 'x-pixpanel-csrf: 1' -d '{"text":"","apply":false}' $B$ZD/import)" 401

say "Bölge dosyası: uygulama"
R=$(imp $Z "$WORK/sample.zone" false true)
check "eklenen" "$(echo "$R" | jget 'd["result"]["added"]')" 11
check "başarısız yok" "$(echo "$R" | jget 'd["result"]["failed"]')" "[]"
check "A (göreli ad)" "$(poll 203.0.113.21 q A imp1.$ZONE)" 203.0.113.21
check "AAAA (kısaltılarak)" "$(q AAAA imp2.$ZONE)" "2001:db8::2"
check "CNAME (@ ve göreli hedef)" "$(q CNAME impc.$ZONE)" "imp1.$ZONE."
check "MX (göreli sunucu)" "$(q MX $ZONE)" "10 mail.$ZONE."
has "kök A (kendi TTL'iyle) var" "$(q A $ZONE)" "198.51.100.10"
has "SPF: iki parça birleşti" "$(q TXT $ZONE)" '"v=spf1 ip4:198.51.100.10 ~all"'
check "tırnaklı, noktalı virgüllü, ters eğik çizgili TXT bozulmadı" "$(q TXT txtq.$ZONE)" '"has \"quote\" and ; semicolon \\ backslash"'
check "SRV" "$(q SRV _imp._tcp.$ZONE)" "5 10 5060 imp1.$ZONE."
check "CAA" "$(q CAA $ZONE)" '0 issue "letsencrypt.org"'
check "ns3" "$(q A ns3.$ZONE)" "203.0.113.30"
check "alt alan adı delegasyonu (NS) panelde" "$(echo "$R" | jget 'any(r["type"]=="NS" and r["name"]=="sub.'$ZONE'" and r["content"]=="ns.example.net" for r in d["records"])')" True
check "bölge dışı ad bölgeye eklenmedi" "$(echo "$R" | jget 'any("example.org" in r["name"] for r in d["records"])')" False
check "DNSSEC açıkken içe aktarılan kayıt da imzalı" "$(poll evet signed A imp1.$ZONE)" evet
check "ikinci uygulama: önizleme yeni kayıt görmüyor" "$(imp $Z "$WORK/sample.zone" false false | jget '(len(d["add"]), d["existing_count"])')" "(0, 12)"
check "ikinci uygulama: hiçbir şey eklenmedi" "$(imp $Z "$WORK/sample.zone" false true | jget 'd["result"]["added"]')" 0

say "Bölge dosyası: değiştirme kipi"
printf 'imp1 IN A 203.0.113.100\n' >"$WORK/add.zone"
check "değiştirmeden: eski kayıt kalır" "$(imp $Z "$WORK/add.zone" false true | jget '(d["result"]["added"], d["result"]["removed"])')" "(1, 0)"
check "imp1 artık iki değerli" "$(poll '203.0.113.100 203.0.113.21' sh -c "dig +short @127.0.0.1 A imp1.$ZONE | sort | tr '\n' ' ' | sed 's/ \$//'")" "203.0.113.100 203.0.113.21"
printf 'imp1 IN A 203.0.113.99\n@ IN NS ns.baska-saglayici.net.\n' >"$WORK/replace.zone"
P=$(imp $Z "$WORK/replace.zone" true false)
check "önizleme: iki eski değer silinecek" "$(echo "$P" | jget 'sorted(r["content"] for r in d["remove"])')" "['203.0.113.100', '203.0.113.21']"
check "önizleme: kök NS eklenmeyecek, nedeni yazılı" "$(echo "$P" | jget 'any("Kök ad sunucusu" in s["reason"] for s in d["skipped"])')" True
imp $Z "$WORK/replace.zone" true true >"$WORK/out.json"
check "değiştirme uygulandı" "$(jget '(d["result"]["added"], d["result"]["removed"], d["result"]["failed"])' <"$WORK/out.json")" "(1, 2, [])"
check "imp1 yalnızca yeni değer" "$(poll 203.0.113.99 q A imp1.$ZONE)" 203.0.113.99
check "başka adlar dokunulmadı" "$(q AAAA imp2.$ZONE) $(q A ns3.$ZONE)" "2001:db8::2 203.0.113.30"
check "kök ad sunucusu kaydı dışarıdan değişmedi" "$(q NS $ZONE | tr ' ' '\n' | grep -c baska-saglayici)" 0

say "Bölge dosyası: dışa aktarılan yeniden içe aktarılınca \"zaten var\""
api GET $ZD/export | jget 'd["content"]' >"$WORK/export.zone"
TOTAL=$(api GET $ZD | jget 'len(d["records"])')
P=$(imp $Z "$WORK/export.zone" false false)
check "yeniden içe aktarmada eklenecek yok" "$(echo "$P" | jget 'len(d["add"])')" 0
check "hepsi zaten var" "$(echo "$P" | jget 'd["existing_count"]')" "$TOTAL"
check "yalnızca SOA atlandı" "$(echo "$P" | jget '[s["reason"] for s in d["skipped"]]')" "['SOA kaydı sunucu tarafından yönetilir']"
check "değiştirme kipinde de silinecek yok" "$(imp $Z "$WORK/export.zone" true false | jget 'len(d["remove"])')" 0

say "Bölge dosyası: bozuk girdi, sınırlar ve enjeksiyon"
# CRLF satır sonları geçerlidir; öteki satırlar kayıt olamaz ya da değer olarak kalır.
{
  printf 't1 IN TXT "x\\" ; IN A 6.6.6.6"\r\n'
  printf 'crlf1 IN A 198.51.100.1\r\n'
  printf 'evil;x IN A 6.6.6.6\n'
  printf 'bad name IN A 6.6.6.6\n'
  printf '$(id) IN A 6.6.6.6\n'
  printf 'a$(id) IN A 6.6.6.6\n'
  printf '$INCLUDE /etc/passwd\n'
  printf 'ctrl IN TXT "a\001b"\n'
  printf 'esc IN CNAME x.example.com.\\032y\n'
  printf '$ORIGIN evil.example.\nout IN A 6.6.6.6\n'
} >"$WORK/inj.zone"
P=$(imp $Z "$WORK/inj.zone" false false)
check "yalnızca iki zararsız kayıt eklenecek" "$(echo "$P" | jget 'sorted((r["type"], r["name"]) for r in d["add"])')" "[('A', 'crlf1.$ZONE'), ('TXT', 't1.$ZONE')]"
check "geri kalan satırlar atlandı" "$(echo "$P" | jget 'd["skipped_total"]')" 8
has "\$INCLUDE desteklenmiyor" "$(echo "$P" | jget '[s["reason"] for s in d["skipped"]]')" "INCLUDE"
imp $Z "$WORK/inj.zone" false true >"$WORK/out.json"
check "uygulama zararsız kayıtları yazdı" "$(jget 'd["result"]["added"]' <"$WORK/out.json")" 2
check "TXT içindeki sahte A kaydı oluşmadı" "$(q A t1.$ZONE)" ""
check "TXT değeri tırnak ve noktalı virgülle birlikte durur" "$(poll '"x\" ; IN A 6.6.6.6"' q TXT t1.$ZONE)" '"x\" ; IN A 6.6.6.6"'
check "CRLF'li satır eklendi" "$(poll 198.51.100.1 q A crlf1.$ZONE)" 198.51.100.1
check "noktalı virgülle bölünen ad oluşmadı" "$(q A evil.$ZONE)" ""
check "boşluklu ad oluşmadı" "$(q A bad.$ZONE)" ""
check "kabuk ifadeli ad oluşmadı" "$(api GET $ZD | jget 'any("(id)" in r["name"] or "root:" in r["content"] for r in d["records"])')" False
check "başka \$ORIGIN'deki kayıt eklenmedi" "$(q A out.$ZONE)" ""

python3 -c 'import json; print(json.dumps({"text": "x IN A 1.1.1.1\n" * 40000, "replace": False, "apply": False}))' >"$WORK/big.json"
check "512 KB'ı aşan metin reddedilir" "$(code POST $ZD/import -d @"$WORK/big.json")" 400
python3 -c 'import json; print(json.dumps({"text": "x IN A 1.1.1.1\n" * 200000, "replace": False, "apply": False}))' >"$WORK/huge.json"
check "gövde sınırını aşan metin reddedilir" "$(code POST $ZD/import -d @"$WORK/huge.json")" 413
python3 -c '
for i in range(5001):
    print("h%d IN A 10.%d.%d.%d" % (i, (i >> 16) & 255, (i >> 8) & 255, i & 255))' >"$WORK/many.zone"
check "5000'den fazla kayıt reddedilir" "$(imp_code $Z "$WORK/many.zone")" 400
head -5000 "$WORK/many.zone" >"$WORK/limit.zone"
check "tam 5000 kayıt önizlenebilir" "$(imp $Z "$WORK/limit.zone" false false | jget 'len(d["add"])')" 5000
: >"$WORK/empty.zone"
check "boş metin" "$(imp $Z "$WORK/empty.zone" false false | jget '(len(d["add"]), d["skipped_total"])')" "(0, 0)"
printf 'a IN A 1.1.1.1\nb IN TXT "kapanmamis\nc IN A 1.1.1.2\n( kapanmayan\n' >"$WORK/broken.zone"
check "kapanmamış tırnak ve parantez: geçerli satırlar alınır, bozuklar listelenir" "$(imp $Z "$WORK/broken.zone" false false | jget '(sorted(r["name"] for r in d["add"]), d["skipped_total"])')" "(['a.$ZONE', 'c.$ZONE'], 2)"

# ---------------------------------------------------------------------------
say "Cloudflare: SRV, CAA, dışa/içe aktarma, DNSSEC bölümü yok (sahte API)"
if ! grep -q 'cloudflare_api.*127.0.0.1:9912' /etc/pixpanel/config.toml 2>/dev/null; then
  echo "  (config.toml'da cloudflare_api = \"http://127.0.0.1:9912\" yok; Cloudflare bölümü atlandı)"
else
  : >/tmp/px-cf.log
  (setsid python3 "$HERE/mock_cloudflare.py" >/dev/null 2>&1 &); sleep 1
  check "anahtar kaydı" "$(code PUT /dns/cloudflare -d "{\"token\":\"$CF_TOKEN\"}")" 200
  CZ=$(api POST /dns/zones -d '{"domain":"bulut.test","provider":"cloudflare"}' | jget 'd["zone"]["id"]')
  CD=/dns/zones/$CZ
  check "Cloudflare bölgesinde DNSSEC bölümü yok" "$(api GET $CD | jget 'd["dnssec"]')" None
  check "Cloudflare bölgesinde DNSSEC açılamaz" "$(code POST $CD/dnssec -d '{"enabled":true}')" 400
  check "SRV yazılır" "$(code POST $CD/records -d "$(rec SRV _sip._tcp '10 5060 sip.bulut.test' 3600 5)")" 200
  check "CAA yazılır" "$(code POST $CD/records -d "$(rec CAA @ '0 issue "letsencrypt.org"' 3600)")" 200
  check "SRV data alanıyla gitti" "$(grep -c '^data {"port": 5060, "priority": 5, "target": "sip.bulut.test", "weight": 10}' /tmp/px-cf.log)" 1
  check "CAA data alanıyla gitti" "$(grep -c '^data {"flags": 0, "tag": "issue", "value": "letsencrypt.org"}' /tmp/px-cf.log)" 1
  CR=$(api GET $CD)
  check "SRV panelin biçiminde okunuyor" "$(echo "$CR" | jget 'next((r["content"], r["priority"]) for r in d["records"] if r["type"]=="SRV")')" "('10 5060 sip.bulut.test', 5)"
  check "CAA panelin biçiminde okunuyor" "$(echo "$CR" | jget 'next(r["content"] for r in d["records"] if r["type"]=="CAA")')" '0 issue "letsencrypt.org"'
  check "aynı SRV ikinci kez" "$(code POST $CD/records -d "$(rec SRV _sip._tcp '10 5060 sip.bulut.test' 3600 5)")" 400
  SRV=$(echo "$CR" | jget 'json.dumps(next(r for r in d["records"] if r["type"]=="SRV"))')
  NEW=$(echo "$SRV" | jget 'd.update(content="20 5061 sip2.bulut.test", priority=7) or json.dumps(d)')
  check "SRV güncellenir" "$(code PUT $CD/records -d "{\"old\":$SRV,\"new\":$NEW}")" 200
  check "güncel SRV okunuyor" "$(api GET $CD | jget 'next((r["content"], r["priority"]) for r in d["records"] if r["type"]=="SRV")')" "('20 5061 sip2.bulut.test', 7)"
  check "SRV silinir" "$(code POST $CD/records/delete -d "{\"record\":$NEW}")" 200
  check "SRV silindi" "$(api GET $CD | jget 'any(r["type"]=="SRV" for r in d["records"])')" False

  CX=$(api GET $CD/export | jget 'd["content"]')
  TAB=$'\t'
  has "dışa aktarılan dosyada CAA" "$CX" "@${TAB}3600${TAB}IN${TAB}CAA${TAB}0 issue \"letsencrypt.org\""
  cat >"$WORK/cf.zone" <<'EOF'
$ORIGIN bulut.test.
$TTL 300
@ IN CAA 0 issue "letsencrypt.org"
@ IN CAA 0 issuewild ";"
_xmpp._tcp IN SRV 5 0 5269 xmpp
web IN A 203.0.113.5
EOF
  P=$(imp $CZ "$WORK/cf.zone" false false)
  check "Cloudflare önizleme: 3 yeni, 1 var" "$(echo "$P" | jget '(len(d["add"]), d["existing_count"])')" "(3, 1)"
  check "Cloudflare uygulama" "$(imp $CZ "$WORK/cf.zone" false true | jget '(d["result"]["added"], d["result"]["failed"])')" "(3, [])"
  check "Cloudflare'de SRV içe aktarıldı" "$(grep -c '^data {"port": 5269, "priority": 5, "target": "xmpp.bulut.test", "weight": 0}' /tmp/px-cf.log)" 1
  check "Cloudflare'de ikinci CAA içe aktarıldı" "$(grep -c '^data {"flags": 0, "tag": "issuewild", "value": ";"}' /tmp/px-cf.log)" 1
  python3 -c '
for i in range(501):
    print("c%d IN A 10.0.%d.%d" % (i, i >> 8, i & 255))' >"$WORK/cf-many.zone"
  check "Cloudflare'e tek seferde 500'den fazla kayıt reddedilir" "$(imp_code $CZ "$WORK/cf-many.zone")" 400
  check "bölge bağlantısını kaldır" "$(code DELETE $CD)" 200
  check "bağlantıyı kes" "$(code DELETE /dns/cloudflare)" 200
  pkill -f "python3 $HERE/[m]ock_cloudflare" 2>/dev/null
fi

# ---------------------------------------------------------------------------
say "DNSSEC: kapatma"
check "kapat" "$(code POST $ZD/dnssec -d '{"enabled":false}')" 200
check "panel kapalı gösteriyor" "$(api GET $ZD | jget '(d["dnssec"]["enabled"], d["dnssec"]["keys"])')" "(False, [])"
check "imzalar kalktı" "$(poll hayir signed A $ZONE)" hayir
check "DNSKEY kalktı" "$(poll '' q DNSKEY $ZONE)" ""
check "bölge imzasız yanıt vermeyi sürdürüyor" "$(q A imp1.$ZONE)" 203.0.113.99
check "açıp kapatma tekrarlanabilir" "$(code POST $ZD/dnssec -d '{"enabled":true}') $(code POST $ZD/dnssec -d '{"enabled":false}')" "200 200"
check "tekrar kapatınca da imzasız" "$(poll hayir signed SOA $ZONE)" hayir

say "Temizlik"
check "bölge sil" "$(code DELETE $ZD)" 200
check "başka hesabın bölgesi sil" "$(status $JA DELETE /dns/zones/$OZ)" 200
check "başka hesabın sitesi sil" "$(code DELETE "/sites/$(site_id $OTHER_ZONE)?delete_files=true")" 200
check "başka hesap sil" "$(code DELETE /users/$(user_id $OTHER))" 200
rm -rf "$WORK" $J $JA
echo "bitti"
