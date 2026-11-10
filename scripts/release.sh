#!/usr/bin/env bash
# Sürüm paketini hazırlar: ikilileri derler, arşivler, özetlerini çıkarır ve imzalar.
# Çıktı: dist/release/<sürüm>/
#
# Kullanım: scripts/release.sh <sürüm> [--dev] [--publish]
#   --dev      hızlı (en iyilenmemiş) derleme; yalnızca sınamalar için
#   --publish  hazırlanan dosyaları GitHub'da v<sürüm> etiketiyle yayınlar
#
# İmzalama anahtarı depoda durmaz; yolu .env içindeki PIXPANEL_RELEASE_KEY ile verilir.
set -euo pipefail

VERSION="${1:?kullanım: scripts/release.sh <sürüm> [--dev] [--publish]}"
shift
PROFILE=release
PUBLISH=0
for arg in "$@"; do
  case "$arg" in
    --dev) PROFILE=debug ;;
    --publish) PUBLISH=1 ;;
    *) echo "bilinmeyen seçenek: $arg" >&2; exit 2 ;;
  esac
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "sürüm 1.2.3 biçiminde olmalı" >&2; exit 2; }

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
set -a; . "$ROOT_DIR/.env"; set +a
KEY="${PIXPANEL_RELEASE_KEY:?PIXPANEL_RELEASE_KEY .env içinde tanımlı değil}"
case "$KEY" in /*|[A-Za-z]:*) ;; *) KEY="$ROOT_DIR/$KEY" ;; esac
test -f "$KEY" || { echo "imzalama anahtarı bulunamadı: $KEY" >&2; exit 1; }

# cargo-zigbuild ve zig, pip ile kullanıcı dizinine kuruludur.
PY_USER_DIR="$APPDATA/Python/Python314"
export PATH="$PY_USER_DIR/Scripts:$PATH"
export CARGO_ZIGBUILD_ZIG_PATH="$(cygpath -w "$PY_USER_DIR/site-packages/ziglang/zig.exe")"

OUT="$ROOT_DIR/dist/release/$VERSION"
rm -rf "$OUT"
mkdir -p "$OUT"

# Arayüz ikiliye gömülür; önce o derlenir.
(cd "$ROOT_DIR/web" && pnpm build >/dev/null)

INSTALLED_TARGETS="$(rustup target list --installed)"
BUILT=0
for arch in x86_64 aarch64; do
  triple="$arch-unknown-linux-musl"
  if ! grep -qx "$triple" <<<"$INSTALLED_TARGETS"; then
    echo "atlandı: $triple hedefi kurulu değil (rustup target add $triple)"
    continue
  fi
  echo "derleniyor: $triple ($PROFILE)"
  flags=(--target "$triple")
  [ "$PROFILE" = release ] && flags+=(--release)
  (cd "$ROOT_DIR/server" && PIXPANEL_VERSION="$VERSION" cargo zigbuild "${flags[@]}" 2>&1 | grep -E "^error|Finished" || true)
  binary="$ROOT_DIR/server/target/$triple/$PROFILE/pixpanel"
  test -f "$binary"

  stage="$(mktemp -d)"
  cp "$binary" "$stage/pixpanel"
  cp "$ROOT_DIR/install/pixpanel.service" "$ROOT_DIR/install/uninstall.sh" "$stage/"
  # Windows dosya sisteminde çalıştırma izni tutulmaz; izinler arşive açıkça yazılır
  # (kurulum betiği her dosyayı kendi izniyle yerine koyar).
  tar -C "$stage" --owner=0 --group=0 --mode=755 -czf "$OUT/pixpanel-linux-$arch.tar.gz" pixpanel pixpanel.service uninstall.sh
  rm -rf "$stage"
  BUILT=$((BUILT + 1))
done
[ "$BUILT" -gt 0 ] || { echo "hiçbir hedef derlenemedi" >&2; exit 1; }

printf '%s\n' "$VERSION" > "$OUT/VERSION"
cp "$ROOT_DIR/install/install.sh" "$OUT/install.sh"
# Windows'taki sha256sum dosya adının önüne `*` koyar; özet dosyası her yerde aynı biçimde olsun.
(cd "$OUT" && sha256sum VERSION install.sh pixpanel-linux-*.tar.gz | sed 's/ \*/  /' > SHA256SUMS)
openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$OUT/SHA256SUMS" -out "$OUT/SHA256SUMS.sig"

echo
echo "hazır: $OUT"
ls -la "$OUT" | awk 'NR > 1 { print "  " $5, $9 }'

if [ "$PUBLISH" = 1 ]; then
  [ "$PROFILE" = release ] || { echo "--dev derlemesi yayınlanmaz" >&2; exit 1; }
  gh release create "v$VERSION" "$OUT"/* --title "PixPanel $VERSION" --generate-notes
fi
