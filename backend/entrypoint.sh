#!/usr/bin/env bash
#
# sulion container entrypoint. Runs as the `dev` user (set via
# USER in the Dockerfile). Self-provisions the dataset layout so the
# TrueNAS operator only has to create the dataset and chown it to
# the dev UID — nothing else.
#
# All state lives under $HOME (which is bind-mounted from the dataset
# root), so anything created here persists across container restarts.
#
# NOTE: this script seeds persistent CONFIG FILES only. It does not
# encode one-time setup actions (no ssh-keygen, no claude login, no
# gh auth). Those are actions the user runs once inside a PTY — they
# do not belong in the shell init path.

set -euo pipefail

HOME_DIR="${HOME:-/home/dev}"
export PATH="${HOME_DIR}/.local/bin:/opt/sulion/bin:${PATH}"

mkdir -p \
  "${HOME_DIR}/.claude" \
  "${HOME_DIR}/.codex" \
  "${HOME_DIR}/.ssh" \
  "${HOME_DIR}/.local/bin" \
  "${HOME_DIR}/.config/gh" \
  "${HOME_DIR}/repos" \
  "${HOME_DIR}/workspaces"

# SSH refuses to read keys from directories that aren't 0700.
chmod 0700 "${HOME_DIR}/.ssh"

# Persistent npm config: user-scope global installs land in ~/.local
# rather than /usr/local (which the non-root dev user can't write).
# Write once, then leave alone.
if [[ ! -f "${HOME_DIR}/.npmrc" ]]; then
  cat > "${HOME_DIR}/.npmrc" <<EOF
prefix=${HOME_DIR}/.local
EOF
fi

# Minimal bashrc seed: PATH for user-local installs, enough aliases
# to stop a new PTY feeling bare. Never overwrites an existing file
# (the user may have customised theirs). Only shell CONFIG — no
# one-time bootstrap actions belong in here.
if [[ ! -f "${HOME_DIR}/.bashrc" ]]; then
  cat > "${HOME_DIR}/.bashrc" <<'EOF'
# History
HISTSIZE=10000
HISTFILESIZE=20000
HISTCONTROL=ignoreboth
shopt -s histappend

# PATH: user-local installs take precedence over system binaries.
export PATH="$HOME/.local/bin:/opt/sulion/bin:$PATH"

# Quality-of-life
alias ll='ls -la'
alias la='ls -A'

# Prompt: user@host:cwd$
PS1='\[\e[36m\]\u@sulion\[\e[0m\]:\[\e[33m\]\w\[\e[0m\]\$ '
EOF
fi

# Make sure Sulion's correlation and activity hooks are registered in
# .claude/settings.json. Claude itself writes to this file during `claude
# login`, so merge idempotently and preserve all user customisations.
SETTINGS="${HOME_DIR}/.claude/settings.json"
SESSION_HOOK_CMD="/opt/sulion/hooks/session-start.sh"
ACTIVITY_HOOK_CMD="/opt/sulion/hooks/activity-hook.sh"

if [[ ! -f "${SETTINGS}" ]]; then
  cat > "${SETTINGS}" <<JSON
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "${SESSION_HOOK_CMD}" }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "${ACTIVITY_HOOK_CMD}" }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          { "type": "command", "command": "${ACTIVITY_HOOK_CMD}" }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "${ACTIVITY_HOOK_CMD}" }
        ]
      }
    ],
    "Notification": [
      {
        "hooks": [
          { "type": "command", "command": "${ACTIVITY_HOOK_CMD}" }
        ]
      }
    ]
  }
}
JSON
else
  TMP="$(mktemp)"
  if jq \
      --arg session_cmd "${SESSION_HOOK_CMD}" \
      --arg activity_cmd "${ACTIVITY_HOOK_CMD}" '
        def add_hook($event; $cmd):
          .hooks[$event] //= []
          | if any(.hooks[$event][]?.hooks[]?; .command == $cmd)
            then .
            else .hooks[$event] += [{"hooks": [{"type": "command", "command": $cmd}]}]
            end;
        .hooks //= {}
        | add_hook("SessionStart"; $session_cmd)
        | add_hook("UserPromptSubmit"; $activity_cmd)
        | add_hook("PreToolUse"; $activity_cmd)
        | add_hook("Stop"; $activity_cmd)
        | add_hook("Notification"; $activity_cmd)
      ' "${SETTINGS}" > "${TMP}"; then
    mv "${TMP}" "${SETTINGS}"
  else
    rm -f "${TMP}"
  fi
fi

exec /usr/local/bin/sulion
