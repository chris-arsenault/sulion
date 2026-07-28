# Dedicated NixOS host

This directory is both a reusable host adapter and the checked-in configuration
for the dedicated Sulion machine. It assumes only x86_64, 32 GB of RAM, a UEFI
installation, and the canonical single-user identity `sulion` at UID/GID 7321.
CPU model, core count, disk vendor, NIC name, and motherboard do not affect any
Sulion module.

## Boundary

The host runs one ordinary system Docker daemon. It runs the Sulion node-side
containers and development containers, and the single-user `sulion` identity
belongs to the `docker` group. The dedicated Compose role mounts
`/var/run/docker.sock` into `sulion-node`. This is intentionally host-level
authority on a dedicated machine; it keeps standard Docker, Compose, BuildKit,
privileged containers, networking, and local Supabase behavior intact.
The node reads the mounted socket's actual group at startup before dropping
privileges, so the Docker GID is not hard-coded and generic Linux hosts use the
same image.

The TrueNAS control plane is not present on this host, and the node-local
ingester sees only transcript directories.

Repositories live at `/home/sulion/repos` on the machine's local filesystem.
The dedicated Compose adapter preserves `/home/sulion` inside the workbench so
bind paths sent to the host's Docker daemon resolve identically. The
shared OCI image still resolves UID/GID 7321 to its portable `dev` account, but
the host login and all host-visible ownership use `sulion`; no second host user
is created. Samba exports the repository directory as `repos`; workspaces,
agent state, Docker state, and deployment secrets are not shared. SMB, SSH,
and development ports are accepted only from `192.168.66.0/24`.
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
| Node startup | enabled; skipped until its root-owned runtime files exist |

No LUKS is an explicit availability choice: this node must recover from a
power interruption without a local unlock. If physical-at-rest protection
later becomes a requirement, add a reviewed TPM-backed unlock design rather
than changing the install ad hoc. Likewise, add swap declaratively only if
measured workloads show memory pressure.

## Fresh installation

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
nix --extra-experimental-features 'nix-command flakes' run \
  github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#bootstrap-enclave \
  -- \
  --disk "$INSTALL_DISK" \
  --key-file /tmp/sulion-enclave.pub \
  --dry-run
```

Review the printed disk identity, public-key fingerprint, flake revision, and
successful Disko evaluation. Then remove `--dry-run` and execute the
installation:

```bash
nix --extra-experimental-features 'nix-command flakes' run \
  github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#bootstrap-enclave \
  -- \
  --disk "$INSTALL_DISK" \
  --key-file /tmp/sulion-enclave.pub
```

The command validates that it is running as root on an x86_64 NixOS installer
booted through UEFI, refuses partition paths and mounted disks, and requires the
full stable disk path to be typed back before erasing. It creates the declared
GPT/ESP/ext4 layout with Disko, mounts it at `/mnt`, and populates it through
the standard `nixos-install` path. It installs the exact resolved flake source,
places the public key in root-owned machine-local state, and uses the standard
NixOS password prompts for the root recovery and `sulion` console/sudo
passwords. It verifies the installed systemd OOM and time-synchronization
executables before unmounting the target. Cleartext password material is not
passed to Nix or written into the repository.

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
systemctl is-active docker.service
docker info
docker compose version
docker run --rm --memory 1g --cpus 2 docker.io/library/alpine:latest true
systemctl is-enabled sulion-stack.service
```

Expected results are hostname `sulion-enclave`, root label `nixos`, boot label
`boot`, UID/GID 7321, an active wired connection, an active system Docker
daemon, a successful limited container, and an enabled Sulion node unit. The
unit remains inactive until its runtime environment exists.

From another LAN machine, connect to `\\sulion-enclave\repos` on Windows or
`smb://sulion-enclave.local/repos` on macOS and create a test directory. On the
node it must appear under `/home/sulion/repos` owned by `sulion:sulion`.

## Apply host configuration updates

Test the repository flake before making it the boot default:

```bash
sudo nixos-rebuild test \
  --flake github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#sulion-enclave
```

Verify SSH, Docker, and Samba from another LAN machine while the test
configuration is active. Then persist it:

```bash
sudo nixos-rebuild switch \
  --flake github:chris-arsenault/sulion/feat/dedicated-nixos-dev-node#sulion-enclave
```

Replace the branch ref with `main` after merge. Application images update
independently through the node release poller below.

## Runtime files

The Nix store contains no runtime secrets. Activation creates root-controlled
paths under `/var/lib/sulion`; provision:

- `/var/lib/sulion/config/ssh/authorized_keys`, owned by `root:sulion` with
  mode `0640` and managed by `sulion-admin-key`; the `sulion` user can read
  public keys for OpenSSH authentication but cannot modify them;
- `/var/lib/sulion/config/runtime.env` with mode `0600`, using
  `deploy/dedicated.env.example` as the field contract;
- `/var/lib/sulion/node/private-key.pk8`, created automatically on the first
  node start and retained as the machine identity.

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
the machine.

The `sulion-stack.service` unit starts only the node, ingester, and
code-intelligence roles. On its first start, the node creates the root-owned
identity key and sends its public-key fingerprint to the control plane:

```bash
sudo systemctl start sulion-stack.service
sudo systemctl status sulion-stack.service
```

Open the authenticated Sulion UI, expand the stats panel, and find
`sulion-enclave` under **Development node**. It shows `pending`, the submitted
fingerprint, and an **Approve node** button. Approval stores that key; the node
reconnects automatically within a few seconds. Nothing is copied between
machines and no access token is handled manually.

The TrueNAS control plane can then redeploy independently of `sulion-node`;
browser terminals reconnect to the surviving PTY. Reloading
`sulion-stack.service` replaces node-side containers and is therefore an
explicit session-affecting action.

At runtime `sulion-node` opens the root-owned key and then drops its process,
filesystem, and PTY work to UID/GID 7321. Ordinary processes cannot read the
key directly, while files they create retain the Samba-visible
`sulion:sulion` identity. Direct Docker access is deliberately not an
isolation boundary: a Docker-capable PTY has host-root-equivalent authority and
could reach root-owned host data. The design trusts this dedicated single user;
the broker master key remains protected by staying on TrueNAS.

## Deploy a CI release

CI publishes every application image under the full Git commit SHA. After the
images and TrueNAS control-plane deployment succeed, CI advances the
`node-release` branch to that commit. `sulion-node-update.timer` polls that
branch every two minutes and deploys a changed SHA through the root-owned
Compose deployment command.

Replacing `sulion-node` terminates its PTYs. Automatic node delivery therefore
assumes running PTYs may be replaced by a successful `main` deployment. Check
the timer and its most recent attempt with:

```bash
systemctl status sulion-node-update.timer
journalctl -u sulion-node-update.service
```

The underlying command remains available for an immediate deployment:

```bash
sudo sulion-node-deploy FULL_40_CHARACTER_GIT_SHA
```

The command accepts no mutable tags. It renders the dedicated Compose role
with a temporary root-only environment, pulls the node, ingester, and
code-intelligence images, applies the stack, verifies that all three containers
are running, and then records the selected SHA in `runtime.env`. A failed
activation leaves the previous SHA recorded so the poller retries. NixOS host
changes remain a separate
`nixos-rebuild switch --flake ...#sulion-enclave` operation.

The one-time repository copy and authority switch are documented in
[`repository-cutover.md`](repository-cutover.md).

## Host checks

Docker should work as the normal user with standard options:

```bash
docker info
docker compose version
docker run --rm --memory 1g --cpus 2 docker.io/library/alpine:latest true
```

LAN clients use `\\sulion-enclave\repos` on Windows or
`smb://sulion-enclave.local/repos` on macOS.

## Tests

```bash
make validate-nix
make test-nix
```

The VM test checks stable identities, inherited POSIX ACLs, ordinary Docker
network/volume/resource-limit/bind-mount options, authenticated SMB writes and
owners, DOS xattrs, Samba macOS modules, and the boot-enabled deployment unit.

## Repairing an existing installation

The fresh-install flow above is the canonical path. If an older installation
must instead be preserved and transitioned in place, use
[`repair-existing-install.md`](repair-existing-install.md).
