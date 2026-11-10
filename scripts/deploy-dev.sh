#!/usr/bin/env bash
# Geliştirme dağıtımı: arka ucu Linux için derler, test makinesine kopyalar ve servisi yeniden başlatır.
# Kullanım: scripts/deploy-dev.sh [kullanıcı@makine]
set -euo pipefail

TARGET_HOST="${1:-root@192.168.1.25}"
TARGET_TRIPLE="x86_64-unknown-linux-musl"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# cargo-zigbuild ve zig, pip ile kullanıcı dizinine kuruludur.
PY_USER_DIR="$APPDATA/Python/Python314"
export PATH="$PY_USER_DIR/Scripts:$PATH"
export CARGO_ZIGBUILD_ZIG_PATH="$(cygpath -w "$PY_USER_DIR/site-packages/ziglang/zig.exe")"

# Arayüz ikili dosyaya gömüldüğü için önce o derlenir.
(cd "$ROOT_DIR/web" && pnpm build >/dev/null)

cd "$ROOT_DIR/server"
cargo zigbuild --target "$TARGET_TRIPLE" 2>&1 | grep -E "^(error|warning: unused)|^\s+-->|Finished" || true
BINARY="target/$TARGET_TRIPLE/debug/capsule"
test -f "$BINARY"

scp -q "$BINARY" "$TARGET_HOST:/usr/local/bin/capsule.new"
scp -q "$ROOT_DIR/install/capsule.service" "$TARGET_HOST:/etc/systemd/system/capsule.service"
ssh -o BatchMode=yes "$TARGET_HOST" '
  set -e
  chmod 755 /usr/local/bin/capsule.new
  mv /usr/local/bin/capsule.new /usr/local/bin/capsule
  mkdir -p /etc/capsule /var/lib/capsule
  systemctl daemon-reload
  systemctl enable --quiet capsule
  systemctl restart capsule
  sleep 1
  systemctl is-active capsule
'
