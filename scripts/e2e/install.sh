#!/usr/bin/env bash
# Kurulum betiğinin temiz makinelerde sınanması. Geliştirme makinesinde çalışır.
# Proxmox'taki (ssh proxmox) verilen konteynerlere sürüm dosyalarını yükler ve
# install-remote.sh'i koşturur. Konteynerler temiz olmalıdır (panel kurulu olmamalı).
#
# Kullanım: scripts/e2e/install.sh <sürüm klasörü> <konteyner no> [konteyner no ...]
#   ör.     scripts/e2e/install.sh dist/release/0.1.0 190 191
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RELEASE="${1:?kullanım: install.sh <sürüm klasörü> <konteyner no> ...}"
shift
[ $# -gt 0 ] || { echo "en az bir konteyner numarası verin"; exit 2; }
PVE=proxmox
STAGE=/tmp/pixpanel-install-e2e

ssh -o BatchMode=yes $PVE "rm -rf $STAGE && mkdir -p $STAGE/rel"
scp -q "$RELEASE"/* "$PVE:$STAGE/rel/" || { echo "  HATA sürüm dosyaları yüklenemedi"; exit 1; }
scp -q "$ROOT/scripts/e2e/install-remote.sh" "$PVE:$STAGE/"

for id in "$@"; do
  echo "=== konteyner $id"
  ssh -o BatchMode=yes $PVE "
    pct exec $id -- rm -rf /root/rel && pct exec $id -- mkdir -p /root/rel &&
    for file in $STAGE/rel/*; do pct push $id \$file /root/rel/\$(basename \$file); done &&
    pct push $id $STAGE/install-remote.sh /root/install-remote.sh &&
    pct exec $id -- bash /root/install-remote.sh
  " 2>&1
done
ssh -o BatchMode=yes $PVE "rm -rf $STAGE"
