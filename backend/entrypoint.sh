#!/usr/bin/env bash
#
# shuttlecraft container entrypoint. Runs as the `dev` user (set via
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

mkdir -p \
  "${HOME_DIR}/.claude" \
  "${HOME_DIR}/.ssh" \
  "${HOME_DIR}/.local/bin" \
  "${HOME_DIR}/.config/gh" \
  "${HOME_DIR}/repos"

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
export PATH="$HOME/.local/bin:$PATH"

# Quality-of-life
alias ll='ls -la'
alias la='ls -A'

# Prompt: user@host:cwd$
PS1='\[\e[36m\]\u@shuttlecraft\[\e[0m\]:\[\e[33m\]\w\[\e[0m\]\$ '
EOF
fi

# Pre-wire the SessionStart hook the first time the dataset sees a
# .claude/ dir. Never overwrite an existing user-customised file.
SETTINGS="${HOME_DIR}/.claude/settings.json"
if [[ ! -f "${SETTINGS}" ]]; then
  cat > "${SETTINGS}" <<'JSON'
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "/opt/shuttlecraft/hooks/session-start.sh" }
        ]
      }
    ]
  }
}
JSON
fi

exec /usr/local/bin/shuttlecraft
