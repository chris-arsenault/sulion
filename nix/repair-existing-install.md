# Repair an existing Sulion NixOS installation

This document is only for preserving and transitioning a machine that already
runs an earlier `sulion-enclave` generation from the root-owned
`/etc/sulion` checkout.

For a new machine—or when erasing the current installation is acceptable—use
the canonical fresh-install flow in [`README.md`](README.md). Do not perform
this repair first.

The repair does not repartition or reinstall anything, and it preserves the
existing `sulion` console/sudo password.

## Create the administration key

On the Windows, macOS, or Linux workstation that will administer the enclave,
generate a dedicated key if it does not already exist.

Linux and macOS:

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

## Import the key and test the repaired generation

At the `sulion-enclave` console, connect wired Ethernet and run:

```bash
sudo git -C /etc/sulion pull --ff-only \
  origin main

sudo nix run /etc/sulion#install-admin-key -- \
  add /home/sulion/repos/sulion-enclave.pub

sudo nixos-rebuild test \
  --flake /etc/sulion#sulion-enclave
```

The key installer validates one Ed25519 public key, prints its SHA256
fingerprint, and atomically adds it to
`/var/lib/sulion/config/ssh/authorized_keys`. Root retains write control; the
`sulion` group receives only the path traversal and public-key read access that
OpenSSH needs while authenticating as `sulion`. The file is not part of Git.

Before making the generation persistent, test from the workstation that owns
the private key.

Linux or macOS:

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

## Subsequent host updates

After this transition, the checkout is no longer required for host updates.
Test and activate the repository flake directly:

```bash
sudo nixos-rebuild test \
  --flake github:chris-arsenault/sulion/main#sulion-enclave
sudo nixos-rebuild switch \
  --flake github:chris-arsenault/sulion/main#sulion-enclave
```

`test` activates the candidate only until reboot; run `switch` only after the
SSH and host checks pass.
