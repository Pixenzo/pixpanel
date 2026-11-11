#!/usr/bin/env bash
# Bütün uçtan uca sınamaları sırayla çalıştırır ve özet verir.
# Geliştirme makinesinde çalışır; panel bilgileri .env dosyasından okunur.
# Kullanım: scripts/e2e/run-all.sh [betik adı ...]     (ad verilmezse hepsi)
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
set -a; . "$ROOT/.env"; set +a
HOST=root@192.168.1.25
HERE="$ROOT/scripts/e2e"

# Test makinesinin içinde koşanlar (panele yerel adresten bağlanırlar).
REMOTE=(features mail dbtools webmail node ftp dns twofactor panelcert redis backup events health wordpress packages backup-mine access stats metrics backup-ftp templates cli ips redis-account impersonate rules clone branding python dbimport tenancy portguard)
# Geliştirme makinesinde koşanlar (bağlantıları dışarıdan denerler).
LOCAL=(firewall fail2ban update)

WANTED=("$@")
wanted() { [ ${#WANTED[@]} -eq 0 ] && return 0; for name in "${WANTED[@]}"; do [ "$name" = "$1" ] && return 0; done; return 1; }

scp -q "$HERE"/*.sh "$HERE"/*.py "$HOST:/root/" || { echo "betikler kopyalanamadı"; exit 1; }
# Bildirim sınamalarının sahte SMTP/Telegram sunucusu.
ssh -o BatchMode=yes "$HOST" 'systemctl stop pxmocks 2>/dev/null; systemd-run --quiet --unit pxmocks python3 /root/mocks.py' 2>/dev/null

declare -A RESULT
run() {
  local name=$1 output failures
  echo "=== $name"
  output=$("${@:2}" 2>&1)
  failures=$(grep -c "HATA" <<<"$output")
  grep "HATA" <<<"$output" | cut -c1-200
  RESULT[$name]="$(grep -c "  OK " <<<"$output") geçti, $failures hata"
  [ "$failures" -eq 0 ]
}

STATUS=0
for name in "${REMOTE[@]}"; do
  wanted "$name" || continue
  run "$name" ssh -o BatchMode=yes "$HOST" "PX_USER='$PIXPANEL_TEST_USER' PX_PASS='$PIXPANEL_TEST_PASSWORD' bash /root/$name.sh" || STATUS=1
done
for name in "${LOCAL[@]}"; do
  wanted "$name" || continue
  run "$name" bash "$HERE/$name.sh" || STATUS=1
done
if wanted unit; then
  echo "=== birim testleri"
  UNIT=$(bash "$ROOT/scripts/test.sh" 2>&1 | grep "test result" | tail -1)
  RESULT[unit]="$UNIT"
  grep -q "0 failed" <<<"$UNIT" || STATUS=1
fi
ssh -o BatchMode=yes "$HOST" 'systemctl stop pxmocks 2>/dev/null' 2>/dev/null

echo
echo "=== Özet"
for name in "${REMOTE[@]}" "${LOCAL[@]}" unit; do
  [ -n "${RESULT[$name]:-}" ] && printf '%-12s %s\n' "$name" "${RESULT[$name]}"
done
exit $STATUS
