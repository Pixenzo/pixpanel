#!/usr/bin/env bash
# Birim testlerini test makinesinde çalıştırır.
# Kod Linux'a özgü çağrılar kullandığından testler Windows'ta derlenmez;
# kaynak test makinesine kopyalanır ve orada `cargo test` çalıştırılır.
# Kullanım: scripts/test.sh [kullanıcı@makine]
set -euo pipefail

TARGET_HOST="${1:-root@192.168.1.25}"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR"
# Arayüz ikili dosyaya gömüldüğü için derlenmiş hâli de gönderilir.
test -d web/dist || (cd web && pnpm build >/dev/null)

tar czf - --exclude=server/target server/Cargo.toml server/Cargo.lock server/src web/dist |
  ssh -o BatchMode=yes "$TARGET_HOST" '
    set -e
    mkdir -p /root/capsule-src
    # Derleme önbelleği (target) korunur; yalnızca kaynak yenilenir.
    rm -rf /root/capsule-src/server/src /root/capsule-src/web
    tar xzf - --no-same-owner -C /root/capsule-src
    cd /root/capsule-src/server
    . ~/.cargo/env
    cargo test 2>&1 | grep -E "^test |test result|^error|panicked"
  '
