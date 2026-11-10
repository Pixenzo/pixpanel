#!/usr/bin/env bash
# PixPanel'i kaldırır. Sitelere, veritabanlarına, postalara ve kurulu bileşenlere
# (nginx, PHP, MariaDB…) dokunmaz; yalnızca panelin kendisini kaldırır.
#
#   pixpanel-uninstall           paneli kaldırır, ayar ve verilerini bırakır
#   pixpanel-uninstall --purge   panelin ayarlarını ve veritabanını da siler
set -euo pipefail

PURGE=0
for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    *) echo "bilinmeyen seçenek: $arg" >&2; exit 2 ;;
  esac
done
[ "$(id -u)" = 0 ] || { echo "HATA: bu betik root olarak çalıştırılmalı" >&2; exit 1; }

systemctl disable --now pixpanel 2>/dev/null || true
rm -f /etc/systemd/system/pixpanel.service /usr/local/bin/pixpanel /usr/local/bin/pixpanel.previous /usr/local/bin/pixpanel.new
systemctl daemon-reload

if [ "$PURGE" = 1 ]; then
  rm -rf /etc/pixpanel /var/lib/pixpanel /var/cache/pixpanel
  echo "PixPanel, ayarları ve veritabanıyla birlikte kaldırıldı."
else
  echo "PixPanel kaldırıldı. Ayarlar (/etc/pixpanel) ve veriler (/var/lib/pixpanel) duruyor;"
  echo "yeniden kurarsanız kaldığı yerden devam eder. Tümüyle silmek için: pixpanel-uninstall --purge"
fi
echo "Siteler (/home), veritabanları ve kurulu servisler yerinde bırakıldı."
rm -f /usr/local/sbin/pixpanel-uninstall
