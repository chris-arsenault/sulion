#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Declaratively erase a disk and install sulion-enclave.

Usage:
  sulion-bootstrap-enclave --disk /dev/disk/by-id/ID [key options]

Required:
  --disk PATH                 Stable whole-disk /dev/disk/by-id path to erase.

Administration key:
  --key-file PATH             Read one ssh-ed25519 public key from PATH.
  --key-fingerprint SHA256:X  Select this key from a forwarded SSH agent.
                              Without --key-file, a forwarded agent is used.

Safety:
  --confirm-disk PATH         Non-interactive confirmation; must equal --disk.
  --dry-run                   Evaluate the Disko layout without erasing.
  -h, --help                  Show this help.
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

disk=
key_file=
key_fingerprint=
confirm_disk=
dry_run=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --disk)
      [[ $# -ge 2 ]] || die "--disk requires a path"
      disk=$2
      shift 2
      ;;
    --key-file)
      [[ $# -ge 2 ]] || die "--key-file requires a path"
      key_file=$2
      shift 2
      ;;
    --key-fingerprint)
      [[ $# -ge 2 ]] || die "--key-fingerprint requires a SHA256 fingerprint"
      key_fingerprint=$2
      shift 2
      ;;
    --confirm-disk)
      [[ $# -ge 2 ]] || die "--confirm-disk requires a path"
      confirm_disk=$2
      shift 2
      ;;
    --dry-run)
      dry_run=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

[[ ${EUID} -eq 0 ]] || die "run this command as root from the NixOS installer"
[[ -n "$disk" ]] || die "--disk is required"
[[ "$disk" == /dev/disk/by-id/* ]] || die "--disk must use a stable /dev/disk/by-id path"
[[ "$disk" != *-part[0-9]* ]] || die "--disk must identify a whole disk, not a partition"
[[ -z "$key_file" || -z "$key_fingerprint" ]] || die "--key-file and --key-fingerprint are mutually exclusive"
[[ "$(uname -m)" == x86_64 ]] || die "the dedicated configuration supports x86_64 only"
[[ -r /etc/os-release ]] || die "cannot identify the installer operating system"
# shellcheck source=/dev/null
source /etc/os-release
[[ "${ID:-}" == nixos ]] || die "run this command from the NixOS installer"
[[ -d /sys/firmware/efi/efivars ]] || die "the installer was not booted in UEFI mode"
[[ -b "$disk" ]] || die "installation disk is not a block device: $disk"

resolved_disk=$(readlink -f -- "$disk")
[[ "$(lsblk -dnro TYPE "$resolved_disk")" == disk ]] || die "installation target is not a whole disk: $disk"
if lsblk -nrpo MOUNTPOINTS "$resolved_disk" | grep -Eq '[^[:space:]]'; then
  die "the installation disk or one of its partitions is mounted"
fi

stage_dir=$(mktemp -d)
cleanup() {
  if mountpoint -q /mnt; then
    umount -R /mnt
  fi
  rm -rf "$stage_dir"
}
trap cleanup EXIT
umask 077

# shellcheck source=/dev/null
source "${SULION_KEY_UTILS:?SULION_KEY_UTILS is not set}"
authorized_key="${stage_dir}/authorized_keys"
if [[ -n "$key_file" ]]; then
  sulion_key_from_file "$key_file" "$authorized_key"
else
  sulion_key_from_agent "$key_fingerprint" "$authorized_key"
fi
key_line=$(<"$authorized_key")
key_fingerprint=$(sulion_key_fingerprint "$key_line")

printf 'Installation target:\n'
lsblk -d -o NAME,SIZE,MODEL,SERIAL,TRAN "$resolved_disk"
printf 'Administration key: %s\n' "$key_fingerprint"
printf 'Flake source: %s\n' "${SULION_BOOTSTRAP_FLAKE:?SULION_BOOTSTRAP_FLAKE is not set}"

disko_args=(
  --mode destroy,format,mount
  --flake "${SULION_BOOTSTRAP_FLAKE}#sulion-enclave"
  --argstr disk "$disk"
)

if [[ "$dry_run" == true ]]; then
  "${SULION_DISKO:?SULION_DISKO is not set}" "${disko_args[@]}" --dry-run
  printf 'Dry run complete. No disk changes were made.\n'
  exit 0
fi

if [[ -n "$confirm_disk" ]]; then
  [[ "$confirm_disk" == "$disk" ]] || die "--confirm-disk must exactly match --disk"
else
  printf '\nTHIS WILL PERMANENTLY ERASE %s.\n' "$disk" >&2
  read -r -p "Type 'erase ${disk}' to continue: " confirmation
  [[ "$confirmation" == "erase ${disk}" ]] || die "installation canceled"
fi

mountpoint -q /mnt && die "/mnt is already in use"
"${SULION_DISKO:?SULION_DISKO is not set}" \
  "${disko_args[@]}" \
  --yes-wipe-all-disks

findmnt /mnt >/dev/null || die "Disko did not mount the target root at /mnt"
findmnt /mnt/boot >/dev/null || die "Disko did not mount the EFI partition at /mnt/boot"

install -D -m 0600 -o root -g root \
  "$authorized_key" \
  /mnt/var/lib/sulion/config/ssh/authorized_keys

export NIX_CONFIG="${NIX_CONFIG:-}
experimental-features = nix-command flakes"
nixos-install \
  --root /mnt \
  --flake "${SULION_BOOTSTRAP_FLAKE}#sulion-enclave"

nixos-enter --root /mnt -c 'passwd sulion'

for service_binary in systemd-oomd systemd-timesyncd; do
  found_service_binary=false
  for service_path in "/mnt/nix/store/"*-systemd-*/lib/systemd/"$service_binary"; do
    [[ -e "$service_path" ]] || continue
    found_service_binary=true
    [[ -x "$service_path" ]] || die "installed service is not executable: ${service_path#/mnt}"
  done
  [[ "$found_service_binary" == true ]] || die "installed service is missing: $service_binary"
done

sync
umount -R /mnt

printf '\nInstallation complete.\n'
printf 'Remove the installer media, reboot, and SSH as sulion using %s.\n' "$key_fingerprint"
