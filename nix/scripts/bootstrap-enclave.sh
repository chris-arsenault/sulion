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
  --dry-run                   Build and print the Disko plan without erasing.
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
password_mount=
cleanup() {
  if [[ -n "$password_mount" ]] && mountpoint -q "$password_mount"; then
    umount "$password_mount"
  fi
  [[ -z "$password_mount" ]] || rmdir "$password_mount" 2>/dev/null || true
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
  --mode format
  --flake "${SULION_BOOTSTRAP_FLAKE}#sulion-enclave"
  --disk main "$disk"
  --extra-files "$authorized_key" /var/lib/sulion/config/ssh/authorized_keys
  --write-efi-boot-entries
)

if [[ "$dry_run" == true ]]; then
  dry_run_mount="${stage_dir}/dry-run-root"
  mkdir -p "$dry_run_mount"
  "${SULION_DISKO_INSTALL:?SULION_DISKO_INSTALL is not set}" \
    "${disko_args[@]}" \
    --mount-point "$dry_run_mount" \
    --dry-run
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

read -r -s -p "New console/sudo password for sulion: " password
printf '\n'
[[ -n "$password" ]] || die "password must not be empty"
read -r -s -p "Confirm password: " password_confirmation
printf '\n'
[[ "$password" == "$password_confirmation" ]] || die "passwords do not match"
unset password_confirmation
password_hash=$(printf '%s\n' "$password" | mkpasswd --method=yescrypt --stdin)
unset password

"${SULION_DISKO_INSTALL:?SULION_DISKO_INSTALL is not set}" "${disko_args[@]}"

udevadm settle
root_partition="${disk}-part2"
[[ -b "$root_partition" ]] || die "installed root partition is missing: $root_partition"
password_mount=$(mktemp -d)
mount "$root_partition" "$password_mount"
printf 'sulion:%s\n' "$password_hash" | chpasswd --root "$password_mount" --encrypted
unset password_hash
sync
umount "$password_mount"
rmdir "$password_mount"
password_mount=

printf '\nInstallation complete.\n'
printf 'Remove the installer media, reboot, and SSH as sulion using %s.\n' "$key_fingerprint"
