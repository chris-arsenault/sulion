#!/usr/bin/env bash

set -euo pipefail

usage() {
  echo "usage: sulion-node-deploy <git-commit-sha>" >&2
  exit 64
}

if [[ "$#" -ne 1 ]]; then
  usage
fi
if [[ "$(id -u)" -ne 0 ]]; then
  echo "sulion-node-deploy must run as root" >&2
  exit 77
fi

tag="$1"
if [[ ! "${tag}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "release must be a full lowercase 40-character Git commit SHA" >&2
  exit 64
fi

env_file="${SULION_DEPLOY_ENV_FILE:?missing SULION_DEPLOY_ENV_FILE}"
source_root="${SULION_DEPLOY_SOURCE:?missing SULION_DEPLOY_SOURCE}"
compose_bin="${SULION_COMPOSE_BIN:?missing SULION_COMPOSE_BIN}"

if [[ ! -f "${env_file}" ]]; then
  echo "runtime environment does not exist: ${env_file}" >&2
  exit 66
fi
if [[ "$(grep -c '^IMAGE_TAG=' "${env_file}")" -ne 1 ]]; then
  echo "${env_file} must contain exactly one IMAGE_TAG entry" >&2
  exit 65
fi

candidate="$(mktemp "${env_file}.candidate.XXXXXX")"
cleanup() {
  rm -f "${candidate}"
}
trap cleanup EXIT
chmod 0600 "${candidate}"
sed "s/^IMAGE_TAG=.*/IMAGE_TAG=${tag}/" "${env_file}" > "${candidate}"

compose=(
  "${compose_bin}"
  --env-file "${candidate}"
  -f "${source_root}/compose.yaml"
  -f "${source_root}/deploy/compose.dedicated.yaml"
)

echo "Validating node release ${tag}"
"${compose[@]}" config --quiet

echo "Pulling node-side images"
"${compose[@]}" pull node ingester code-intel

echo "Activating node release"
"${compose[@]}" up -d --remove-orphans

for container in sulion-node sulion-ingester sulion-code-intel; do
  if [[ "$(docker inspect --format '{{.State.Running}}' "${container}" 2>/dev/null)" != "true" ]]; then
    echo "${container} is not running after activation" >&2
    exit 1
  fi
done

install -m 0600 -o root -g root "${candidate}" "${env_file}"
echo "Sulion node release ${tag} is running"
