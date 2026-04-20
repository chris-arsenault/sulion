#!/usr/bin/env bash
#
# sulion SessionStart hook. Installed into a user's
# ~/.claude/settings.json under hooks.SessionStart, this script tells
# the sulion backend that a new Claude session (UUID) has started
# inside a known PTY (also UUID).
#
# Contract:
#   - SULION_PTY_ID must be in the environment. It is, because
#     the backend injects it when it forks the PTY shell.
#   - Claude's hook payload is delivered on stdin as JSON with a
#     `session_id` field.
#   - We post one JSON line to SULION_CORRELATE_SOCK (default
#     /run/sulion/correlate.sock), then exit 0.
#
# Failure MUST NOT crash Claude — every error path exits 0.

set -u

SOCK="${SULION_CORRELATE_SOCK:-/run/sulion/correlate.sock}"
PTY_ID="${SULION_PTY_ID:-}"

if [[ -z "${PTY_ID}" ]]; then
  # Not running under sulion — silent no-op.
  exit 0
fi

# Parse Claude's stdin JSON. Use jq (installed in the image) rather
# than `python3 - <<PY`, which silently breaks: `python3 -` reads
# its script from stdin, so the heredoc consumes stdin entirely and
# `json.load(sys.stdin)` in the script has nothing to read. jq reads
# stdin by default.
CLAUDE_SESSION_UUID="$(jq -r '.session_id // empty' 2>/dev/null)"

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
