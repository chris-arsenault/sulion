sulion_key_fingerprint() {
  local key_line=$1
  local key_file
  local key_details
  local fingerprint

  key_file=$(mktemp)
  chmod 0600 "$key_file"
  printf '%s\n' "$key_line" >"$key_file"
  if ! key_details=$(ssh-keygen -E sha256 -lf "$key_file" 2>&1); then
    rm -f "$key_file"
    printf 'Invalid SSH public key: %s\n' "$key_details" >&2
    return 1
  fi
  rm -f "$key_file"

  read -r _ fingerprint _ <<<"$key_details"
  printf '%s\n' "$fingerprint"
}

sulion_validate_key_line() {
  local key_line=${1%$'\r'}

  if [[ ! "$key_line" =~ ^ssh-ed25519[[:space:]]+[A-Za-z0-9+/=]+([[:space:]].*)?$ ]]; then
    printf 'Expected one unrestricted ssh-ed25519 public key.\n' >&2
    return 1
  fi
  sulion_key_fingerprint "$key_line" >/dev/null
}

sulion_key_from_file() {
  local source_file=$1
  local destination_file=$2
  local line
  local -a keys=()

  if [[ ! -f "$source_file" ]]; then
    printf 'Public-key file does not exist: %s\n' "$source_file" >&2
    return 1
  fi

  while IFS= read -r line || [[ -n "$line" ]]; do
    line=${line%$'\r'}
    [[ -z "$line" || "$line" == \#* ]] && continue
    keys+=("$line")
  done <"$source_file"

  if [[ ${#keys[@]} -ne 1 ]]; then
    printf 'Public-key file must contain exactly one non-comment key; found %s.\n' "${#keys[@]}" >&2
    return 1
  fi

  sulion_validate_key_line "${keys[0]}"
  printf '%s\n' "${keys[0]}" >"$destination_file"
  chmod 0600 "$destination_file"
}

sulion_key_from_agent() {
  local requested_fingerprint=$1
  local destination_file=$2
  local agent_output
  local fingerprint
  local selection
  local key_line
  local -a keys=()
  local -a fingerprints=()

  if [[ -z ${SSH_AUTH_SOCK:-} ]]; then
    printf 'No forwarded SSH agent is available; use --key-file instead.\n' >&2
    return 1
  fi
  if ! agent_output=$(ssh-add -L 2>&1); then
    printf 'Could not read keys from the forwarded SSH agent: %s\n' "$agent_output" >&2
    return 1
  fi

  while IFS= read -r key_line; do
    key_line=${key_line%$'\r'}
    [[ "$key_line" == ssh-ed25519\ * ]] || continue
    sulion_validate_key_line "$key_line"
    keys+=("$key_line")
    fingerprints+=("$(sulion_key_fingerprint "$key_line")")
  done <<<"$agent_output"

  if [[ ${#keys[@]} -eq 0 ]]; then
    printf 'The forwarded agent has no ssh-ed25519 keys.\n' >&2
    return 1
  fi

  if [[ -n "$requested_fingerprint" ]]; then
    for selection in "${!keys[@]}"; do
      if [[ "${fingerprints[$selection]}" == "$requested_fingerprint" ]]; then
        printf '%s\n' "${keys[$selection]}" >"$destination_file"
        chmod 0600 "$destination_file"
        return 0
      fi
    done
    printf 'The forwarded agent does not contain %s.\n' "$requested_fingerprint" >&2
    return 1
  fi

  if [[ ${#keys[@]} -eq 1 ]]; then
    selection=0
  else
    if [[ ! -t 0 ]]; then
      printf 'The forwarded agent has multiple keys; pass --key-fingerprint.\n' >&2
      return 1
    fi
    printf 'Forwarded ssh-ed25519 keys:\n' >&2
    for selection in "${!keys[@]}"; do
      printf '  %s) %s %s\n' \
        "$((selection + 1))" \
        "${fingerprints[$selection]}" \
        "${keys[$selection]#ssh-ed25519 * }" >&2
    done
    read -r -p "Select the administration key [1-${#keys[@]}]: " selection
    if [[ ! "$selection" =~ ^[0-9]+$ ]] || ((selection < 1 || selection > ${#keys[@]})); then
      printf 'Invalid key selection.\n' >&2
      return 1
    fi
    selection=$((selection - 1))
  fi

  fingerprint=${fingerprints[$selection]}
  printf 'Selected administration key %s.\n' "$fingerprint" >&2
  printf '%s\n' "${keys[$selection]}" >"$destination_file"
  chmod 0600 "$destination_file"
}
