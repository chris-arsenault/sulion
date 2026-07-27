# Dedicated NixOS host

This directory is both a reusable host adapter and the checked-in configuration
for the dedicated Sulion machine. It assumes only x86_64, 32 GB of RAM, a UEFI
installation, and the canonical single-user identity `dev` at UID/GID 7321.
CPU model, core count, disk vendor, NIC name, and motherboard do not affect any
Sulion module.

## Boundary

The host runs two Docker daemons:

- root-owned system Docker runs the Sulion application;
- the lingering `dev` user owns a rootless daemon at
  `/run/user/7321/docker.sock`.

The dedicated Compose role mounts only the latter into the workbench. The `dev`
user is intentionally not a member of the system `docker` group.

Repositories live at `/home/dev/repos` on the machine's local filesystem.
Samba exports that directory as `repos`; workspaces, agent state, Docker state,
the broker key, and deployment secrets are not shared. SMB, WSD, mDNS, the
frontend, SSH, and development ports are accepted only from
`192.168.66.0/24`. The system-Docker bridge has the stable name `sulion0`, and
only that interface may reach the host-network backend on port 8080.

## Installation leaf

The checked-in hardware leaf uses two filesystem labels:

- `nixos` — ext4 root with ACL support;
- `boot` — the UEFI system partition mounted at `/boot`.

That is a portable installation contract, not a Dell-specific configuration.
If the installed layout uses different labels, RAID, encryption, or another
filesystem, replace only
`nix/hosts/dedicated/hardware-configuration.nix` with the output from:

```bash
sudo nixos-generate-config --root /mnt
```

The rest of the host configuration remains unchanged.

Before switching, add an SSH public key for `dev` in the dedicated host
configuration or plan to finish from the local console. Password SSH and root
SSH are disabled.

Evaluate and activate from a copy of this repository:

```bash
sudo nixos-rebuild test --flake .#sulion-dedicated
sudo nixos-rebuild switch --flake .#sulion-dedicated
```

Then provision the two interactive identities:

```bash
sudo passwd dev
sudo smbpasswd -a dev
```

The Unix and Samba accounts represent the same single user. Do not configure
`force user` or create separate per-client identities.

## Runtime files

The Nix store contains no runtime secrets. Activation creates root-only
directories under `/var/lib/sulion`; provision:

- `/var/lib/sulion/config/runtime.env` with mode `0600`, using
  `deploy/dedicated.env.example` as the field contract;
- `/var/lib/sulion/broker/master.key` as exactly 32 random bytes, owned by
  `7322:7322` with mode `0400`;
- optional Restic credentials under `/var/lib/sulion/secrets`.

The `sulion-stack.service` unit is installed but deliberately disabled during
this phase. Do not start it until matching OCI images for this branch have been
published. Once they exist:

```bash
sudo systemctl start sulion-stack.service
sudo systemctl status sulion-stack.service
```

## Host checks

Rootless Docker should work as the normal user with standard options:

```bash
docker info
docker compose version
docker run --rm --memory 1g --cpus 2 docker.io/library/alpine:latest true
```

The system daemon remains inaccessible:

```bash
DOCKER_HOST=unix:///var/run/docker.sock docker info
# permission denied
```

LAN clients use `\\sulion-node\repos` on Windows or
`smb://sulion-node.local/repos` on macOS.

## Backups

`sulion.backup` provides a low-priority asynchronous Restic timer for the local
repository tree, Samba identity/ACL state, and broker state. It is disabled
until a TrueNAS Restic repository and root-readable credentials are chosen.
Enabling it never mounts the repository working tree from TrueNAS.

## Tests

```bash
make validate-nix
make test-nix
```

The VM test checks stable identities, inherited POSIX ACLs, rootless Docker
network/volume/resource-limit/bind-mount options, denial of the system Docker
socket, authenticated SMB writes and owners, DOS xattrs, Samba macOS modules,
and the disabled deployment unit.
