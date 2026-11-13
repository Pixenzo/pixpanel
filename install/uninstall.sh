#!/usr/bin/env bash
# PixPanel'i kaldırır. Sitelere, veritabanlarına, postalara ve kurulu bileşenlere
# (nginx, PHP, MariaDB…) dokunmaz; yalnızca panelin kendisini kaldırır.
#
#   pixpanel-uninstall           paneli kaldırır, ayar ve verilerini bırakır
#   pixpanel-uninstall --purge   panelin ayarlarını ve veritabanını da siler
#
# İletiler kurulumda seçilen dilde yazılır (/etc/pixpanel/language; yoksa İngilizce).
set -euo pipefail

LANGUAGE="$(cat /etc/pixpanel/language 2>/dev/null || echo en)"
m() { if [ "$LANGUAGE" = tr ]; then printf '%s' "$1"; else printf '%s' "$2"; fi; }

PURGE=0
for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    *) echo "$(m 'bilinmeyen seçenek' 'unknown option'): $arg" >&2; exit 2 ;;
  esac
done
[ "$(id -u)" = 0 ] || { echo "$(m 'HATA: bu betik root olarak çalıştırılmalı' 'ERROR: this script must be run as root')" >&2; exit 1; }

systemctl disable --now pixpanel 2>/dev/null || true
rm -f /etc/systemd/system/pixpanel.service /usr/local/bin/pixpanel /usr/local/bin/pixpanel.previous /usr/local/bin/pixpanel.new
systemctl daemon-reload

if [ "$PURGE" = 1 ]; then
  rm -rf /etc/pixpanel /var/lib/pixpanel /var/cache/pixpanel
  echo "$(m 'PixPanel, ayarları ve veritabanıyla birlikte kaldırıldı.' 'PixPanel was removed together with its settings and database.')"
else
  echo "$(m 'PixPanel kaldırıldı. Ayarlar (/etc/pixpanel) ve veriler (/var/lib/pixpanel) duruyor;' 'PixPanel was removed. Settings (/etc/pixpanel) and data (/var/lib/pixpanel) are kept;')"
  echo "$(m 'yeniden kurarsanız kaldığı yerden devam eder. Tümüyle silmek için: pixpanel-uninstall --purge' 'reinstalling continues where it left off. To delete everything: pixpanel-uninstall --purge')"
fi
echo "$(m 'Siteler (/home), veritabanları ve kurulu servisler yerinde bırakıldı.' 'Sites (/home), databases and installed services were left in place.')"
rm -f /usr/local/sbin/pixpanel-uninstall
