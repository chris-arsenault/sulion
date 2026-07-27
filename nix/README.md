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

The dedicated Compose role mounts only the latter into `sulion-node`. The
control process has no development filesystem or Docker mount, and the
node-local ingester sees only transcript directories. The `dev` user is
intentionally not a member of the system `docker` group.

Repositories live at `/home/dev/repos` on the machine's local filesystem.
Samba exports that directory as `repos`; workspaces, agent state, Docker state,
the broker key, and deployment secrets are not shared. SMB, WSD, mDNS, the
frontend, SSH, and development ports are accepted only from
`192.168.66.0/24`. The system-Docker bridge has the stable name `sulion0`, and
only that interface may reach the host-network backend on port 8080.

## Fresh installation contract

This is the canonical bare-metal layout for the dedicated machine. Use the
[NixOS 26.05 minimal x86_64 ISO](https://nixos.org/download/) and its manual
installer, not the graphical installer. The repository supplies the complete
system configuration, so do not create a parallel `/etc/nixos/configuration.nix`
or improvise different storage choices during installation.

| Parameter | Chosen value |
| --- | --- |
| Firmware | UEFI only; GPT partition table; Secure Boot disabled |
| Storage controller | Native AHCI or NVMe mode; firmware RAID disabled |
| Virtualization | Intel VT-x/VT-d or AMD-V/IOMMU enabled |
| Bootloader | systemd-boot in a 1 GiB EFI System Partition |
| Root filesystem | ext4, label `nixos`, 1% reserved blocks, POSIX ACLs |
| Other filesystems | none; `/home`, repositories, Nix, and Docker remain on root |
| Disk encryption | none, so the dedicated node can reboot unattended |
| Swap and hibernation | none; the machine has 32 GB RAM and does not hibernate |
| Host name | `sulion-node` |
| Network | wired DHCP on `192.168.66.0/24`; no static address in the host |
| Time zone | UTC |
| Console | US keymap, English UTF-8 environment, no graphical desktop |
| Interactive identity | `dev`, UID/GID 7321, wheel member |
| Remote login | SSH keys only; root SSH and SSH passwords disabled |
| Application startup | `sulion-stack.service` installed but disabled |

No LUKS is an explicit availability choice: this node must recover from a
power interruption without a local unlock. If physical-at-rest protection
later becomes a requirement, add a reviewed TPM-backed unlock design rather
than changing the install ad hoc. Likewise, add swap declaratively only if
measured workloads show memory pressure.

The firmware names vary, but set the machine to UEFI boot, disable Secure Boot
and storage RAID, enable CPU virtualization, and select the NixOS USB's UEFI
boot entry. Once the installer shell appears, become root and confirm that it
really booted through UEFI:

```bash
sudo -i
test -d /sys/firmware/efi/efivars
ip -brief address
```

The second command must show a working LAN address. The installation and flake
input both require network access.

### Select the installation disk

List whole disks and their stable identifiers:

```bash
lsblk -d -o NAME,SIZE,MODEL,SERIAL,TRAN
ls -l /dev/disk/by-id
```

Set `INSTALL_DISK` to the one whole-disk identifier for the dedicated system
disk. This is the only machine-specific installation value:

```bash
export INSTALL_DISK=/dev/disk/by-id/REPLACE_WITH_THE_SYSTEM_DISK
test -b "$INSTALL_DISK"
lsblk -o NAME,SIZE,MODEL,SERIAL,TYPE,MOUNTPOINTS "$INSTALL_DISK"
```

Stop unless the final command shows exactly the disk that may be erased. Never
use `/dev/sdX`, `/dev/nvmeXnY`, a partition path, or an unresolved shell value
as `INSTALL_DISK`.

### Erase, partition, and format

The following block permanently erases `INSTALL_DISK`. It creates a 1 GiB
FAT32 EFI System Partition and gives the remaining space to ext4:

```bash
wipefs --all "$INSTALL_DISK"
parted --script "$INSTALL_DISK" -- mklabel gpt
parted --script "$INSTALL_DISK" -- mkpart ESP fat32 1MiB 1025MiB
parted --script "$INSTALL_DISK" -- set 1 esp on
parted --script "$INSTALL_DISK" -- mkpart nixos ext4 1025MiB 100%
partprobe "$INSTALL_DISK"
udevadm settle

export INSTALL_ESP="${INSTALL_DISK}-part1"
export INSTALL_ROOT="${INSTALL_DISK}-part2"
test -b "$INSTALL_ESP"
test -b "$INSTALL_ROOT"

mkfs.fat -F 32 -n boot "$INSTALL_ESP"
mkfs.ext4 -F -L nixos -m 1 "$INSTALL_ROOT"

mount /dev/disk/by-label/nixos /mnt
mkdir -p /mnt/boot
mount -o umask=0077 /dev/disk/by-label/boot /mnt/boot
findmnt /mnt
findmnt /mnt/boot
```

The checked-in hardware leaf consumes those `nixos` and `boot` filesystem
labels and includes the normal NVMe, AHCI, SATA, and USB storage modules. Do
not replace it with generated UUID-based configuration for this layout.

### Install the repository-defined system

Clone this branch directly into the target filesystem, then install its
`sulion-dedicated` flake output:

```bash
mkdir -p /mnt/etc
nix-shell -p git --run \
  'git clone --branch feat/dedicated-nixos-dev-node --single-branch https://github.com/chris-arsenault/sulion.git /mnt/etc/sulion'

nixos-install --flake /mnt/etc/sulion#sulion-dedicated
nixos-enter --root /mnt -c 'passwd dev'
sync
reboot
```

`nixos-install` prompts for a root recovery password; set one even though root
cannot log in over SSH. The separate `passwd dev` command creates the local
console and sudo password before reboot.

After removing the installer USB, log in as `dev` on the local console and add
the Samba password:

```bash
sudo smbpasswd -a dev
```

The Unix and Samba accounts represent the same single user. They may use the
same human-entered password, but Samba stores its own credential verifier. Do
not configure `force user` or create separate per-client identities.

The first installation intentionally requires the local console because the
repository cannot contain a personal SSH public key. Before operating the node
remotely, add the chosen public key to
`users.users.dev.openssh.authorizedKeys.keys` in
`nix/hosts/dedicated/default.nix`, then apply it locally:

```bash
cd /etc/sulion
sudo nixos-rebuild test --flake .#sulion-dedicated
sudo nixos-rebuild switch --flake .#sulion-dedicated
```

Do not enable SSH password authentication as a shortcut.

### First-boot acceptance

Run these as `dev`:

```bash
hostnamectl hostname
findmnt /
findmnt /boot
id
systemctl --user is-active docker.service
docker info
docker compose version
docker run --rm --memory 1g --cpus 2 docker.io/library/alpine:latest true
systemctl is-enabled sulion-stack.service
```

Expected results are hostname `sulion-node`, root label `nixos`, boot label
`boot`, UID/GID 7321, an active rootless Docker daemon, a successful limited
container, and a disabled Sulion application unit. `systemctl is-enabled`
intentionally exits nonzero while printing `disabled`.

From another LAN machine, connect to `\\sulion-node\repos` on Windows or
`smb://sulion-node.local/repos` on macOS and create a test directory. On the
node it must appear under `/home/dev/repos` owned by `dev:dev`.

## Runtime files

The Nix store contains no runtime secrets. Activation creates root-only
directories under `/var/lib/sulion`; provision:

- `/var/lib/sulion/config/runtime.env` with mode `0600`, using
  `deploy/dedicated.env.example` as the field contract;
- `/var/lib/sulion/node/private-key.pk8`, owned by `root:root` with mode `0600`;
- `/var/lib/sulion/broker/master.key` as exactly 32 random bytes, owned by
  `7322:7322` with mode `0400`;
- optional Restic credentials under `/var/lib/sulion/secrets`.

The checked-in dedicated identity is
`019d4f28-88ac-7a80-932c-b0f53a0708f4`. Keep that value in
`SULION_NODE_ID`; the node key, rather than hardware properties, authenticates
the machine. Generate the key once with the exact backend image selected by
`IMAGE_TAG`:

```bash
SULION_BACKEND_IMAGE="$(
  sudo bash -c '
    set -a
    source /var/lib/sulion/config/runtime.env
    printf "%s/backend:%s" "$SULION_IMAGE_REGISTRY" "$IMAGE_TAG"
  '
)"
sudo docker pull "${SULION_BACKEND_IMAGE}"
sudo docker run --rm --user root \
  -v /var/lib/sulion/node:/var/lib/sulion-node \
  --entrypoint /usr/local/bin/sulion-node \
  "${SULION_BACKEND_IMAGE}" \
  keygen --output /var/lib/sulion-node/private-key.pk8
```

The `sulion-stack.service` unit is installed but deliberately disabled until
matching OCI images and the runtime env file exist. Start it once; the
unenrolled node will retry safely while the control plane comes online:

```bash
sudo systemctl start sulion-stack.service
sudo systemctl status sulion-stack.service
```

From an authenticated LAN shell, mint a five-minute token targeted to the
checked-in node ID. Supply a current Sulion access token only to this process;
do not add it to `runtime.env`:

```bash
read -rsp "Sulion access token: " SULION_ADMIN_ACCESS_TOKEN
echo
NODE_ENROLLMENT_TOKEN="$(
  curl -fsS http://sulion-node:30080/api/nodes/enrollment-tokens \
    -H "Authorization: Bearer ${SULION_ADMIN_ACCESS_TOKEN}" \
    -H "Content-Type: application/json" \
    --data '{"display_name":"dedicated NixOS node","target_node_id":"019d4f28-88ac-7a80-932c-b0f53a0708f4","ttl_seconds":300}' |
    jq -er .token
)"
unset SULION_ADMIN_ACCESS_TOKEN
```

Complete enrollment on the NixOS host using the same private-key path:

```bash
sudo docker run --rm --network host --user root \
  -v /var/lib/sulion/node:/var/lib/sulion-node:ro \
  --entrypoint /usr/local/bin/sulion-node \
  "${SULION_BACKEND_IMAGE}" \
  enroll --control-url http://127.0.0.1:8080 \
  --token "${NODE_ENROLLMENT_TOKEN}" \
  --key /var/lib/sulion-node/private-key.pk8
unset NODE_ENROLLMENT_TOKEN
unset SULION_BACKEND_IMAGE
sudo systemctl reload sulion-stack.service
```

The returned `node_id` must equal the checked-in ID. Thereafter the system
deployer can update or restart the control container independently of
`sulion-node`; browser terminals reconnect to the surviving PTY.

At runtime `sulion-node` opens the root-only key and then drops its process,
filesystem, PTY, and Docker work to UID/GID 7321. Agent shells therefore cannot
read or replace the enrolled node credential, while every file they create
still has the Samba-visible `dev:dev` identity.

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
