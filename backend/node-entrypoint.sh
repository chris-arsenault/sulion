#!/usr/bin/env bash

set -euo pipefail

if [[ "$(id -u)" != "0" ]]; then
  echo "sulion-node entrypoint must start as root to open its private key" >&2
  exit 77
fi

install -d -o dev -g dev -m 0750 /run/sulion
runuser -u dev -- env HOME=/home/dev /opt/sulion/entrypoint.sh /usr/bin/true
exec /usr/local/bin/sulion-node
