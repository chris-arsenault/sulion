#!/usr/bin/env bash

set -euo pipefail

if [[ "$(id -u)" != "0" ]]; then
  echo "sulion-node entrypoint must start as root to open its private key" >&2
  exit 77
fi

NODE_HOME="${HOME:-/home/sulion}"
install -d -o sulion -g sulion -m 0750 /run/sulion
runuser -u sulion -- env HOME="${NODE_HOME}" /opt/sulion/entrypoint.sh /usr/bin/true
exec /usr/local/bin/sulion-node
