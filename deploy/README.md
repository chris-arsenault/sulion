# Portable deployment bundle

The root [`compose.yaml`](../compose.yaml) is Sulion's common application
definition. Host adapters are small overlays:

- `compose.truenas.yaml` preserves the current Komodo-managed, brokered-Docker
  deployment;
- `compose.standalone.yaml` runs the combined application on a generic Linux
  Docker host; and
- `compose.dedicated.yaml` runs a split control process, development node, and
  ingester on the dedicated host. Only the node receives the development home
  and direct access to the `dev` user's rootless Docker daemon.

The backend image is deliberately a shared release artifact in this phase: it
contains the `sulion`, `sulion-node`, and `sulion-ingester` binaries plus the
PTY workbench. Compose selects a different entry point for each role. This
avoids duplicating the large workbench image while preserving independently
restartable processes. Phase 6 introduces the remote TrueNAS control-plane
selection; a later image optimization may make the control image smaller
without changing the runtime boundary.

Render a role before applying it:

```bash
docker compose \
  --env-file /var/lib/sulion/config/runtime.env \
  -f compose.yaml \
  -f deploy/compose.dedicated.yaml \
  config
```

The dedicated host uses two daemons. Root-owned system Docker runs Sulion.
Only `sulion-node` receives `/home/dev` and
`/run/user/7321/docker.sock`, owned by the `dev` user's rootless Docker daemon;
it never receives `/var/run/docker.sock`. The control process has no source,
workspace, transcript, or Docker mount. The ingester receives only read-only
Claude and Codex transcript mounts. Because the node uses host networking and
mounts `/home/dev` at the same path, ordinary Docker bind mounts, published
ports, Compose, BuildKit, and interactive commands retain normal Docker
semantics.

Copy `dedicated.env.example` to the root-readable runtime path and replace every
placeholder through the host's secret provisioning path. Do not commit the
result.
