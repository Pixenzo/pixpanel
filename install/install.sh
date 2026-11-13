#!/usr/bin/env bash
# PixPanel kurulum betiği. Debian 13 ve Ubuntu 26.04 üzerinde çalışır.
#
#   curl -fsSL https://github.com/Pixenzo/pixpanel/releases/latest/download/install.sh | bash
#
# Seçenekler (boruyla çalıştırırken: ... | bash -s -- --lang tr --port 9443):
#   --lang DİL       betiğin dili: en (varsayılan) ya da tr. Verilirse ilk yöneticinin panel dili de
#                    bu olur; verilmezse panel dili ilk girişte tarayıcıdan belirlenir.
#   --url ADRES      sürüm dosyalarının indirileceği adres
#   --from-dir YOL   dosyaları indirmek yerine bu klasörden al (çevrimdışı kurulum)
#   --port PORT      panelin dinleyeceği port (varsayılan 8443; yalnızca ilk kurulumda)
#   --admin AD       ilk yönetici hesabının adı (varsayılan admin)
#   --no-databases   ilk kurulumda MariaDB ve PostgreSQL'i kurma (sonradan Bileşenler sayfasından kurulur)
#   --force          desteklenmeyen dağıtımda da kur
#
# Betik yeniden çalıştırılırsa paneli son sürüme yükseltir; ayarlara ve verilere dokunmaz.
# Seçilen dil /etc/pixpanel/language dosyasına yazılır; yükseltmede ve kaldırmada o dil kullanılır.
set -euo pipefail

RELEASE_URL="${PIXPANEL_RELEASE_URL:-https://github.com/Pixenzo/pixpanel/releases/latest/download}"
FROM_DIR=""
PORT=8443
ADMIN=admin
FORCE=0
DATABASES=1
LANGUAGE=""

# Sürümleri imzalayan anahtarın açık yarısı. İmzası bu anahtarla doğrulanmayan dosya kurulmaz.
PUBLIC_KEY='-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAs8cChFy/C32eKgzf57C6Km1GnzD5BbLefIPgC+guBWA=
-----END PUBLIC KEY-----'

BINARY=/usr/local/bin/pixpanel
CONFIG_DIR=/etc/pixpanel
DATA_DIR=/var/lib/pixpanel

# m <türkçe> <english>: seçili dildeki metni verir. Dil seçenekler okunmadan önce de gerekebilir
# (hatalı seçenek iletisi); o ana kadar komut satırında --lang tr görüldüyse Türkçe yazılır.
LANGUAGE_EARLY=en
for arg in "$@"; do [ "$arg" = tr ] && [ "${previous:-}" = --lang ] && LANGUAGE_EARLY=tr; previous=$arg; done
m() { if [ "${LANGUAGE:-$LANGUAGE_EARLY}" = tr ]; then printf '%s' "$1"; else printf '%s' "$2"; fi; }
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '%s %s\n' "$(m HATA: ERROR:)" "$*" >&2; exit 1; }
need() { [ -n "${2:-}" ] || die "$1 $(m 'bir değer ister' 'needs a value')"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --lang) need "$1" "${2:-}"; LANGUAGE="$2"; shift 2 ;;
    --url) need "$1" "${2:-}"; RELEASE_URL="$2"; shift 2 ;;
    --from-dir) need "$1" "${2:-}"; FROM_DIR="$2"; shift 2 ;;
    --port) need "$1" "${2:-}"; PORT="$2"; shift 2 ;;
    --admin) need "$1" "${2:-}"; ADMIN="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --no-databases) DATABASES=0; shift ;;
    *) die "$(m 'bilinmeyen seçenek' 'unknown option'): $1" ;;
  esac
done

# Dil açıkça verilmediyse önceki kurulumda seçilen dil, o da yoksa İngilizce.
EXPLICIT_LANGUAGE="$LANGUAGE"
if [ -z "$LANGUAGE" ]; then
  LANGUAGE="$(cat "$CONFIG_DIR/language" 2>/dev/null || true)"
fi
case "$LANGUAGE" in
  tr|en) ;;
  "") LANGUAGE=en ;;
  *) LANGUAGE=en; die "$(m 'geçersiz dil' 'invalid language'): $EXPLICIT_LANGUAGE (en, tr)" ;;
esac

[ "$(id -u)" = 0 ] || die "$(m 'bu betik root olarak çalıştırılmalı' 'this script must be run as root')"
[[ "$PORT" =~ ^[0-9]+$ ]] && [ "$PORT" -ge 1 ] && [ "$PORT" -le 65535 ] || die "$(m 'geçersiz port' 'invalid port'): $PORT"
[[ "$ADMIN" =~ ^[a-z][a-z0-9_-]{2,31}$ ]] || die "$(m \
  'yönetici adı küçük harfle başlamalı; 3–32 karakter, küçük harf, rakam, _ ve - içerebilir' \
  'the administrator name must start with a lowercase letter and be 3–32 characters of lowercase letters, digits, _ and -')"
command -v systemctl >/dev/null || die "$(m 'systemd bulunamadı' 'systemd not found')"

. /etc/os-release
SUPPORTED=0
case "${ID:-}:${VERSION_ID:-}" in
  debian:13|ubuntu:26.04) SUPPORTED=1 ;;
esac
if [ "$SUPPORTED" = 0 ]; then
  DISTRO="${PRETTY_NAME:-$(m bilinmiyor unknown)}"
  [ "$FORCE" = 1 ] || die "$(m \
    "desteklenmeyen dağıtım: $DISTRO. PixPanel Debian 13 ve Ubuntu 26.04 ister; eski sürümlerde posta, DNS ve yedekleme bileşenleri çalışmaz (yine de denemek için --force)" \
    "unsupported distribution: $DISTRO. PixPanel needs Debian 13 or Ubuntu 26.04; mail, DNS and backup components do not work on older releases (use --force to try anyway)")"
  say "$(m "UYARI: $DISTRO sınanmadı; --force ile devam ediliyor" "WARNING: $DISTRO is not tested; continuing because of --force")"
fi

case "$(uname -m)" in
  x86_64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "$(m 'desteklenmeyen işlemci mimarisi' 'unsupported CPU architecture'): $(uname -m)" ;;
esac
ARCHIVE="pixpanel-linux-$ARCH.tar.gz"

say "$(m 'Gerekli paketler kuruluyor' 'Installing required packages')"
export DEBIAN_FRONTEND=noninteractive
# Panel arka planda paket işlemi yürütüyor olabilir (ör. güncelleme denetimi); kilit beklenir.
APT_WAIT=(-o DPkg::Lock::Timeout=600)
apt-get "${APT_WAIT[@]}" update -qq
apt-get "${APT_WAIT[@]}" install -y -qq ca-certificates curl openssl tar >/dev/null

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fetch() {
  if [ -n "$FROM_DIR" ]; then
    cp "$FROM_DIR/$1" "$WORK/$1" || die "$(m "$1 bulunamadı" "$1 not found"): $FROM_DIR"
  else
    curl -fsSL --retry 3 -o "$WORK/$1" "$RELEASE_URL/$1" || die "$(m "$1 indirilemedi" "could not download $1"): $RELEASE_URL"
  fi
}

say "$(m 'Sürüm dosyaları alınıyor' 'Fetching release files')"
for file in SHA256SUMS SHA256SUMS.sig VERSION "$ARCHIVE"; do fetch "$file"; done

say "$(m 'İmza ve özetler doğrulanıyor' 'Verifying signature and checksums')"
printf '%s\n' "$PUBLIC_KEY" > "$WORK/release.pub"
openssl pkeyutl -verify -pubin -inkey "$WORK/release.pub" -rawin -in "$WORK/SHA256SUMS" -sigfile "$WORK/SHA256SUMS.sig" >/dev/null 2>&1 \
  || die "$(m 'sürümün imzası doğrulanamadı; kurulum durduruldu' 'the release signature could not be verified; installation stopped')"
(cd "$WORK" && grep -E "^[0-9a-f]{64} [ *](VERSION|$ARCHIVE)\$" SHA256SUMS > wanted.sums && [ "$(wc -l < wanted.sums)" = 2 ] && sha256sum -c --quiet wanted.sums) \
  || die "$(m 'indirilen dosyalar bozuk ya da değiştirilmiş (özet uyuşmuyor)' 'the downloaded files are corrupted or have been modified (checksum mismatch)')"
VERSION="$(tr -d '[:space:]' < "$WORK/VERSION")"

tar -C "$WORK" -xzf "$WORK/$ARCHIVE" pixpanel pixpanel.service uninstall.sh
chmod 755 "$WORK/pixpanel"
"$WORK/pixpanel" --version >/dev/null || die "$(m 'indirilen ikili bu sunucuda çalışmıyor' 'the downloaded binary does not run on this server')"

FIRST_INSTALL=1
[ -f "$DATA_DIR/pixpanel.db" ] && FIRST_INSTALL=0

say "$(m "PixPanel $VERSION kuruluyor" "Installing PixPanel $VERSION")"
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
  # Araçlar varsayılan olarak 8444'te çalışır; panel portu o seçildiyse bir sonrakine geçer.
  [ "$PORT" = 8444 ] && printf 'tools_port = 8445\n' >> "$CONFIG_DIR/config.toml"
  chmod 600 "$CONFIG_DIR/config.toml"
else
  PORT="$(sed -n 's/^listen_port *= *\([0-9]\+\).*/\1/p' "$CONFIG_DIR/config.toml" | head -1)"
  PORT="${PORT:-8443}"
fi
printf '%s\n' "$LANGUAGE" > "$CONFIG_DIR/language"
chmod 644 "$CONFIG_DIR/language"
systemctl daemon-reload

CREDENTIALS=""
if [ "$FIRST_INSTALL" = 1 ]; then
  # Hesap, panel başlamadan önce oluşturulur; parola yalnızca burada bir kez gösterilir.
  # Dil açıkça seçildiyse yöneticinin panel dili de odur; seçilmediyse ilk girişte tarayıcıdan belirlenir.
  LANGUAGE_ARGS=()
  [ -n "$EXPLICIT_LANGUAGE" ] && LANGUAGE_ARGS=(--language "$LANGUAGE")
  CREDENTIALS="$(PIXPANEL_LANG="$LANGUAGE" "$BINARY" create-admin --username "$ADMIN" "${LANGUAGE_ARGS[@]}")" \
    || die "$(m 'yönetici hesabı oluşturulamadı' 'could not create the administrator account')"
fi

systemctl enable --quiet pixpanel
systemctl restart pixpanel

say "$(m 'Panelin başlaması bekleniyor' 'Waiting for the panel to start')"
STARTED=0
for _ in $(seq 1 30); do
  if systemctl is-active --quiet pixpanel && (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null; then STARTED=1; break; fi
  sleep 1
done
[ "$STARTED" = 1 ] || die "$(m 'panel başlamadı; ayrıntı için' 'the panel did not start; for details run'): journalctl -u pixpanel -n 50"

# İlk kurulumda iki veritabanı sunucusu da panelin kendi bileşen kurulumuyla kurulur (panelin
# beklediği ayarlar, yerel erişim ve güvenlik adımları böylece tek yerde kalır). Kurulamayan
# olursa kurulum durmaz; yönetici Bileşenler sayfasından yeniden dener.
if [ "$FIRST_INSTALL" = 1 ] && [ "$DATABASES" = 1 ]; then
  for component in mariadb postgresql; do
    case "$component" in mariadb) label=MariaDB ;; *) label=PostgreSQL ;; esac
    say "$(m "$label kuruluyor" "Installing $label")"
    if "$BINARY" api POST "/components/$component/install" --wait >"$WORK/$component.log" 2>&1; then
      :
    elif grep -q 'HTTP 409' "$WORK/$component.log"; then
      # Sunucuda zaten kurulu (ör. silinip yeniden kurulan panel): yapılacak bir şey yok.
      say "$(m "$label zaten kurulu" "$label is already installed")"
    else
      tail -n 15 "$WORK/$component.log" | sed 's/^/    /' >&2
      say "$(m "UYARI: $label kurulamadı; panelin Bileşenler sayfasından yeniden deneyebilirsiniz" "WARNING: $label could not be installed; you can retry from the Components page of the panel")"
    fi
  done
fi

ADDRESS="$(hostname -I 2>/dev/null | awk '{print $1}')"
echo
say "$(m "PixPanel $VERSION hazır" "PixPanel $VERSION is ready")"
echo "    $(m Adres Address): https://${ADDRESS:-$(m sunucu-adresi server-address)}:$PORT"
TOOLS_PORT="$(sed -n 's/^tools_port *= *\([0-9]\+\).*/\1/p' "$CONFIG_DIR/config.toml" | head -1)"
echo "    $(m 'Veritabanı araçları ve web postası' 'Database tools and webmail'): https://${ADDRESS:-$(m sunucu-adresi server-address)}:${TOOLS_PORT:-8444} ($(m 'bu port da dışarıya açık olmalı' 'this port must be reachable too'))"
if [ -n "$CREDENTIALS" ]; then
  echo
  printf '%s\n' "$CREDENTIALS" | sed 's/^/    /'
  echo
  echo "    $(m 'Bu parola bir daha gösterilmez; şimdi kaydedin ve ilk girişte değiştirin.' 'This password is not shown again; save it now and change it after your first sign-in.')"
  echo "    $(m 'Tarayıcı ilk girişte sertifika uyarısı verir (panel kendi imzaladığı sertifikayla' 'Your browser shows a certificate warning at first (the panel starts with a self-signed')"
  echo "    $(m 'başlar); Ayarlar sayfasından gerçek sertifika alabilirsiniz.' 'certificate); you can get a real certificate from the Settings page.')"
else
  echo "    $(m 'Var olan kurulum yükseltildi; hesaplar ve ayarlar olduğu gibi duruyor.' 'The existing installation was upgraded; accounts and settings are unchanged.')"
fi
echo "    $(m 'Kaldırmak için' 'To uninstall'): pixpanel-uninstall"
