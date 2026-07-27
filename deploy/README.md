# Portable deployment bundle

The root [`compose.yaml`](../compose.yaml) is Sulion's common application
definition. Host adapters are small overlays:

- `compose.truenas.yaml` preserves the current Komodo-managed, brokered-Docker
  deployment;
- `compose.standalone.yaml` runs the combined application on a generic Linux
  Docker host; and
- `compose.dedicated.yaml` runs that combined application on the dedicated
  development host with direct access to the `dev` user's rootless Docker
  daemon.

The dedicated overlay is a transitional but functional deployment. It keeps
all Sulion services on one machine until the development runtime is extracted.
After that extraction, `compose.control-plane.yaml` and
`compose.dev-node.yaml` will select the physically split services from the same
common definition.

Render a role before applying it:

```bash
docker compose \
  --env-file /var/lib/sulion/config/runtime.env \
  -f compose.yaml \
  -f deploy/compose.dedicated.yaml \
  config
```

The dedicated host uses two daemons. Root-owned system Docker runs the Sulion
application. The backend container receives only
`/run/user/7321/docker.sock`, owned by the `dev` user's rootless Docker daemon.
It does not receive `/var/run/docker.sock`. Because the backend uses host
networking and mounts `/home/dev` at the same path, ordinary Docker bind
mounts, published ports, Compose, BuildKit, and interactive commands have their
normal Docker semantics.

Copy `dedicated.env.example` to the root-readable runtime path and replace every
placeholder through the host's secret provisioning path. Do not commit the
result.
