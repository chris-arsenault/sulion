# ADR 0021: Claude Code Seeded Natively into the Persistent Home

## Status

Accepted, 2026-09-11.

## Context

The devenv and backend toolset images installed Claude Code with
`npm install -g` as root. Inside a PTY the `sulion` user therefore ran a
binary in a root-owned tree that it could never upgrade: `claude --update`
reported "npm global folder isn't writable" and "Insufficient permissions to
install update", and only `sudo claude --update` succeeded. The entrypoint
also wrote `~/.npmrc` once at first start, so a `prefix=` pointing at a
previous home location (`/home/dev`) persisted across every image roll,
together with a dangling `~/.local/bin/claude` symlink into that dead path.

Claude Code's own updater expects a native install under `~/.local/bin` and
`~/.local/share/claude/versions`, upgrades in the background during sessions,
and needs no privileges when that layout is owned by the running user. The
persistent home on the dedicated node already survives image rolls (ADR
0017), while the image itself is content-addressed and rolls devenv
containers only when toolset content changes.

The user declined any manual repair and asked for the shipped toolset
container to be fixed so that upgrades work automatically.

## Decision

Both toolset images run Anthropic's native installer at build time and move
the resulting versioned binary to `/opt/sulion/seed/claude/versions`. The
npm package is removed from the images.

On every start the entrypoint re-asserts `prefix=$HOME/.local` in `~/.npmrc`,
keeping other lines. When `~/.local/bin/claude` is missing or dangling it
copies the seed into `~/.local/share/claude/versions`, selects the highest
version present, and links the launcher to it. A working launcher is left
alone. From then on Claude Code's own updater upgrades the copy in the
persistent home as the `sulion` user, with no `sudo`, and the upgraded copy
survives image rolls.

The devenv and backend copies of the Dockerfile steps and entrypoint logic
are kept byte-identical by `make validate-deploy`. Codex and the TypeScript
tooling stay on npm, since Codex already self-updates into `~/.codex/packages`
with its own `~/.local/bin/codex` link.

## Alternatives considered

- **Root-owned global npm install.** The prior layout. `9f697c9` names the
  failure: the `sulion` user could never upgrade it, and a root-owned copy
  shadows any self-updating one on the PATH. `devenv/docs/toolset.md` now
  forbids `npm install -g @anthropic-ai/claude-code` for the same reason.
- **Manual repair on the host.** The first response to the symptom was a
  sequence of `rm`, `.npmrc` edit, `claude install`, and
  `npm uninstall -g`. The user rejected doing any of it by hand; a fix had
  to live in the image so every fresh or stale home converges on start.
- **A user-writable npm prefix instead of the native installer.** One of the
  updater's own suggestions (`npm config set prefix ~/.npm-global`, or a
  sudo-free npm via nvm). Rejected: the user's `~/.claude.json` already
  expected a native installation, the native layout is what the background
  updater targets without extra configuration, and a native binary seed
  costs about the same image space as the npm package's bundled binary.
- **Repairing `~/.npmrc` only once.** The pre-change entrypoint behavior,
  which is how the stale prefix survived a home-directory move. The prefix
  is now re-asserted on every start.

## Consequences

- Claude Code upgrades are the agent's own concern and happen inside the
  persistent home; this is distinct from the explicit per-session toolset
  upgrade in ADR 0017, which replaces the container, not the binary.
- The image version of Claude Code is only a first seed. Two sessions on
  the same image may run different Claude versions depending on when each
  home last self-updated.
- A home with a working `~/.local/bin/claude` is never touched by the
  entrypoint, so a user-managed launcher is respected.
- Each toolset image carries roughly 218 MB of seed binary.
- A stale `autoUpdates: false` in `~/.claude.json` does not block native
  updates when `autoUpdatesProtectedForNative` is set; only
  `DISABLE_UPDATES` or `DISABLE_AUTOUPDATER` in the environment would, and
  the repository sets neither.

## References

- Toolset documentation:
  [`../../devenv/docs/toolset.md`](../../devenv/docs/toolset.md), "Agent CLI
  Updates"; [`../deploy.md`](../deploy.md#pty-and-toolset-releases)
- Image and entrypoint: `devenv/Dockerfile`, `devenv/entrypoint.sh`
- Container lifetime this relies on:
  [ADR 0017](0017-devenv-server-owns-pty-masters.md)
- Commit: `9f697c9`
- Transcript: session `5cfa9e17-b632-4bc6-bf60-6f8b41a2334b`, turns `784`
  (symptom and manual repair), `265929` (user: fix the container; design
  and verification)
- CHANGELOG Unreleased, "PTY toolset"
