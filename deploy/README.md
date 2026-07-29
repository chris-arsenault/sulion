# Portable deployment bundle

The root [`compose.yaml`](../compose.yaml) is both Sulion's common service graph
and the production TrueNAS control-plane selection. Host adapters are small
overlays:

- no overlay runs frontend, control API, broker, and retrieval on TrueNAS;
- `compose.standalone.yaml` runs the combined application on a generic
  Linux host;
- `compose.truenas.yaml` adds the TrueNAS brokered-Docker policy when used
  together with the standalone overlay; and
- `compose.truenas-standalone.yaml` merges those files into the single Compose
  entry point required by Komodo; and
- `compose.dedicated.yaml` runs only the development node, ingester, and code
  intelligence on the dedicated host. Only the node receives the development
  home and direct access to the dedicated host's Docker daemon.

`platform.yml` is the TrueNAS topology selector. Keep
`truenas_compose_path: compose.yaml` for the split control plane. To select
combined TrueNAS host operation, change only that value to
`deploy/compose.truenas-standalone.yaml` and push the commit to `main`. Both
paths are rendered by CI on every change.

The backend image is deliberately a shared release artifact in this phase: it
contains the `sulion`, `sulion-node`, and `sulion-ingester` binaries plus the
PTY workbench. Compose selects a different entry point for each role. This
avoids duplicating the large workbench image while preserving independently
restartable processes. A later image optimization may make the control image
smaller without changing the runtime boundary.

Render a role before applying it. The dedicated host keeps host-owned and
control-plane-delivered values in separate files, delivered last so it wins:

```bash
docker compose \
  --env-file /var/lib/sulion/config/bootstrap.env \
  --env-file /var/lib/sulion/node/delivered.env \
  -f compose.yaml \
  -f deploy/compose.dedicated.yaml \
  config
```

The dedicated host uses one system Docker daemon for both Sulion services and
development containers. Only `sulion-node` receives `/home/sulion` and
`/var/run/docker.sock`. This is intentional on the single-user dedicated host.
There is no control process, broker, retrieval service, frontend, or
constrained runner on this host. The ingester receives only read-only Claude
and Codex transcript mounts. Because the node uses host networking and mounts
`/home/sulion` at the same path, ordinary Docker bind
mounts, published ports, Compose, BuildKit, and interactive commands retain
normal Docker semantics.

On the NixOS enclave neither file is written by hand: `sulion-node-bootstrap`
generates the host half, and `sulion-node` writes the delivered half itself
once an operator approves its identity key in the UI. See
[`../nix/README.md`](../nix/README.md).

`dedicated.env.example` documents the complete field contract for a generic
Linux node that is *not* using the NixOS bootstrap. Such a host must be given
the shared credentials some other way; do not commit the result.
