#!/usr/bin/env bash
# Hesapların SFTP ve kabuk (SSH) erişiminin sınaması. Test makinesinde çalışır;
# gerçek sftp/ssh bağlantılarıyla dener.
# Kullanım: PX_USER=... PX_PASS=... bash access.sh
set -uo pipefail
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-acc
JC=/tmp/pxjar-acc-customer
JO=/tmp/pxjar-acc-other
CUSTOMER=sshmusteri
OTHER=sshkomsu
PASS='Erisim-Parola-13579'
KEY=/tmp/px-access-key
SSH_OPTS="-i $KEY -o IdentitiesOnly=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o BatchMode=yes -o ConnectTimeout=8 -o LogLevel=ERROR"
call() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
status() { local jar=$1 m=$2 p=$3; shift 3; curl -sk -o /dev/null -w '%{http_code}' -b $jar -c $jar -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X "$m" "$B$p" "$@"; }
api() { call $J "$@"; }
code() { status $J "$@"; }
say() { printf '\n== %s\n' "$*"; }
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
keybody() { python3 -c 'import json,sys; print(json.dumps({"name": sys.argv[1], "public_key": sys.argv[2]}))' "$1" "$2"; }
# sftp_run <komutlar>: müşteri olarak SFTP oturumu açıp komutları çalıştırır.
sftp_run() { printf '%s\n' "$1" | sftp $SSH_OPTS -P "$PORT" $CUSTOMER@127.0.0.1 2>&1; }
ssh_run() { ssh $SSH_OPTS -p "$PORT" $CUSTOMER@127.0.0.1 "$@" 2>&1; }

cleanup() {
  for name in $CUSTOMER $OTHER; do
    local id; id=$(api GET /users | jget "next((u['id'] for u in d if u['username']=='$name'), '')")
    [ -z "$id" ] && continue
    api PUT /users/$id/access -d '{"mode":"none"}' >/dev/null
    for site in $(api GET /sites | jget "' '.join(str(s['id']) for s in d if s['owner']=='$name')"); do api DELETE "/sites/$site?delete_files=true" >/dev/null; done
    api DELETE /users/$id >/dev/null
  done
  rm -f $KEY $KEY.pub
}

rm -f $J $JC $JO
login $J "$PX_USER" "$PX_PASS" >/dev/null
cleanup

say "Hazırlık"
check "müşteri ve komşusu" "$(code POST /users -d "{\"username\":\"$CUSTOMER\",\"password\":\"$PASS\",\"role\":\"user\"}") $(code POST /users -d "{\"username\":\"$OTHER\",\"password\":\"$PASS\",\"role\":\"user\"}")" "200 200"
check "girişler" "$(login $JC $CUSTOMER "$PASS") $(login $JO $OTHER "$PASS")" "200 200"
C=$(api GET /users | jget "next(u['id'] for u in d if u['username']=='$CUSTOMER')")
call $JC POST /sites -d '{"domain":"ssh-musteri.test","kind":"static"}' >/dev/null
ssh-keygen -q -t ed25519 -N '' -C 'sinama@makine' -f $KEY
PUB=$(cat $KEY.pub)
R=$(call $JC GET /users/$C/access)
PORT=$(jget 'd["port"]' <<<"$R")
check "başlangıçta erişim kapalı" "$(jget '(d["available"], d["mode"], d["keys"], d["can_change_mode"])' <<<"$R")" "(True, 'none', [], False)"

say "Yetki"
check "müşteri kendi erişimini açamaz" "$(status $JC PUT /users/$C/access -d '{"mode":"sftp"}')" 403
check "komşu başkasının erişimini göremez" "$(status $JO GET /users/$C/access)" 404
check "komşu başkasına anahtar ekleyemez" "$(status $JO POST /users/$C/access/keys -d "$(keybody x "$PUB")")" 404
check "geçersiz düzey" "$(code PUT /users/$C/access -d '{"mode":"root"}')" 400
check "erişim kapalıyken parola verilemez" "$(status $JC PUT /users/$C/access/password -d '{"password":"Uzun-Sftp-Parola-123"}')" 400

say "Açık anahtarlar"
check "seçenek eklenmiş anahtar" "$(status $JC POST /users/$C/access/keys -d "$(keybody x "command=\"id\" $PUB")")" 400
check "özel anahtar" "$(status $JC POST /users/$C/access/keys -d "$(keybody x "$(head -2 $KEY | tr '\n' ' ')")")" 400
check "bozuk gövde" "$(status $JC POST /users/$C/access/keys -d "$(keybody x 'ssh-ed25519 bozuk!!')")" 400
R=$(call $JC POST /users/$C/access/keys -d "$(keybody 'dizüstü' "$PUB")")
check "anahtar eklendi, parmak izi ssh-keygen ile aynı" "$(jget 'd["keys"][0]["fingerprint"]' <<<"$R")" "$(ssh-keygen -lf $KEY.pub | awk '{print $2}')"
check "aynı anahtar ikinci kez" "$(status $JC POST /users/$C/access/keys -d "$(keybody y "$PUB")")" 409
check "erişim kapalıyken anahtar işe yaramaz" "$(ssh_run true >/dev/null; echo $?)" 255

say "SFTP"
R=$(api PUT /users/$C/access -d '{"mode":"sftp"}')
check "yönetici SFTP'yi açar" "$(jget '(d["mode"], d["username"])' <<<"$R")" "('sftp', '$CUSTOMER')"
check "sshd yapılandırması geçerli" "$(sshd -t 2>&1 | wc -l)" 0
check "kabuk verilmedi" "$(getent passwd $CUSTOMER | cut -d: -f7)" /usr/sbin/nologin
check "anahtar dosyası root'a ait, kullanıcı değiştiremez" "$(stat -c '%U %a' /etc/pixpanel/ssh/$CUSTOMER.keys)" "root 644"
OUT=$(sftp_run 'ls /')
has "SFTP ile girer ve sitelerini görür" "$OUT" "sites"
check "ev klasörüne hapsedilmiştir" "$(sftp_run 'ls /etc/passwd' | grep -c 'not found\|No such file')" 1
check "başka hesabın klasörüne çıkamaz" "$(sftp_run "ls /../$PX_USER" | grep -c 'deneme.test')" 0
echo "sftp ile geldi" > /tmp/px-access-upload.txt
sftp_run "put /tmp/px-access-upload.txt /sites/ssh-musteri.test/public/yukleme.txt" >/dev/null
check "dosya yükler; dosya kendisine aittir" "$(cat /home/$CUSTOMER/sites/ssh-musteri.test/public/yukleme.txt) $(stat -c %U /home/$CUSTOMER/sites/ssh-musteri.test/public/yukleme.txt)" "sftp ile geldi $CUSTOMER"
has "komut çalıştıramaz" "$(ssh_run id)" "sftp"
check "port yönlendiremez" "$(ssh $SSH_OPTS -p "$PORT" -N -L 127.0.0.1:18443:127.0.0.1:8443 $CUSTOMER@127.0.0.1 >/dev/null 2>&1 & sleep 2; curl -sk -m 3 -o /dev/null -w '%{http_code}' https://127.0.0.1:18443/; kill %1 2>/dev/null)" 000
check "kısa parola" "$(status $JC PUT /users/$C/access/password -d '{"password":"kisa"}')" 400
check "satır sonlu parola" "$(status $JC PUT /users/$C/access/password -d '{"password":"Uzun-Parola-123\nroot:x"}')" 400
check "parola belirler" "$(call $JC PUT /users/$C/access/password -d '{"password":"Uzun-Sftp-Parola-123"}' | jget 'd["password_set"]')" True
check "parola Linux hesabına işlendi" "$(passwd -S $CUSTOMER | awk '{print $2}')" P

say "Kabuk"
check "komut satırından başka hesap kabuk veremez (yalnız yönetici)" "$(status $JC PUT /users/$C/access -d '{"mode":"shell"}')" 403
R=$(api PUT /users/$C/access -d '{"mode":"shell"}')
check "yönetici kabuk verir; parola geçersiz sayılır" "$(jget '(d["mode"], d["password_set"])' <<<"$R")" "('shell', False)"
check "anahtarla girer ve kendi kimliğiyle komut çalıştırır" "$(ssh_run id -un)" "$CUSTOMER"
check "kabukta parola verilemez" "$(status $JC PUT /users/$C/access/password -d '{"password":"Uzun-Sftp-Parola-123"}')" 400
check "başka hesabın dosyalarını okuyamaz" "$(ssh_run cat /home/$PX_USER/sites/deneme.test/public/index.php 2>&1 | grep -c 'Permission denied\|No such file')" 1

say "Kapatma"
K=$(call $JC GET /users/$C/access | jget 'd["keys"][0]["id"]')
check "komşu anahtarı silemez" "$(status $JO DELETE /users/$C/access/keys/$K)" 404
check "anahtarı siler" "$(call $JC DELETE /users/$C/access/keys/$K | jget 'len(d["keys"])')" 0
check "silinen anahtarla girilemez" "$(ssh_run true >/dev/null; echo $?)" 255
call $JC POST /users/$C/access/keys -d "$(keybody 'dizüstü' "$PUB")" >/dev/null
check "erişimi kapat" "$(api PUT /users/$C/access -d '{"mode":"none"}' | jget 'd["mode"]')" none
check "kapalıyken anahtar ekli olsa da girilemez" "$(ssh_run true >/dev/null; echo $?) $(getent passwd $CUSTOMER | cut -d: -f7)" "255 /usr/sbin/nologin"
check "parola kilitlendi" "$(passwd -S $CUSTOMER | awk '{print $2}')" L

say "Temizlik"
cleanup
check "hesap silinince anahtar dosyası kalmaz" "$([ -e /etc/pixpanel/ssh/$CUSTOMER.keys ] || echo yok)" yok
rm -f /tmp/px-access-upload.txt
echo "bitti"
