#!/usr/bin/env bash
# Temiz makine sınamalarının konteynerlerini yönetir: Proxmox'ta (ssh proxmox) anlık
# görüntü alır ve o görüntüye geri döner. Geliştirme makinesinde çalışır.
#
# Kullanım: scripts/e2e/ct.sh snapshot <konteyner no> ...   bugünkü hâli "temiz" olarak sakla
#           scripts/e2e/ct.sh rollback <konteyner no> ...   "temiz" hâle dön ve başlat
#           scripts/e2e/ct.sh status   <konteyner no> ...
#           scripts/e2e/ct.sh create   <konteyner no> ...   konteyneri sıfırdan kur ve "temiz" görüntüsünü al
#           scripts/e2e/ct.sh destroy  <konteyner no> ...   konteyneri sil
#
# Bilinen konteynerler: 190 = Debian 13, 191 = Ubuntu 26.04. Adresleri 192.168.1.<no>
# olarak sabit verilir (ağda 192.168.5.x dağıtan ikinci bir DHCP sunucusu var).
#
# Yanlış makineyi geri sarmamak için yalnızca adı "pixpanel-kurulum-" ile başlayan
# konteynerlere dokunur.
set -uo pipefail
PVE=proxmox
SNAPSHOT=temiz
ACTION="${1:?kullanım: ct.sh snapshot|rollback|status|create|destroy <konteyner no> ...}"
shift
[ $# -gt 0 ] || { echo "en az bir konteyner numarası verin" >&2; exit 2; }

pve() { ssh -o BatchMode=yes "$PVE" "$@"; }

guard() {
  local id=$1 name
  [[ "$id" =~ ^[0-9]+$ ]] || { echo "geçersiz konteyner numarası: $id" >&2; return 1; }
  name=$(pve "pct config $id 2>/dev/null | sed -n 's/^hostname: //p'")
  case "$name" in
    pixpanel-kurulum-*) return 0 ;;
    *) echo "konteyner $id (${name:-yok}) bir sınama konteyneri değil; dokunulmadı" >&2; return 1 ;;
  esac
}

# Konteyner açılıp ağa çıkana kadar bekler.
wait_ready() {
  local id=$1
  for _ in $(seq 1 60); do
    pve "pct exec $id -- sh -c 'systemctl is-system-running 2>/dev/null | grep -qE \"running|degraded\" && getent hosts deb.debian.org >/dev/null'" 2>/dev/null && return 0
    sleep 2
  done
  echo "konteyner $id hazır olmadı" >&2
  return 1
}

# create <no>: kalıbı (gerekirse indirip) kullanarak konteyneri kurar.
create() {
  local id=$1 template name
  case "$id" in
    190) template=debian-13-standard; name=pixpanel-kurulum-deb13 ;;
    191) template=ubuntu-26.04-standard; name=pixpanel-kurulum-ubu26 ;;
    *) echo "konteyner $id için kalıp tanımlı değil (190: Debian 13, 191: Ubuntu 26.04)" >&2; return 1 ;;
  esac
  pve "pct config $id >/dev/null 2>&1" && { echo "konteyner $id zaten var" >&2; return 1; }
  pve "
    set -e
    file=\$(pveam list local | awk '{print \$1}' | grep \"/$template\" | sort | tail -1)
    if [ -z \"\$file\" ]; then
      pveam update >/dev/null
      latest=\$(pveam available --section system | awk '{print \$2}' | grep \"^$template\" | sort | tail -1)
      pveam download local \"\$latest\" >/dev/null
      file=local:vztmpl/\$latest
    fi
    pct create $id \"\$file\" --hostname $name --unprivileged 1 --features nesting=1 --cores 2 --memory 2048 --swap 512 \
      --rootfs local-lvm:16 --net0 name=eth0,bridge=vmbr0,gw=192.168.1.1,ip=192.168.1.$id/24,type=veth --nameserver 1.1.1.1 \
      --description 'PixPanel kurulum sinamasi (gecici)' >/dev/null
    pct start $id
  " || return 1
  wait_ready "$id" || return 1
  pve "pct snapshot $id $SNAPSHOT --description 'PixPanel kurulmamis temiz hal'" >/dev/null && echo "konteyner $id ($name): kuruldu, anlık görüntü alındı"
}

STATUS=0
for id in "$@"; do
  if [ "$ACTION" = create ]; then create "$id" || STATUS=1; continue; fi
  guard "$id" || { STATUS=1; continue; }
  case "$ACTION" in
    snapshot)
      pve "pct listsnapshot $id | grep -qw $SNAPSHOT && pct delsnapshot $id $SNAPSHOT; pct snapshot $id $SNAPSHOT --description 'PixPanel kurulmamis temiz hal'" \
        && echo "konteyner $id: anlık görüntü alındı" || STATUS=1
      ;;
    rollback)
      pve "pct listsnapshot $id | grep -qw $SNAPSHOT" || { echo "konteyner $id: '$SNAPSHOT' anlık görüntüsü yok (önce: ct.sh snapshot $id)" >&2; STATUS=1; continue; }
      pve "pct rollback $id $SNAPSHOT --start 1" >/dev/null && wait_ready "$id" \
        && echo "konteyner $id: temiz hâle dönüldü" || STATUS=1
      ;;
    destroy)
      pve "pct stop $id 2>/dev/null; pct destroy $id --purge 1" >/dev/null && echo "konteyner $id: silindi" || STATUS=1
      ;;
    status)
      pve "pct status $id; pct listsnapshot $id; pct exec $id -- sh -c '. /etc/os-release; echo \$PRETTY_NAME; df -h / | tail -1; test -e /usr/local/bin/pixpanel && echo panel kurulu || echo panel yok'"
      ;;
    *) echo "bilinmeyen işlem: $ACTION" >&2; exit 2 ;;
  esac
done
exit $STATUS
