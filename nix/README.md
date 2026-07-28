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
exposes no Sulion API or frontend. All of its outbound Sulion traffic — control
channel, secret broker, and retrieval — stays on the LAN and, past enrollment,
inside the WireGuard tunnel. Nothing this host sends reaches the public
hostname.

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
| Node startup | enabled at boot; waits in the UI for one approval |

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
unit starts at boot and waits for its one approval in the UI.

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

The Nix store contains no runtime secrets, and **nothing here is provisioned by
hand**. Activation creates root-controlled paths under `/var/lib/sulion` and
the machine fills them in itself:

- `/var/lib/sulion/config/bootstrap.env`, written by
  `sulion-node-bootstrap.service` at every boot. It holds only values this
  repository already knows — node ID, control URL, host paths, registry, dev
  port range — plus the current release SHA and a machine-local code
  intelligence token. No shared credential appears here.
- `/var/lib/sulion/config/code-intel.token`, generated once with mode `0600`.
  The node and code-intelligence containers talk over this host's loopback
  only, so their shared secret never leaves the machine and is never compared
  against the control plane.
- `/var/lib/sulion/node/private-key.pk8`, created on the first node start and
  retained as the machine identity.
- `/var/lib/sulion/node/control-key.pub`, the control plane this node paired
  with. Written on first pairing and required to match on every connection
  after, so a machine that later answers on the same address cannot take the
  node over. If the control plane is legitimately replaced, delete this file to
  re-enter first-pairing and approve the node again.
- `/var/lib/sulion/node/tunnel-private.key`, this host's WireGuard key,
  generated on first boot. Its public half is offered during pairing and
  approved along with the identity key.
- `/var/lib/sulion/node/wg0.conf`, rendered by `sulion-node` from the peering
  the control plane granted. `sulion-node-tunnel.path` applies it, bringing the
  interface up with `wg-quick` or reloading it in place with `wg syncconf` so a
  rotation does not drop live sessions.
- `/var/lib/sulion/node/delivered.env`, written by `sulion-node` itself after
  an operator approves it and **only over the tunnel**. This is where the database credentials, retrieval
  token, and broker registration token arrive, over the authenticated node
  channel. Mode `0600`, root-only, and unreadable by the `sulion` user.
- `/var/lib/sulion/config/ssh/authorized_keys`, owned by `root:sulion` with
  mode `0640` and managed by `sulion-admin-key`; the `sulion` user can read
  public keys for OpenSSH authentication but cannot modify them.

Compose reads `bootstrap.env` and `delivered.env` as two `--env-file`
arguments, delivered last so control-plane values win. They are kept apart on
purpose: the host owns one, the control plane owns the other, and neither
rewrites the other in place.

The broker database and master key remain on TrueNAS. They must not be copied
to this host, and the control plane does not forward them.

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
`019d4f28-88ac-7a80-932c-b0f53a0708f4`. The node key, rather than hardware
properties, authenticates the machine.

## Bringing a node up

There is one step, and it happens in the browser.

Boot the machine. `sulion-node-bootstrap.service` writes the host environment
and resolves the current release; `sulion-stack.service` starts the node,
ingester, and code-intelligence roles. The node creates its root-owned identity
key and its WireGuard key, connects to the control plane's LAN-bound node port,
and submits its fingerprint. It has no credentials yet, so it waits.

Open the authenticated Sulion UI, expand the stats panel, and find
`sulion-enclave` under **Development node**. It shows `pending`, the submitted
fingerprint, and an **Approve node** button. Press it.

Approval accepts both keys and allocates the node a tunnel address. On the next
reconnect — a few seconds — the control plane returns the peering. The node
writes `wg0.conf`, `sulion-node-tunnel.path` brings the interface up, and the
node reconnects over the tunnel. Only then are credentials delivered: they are
refused on any connection that did not arrive through it. The node writes
`delivered.env`, `sulion-node-activate.path` notices, and the stack comes up
around the new values.

Nothing is copied between machines, no credential is typed on the enclave, and
nothing but public keys ever crosses the cleartext hop.

To watch it happen:

```bash
journalctl -fu sulion-stack.service -u sulion-node-activate.service \
  -u sulion-node-tunnel.service
docker logs -f sulion-node
sudo wg show wg0
```

Before approval the node logs `awaiting operator approval` every few seconds.
After approval it logs `wrote delivered node runtime configuration`, then
`delivered configuration is newer than this container's environment` once,
before the host rebuilds it.

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
are running, and then records the selected SHA in `bootstrap.env`. A failed
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
