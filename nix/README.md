# Dedicated NixOS host

This directory is both a reusable host adapter and the checked-in configuration
for the dedicated Sulion machine. It assumes only x86_64, 32 GB of RAM, a UEFI
installation, and the canonical single-user identity `sulion` at UID/GID 7321.
CPU model, core count, disk vendor, NIC name, and motherboard do not affect any
Sulion module.

## Boundary

The host runs two Docker daemons:

- root-owned system Docker runs the Sulion node, ingester, and code-intelligence
  containers;
- the lingering `sulion` user owns a rootless daemon at
  `/run/user/7321/docker.sock`.

The dedicated Compose role mounts only the latter into `sulion-node`. The
TrueNAS control plane is not present on this host, and the node-local ingester
sees only transcript directories. The `sulion` user is intentionally not a
member of the system `docker` group.

Repositories live at `/home/sulion/repos` on the machine's local filesystem.
The dedicated Compose adapter preserves `/home/sulion` inside the workbench so
bind paths sent to the host's rootless Docker daemon resolve identically. The
shared OCI image still resolves UID/GID 7321 to its portable `dev` account, but
the host login and all host-visible ownership use `sulion`; no second host user
is created. Samba exports the repository directory as `repos`; workspaces,
agent state, Docker state, and deployment secrets are not shared. SMB, WSD,
mDNS, SSH, and development ports are accepted only from `192.168.66.0/24`.
The node initiates its authenticated control connection outbound; this host
exposes no Sulion API or frontend.

SSH is a LAN-only break-glass administration path for rebuilding NixOS,
rotating host keys, and recovering when the Sulion control plane is unavailable.
It is not involved in normal browser terminals or node/control traffic. The
administration key belongs on the Windows, macOS, or Linux workstation from
which you will repair the host; only its public half is installed on the
enclave.

## Host contract

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
| Host name | `sulion-enclave` |
| Network | wired DHCP on `192.168.66.0/24`; optional Wi-Fi after installation |
| Time zone | UTC |
| Console | US keymap, English UTF-8 environment, no graphical desktop |
| Interactive identity | `sulion`, UID/GID 7321, wheel and NetworkManager member |
| Remote login | SSH keys only; root SSH and SSH passwords disabled |
| Node startup | `sulion-stack.service` installed but disabled |

No LUKS is an explicit availability choice: this node must recover from a
power interruption without a local unlock. If physical-at-rest protection
later becomes a requirement, add a reviewed TPM-backed unlock design rather
than changing the install ad hoc. Likewise, add swap declaratively only if
measured workloads show memory pressure.

## Complete the already-installed machine

Use this path for the machine that is running an earlier generation from
`/etc/sulion`. It does not repartition or reinstall anything, and the existing
`sulion` console/sudo password is preserved.

On the workstation that will administer the enclave, generate a dedicated key
if it does not already exist. Linux and macOS:

```bash
ssh-keygen -t ed25519 -a 64 \
  -f ~/.ssh/sulion-enclave \
  -C "$(whoami)@sulion-enclave"
```

Windows PowerShell:

```powershell
ssh-keygen.exe -t ed25519 -a 64 `
  -f "$env:USERPROFILE\.ssh\sulion-enclave" `
  -C "$env:USERNAME@sulion-enclave"
```

The file without `.pub` is the private key and never leaves that workstation.
Copy the `.pub` file itself—not its text—into the `repos` SMB share. A USB drive
is also acceptable if SMB is not configured yet.

At the `sulion-enclave` console, connect wired Ethernet and run:

```bash
sudo git -C /etc/sulion pull --ff-only \
  origin feat/dedicated-nixos-dev-node

sudo nix run /etc/sulion#install-admin-key -- \
  add /home/sulion/repos/sulion-enclave.pub

sudo nixos-rebuild test \
  --flake /etc/sulion#sulion-enclave
```

The key installer validates one Ed25519 public key, prints its SHA256
fingerprint, and atomically adds it to
`/var/lib/sulion/config/ssh/authorized_keys`. That file and its parent directory
stay owned by root and are not part of Git.

Before making the generation persistent, test from the workstation that owns
the private key. Linux or macOS:

```bash
ssh -i ~/.ssh/sulion-enclave sulion@sulion-enclave.local
```

Windows PowerShell:

```powershell
ssh.exe -i "$env:USERPROFILE\.ssh\sulion-enclave" sulion@sulion-enclave
```

Use the DHCP address if hostname discovery is unavailable. Once the connection
works, return to the enclave console and persist the tested generation:

```bash
sudo nixos-rebuild switch \
  --flake /etc/sulion#sulion-enclave
```

The copied `.pub` file is not secret, but it can now be removed from the shared
repository directory. Do not change ownership of `/etc/sulion`; it remains a
root-owned migration checkout.

After this transition, the checkout is no longer required for host updates.
Test and activate the repository flake directly:

```bash
sudo nixos-rebuild test \
  --flake github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#sulion-enclave
sudo nixos-rebuild switch \
  --flake github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#sulion-enclave
```

Replace the branch ref with `main` after the work is merged. `test` activates
the candidate only until reboot; run `switch` only after the SSH and host checks
pass.

## Automated fresh installation

The fresh-install path uses the checked-in Disko layout and one bootstrap
command. It replaces manual partition commands, cloning into `/etc`, and
transcribing public-key text.

The firmware names vary, but set the machine to UEFI boot, disable Secure Boot
and storage RAID, enable CPU virtualization, and select the NixOS USB's UEFI
boot entry. Once the installer shell appears, set a temporary installer-only
root password, then confirm UEFI and wired networking:

```bash
sudo -i
passwd
test -d /sys/firmware/efi/efivars
ip -brief address
```

The final command must show a working wired LAN address. The installation and
flake inputs require network access. The password exists only in the live
installer and disappears when it reboots.

On the administration workstation, generate the dedicated key using the
commands in the previous section. Copy only its `.pub` file to the installer,
then connect.

Linux or macOS:

```bash
scp ~/.ssh/sulion-enclave.pub root@INSTALLER_IP:/tmp/sulion-enclave.pub
ssh root@INSTALLER_IP
```

Windows PowerShell:

```powershell
scp.exe "$env:USERPROFILE\.ssh\sulion-enclave.pub" `
  "root@INSTALLER_IP:/tmp/sulion-enclave.pub"
ssh.exe root@INSTALLER_IP
```

This temporary password-authenticated connection is only to the live installer.
The installed system permits key authentication for `sulion` and disables both
SSH passwords and root SSH.

### Select the installation disk

List whole disks and their stable identifiers:

```bash
lsblk -d -o NAME,SIZE,MODEL,SERIAL,TRAN
ls -l /dev/disk/by-id
```

Set `INSTALL_DISK` to the one whole-disk identifier for the dedicated system
disk. This is the only machine-specific value that identifies a destructive
target:

```bash
export INSTALL_DISK=/dev/disk/by-id/REPLACE_WITH_THE_SYSTEM_DISK
test -b "$INSTALL_DISK"
lsblk -o NAME,SIZE,MODEL,SERIAL,TYPE,MOUNTPOINTS "$INSTALL_DISK"
```

Stop unless the final command shows exactly the disk that may be erased. Never
use `/dev/sdX`, `/dev/nvmeXnY`, a partition path, or an unresolved shell value
as `INSTALL_DISK`.

### Preview and install

First ask Disko to evaluate and print the exact plan. This builds the complete
configuration but makes no disk changes:

```bash
nix run \
  github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#bootstrap-enclave \
  -- \
  --disk "$INSTALL_DISK" \
  --key-file /tmp/sulion-enclave.pub \
  --dry-run
```

Review the printed disk identity, public-key fingerprint, flake revision, and
Disko commands. Then remove `--dry-run` and execute the installation:

```bash
nix run \
  github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#bootstrap-enclave \
  -- \
  --disk "$INSTALL_DISK" \
  --key-file /tmp/sulion-enclave.pub
```

The command validates that it is running as root on an x86_64 NixOS installer
booted through UEFI, refuses partition paths and mounted disks, and requires the
full stable disk path to be typed back before erasing. It creates the declared
GPT/ESP/ext4 layout, installs the exact resolved flake source, places the public
key in root-owned machine-local state, and prompts twice for the initial
`sulion` console/sudo password. Cleartext password material is not passed to Nix
or written into the repository.

When it reports completion:

```bash
reboot
```

After removing the installer USB, log in as `sulion` on the local console and add
the Samba password:

```bash
sudo smbpasswd -a sulion
```

The Unix and Samba accounts represent the same single user. They may use the
same human-entered password, but Samba stores its own credential verifier. Do
not configure `force user` or create separate per-client identities.

### Optional Wi-Fi

The supported installation and update path is wired Ethernet. Once the current
flake has been activated, NetworkManager and `nmtui` are available if Wi-Fi is
also desired:

```bash
sudo nmtui
nmcli -f NAME,TYPE,DEVICE connection show --active
```

Choose **Activate a connection** in `nmtui`. NetworkManager stores the selected
profile outside the Nix store and reconnects automatically after reboot.

From the client that owns the private key, verify SSH. On Linux or macOS:

```bash
ssh -i ~/.ssh/sulion-enclave sulion@sulion-enclave.local
```

On Windows PowerShell:

```powershell
ssh.exe -i "$env:USERPROFILE\.ssh\sulion-enclave" sulion@sulion-enclave
```

Use the machine's DHCP address if local hostname discovery is unavailable. Do
not enable SSH password authentication as a shortcut.

### First-boot acceptance

Run these as `sulion`:

```bash
hostnamectl hostname
findmnt /
findmnt /boot
id
nmcli -f NAME,TYPE,DEVICE connection show --active
systemctl --user is-active docker.service
docker info
docker compose version
docker run --rm --memory 1g --cpus 2 docker.io/library/alpine:latest true
systemctl is-enabled sulion-stack.service
```

Expected results are hostname `sulion-enclave`, root label `nixos`, boot label
`boot`, UID/GID 7321, an active wired connection, an active rootless Docker
daemon, a successful limited container, and a disabled Sulion node unit.
`systemctl is-enabled` intentionally exits nonzero while printing `disabled`.

From another LAN machine, connect to `\\sulion-enclave\repos` on Windows or
`smb://sulion-enclave.local/repos` on macOS and create a test directory. On the
node it must appear under `/home/sulion/repos` owned by `sulion:sulion`.

## Runtime files

The Nix store contains no runtime secrets. Activation creates root-only
directories under `/var/lib/sulion`; provision:

- `/var/lib/sulion/config/ssh/authorized_keys` with mode `0600`, managed by
  `sulion-admin-key`;
- `/var/lib/sulion/config/runtime.env` with mode `0600`, using
  `deploy/dedicated.env.example` as the field contract;
- `/var/lib/sulion/node/private-key.pk8`, owned by `root:root` with mode `0600`;
- optional Restic credentials under `/var/lib/sulion/secrets`.

The broker database and master key remain on TrueNAS. They must not be copied
to this host.

Manage additional break-glass administration keys as root. Commands accept
public-key files rather than raw keys on the command line:

```bash
sudo sulion-admin-key list
sudo sulion-admin-key add /path/to/another-workstation.pub
sudo sulion-admin-key remove SHA256:EXACT_FINGERPRINT
sudo sulion-admin-key replace /path/to/replacement.pub
```

The command refuses to remove the final key. `replace` is the recovery-safe way
to rotate the only authorized key.

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
matching OCI images and the runtime env file exist. It starts only the node,
ingester, and code-intelligence roles. Start it once; the unenrolled node will
retry safely while the TrueNAS control plane comes online:

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
  curl -fsS https://sulion.services.ahara.io/api/nodes/enrollment-tokens \
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
  enroll --control-url https://sulion.services.ahara.io \
  --token "${NODE_ENROLLMENT_TOKEN}" \
  --key /var/lib/sulion-node/private-key.pk8
unset NODE_ENROLLMENT_TOKEN
unset SULION_BACKEND_IMAGE
sudo systemctl reload sulion-stack.service
```

The returned `node_id` must equal the checked-in ID. The TrueNAS control plane
can then redeploy independently of `sulion-node`; browser terminals reconnect
to the surviving PTY. Reloading `sulion-stack.service` updates only the
node-side containers and remains session-affecting until Phase 7 adds the
deployment drain gate.

At runtime `sulion-node` opens the root-only key and then drops its process,
filesystem, PTY, and Docker work to UID/GID 7321. Agent shells therefore cannot
read or replace the enrolled node credential, while every file they create
still has the Samba-visible `sulion:sulion` identity on the host.

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

LAN clients use `\\sulion-enclave\repos` on Windows or
`smb://sulion-enclave.local/repos` on macOS.

## Backups

`sulion.backup` provides a low-priority asynchronous Restic timer for the local
repository tree, Samba identity/ACL state, and enrolled node state. It is
disabled until a TrueNAS Restic repository and root-readable credentials are
chosen. Enabling it never mounts the repository working tree from TrueNAS.

## Tests

```bash
make validate-nix
make test-nix
```

The VM test checks stable identities, inherited POSIX ACLs, rootless Docker
network/volume/resource-limit/bind-mount options, denial of the system Docker
socket, authenticated SMB writes and owners, DOS xattrs, Samba macOS modules,
and the disabled deployment unit.
