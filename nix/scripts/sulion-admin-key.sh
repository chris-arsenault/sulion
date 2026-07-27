#!/usr/bin/env bash
set -euo pipefail

readonly target=${SULION_ADMIN_AUTHORIZED_KEYS_FILE:-/var/lib/sulion/config/ssh/authorized_keys}

usage() {
  cat <<'EOF'
Manage the root-owned break-glass SSH keys for sulion-enclave.

Usage:
  sulion-admin-key add PUBLIC_KEY_FILE
  sulion-admin-key replace PUBLIC_KEY_FILE
  sulion-admin-key remove SHA256:FINGERPRINT
  sulion-admin-key list

Key material is accepted from a file, never as a shell argument.
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

# shellcheck source=/dev/null
source "${SULION_KEY_UTILS:?SULION_KEY_UTILS is not set}"

command_name=${1:-}
case "$command_name" in
  -h | --help)
    usage
    exit 0
    ;;
  add | replace | remove | list)
    shift
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

[[ ${EUID} -eq 0 ]] || die "run this command through sudo"
target_dir=$(dirname "$target")
install -d -m 0700 -o root -g root "$target_dir"
exec 9>"${target}.lock"
chmod 0600 "${target}.lock"
flock 9

[[ ! -L "$target" ]] || die "authorized-key path must not be a symlink: $target"
if [[ ! -e "$target" ]]; then
  install -m 0600 -o root -g root /dev/null "$target"
fi
[[ -f "$target" ]] || die "authorized-key path is not a regular file: $target"
chown root:root "$target"
chmod 0600 "$target"

case "$command_name" in
  add | replace)
    [[ $# -eq 1 ]] || die "$command_name requires one public-key file"
    normalized_key=$(mktemp)
    output_file=$(mktemp "${target_dir}/.authorized_keys.XXXXXX")
    trap 'rm -f "${normalized_key:-}" "${output_file:-}"' EXIT
    sulion_key_from_file "$1" "$normalized_key"
    key_line=$(<"$normalized_key")

    if [[ "$command_name" == add ]] && grep -Fqx -- "$key_line" "$target"; then
      printf 'Key already authorized: %s\n' "$(sulion_key_fingerprint "$key_line")"
      exit 0
    fi

    if [[ "$command_name" == add ]]; then
      grep -Ev '^[[:space:]]*(#|$)' "$target" >"$output_file" || true
    fi
    printf '%s\n' "$key_line" >>"$output_file"
    chown root:root "$output_file"
    chmod 0600 "$output_file"
    mv -f "$output_file" "$target"
    printf '%s key: %s\n' \
      "$([[ "$command_name" == add ]] && printf 'Authorized' || printf 'Replaced with')" \
      "$(sulion_key_fingerprint "$key_line")"
    ;;
  remove)
    [[ $# -eq 1 ]] || die "remove requires one SHA256 fingerprint"
    requested_fingerprint=$1
    [[ "$requested_fingerprint" == SHA256:* ]] || die "fingerprint must start with SHA256:"
    output_file=$(mktemp "${target_dir}/.authorized_keys.XXXXXX")
    trap 'rm -f "${output_file:-}"' EXIT
    removed=0
    while IFS= read -r key_line || [[ -n "$key_line" ]]; do
      key_line=${key_line%$'\r'}
      [[ -z "$key_line" || "$key_line" == \#* ]] && continue
      sulion_validate_key_line "$key_line"
      if [[ "$(sulion_key_fingerprint "$key_line")" == "$requested_fingerprint" ]]; then
        removed=$((removed + 1))
      else
        printf '%s\n' "$key_line" >>"$output_file"
      fi
    done <"$target"
    ((removed > 0)) || die "fingerprint is not authorized: $requested_fingerprint"
    [[ -s "$output_file" ]] || die "refusing to remove the final administration key; replace it first"
    chown root:root "$output_file"
    chmod 0600 "$output_file"
    mv -f "$output_file" "$target"
    printf 'Removed key: %s\n' "$requested_fingerprint"
    ;;
  list)
    [[ $# -eq 0 ]] || die "list takes no arguments"
    found=0
    while IFS= read -r key_line || [[ -n "$key_line" ]]; do
      key_line=${key_line%$'\r'}
      [[ -z "$key_line" || "$key_line" == \#* ]] && continue
      sulion_validate_key_line "$key_line"
      printf '%s  %s\n' \
        "$(sulion_key_fingerprint "$key_line")" \
        "${key_line#ssh-ed25519 * }"
      found=$((found + 1))
    done <"$target"
    ((found > 0)) || printf 'No administration keys are authorized.\n'
    ;;
esac
