#!/usr/bin/env bash
# Site başına FTP hesaplarının sınaması. Test makinesinde çalışır.
# Kullanım: PX_USER=... PX_PASS=... bash ftp.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-ftp
FTP_PASS='Ftp-Parola-24680'
api() { local m=$1 p=$2; shift 2; curl -sk -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
code() { local m=$1 p=$2; shift 2; curl -sk -o /dev/null -w '%{http_code}' -b $J -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | head -c 240)]"; fi; }
jget() { python3 -c "
import sys,json
raw=sys.stdin.read()
try: d=json.loads(raw)
except Exception: print('(JSON degil: ' + raw[:200] + ')'); sys.exit()
try: print($1)
except Exception as e: print('(alan yok: %s %s)' % (e, raw[:200]))"; }
wait_job() { for _ in $(seq 1 240); do s=$(api GET "/jobs/$1" | jget 'd["job"]["status"]'); [ "$s" != running ] && { echo "$s"; return; }; sleep 1; done; echo timeout; }
# ftp <kullanıcı> <parola> <python ifadesi>  (f: oturum açmış FTP_TLS nesnesi)
ftp() { python3 - "$1" "$2" "$3" <<'PY'
import ftplib, io, ssl, sys
user, password, expression = sys.argv[1:4]
try:
    f = ftplib.FTP_TLS(context=ssl._create_unverified_context())
    f.connect("127.0.0.1", 21, timeout=15)
    f.login(user, password)
    f.prot_p()
    print(eval(expression))
    f.quit()
except Exception as error:
    print("hata:", str(error)[:120])
PY
}

rm -f $J
api POST /auth/login -d "{\"username\":\"$PX_USER\",\"password\":\"$PX_PASS\"}" >/dev/null

say "Kurulum"
if [ "$(api GET /components | jget 'next(c["installed"] for c in d if c["id"]=="ftp")')" != True ]; then
  check "FTP sunucusu kurulumu" "$(wait_job "$(api POST /components/ftp/install | jget 'd["job_id"]')")" success
fi
S=$(api GET /sites | jget 'next(s["id"] for s in d if s["domain"]=="statik.test")')
api GET /sites/$S/ftp | jget '(d["installed"], d["running"], d["passive_ports"])'
for old in $(api GET /sites/$S/ftp | jget '" ".join(str(a["id"]) for a in d["accounts"])'); do api DELETE /sites/$S/ftp/$old >/dev/null; done

say "Hesaplar"
check "geçersiz ad" "$(code POST /sites/$S/ftp -d "{\"name\":\"../x\",\"password\":\"$FTP_PASS\"}")" 400
check "seçenek gibi ad" "$(code POST /sites/$S/ftp -d "{\"name\":\"-u0\",\"password\":\"$FTP_PASS\"}")" 400
check "kısa parola" "$(code POST /sites/$S/ftp -d '{"name":"statikftp","password":"kisa"}')" 400
check "satır sonlu parola" "$(code POST /sites/$S/ftp -d '{"name":"statikftp","password":"Parola-123456\nikinci"}')" 400
check "kök dışı klasör" "$(code POST /sites/$S/ftp -d "{\"name\":\"statikftp\",\"password\":\"$FTP_PASS\",\"subdir\":\"../../..\"}")" 400
ln -sfn /etc /home/admin/sites/statik.test/kacis
check "kök dışını gösteren bağ" "$(code POST /sites/$S/ftp -d "{\"name\":\"statikftp\",\"password\":\"$FTP_PASS\",\"subdir\":\"kacis\"}")" 400
rm -f /home/admin/sites/statik.test/kacis
check "olmayan klasör" "$(code POST /sites/$S/ftp -d "{\"name\":\"statikftp\",\"password\":\"$FTP_PASS\",\"subdir\":\"yok\"}")" 400
check "hesap oluştur" "$(code POST /sites/$S/ftp -d "{\"name\":\"statikftp\",\"password\":\"$FTP_PASS\"}")" 200
check "aynı ad ikinci kez" "$(code POST /sites/$S/ftp -d "{\"name\":\"statikftp\",\"password\":\"$FTP_PASS\"}")" 409
check "alt klasöre hapsedilmiş hesap" "$(code POST /sites/$S/ftp -d "{\"name\":\"statikpublic\",\"password\":\"$FTP_PASS\",\"subdir\":\"public\"}")" 200

say "Bağlantı"
has "oturum ve listeleme" "$(ftp statikftp "$FTP_PASS" 'sorted(f.nlst())')" "'public'"
check "yanlış parola" "$(ftp statikftp yanlis-parola-1 'f.pwd()' | cut -c1-5)" "hata:"
check "sistem kullanıcısı giremez" "$(ftp admin "$PX_PASS" 'f.pwd()' | cut -c1-5)" "hata:"
check "hapis: kök görünümü" "$(ftp statikftp "$FTP_PASS" '(f.cwd("/"), f.cwd(".."), f.cwd(".."), f.pwd())[-1]')" "/"
has "hapis: /etc görünmez" "$(ftp statikftp "$FTP_PASS" 'f.cwd("/etc")')" "hata:"
ftp statikftp "$FTP_PASS" 'f.storbinary("STOR public/ftp-deneme.txt", io.BytesIO(b"ftp ile geldi\n"))' >/dev/null
check "yükleme" "$(cat /home/admin/sites/statik.test/public/ftp-deneme.txt)" "ftp ile geldi"
check "dosya sitenin kullanıcısına ait, grup okuyabilir" "$(stat -c '%U %G %a' /home/admin/sites/statik.test/public/ftp-deneme.txt)" "admin www-data 640"
check "web sunucusu yüklenen dosyayı sunuyor" "$(curl -s -H 'Host: statik.test' http://127.0.0.1/ftp-deneme.txt)" "ftp ile geldi"
has "alt klasör hesabı yalnızca kendi klasörünü görür" "$(ftp statikpublic "$FTP_PASS" 'sorted(f.nlst())')" "ftp-deneme.txt"
A=$(api GET /sites/$S/ftp | jget 'next(a["id"] for a in d["accounts"] if a["name"]=="statikftp")')
check "parola değiştir" "$(code PATCH /sites/$S/ftp/$A -d '{"password":"Yeni-Ftp-Parola-13579"}')" 200
check "eski parola geçersiz" "$(ftp statikftp "$FTP_PASS" 'f.pwd()' | cut -c1-5)" "hata:"
check "yeni parola geçerli" "$(ftp statikftp Yeni-Ftp-Parola-13579 'f.pwd()')" "/"
check "başka sitenin hesabına erişim" "$(code PATCH /sites/1/ftp/$A -d '{"password":"Yeni-Ftp-Parola-13579"}')" 404

say "Temizlik"
check "hesabı sil" "$(code DELETE /sites/$S/ftp/$A)" 200
check "silinen hesap giremez" "$(ftp statikftp Yeni-Ftp-Parola-13579 'f.pwd()' | cut -c1-5)" "hata:"
B2=$(api GET /sites/$S/ftp | jget 'next(a["id"] for a in d["accounts"] if a["name"]=="statikpublic")')
api DELETE /sites/$S/ftp/$B2 >/dev/null
rm -f /home/admin/sites/statik.test/public/ftp-deneme.txt
check "hesap kalmadı" "$(pure-pw list | wc -l)" 0
