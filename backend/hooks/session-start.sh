#!/usr/bin/env bash
#
# shuttlecraft SessionStart hook. Installed into a user's
# ~/.claude/settings.json under hooks.SessionStart, this script tells
# the shuttlecraft backend that a new Claude session (UUID) has started
# inside a known PTY (also UUID).
#
# Contract:
#   - SHUTTLECRAFT_PTY_ID must be in the environment. It is, because
#     the backend injects it when it forks the PTY shell.
#   - Claude's hook payload is delivered on stdin as JSON with a
#     `session_id` field.
#   - We post one JSON line to SHUTTLECRAFT_CORRELATE_SOCK (default
#     /run/shuttlecraft/correlate.sock), then exit 0.
#
# Failure MUST NOT crash Claude — every error path exits 0.

set -u

SOCK="${SHUTTLECRAFT_CORRELATE_SOCK:-/run/shuttlecraft/correlate.sock}"
PTY_ID="${SHUTTLECRAFT_PTY_ID:-}"

if [[ -z "${PTY_ID}" ]]; then
  # Not running under shuttlecraft — silent no-op.
  exit 0
fi

# Parse Claude's stdin JSON. python3 is present in the backend image;
# if it's missing for some reason, degrade gracefully.
CLAUDE_SESSION_UUID="$(
  python3 - <<'PY' 2>/dev/null || true
import json, sys
try:
    d = json.load(sys.stdin)
    print(d.get("session_id", ""), end="")
except Exception:
    pass
PY
)"

if [[ -z "${CLAUDE_SESSION_UUID}" ]]; then
  exit 0
fi

# Deliver with a short connect timeout so a dead socket doesn't hang
# Claude. `socat` is the reliable tool for Unix-socket write-from-bash,
# but fall back to python if it's not installed.
PAYLOAD="{\"pty_id\":\"${PTY_ID}\",\"claude_session_uuid\":\"${CLAUDE_SESSION_UUID}\"}"

if command -v socat >/dev/null 2>&1; then
  printf '%s\n' "$PAYLOAD" | timeout 2s socat - "UNIX-CONNECT:${SOCK}" >/dev/null 2>&1 || true
else
  python3 - "$SOCK" "$PAYLOAD" <<'PY' >/dev/null 2>&1 || true
import socket, sys
sock_path, payload = sys.argv[1], sys.argv[2]
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2.0)
    s.connect(sock_path)
    s.sendall((payload + "\n").encode())
    try:
        s.recv(64)
    except Exception:
        pass
    s.close()
except Exception:
    pass
PY
fi

exit 0
