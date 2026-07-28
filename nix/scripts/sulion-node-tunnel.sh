#!/usr/bin/env bash

# Brings up the node end of the control-plane tunnel.
#
# Triggered by a path unit watching the configuration `sulion-node` writes after
# an operator approves it. The node cannot configure the interface itself — it
# does not own the host network namespace — so it renders the config and this
# applies it, the same shape as delivered credentials.

set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "sulion-node-tunnel must run as root" >&2
  exit 77
fi

source_config="${SULION_TUNNEL_CONF_SOURCE:?missing SULION_TUNNEL_CONF_SOURCE}"
interface="${SULION_TUNNEL_INTERFACE:-wg0}"
applied_config="/etc/wireguard/${interface}.conf"

# Two runs must never configure the interface at once. The path unit can fire
# more than once for a single delivery, and `wg-quick up` tears the interface
# down on any error — so a collision does not just duplicate work, it destroys
# the tunnel the other run just built.
exec 9>"/run/sulion-node-tunnel.${interface}.lock"
flock 9

if [[ ! -s "${source_config}" ]]; then
  echo "no tunnel peering yet; nothing to bring up"
  exit 0
fi

# The interface line is written last, so its presence marks a complete render
# rather than a file caught mid-write.
if ! grep -q '^Endpoint = ' "${source_config}"; then
  echo "tunnel peering is incomplete; not applying"
  exit 0
fi

install -d -m 0700 -o root -g root /etc/wireguard

if cmp -s "${source_config}" "${applied_config}" && ip link show "${interface}" >/dev/null 2>&1; then
  echo "tunnel ${interface} is already current"
  exit 0
fi

install -m 0600 -o root -g root "${source_config}" "${applied_config}"

if ip link show "${interface}" >/dev/null 2>&1; then
  # syncconf keeps the interface up across a peering change, so an address or
  # key rotation does not drop live sessions on the way through.
  echo "reloading tunnel ${interface}"
  wg syncconf "${interface}" <(wg-quick strip "${applied_config}")
else
  echo "bringing up tunnel ${interface}"
  wg-quick up "${interface}"
fi

wg show "${interface}"
