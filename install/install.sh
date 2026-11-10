#!/usr/bin/env bash
# PixPanel kurulum betiği. Debian 13 ve Ubuntu 26.04 üzerinde çalışır.
#
#   curl -fsSL https://github.com/Pixenzo/pixpanel/releases/latest/download/install.sh | bash
#
# Seçenekler (boruyla çalıştırırken: ... | bash -s -- --port 9443):
#   --url ADRES      sürüm dosyalarının indirileceği adres
#   --from-dir YOL   dosyaları indirmek yerine bu klasörden al (çevrimdışı kurulum)
#   --port PORT      panelin dinleyeceği port (varsayılan 8443; yalnızca ilk kurulumda)
#   --admin AD       ilk yönetici hesabının adı (varsayılan admin)
#   --force          desteklenmeyen dağıtımda da kur
#
# Betik yeniden çalıştırılırsa paneli son sürüme yükseltir; ayarlara ve verilere dokunmaz.
set -euo pipefail

RELEASE_URL="${PIXPANEL_RELEASE_URL:-https://github.com/Pixenzo/pixpanel/releases/latest/download}"
FROM_DIR=""
PORT=8443
ADMIN=admin
FORCE=0

# Sürümleri imzalayan anahtarın açık yarısı. İmzası bu anahtarla doğrulanmayan dosya kurulmaz.
PUBLIC_KEY='-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAs8cChFy/C32eKgzf57C6Km1GnzD5BbLefIPgC+guBWA=
-----END PUBLIC KEY-----'

BINARY=/usr/local/bin/pixpanel
CONFIG_DIR=/etc/pixpanel
DATA_DIR=/var/lib/pixpanel

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'HATA: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --url) RELEASE_URL="${2:?--url bir adres ister}"; shift 2 ;;
    --from-dir) FROM_DIR="${2:?--from-dir bir klasör ister}"; shift 2 ;;
    --port) PORT="${2:?--port bir sayı ister}"; shift 2 ;;
    --admin) ADMIN="${2:?--admin bir ad ister}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    *) die "bilinmeyen seçenek: $1" ;;
  esac
done

[ "$(id -u)" = 0 ] || die "bu betik root olarak çalıştırılmalı"
[[ "$PORT" =~ ^[0-9]+$ ]] && [ "$PORT" -ge 1 ] && [ "$PORT" -le 65535 ] || die "geçersiz port: $PORT"
[[ "$ADMIN" =~ ^[a-z][a-z0-9_-]{2,31}$ ]] || die "yönetici adı küçük harfle başlamalı; 3–32 karakter, küçük harf, rakam, _ ve - içerebilir"
command -v systemctl >/dev/null || die "systemd bulunamadı"

. /etc/os-release
SUPPORTED=0
case "${ID:-}:${VERSION_ID:-}" in
  debian:13|ubuntu:26.04) SUPPORTED=1 ;;
esac
if [ "$SUPPORTED" = 0 ]; then
  [ "$FORCE" = 1 ] || die "desteklenmeyen dağıtım: ${PRETTY_NAME:-bilinmiyor}. PixPanel Debian 13 ve Ubuntu 26.04 ister; eski sürümlerde posta, DNS ve yedekleme bileşenleri çalışmaz (yine de denemek için --force)"
  say "UYARI: ${PRETTY_NAME:-bu dağıtım} sınanmadı; --force ile devam ediliyor"
fi

case "$(uname -m)" in
  x86_64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "desteklenmeyen işlemci mimarisi: $(uname -m)" ;;
esac
ARCHIVE="pixpanel-linux-$ARCH.tar.gz"

say "Gerekli paketler kuruluyor"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl openssl tar >/dev/null

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fetch() {
  if [ -n "$FROM_DIR" ]; then
    cp "$FROM_DIR/$1" "$WORK/$1" || die "$1 bulunamadı: $FROM_DIR"
  else
    curl -fsSL --retry 3 -o "$WORK/$1" "$RELEASE_URL/$1" || die "$1 indirilemedi: $RELEASE_URL"
  fi
}

say "Sürüm dosyaları alınıyor"
for file in SHA256SUMS SHA256SUMS.sig VERSION "$ARCHIVE"; do fetch "$file"; done

say "İmza ve özetler doğrulanıyor"
printf '%s\n' "$PUBLIC_KEY" > "$WORK/release.pub"
openssl pkeyutl -verify -pubin -inkey "$WORK/release.pub" -rawin -in "$WORK/SHA256SUMS" -sigfile "$WORK/SHA256SUMS.sig" >/dev/null 2>&1 \
  || die "sürümün imzası doğrulanamadı; kurulum durduruldu"
(cd "$WORK" && grep -E "^[0-9a-f]{64} [ *](VERSION|$ARCHIVE)\$" SHA256SUMS > wanted.sums && [ "$(wc -l < wanted.sums)" = 2 ] && sha256sum -c --quiet wanted.sums) \
  || die "indirilen dosyalar bozuk ya da değiştirilmiş (özet uyuşmuyor)"
VERSION="$(tr -d '[:space:]' < "$WORK/VERSION")"

tar -C "$WORK" -xzf "$WORK/$ARCHIVE" pixpanel pixpanel.service uninstall.sh
chmod 755 "$WORK/pixpanel"
"$WORK/pixpanel" --version >/dev/null || die "indirilen ikili bu sunucuda çalışmıyor"

FIRST_INSTALL=1
[ -f "$DATA_DIR/pixpanel.db" ] && FIRST_INSTALL=0

say "PixPanel $VERSION kuruluyor"
install -m 755 "$WORK/pixpanel" "$BINARY.new"
mv "$BINARY.new" "$BINARY"
install -m 644 "$WORK/pixpanel.service" /etc/systemd/system/pixpanel.service
install -m 755 "$WORK/uninstall.sh" /usr/local/sbin/pixpanel-uninstall
install -d -m 755 "$CONFIG_DIR"
# Klasörün içine girilebilir ama listelenemez: web sunucusu (doğrulama dosyaları, web
# postası) ve site kullanıcıları (yayınlama araçları) alt klasörlere ulaşmak zorunda.
# Gizli dosyalar (veritabanı, anahtarlar) kendi izinleriyle korunur.
install -d -m 711 "$DATA_DIR"
if [ ! -f "$CONFIG_DIR/config.toml" ]; then
  printf 'listen_port = %s\n' "$PORT" > "$CONFIG_DIR/config.toml"
  chmod 600 "$CONFIG_DIR/config.toml"
else
  PORT="$(sed -n 's/^listen_port *= *\([0-9]\+\).*/\1/p' "$CONFIG_DIR/config.toml" | head -1)"
  PORT="${PORT:-8443}"
fi
systemctl daemon-reload

CREDENTIALS=""
if [ "$FIRST_INSTALL" = 1 ]; then
  # Hesap, panel başlamadan önce oluşturulur; parola yalnızca burada bir kez gösterilir.
  CREDENTIALS="$("$BINARY" create-admin --username "$ADMIN")" || die "yönetici hesabı oluşturulamadı"
fi

systemctl enable --quiet pixpanel
systemctl restart pixpanel

say "Panelin başlaması bekleniyor"
STARTED=0
for _ in $(seq 1 30); do
  if systemctl is-active --quiet pixpanel && (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then STARTED=1; break; fi
  sleep 1
done
[ "$STARTED" = 1 ] || die "panel başlamadı; ayrıntı için: journalctl -u pixpanel -n 50"

ADDRESS="$(hostname -I 2>/dev/null | awk '{print $1}')"
echo
say "PixPanel $VERSION hazır"
echo "    Adres: https://${ADDRESS:-sunucu-adresi}:$PORT"
if [ -n "$CREDENTIALS" ]; then
  echo
  printf '%s\n' "$CREDENTIALS" | sed 's/^/    /'
  echo
  echo "    Bu parola bir daha gösterilmez; şimdi kaydedin ve ilk girişte değiştirin."
  echo "    Tarayıcı ilk girişte sertifika uyarısı verir (panel kendi imzaladığı sertifikayla"
  echo "    başlar); Ayarlar sayfasından gerçek sertifika alabilirsiniz."
else
  echo "    Var olan kurulum yükseltildi; hesaplar ve ayarlar olduğu gibi duruyor."
fi
echo "    Kaldırmak için: pixpanel-uninstall"
