#!/usr/bin/env bash
# Kurulum betiğinin temiz makinede koşan sınaması; install.sh (e2e) tarafından çağrılır.
# /root/rel altında sürüm dosyalarını bekler. Makinede panel KURULU OLMAMALIDIR.
set -uo pipefail
REL=/root/rel
B=https://127.0.0.1:8443/api/v1
J=/tmp/pxjar-install
say() { printf '\n== %s\n' "$*"; }
check() { if [ "$2" = "$3" ]; then echo "  OK   $1"; else echo "  HATA $1: beklenen=[$3] gelen=[$2]"; fi; }
has() { if grep -qF -- "$3" <<<"$2"; then echo "  OK   $1"; else echo "  HATA $1: [$3] bulunamadı: [$(tr -d '\n' <<<"$2" | tail -c 300)]"; fi; }
login() { curl -sk -o /dev/null -w '%{http_code}' -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"$1\",\"password\":\"$2\"}"; }
. /etc/os-release
RELEASE=$(cat $REL/VERSION)
echo "Makine: $PRETTY_NAME"

[ -e /usr/local/bin/pixpanel ] && { echo "  HATA bu makinede panel zaten kurulu; sınama temiz makine ister"; exit 1; }
# Sınamanın kendi gereksinimi (JSON ve SQLite okuma); panelin değil.
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3 >/dev/null 2>&1

say "Reddedilmesi gerekenler"
cp -r $REL /root/rel-bozuk
echo x >> /root/rel-bozuk/pixpanel-linux-x86_64.tar.gz
OUT=$(bash $REL/install.sh --from-dir /root/rel-bozuk 2>&1); RC=$?
check "değiştirilmiş arşivle kurulum durur" "$RC" 1
has "nedeni söylenir (varsayılan dil İngilizce)" "$OUT" "checksum mismatch"
OUT=$(bash $REL/install.sh --lang tr --from-dir /root/rel-bozuk 2>&1)
has "--lang tr ile Türkçe" "$OUT" "özet uyuşmuyor"
cp $REL/pixpanel-linux-x86_64.tar.gz /root/rel-bozuk/
echo "# eklendi" >> /root/rel-bozuk/SHA256SUMS
OUT=$(bash $REL/install.sh --from-dir /root/rel-bozuk 2>&1); RC=$?
check "imzası bozuk sürümle kurulum durur" "$RC" 1
has "nedeni söylenir" "$OUT" "signature could not be verified"
check "hiçbir şey kurulmadı" "$([ -e /usr/local/bin/pixpanel ] || [ -e /etc/systemd/system/pixpanel.service ] || echo yok)" yok
OUT=$(bash $REL/install.sh --from-dir $REL --port 99999 2>&1); check "geçersiz port" "$?" 1
OUT=$(bash $REL/install.sh --from-dir $REL --admin 'Root; id' 2>&1); check "geçersiz yönetici adı" "$?" 1
OUT=$(bash $REL/install.sh --from-dir $REL --lang de 2>&1); check "geçersiz dil" "$?" 1
has "geçersiz dil iletisi" "$OUT" "invalid language"
OUT=$(bash $REL/install.sh --lang tr --bilinmeyen 2>&1); has "bilinmeyen seçenek Türkçe" "$OUT" "bilinmeyen seçenek"
rm -rf /root/rel-bozuk

say "İlk kurulum"
OUT=$(bash $REL/install.sh --lang tr --from-dir $REL --admin yonetici 2>&1); RC=$?
check "kurulum" "$RC" 0
[ "$RC" = 0 ] || echo "$OUT" | tail -15
has "sürüm bildirildi" "$OUT" "PixPanel $RELEASE hazır"
PASSWORD=$(sed -n 's/.*Parola *: *//p' <<<"$OUT" | head -1)
has "çıktı Türkçe" "$OUT" "Bu parola bir daha gösterilmez"
check "seçilen dil kaydedildi" "$(cat /etc/pixpanel/language)" tr
check "parola bir kez gösterildi" "$([ ${#PASSWORD} -ge 16 ] && echo evet)" evet
check "servis çalışıyor ve açılışta başlar" "$(systemctl is-active pixpanel) $(systemctl is-enabled pixpanel)" "active enabled"
check "kurulu sürüm" "$(/usr/local/bin/pixpanel --version)" "pixpanel $RELEASE"
check "arayüz yayında" "$(curl -sk -o /dev/null -w '%{http_code}' https://127.0.0.1:8443/)" 200
check "verilen hesapla giriş" "$(login yonetici "$PASSWORD")" 200
check "yanlış parolayla giriş olmaz" "$(login yonetici yanlis-parola-123)" 400
check "yapılandırma ve veri izinleri" "$(stat -c '%a' /etc/pixpanel/config.toml /var/lib/pixpanel | tr '\n' ' ')" "600 711 "
check "veri klasörü listelenemez" "$(runuser -u nobody -- ls /var/lib/pixpanel 2>&1 | grep -c "Permission denied")" 1
check "veritabanını başkası okuyamaz" "$(runuser -u nobody -- cat /var/lib/pixpanel/pixpanel.db 2>&1 | grep -c "Permission denied")" 1
check "komut satırı kurulum dilinde (Türkçe)" "$(/usr/local/bin/pixpanel site list | head -1 | awk '{print $1, $2}')" "ALAN ADI"
check "komut satırı yardımı Türkçe" "$(/usr/local/bin/pixpanel --help | head -1)" "Sunucu ve hosting yönetim paneli"
check "veritabanı yalnız root'a açık" "$(stat -c '%a %U' /var/lib/pixpanel/pixpanel.db)" "600 root"
login yonetici "$PASSWORD" >/dev/null
check "yöneticinin panel dili seçilen dil" "$(curl -sk -b $J $B/auth/me | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("language") or d["user"]["language"])')" tr

say "Yeniden çalıştırma (yükseltme)"
OUT=$(bash $REL/install.sh --from-dir $REL --port 9999 2>&1); RC=$?
check "ikinci çalıştırma" "$RC" 0
has "yükseltme kaydedilen dilde (Türkçe) yazar" "$OUT" "Var olan kurulum yükseltildi"
check "yeni parola üretilmedi" "$(grep -c -E 'Parola|Password' <<<"$OUT")" 0
OUT=$(bash $REL/install.sh --lang en --from-dir $REL 2>&1)
has "dil sonradan değiştirilebilir" "$OUT" "The existing installation was upgraded"
check "yeni dil kaydedildi" "$(cat /etc/pixpanel/language)" en
check "komut satırı yeni dilde (İngilizce)" "$(/usr/local/bin/pixpanel site list | head -1 | awk '{print $1}')" DOMAIN
check "setlang dili değiştirir" "$(/usr/local/bin/pixpanel setlang tr; cat /etc/pixpanel/language)" "Komut satırının dili Türkçe olarak ayarlandı.
tr"
check "geçersiz dil reddedilir" "$(/usr/local/bin/pixpanel setlang de >/dev/null 2>&1; echo $?) $(cat /etc/pixpanel/language)" "1 tr"
check "hata iletisi seçili dilde" "$(/usr/local/bin/pixpanel reset-password yok-boyle-biri 2>&1)" "Hata: kullanıcı bulunamadı: yok-boyle-biri"
/usr/local/bin/pixpanel setlang en >/dev/null
check "yöneticinin panel diline yükseltmede dokunulmaz" "$(login yonetici "$PASSWORD" >/dev/null; curl -sk -b $J $B/auth/me | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("language") or d["user"]["language"])')" tr
check "port değişmedi" "$(grep -c 'listen_port = 8443' /etc/pixpanel/config.toml)" 1
check "eski hesapla giriş sürüyor" "$(login yonetici "$PASSWORD")" 200

say "Kaldırma"
OUT=$(/usr/local/sbin/pixpanel-uninstall 2>&1)
has "kaldırma kaydedilen dilde (İngilizce) yazar" "$OUT" "PixPanel was removed"
check "servis ve ikili kalktı" "$([ -e /usr/local/bin/pixpanel ] || [ -e /etc/systemd/system/pixpanel.service ] || echo yok)" yok
check "veriler bırakıldı" "$([ -f /var/lib/pixpanel/pixpanel.db ] && echo var)" var
OUT=$(bash $REL/install.sh --from-dir $REL 2>&1) || echo "$OUT" | tail -8
check "yeniden kurulunca eski hesap geçerli" "$(login yonetici "$PASSWORD")" 200
/usr/local/sbin/pixpanel-uninstall --purge >/dev/null 2>&1
check "tümüyle silme" "$([ -e /var/lib/pixpanel ] || [ -e /etc/pixpanel ] || echo yok)" yok

say "Dil seçmeden kurulum"
OUT=$(bash $REL/install.sh --from-dir $REL 2>&1); RC=$?
check "kurulum" "$RC" 0
has "çıktı İngilizce" "$OUT" "PixPanel $RELEASE is ready"
has "parola İngilizce başlıkla" "$OUT" "Password:"
PASSWORD=$(sed -n 's/.*Password *: *//p' <<<"$OUT" | head -1)
check "varsayılan dil kaydedildi" "$(cat /etc/pixpanel/language)" en
check "yöneticinin panel dili boş (ilk girişte tarayıcıdan)" "[$(python3 -c "import sqlite3; print(sqlite3.connect('/var/lib/pixpanel/pixpanel.db').execute(\"select language from users where username='admin'\").fetchone()[0])")]" "[]"
curl -sk -o /dev/null -c $J -H 'x-pixpanel-csrf: 1' -H 'content-type: application/json' -X POST $B/auth/login -d "{\"username\":\"admin\",\"password\":\"$PASSWORD\",\"language\":\"tr\"}"
check "ilk girişte tarayıcı dili kaydedilir" "$(curl -sk -b $J $B/auth/me | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("language") or d["user"]["language"])')" tr
/usr/local/sbin/pixpanel-uninstall --purge >/dev/null 2>&1
check "yeniden tümüyle silme" "$([ -e /var/lib/pixpanel ] || [ -e /etc/pixpanel ] || echo yok)" yok
echo "bitti"
