#!/usr/bin/env bash
# Dosya yöneticisinde arşiv açma sınırlarının sınaması: küçük arşiv açılır, yüksek sıkıştırma
# oranlı (zip bombası) arşiv reddedilir ve geride dosya bırakmaz, arşivdeki "../" girdileri hedefin
# dışına yazamaz, başka hesabın dosya alanı kullanılamaz.
# Girdi sayısı ve toplam boyut sınırları üretimde sabittir (ortam değişkeniyle düşürülemez); onlar
# `fsop.rs` birim testlerinde sınanır. Burada yalnızca oran sınırı uçtan uca sınanır.
# Test makinesinde çalışır. Kullanım: PX_USER=... PX_PASS=... bash files-extract.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
DB=/var/lib/pixpanel/pixpanel.db
J=/tmp/pxjar-fx
JA=/tmp/pxjar-fx-a
JB=/tmp/pxjar-fx-b
A=arsivbir
O=arsivik
PASS='Arsiv-Sinama-Parolasi-1'
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { case "$2" in *"$3"*) echo "  OK   $1" ;; *) echo "  HATA $1: [$3] yok, gelen=[$(printf %s "$2" | head -c 300)]" ;; esac; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
user_id() { api GET /users | jget "next((u['id'] for u in d if u['username']=='$1'), '')"; }
login() { curl -sk -o /dev/null -c "$1" -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$2\",\"password\":\"$PASS\"}"; }
# fop <jar> <kapsam kullanıcı kimliği> <istek JSON>: dosya işlemini çalıştırır, yanıt gövdesini yazar.
fop() { call "$1" POST /files/op -d "{\"scope\":\"user:$2\",\"request\":$3}"; }
fcode() { status "$1" POST /files/op -d "{\"scope\":\"user:$2\",\"request\":$3}"; }
extract() { printf '{"op":"extract","path":"%s","dest":"%s"}' "$1" "$2"; }

rm -f $J $JA $JB
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null
for name in $A $O; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done

say "Hazırlık"
check "iki müşteri" "$(status $J POST /users -d "{\"username\":\"$A\",\"password\":\"$PASS\",\"role\":\"user\"}") $(status $J POST /users -d "{\"username\":\"$O\",\"password\":\"$PASS\",\"role\":\"user\"}")" "200 200"
login $JA $A; login $JB $O
UA=$(user_id $A)
# İlk işlem hesabın Linux kullanıcısını ve ev klasörünü oluşturur.
check "hesabın dosya alanı açılır" "$(fcode $JA $UA '{"op":"list","path":"/"}')" 200
# Ev klasörünün üst düzeyi hesaba kapalıdır; sınama hesabın yazabildiği tmp altındaki bir klasörde yürür.
W=/tmp/px-arsiv
HOME_A=/home/$A$W
mkdir -p $HOME_A && chown $A: /home/$A/tmp $HOME_A

# Arşivleri hesabın klasörüne, hesap adına yazar (python3 ile; zip aracı olmayabilir).
python3 - "$HOME_A" <<'PY'
import os, sys, tarfile, zipfile, io
home = sys.argv[1]

def put_zip(name, entries):
    with zipfile.ZipFile(os.path.join(home, name), "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for arc, data in entries:
            z.writestr(arc, data)

def put_tar(name, entries):
    with tarfile.open(os.path.join(home, name), "w:gz") as t:
        for arc, data in entries:
            info = tarfile.TarInfo(arc)
            info.size = len(data)
            t.addfile(info, io.BytesIO(data))

put_zip("kucuk.zip", [("a.txt", b"merhaba"), ("alt/b.txt", b"dunya")])
put_tar("kucuk.tar.gz", [("c.txt", b"merhaba"), ("alt/d.txt", b"dunya")])
put_zip("disari.zip", [("../disari-zip.txt", b"x"), ("/mutlak-zip.txt", b"x"), ("iyi.txt", b"iyi")])
put_tar("disari.tar.gz", [("../disari-tar.txt", b"x"), ("iyi-tar.txt", b"iyi")])

# Zip bombası: 400 MB sıfır, yaklaşık 1030:1 oranla sıkışır (sınır 1000:1). Parça parça yazılır.
with zipfile.ZipFile(os.path.join(home, "bomba.zip"), "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    with z.open("sifir.bin", "w", force_zip64=True) as f:
        chunk = bytes(1024 * 1024)
        for _ in range(400):
            f.write(chunk)
PY
chown -R $A:$A "$HOME_A" 2>/dev/null || chown -R "$(stat -c %u $HOME_A):$(stat -c %g $HOME_A)" "$HOME_A"
BOMB_SIZE=$(stat -c %s $HOME_A/bomba.zip)
check "bomba arşivi küçük, içeriği büyük" "$([ "$BOMB_SIZE" -lt 1000000 ] && echo evet)" evet

say "Normal arşivler açılır"
check "zip açılır" "$(fcode $JA $UA "$(extract $W/kucuk.zip $W/zip-hedef)")" 200
check "zip içeriği" "$(cat $HOME_A/zip-hedef/a.txt $HOME_A/zip-hedef/alt/b.txt | tr -d '\n')" "merhabadunya"
check "dosyalar hesabın kullanıcısına ait" "$(stat -c %U $HOME_A/zip-hedef/a.txt)" "$A"
check "tar.gz açılır" "$(fcode $JA $UA "$(extract $W/kucuk.tar.gz $W/tar-hedef)")" 200
check "tar.gz içeriği" "$(cat $HOME_A/tar-hedef/c.txt $HOME_A/tar-hedef/alt/d.txt | tr -d '\n')" "merhabadunya"
# Var olan klasöre açma: aynı adlı klasörler birleşir, var olan dosyalar yerinde kalır.
mkdir -p $HOME_A/birles/alt && echo kalir > $HOME_A/birles/alt/eski.txt && chown -R $A:$A $HOME_A/birles
check "var olan klasöre açma" "$(fcode $JA $UA "$(extract $W/kucuk.zip $W/birles)")" 200
check "klasörler birleşir, eski dosya kalır" "$(ls $HOME_A/birles/alt | tr '\n' ' ')" "b.txt eski.txt "
check "geçici açma klasörü kalmaz" "$(find $HOME_A -name '.pixpanel-acma-*' | wc -l)" 0
check "istekteki kota alanı yok sayılır" "$(fcode $JA $UA '{"op":"extract","path":"'"$W"'/kucuk.zip","dest":"'"$W"'/kota-hedef","disk_budget":1}')" 200
check "kota alanına rağmen açıldı" "$(ls $HOME_A/kota-hedef | tr '\n' ' ')" "a.txt alt "
check "açma denetim günlüğünde" "$(sqlite3 $DB "SELECT count(*) > 0 FROM audit_log WHERE action='file.extract' AND target LIKE '%px-arsiv/kucuk.zip%'")" 1

say "Zip bombası reddedilir"
R=$(fop $JA $UA "$(extract $W/bomba.zip $W/bomba-hedef)")
has "ileti nedeni söyler" "$R" "sıkıştırma oranı"
has "ileti zip bombasını anar" "$R" "zip bombası"
check "geride klasör kalmaz" "$([ -e $HOME_A/bomba-hedef ] && echo var || echo yok)" yok
check "var olan boş klasöre de dosya bırakmaz" "$(mkdir -p $HOME_A/bos-hedef && chown $A:$A $HOME_A/bos-hedef && fcode $JA $UA "$(extract $W/bomba.zip $W/bos-hedef)") $(ls -A $HOME_A/bos-hedef | wc -l)" "400 0"
check "geçici açma klasörü kalmaz" "$(find $HOME_A -name '.pixpanel-acma-*' | wc -l)" 0
check "diskte yer tutulmadı" "$(du -sm $HOME_A | cut -f1 | awk '{print ($1 < 50) ? "evet" : "hayır"}')" evet

say "Desteklenmeyen tür ve yol kaçışı"
R=$(fop $JA $UA "$(extract $W/kucuk.zip.rar $W/rar-hedef)")
has "desteklenmeyen tür" "$R" "Desteklenen arşiv türleri"
check "desteklenmeyen türde klasör açılmaz" "$([ -e $HOME_A/rar-hedef ] && echo var || echo yok)" yok
check "zip: ../ ve mutlak yollu girdiler atlanır, iyisi açılır" "$(fcode $JA $UA "$(extract $W/disari.zip $W/a/b)")" 200
check "zip: iyi dosya hedefte" "$(ls $HOME_A/a/b | tr '\n' ' ')" "iyi.txt "
check "zip: hedefin dışına yazılmadı" "$(ls $HOME_A/a/disari-zip.txt $HOME_A/disari-zip.txt $HOME_A/mutlak-zip.txt /tmp/disari-zip.txt 2>/dev/null | wc -l)" 0
check "tar.gz: ../ girdisi hedefin dışına yazamaz" "$(fcode $JA $UA "$(extract $W/disari.tar.gz $W/c/d)")" 200
check "tar.gz: iyi dosya hedefte" "$(ls $HOME_A/c/d | tr '\n' ' ')" "iyi-tar.txt "
check "tar.gz: hedefin dışına yazılmadı" "$(ls $HOME_A/c/disari-tar.txt $HOME_A/disari-tar.txt 2>/dev/null | wc -l)" 0
check "hesap klasörünün dışına çıkan hedef" "$(fcode $JA $UA "$(extract $W/kucuk.zip $W/../../tmp/kacis)")" 400
check "hedefte kaçış yok" "$([ -e /tmp/kacis ] && echo var || echo yok)" yok

say "Yetki"
check "oturumsuz istek" "$(curl -sk -o /dev/null -w '%{http_code}' -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/files/op -d "{\"scope\":\"user:$UA\",\"request\":$(extract $W/kucuk.zip $W/oturumsuz)}")" 401
check "başka hesabın dosya alanında arşiv açılamaz" "$(fcode $JB $UA "$(extract $W/kucuk.zip $W/baska)")" 404
check "başka hesabın alanına dosya düşmedi" "$([ -e $HOME_A/baska ] && echo var || echo yok)" yok

say "Temizlik"
for name in $A $O; do id=$(user_id $name); [ -n "$id" ] && api DELETE "/users/$id" >/dev/null; done
rm -rf "$HOME_A" /tmp/kacis
rm -f $J $JA $JB
echo "bitti"
