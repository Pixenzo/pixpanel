#!/usr/bin/env bash
# Panelin kendini güncellemesinin sınaması. Geliştirme makinesinde çalışır: sınama
# sürümlerini hazırlar, test makinesine yükler ve oradaki denetimleri (update-remote.sh)
# koşturur. Bitince test makinesine olağan geliştirme derlemesini geri yükler.
# Kullanım: scripts/e2e/update.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
set -a; . "$ROOT/.env"; set +a
HOST=root@192.168.1.25
KEY="$PIXPANEL_RELEASE_KEY"
case "$KEY" in /*|[A-Za-z]:*) ;; *) KEY="$ROOT/$KEY" ;; esac
STAGE="$ROOT/dist/e2e-update"

sign() { (cd "$1" && sha256sum VERSION pixpanel-linux-x86_64.tar.gz | sed 's/ \*/  /' > SHA256SUMS) && openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$1/SHA256SUMS" -out "$1/SHA256SUMS.sig"; }

echo "== Sınanacak derleme yükleniyor"
bash "$ROOT/scripts/deploy-dev.sh" 2>&1 | tail -1 | grep -qx active || { echo "  HATA geliştirme derlemesi yüklenemedi"; exit 1; }

echo "== Sınama sürümleri hazırlanıyor"
bash "$ROOT/scripts/release.sh" 9.0.1 --dev >/dev/null 2>&1 || { echo "  HATA sürüm paketi hazırlanamadı"; exit 1; }
rm -rf "$STAGE"; mkdir -p "$STAGE"
cp -r "$ROOT/dist/release/9.0.1" "$STAGE/good"

# İmzası geçerli ama başlamayan sürüm: yalnızca sürüm numarasını söyleyen sahte ikili.
mkdir -p "$STAGE/broken" "$STAGE/tmp"
printf '#!/bin/sh\n[ "$1" = "--version" ] && { echo "pixpanel 9.0.2"; exit 0; }\nexit 1\n' > "$STAGE/tmp/pixpanel"
chmod 755 "$STAGE/tmp/pixpanel"
tar -C "$STAGE/tmp" -czf "$STAGE/broken/pixpanel-linux-x86_64.tar.gz" pixpanel
echo 9.0.2 > "$STAGE/broken/VERSION"
sign "$STAGE/broken"

# Çalışandan eski sürüm.
cp -r "$STAGE/broken" "$STAGE/old"
echo 0.0.1 > "$STAGE/old/VERSION"
sign "$STAGE/old"

# İmzalandıktan sonra özet dosyası değiştirilmiş sürüm.
cp -r "$STAGE/good" "$STAGE/badsig"
echo "# eklendi" >> "$STAGE/badsig/SHA256SUMS"

# İmzası ve özeti geçerli, ama arşivi sonradan değiştirilmiş sürüm.
cp -r "$STAGE/good" "$STAGE/badarchive"
cp "$STAGE/broken/pixpanel-linux-x86_64.tar.gz" "$STAGE/badarchive/pixpanel-linux-x86_64.tar.gz"
rm -rf "$STAGE/tmp"

ssh -o BatchMode=yes "$HOST" 'rm -rf /root/px-release && mkdir -p /root/px-release'
scp -q -r "$STAGE"/* "$HOST:/root/px-release/" || { echo "  HATA sürümler yüklenemedi"; exit 1; }
scp -q "$ROOT/scripts/e2e/update-remote.sh" "$HOST:/root/"

ssh -o BatchMode=yes "$HOST" "PX_USER='$PIXPANEL_TEST_USER' PX_PASS='$PIXPANEL_TEST_PASSWORD' bash /root/update-remote.sh"

echo
echo "== Olağan geliştirme derlemesi geri yükleniyor"
if bash "$ROOT/scripts/deploy-dev.sh" 2>&1 | tail -1 | grep -qx active; then
  VERSION=$(ssh -o BatchMode=yes "$HOST" '/usr/local/bin/pixpanel --version; rm -rf /root/px-release /usr/local/bin/pixpanel.previous')
  [ "$VERSION" != "pixpanel 9.0.1" ] && echo "  OK   test makinesi olağan sürüme döndü" || echo "  HATA test makinesi hâlâ sınama sürümünde: $VERSION"
else
  echo "  HATA geliştirme derlemesi geri yüklenemedi"
fi
rm -rf "$STAGE" "$ROOT/dist/release/9.0.1"
