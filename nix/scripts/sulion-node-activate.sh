#!/usr/bin/env bash

# Applies configuration the node received from the control plane.
#
# Triggered by a path unit watching the delivered environment file. The node
# writes that file as root after it authenticates and an operator approves it;
# this brings the stack up around the new values so the node, ingester, and
# code-intelligence containers all see them.

set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "sulion-node-activate must run as root" >&2
  exit 77
fi

delivered_file="${SULION_DELIVERED_ENV_FILE:?missing SULION_DELIVERED_ENV_FILE}"

if [[ ! -s "${delivered_file}" ]]; then
  echo "no delivered node configuration yet; nothing to activate"
  exit 0
fi

# The digest line is written last in the node's rendering, so its presence marks
# a complete delivery rather than a placeholder file.
if ! grep -q '^SULION_NODE_CONFIG_DIGEST=' "${delivered_file}"; then
  echo "delivered node configuration is incomplete; not activating"
  exit 0
fi

echo "activating delivered node configuration"
exec systemctl reload-or-restart sulion-stack.service
