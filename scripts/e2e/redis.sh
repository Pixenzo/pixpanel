#!/usr/bin/env bash
# Redis tarayıcısının sınaması: süzme, sayfalama, düzenleme ve konsol. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash redis.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-redis
DB=5
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
edit() { python3 -c 'import json,sys; d={"db":int(sys.argv[1]),"key":sys.argv[2],"op":sys.argv[3]}; d.update(json.loads(sys.argv[4])); print(json.dumps(d))' $DB "$1" "$2" "$3"; }
cmd() { api POST /redis/command -d "$(python3 -c 'import json,sys; print(json.dumps({"db":int(sys.argv[1]),"command":sys.argv[2]}))' $DB "$1")"; }
r() { redis-cli -n $DB "$@"; }

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
r flushdb >/dev/null
for i in $(seq 1 450); do echo "SET oturum:$i deger$i"; done | redis-cli -n $DB >/dev/null
r hset kullanici:1 ad Ali soyad Veli yas 30 >/dev/null
for i in $(seq 1 450); do echo "RPUSH kuyruk:isler is$i"; done | redis-cli -n $DB >/dev/null
r sadd etiketler a b c >/dev/null
r zadd puanlar 10 ali 20 veli >/dev/null
r xadd olaylar '*' tur giris kim ali >/dev/null

say "Listeleme ve süzme"
P1=$(api GET "/redis/keys?db=$DB")
check "ilk istek boş dönmez" "$(echo "$P1" | jget 'len(d["keys"]) >= 200')" True
check "imleçle devam edince hepsi gelir" "$(python3 - <<PY
import json,subprocess
seen=set(); cursor="0"
while True:
    out=subprocess.run(["curl","-sk","-b","$J","$B/redis/keys?db=$DB&cursor="+cursor],capture_output=True,text=True).stdout
    d=json.loads(out); seen.update(k["key"] for k in d["keys"]); cursor=d["cursor"]
    if cursor=="0": break
print(len(seen))
PY
)" 455
check "tür süzgeci" "$(api GET "/redis/keys?db=$DB&type=hash" | jget '[k["key"] for k in d["keys"]]')" "['kullanici:1']"
check "kalıp" "$(api GET "/redis/keys?db=$DB&pattern=kuyruk:*" | jget '[(k["key"], k["type"]) for k in d["keys"]]')" "[('kuyruk:isler', 'list')]"
check "geçersiz tür" "$(code GET "/redis/keys?db=$DB&type=x")" 400

say "Ayrıntı ve sayfalama"
L=$(api GET "/redis/key?db=$DB&key=kuyruk:isler")
check "liste ilk sayfa" "$(echo "$L" | jget '(d["length"], len(d["content"]["items"]), d["content"]["items"][0], d["next"])')" "(450, 200, 'is1', '200')"
check "liste ikinci sayfa" "$(api GET "/redis/key?db=$DB&key=kuyruk:isler&cursor=200" | jget '(d["content"]["items"][0], d["next"])')" "('is201', '400')"
check "liste son sayfa" "$(api GET "/redis/key?db=$DB&key=kuyruk:isler&cursor=400" | jget '(len(d["content"]["items"]), d["next"])')" "(50, '')"
check "hash alan araması" "$(api GET "/redis/key?db=$DB&key=kullanici:1&filter=ad" | jget 'sorted(f["field"] for f in d["content"]["fields"])')" "['ad', 'soyad']"
check "sıralı küme" "$(api GET "/redis/key?db=$DB&key=puanlar" | jget '[(p["member"], p["score"]) for p in d["content"]["pairs"]]')" "[('ali', '10'), ('veli', '20')]"
check "akış" "$(api GET "/redis/key?db=$DB&key=olaylar" | jget '(d["length"], d["content"]["entries"][0]["fields"][0])')" "(1, {'field': 'tur', 'value': 'giris'})"
check "kodlama ve boyut" "$(api GET "/redis/key?db=$DB&key=kullanici:1" | jget '(bool(d["encoding"]), d["size"] > 0)')" "(True, True)"

say "Düzenleme"
check "metin yaz" "$(code POST /redis/key -d "$(edit yeni:anahtar set_string '{"value":"{\"a\": 1, \"b\": \"ç ğ\"}"}')")" 200
check "yazılan değer" "$(r get yeni:anahtar)" '{"a": 1, "b": "ç ğ"}'
check "süre ver" "$(code POST /redis/key -d "$(edit yeni:anahtar set_ttl '{"seconds":300}')")" 200
check "süre uygulandı" "$([ "$(r ttl yeni:anahtar)" -gt 200 ] && echo evet)" evet
check "değer yazmak süreyi korur" "$(api POST /redis/key -d "$(edit yeni:anahtar set_string '{"value":"x"}')" >/dev/null; [ "$(r ttl yeni:anahtar)" -gt 200 ] && echo evet)" evet
check "süreyi kaldır" "$(api POST /redis/key -d "$(edit yeni:anahtar set_ttl '{"seconds":null}')" >/dev/null; r ttl yeni:anahtar)" -1
check "yeniden adlandır" "$(code POST /redis/key -d "$(edit yeni:anahtar rename '{"new_key":"yeni:ad"}')")" 200
check "var olan ada yeniden adlandırma" "$(code POST /redis/key -d "$(edit yeni:ad rename '{"new_key":"etiketler"}')")" 409
check "hash alanı" "$(api POST /redis/key -d "$(edit kullanici:1 hash_set '{"field":"sehir","value":"İzmir"}')" >/dev/null; r hget kullanici:1 sehir)" "İzmir"
check "hash alanı sil" "$(api POST /redis/key -d "$(edit kullanici:1 hash_delete '{"field":"yas"}')" >/dev/null; r hexists kullanici:1 yas)" 0
check "liste öğesi değiştir" "$(api POST /redis/key -d "$(edit kuyruk:isler list_set '{"index":0,"value":"ilk"}')" >/dev/null; r lindex kuyruk:isler 0)" ilk
check "listeye ekle ve sil" "$(api POST /redis/key -d "$(edit kuyruk:isler list_push '{"value":"son"}')" >/dev/null; api POST /redis/key -d "$(edit kuyruk:isler list_remove '{"value":"is2"}')" >/dev/null; r llen kuyruk:isler)" 450
check "kümeye ekle/çıkar" "$(api POST /redis/key -d "$(edit etiketler set_add '{"member":"d"}')" >/dev/null; api POST /redis/key -d "$(edit etiketler set_remove '{"member":"a"}')" >/dev/null; r smembers etiketler | sort | tr '\n' ' ')" "b c d "
check "sıralı kümede puan" "$(api POST /redis/key -d "$(edit puanlar zset_add '{"member":"ali","score":99.5}')" >/dev/null; r zscore puanlar ali)" 99.5
check "tür uyuşmazlığı Redis hatası olarak döner" "$(code POST /redis/key -d "$(edit etiketler hash_set '{"field":"a","value":"b"}')")" 400
check "bilinmeyen işlem" "$(code POST /redis/key -d "$(edit etiketler bilinmeyen '{}')")" 422
check "satır sonu içeren değer komut olmaz" "$(api POST /redis/key -d "$(edit enj set_string '{"value":"a\r\nFLUSHDB\r\n"}')" >/dev/null; r dbsize)" "$(r dbsize)"

say "Konsol"
check "basit komut" "$(cmd 'GET yeni:ad' | jget '(d["reply"]["kind"], d["reply"]["value"])')" "('string', 'x')"
check "tırnaklı argüman" "$(cmd 'SET "boşluklu anahtar" "iki kelime"' | jget 'd["reply"]["value"]'; r get 'boşluklu anahtar')" "OK
iki kelime"
check "dizi yanıtı" "$(cmd 'HGETALL kullanici:1' | jget '(d["reply"]["kind"], d["reply"]["total"])')" "('array', 6)"
check "Redis hatası gösterilir" "$(cmd 'INCR kullanici:1' | jget 'd["reply"]["kind"]')" error
for blocked in 'FLUSHALL' 'config set dir /tmp' 'SHUTDOWN NOSAVE' 'EVAL "return 1" 0' 'DEBUG SLEEP 5' 'MONITOR' 'ACL SETUSER x' 'SELECT 0' 'KEYS *' 'REPLICAOF 1.2.3.4 6379' 'MODULE LOAD /tmp/x.so'; do
  check "engellenen komut: ${blocked%% *}" "$(api POST /redis/command -o /dev/null -w '%{http_code}' -d "$(python3 -c 'import json,sys; print(json.dumps({"db":5,"command":sys.argv[1]}))' "$blocked")")" 400
done
check "kapanmamış tırnak" "$(api POST /redis/command -o /dev/null -w '%{http_code}' -d '{"db":5,"command":"SET a \"b"}')" 400
check "veriler yerinde" "$([ "$(r dbsize)" -gt 400 ] && echo evet)" evet
check "oturumsuz erişim" "$(curl -sk -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' $B/redis/command -d '{"command":"PING"}')" 401
r flushdb >/dev/null
echo "bitti"
