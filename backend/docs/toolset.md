# Sulion PTY Toolset

This file is baked into the Sulion backend image at `/opt/sulion/docs/toolset.md`.
It lives outside `/home/dev`, so it is not hidden by the workspace bind mount and
updates whenever the image updates.

## Shell

PTY sessions run as user `dev` in `/home/dev`. The workspace and repos are
bind-mounted there, so user state persists across image updates.

Useful defaults:

- `/opt/sulion/bin` is on `PATH`.
- `~/.local/bin` is on `PATH` before system paths.
- `sudo` is installed for the `dev` user without a password.

Use `sudo` only for container-local package or file work. Host resources are not
available unless compose explicitly mounts them.

## Agent Shortcuts

Sulion installs wrapper shortcuts for agent sessions:

```sh
cl
co
```

They expand to Sulion-managed agent launchers:

- `cl`: Claude Code with Sulion correlation hooks
- `co`: Codex with Sulion correlation hooks

Use the wrappers instead of invoking raw `claude` or `codex` when you want the
session to appear correctly in Sulion timelines.

## Credentials

Sulion supports exactly two credential paths.

### `with-cred`

Use this for env-based secrets:

```sh
with-cred <secret-id> -- <command...>
with-cred -- <command...>
```

Examples:

```sh
with-cred claude-api -- claude
with-cred openai-api -- codex
with-cred -- make test
```

Rules:

- `with-cred <secret-id> -- ...` injects one enabled secret bundle.
- `with-cred -- ...` injects all currently enabled `with-cred` bundles for this PTY.
- It works only after the user grants that secret to this PTY in the Sulion UI.
- Secret values are injected only into the child process, not into the shell.
- Each request is signed with this PTY's Sulion-managed key; there is no shared
  credential token for all terminals.

### `aws`

Use `aws` normally:

```sh
aws sts get-caller-identity
aws s3 ls
```

The `aws` command is a Sulion wrapper. It redeems the currently enabled AWS
secret for this PTY and then runs the real AWS CLI. If no AWS secret is enabled,
it fails with an access error.

Do not try to fetch credentials from files, AWS SSM, or external vaults from a
PTY. Ask the user to enable the needed Sulion secret for the terminal.

## Python

System Python is available as `python3`.

Preferred project workflow:

```sh
uv sync
uv run <command>
uv add <package>
```

For one-off tools:

```sh
uvx <tool> [args...]
```

Image-level Python packages available without a project env:

- `Pillow`
- `rembg[cpu]`
- `click`
- `duckdb`

Do not assume pyenv is installed. Use `uv` for Python project environments and
tool execution.

## DuckDB

DuckDB is installed globally in two forms:

```sh
duckdb
python3 -c 'import duckdb; print(duckdb.__version__)'
```

Use the CLI for quick local queries and the Python binding for scripts or
notebooks that need embedded analytical queries without provisioning a server.

## JavaScript

Node and pnpm are installed globally.

Common commands:

```sh
pnpm install
pnpm exec <tool>
pnpm test
```

User-level global npm installs go under `~/.local` so they persist on the
workspace dataset and shadow system binaries only for that user.

## Rust

Rust is installed system-wide through rustup. The default toolchain is:

```text
stable-x86_64-unknown-linux-gnu
```

`cargo`, `rustc`, `rustfmt`, and `cargo clippy` are available from a plain
shell. If another named local toolchain is broken, prefer the stable toolchain
above instead of debugging unrelated rustup state.

## .NET

`dotnet` is installed globally with SDKs for both repo families currently used
in the workspace:

- .NET 8
- .NET 10.0.100

`global.json` SDK resolution should work for projects pinned to either SDK.

## Infrastructure

Terraform is installed globally as `terraform`.

Use repo-local Terraform commands from the repo that owns the infrastructure
configuration. Do not infer or fetch cloud credentials directly; use Sulion
credential grants when a command needs access.

## Containers

Rootless Podman is installed. `docker` is a compatibility shim to `podman`.

Use:

```sh
podman ps
docker ps
```

Both commands target the container-local rootless Podman setup, not a host
Docker socket.

## Other Tools

Common tools installed in the image:

- `git`
- `git-lfs`
- `gh`
- `jq`
- `rg`
- `fd`
- `vim`
- `nano`
- `less`
- `psql`
- `socat`
- `ssh`

Prefer `rg` for text search and `fd` or `rg --files` for file discovery.
