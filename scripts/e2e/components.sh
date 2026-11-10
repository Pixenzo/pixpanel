#!/usr/bin/env bash
# Bileşenlerin temiz makinede panelden sıfırdan kurulmasının sınaması. Geliştirme
# makinesinde çalışır. Her konteyneri "temiz" anlık görüntüsüne döndürür (ct.sh), paneli
# sürüm dosyalarından kurar, components-remote.sh ile bütün bileşenleri kurup dener.
#
# Kullanım: scripts/e2e/components.sh [--keep] <sürüm klasörü> <konteyner no> ...
#   ör.     scripts/e2e/components.sh dist/release/0.1.0 190 191
#   --keep  sınama bitince konteyneri temiz hâle döndürme (elle incelemek için)
#   PX_ONLY="mail roundcube" ile yalnızca sayılan bileşenler (ve nginx) kurulur.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/scripts/e2e"
KEEP=0
[ "${1:-}" = "--keep" ] && { KEEP=1; shift; }
RELEASE="${1:?kullanım: components.sh [--keep] <sürüm klasörü> <konteyner no> ...}"
shift
[ $# -gt 0 ] || { echo "en az bir konteyner numarası verin"; exit 2; }
PVE=proxmox
STAGE=/tmp/pixpanel-components-e2e-$$

ssh -o BatchMode=yes $PVE "rm -rf $STAGE && mkdir -p $STAGE/rel $STAGE/e2e"
scp -q "$RELEASE"/* "$PVE:$STAGE/rel/" || { echo "  HATA sürüm dosyaları yüklenemedi"; exit 1; }
scp -q "$HERE"/*.sh "$HERE"/*.py "$PVE:$STAGE/e2e/" || { echo "  HATA betikler yüklenemedi"; exit 1; }

STATUS=0
for id in "$@"; do
  echo "=== konteyner $id"
  bash "$HERE/ct.sh" rollback "$id" || { echo "  HATA konteyner $id temiz hâle döndürülemedi"; STATUS=1; continue; }
  OUTPUT=$(ssh -o BatchMode=yes $PVE "
    pct exec $id -- mkdir -p /root/rel /root/e2e &&
    for file in $STAGE/rel/*; do pct push $id \$file /root/rel/\$(basename \$file); done &&
    for file in $STAGE/e2e/*; do pct push $id \$file /root/e2e/\$(basename \$file); done &&
    pct exec $id -- env PX_ONLY='${PX_ONLY:-}' bash /root/e2e/components-remote.sh
  " 2>&1)
  echo "$OUTPUT"
  echo "--- konteyner $id: $(grep -c '  OK ' <<<"$OUTPUT") geçti, $(grep -c 'HATA' <<<"$OUTPUT") hata"
  grep -q "HATA" <<<"$OUTPUT" && STATUS=1
  grep -q "^bitti$" <<<"$OUTPUT" || { echo "  HATA sınama sonuna kadar koşmadı"; STATUS=1; }
  [ "$KEEP" = 1 ] || bash "$HERE/ct.sh" rollback "$id" >/dev/null
done
ssh -o BatchMode=yes $PVE "rm -rf $STAGE"
exit $STATUS
