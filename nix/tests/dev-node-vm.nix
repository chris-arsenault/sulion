{ nixpkgs, system }:
let
  pkgs = nixpkgs.legacyPackages.${system};
in
pkgs.testers.runNixOSTest {
  name = "sulion-enclave";
  requiredFeatures.kvm = false;

  nodes.machine =
    { ... }:
    {
      imports = [
        ../modules/sulion-node.nix
        ../modules/sulion-samba.nix
        ../modules/sulion-deployer.nix
        ../modules/sulion-backup.nix
      ];

      sulion = {
        enable = true;
        lanCidr = "192.168.0.0/16";
        samba.enable = true;
        deployer.enable = true;
      };

      virtualisation.memorySize = 3072;
      virtualisation.diskSize = 8192;
      networking.hostName = "sulion-enclave-test";
      system.stateVersion = "26.05";
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    machine.succeed("test $(hostname) = sulion-enclave-test")
    machine.succeed("test $(id -u sulion) = 7321")
    machine.succeed("test $(id -g sulion) = 7321")
    machine.succeed("id -nG sulion | tr ' ' '\\n' | grep -Fx networkmanager")
    machine.fail("id dev")
    machine.succeed("test $(stat -c %U:%G /home/sulion/repos) = sulion:sulion")
    machine.succeed("getfacl -cp /home/sulion/repos | grep -F 'default:user::rwx'")
    machine.succeed("getfacl -cp /home/sulion/repos | grep -F 'default:group::rwx'")
    machine.succeed("getfacl -cp /home/sulion/repos | grep -F 'default:other::---'")
    machine.succeed("test $(stat -c %U:%G /home/sulion/.local) = sulion:sulion")
    machine.succeed("test $(stat -c %U:%G /home/sulion/.local/share) = sulion:sulion")
    machine.succeed("test $(stat -c %U:%G /home/sulion/.local/share/docker) = sulion:sulion")
    machine.succeed("test $(stat -c %U:%G /var/lib/sulion/node) = root:root")
    machine.succeed("test $(stat -c %a /var/lib/sulion/node) = 700")
    machine.succeed("test $(stat -c %U:%G /var/lib/sulion/config/ssh) = root:sulion")
    machine.succeed("test $(stat -c %a /var/lib/sulion/config/ssh) = 710")
    machine.succeed(
      "test $(stat -c %U:%G /var/lib/sulion/config/ssh/authorized_keys) = root:sulion"
    )
    machine.succeed("test $(stat -c %a /var/lib/sulion/config/ssh/authorized_keys) = 640")
    machine.fail("id sulion-broker")
    machine.fail("test -e /var/lib/sulion/broker")
    machine.wait_for_unit("NetworkManager.service")

    machine.succeed("ssh-keygen -q -t ed25519 -N '' -f /tmp/enclave-admin")
    machine.succeed("sulion-admin-key add /tmp/enclave-admin.pub")
    machine.succeed("test $(wc -l </var/lib/sulion/config/ssh/authorized_keys) = 1")
    machine.succeed("sulion-admin-key add /tmp/enclave-admin.pub")
    machine.succeed("test $(wc -l </var/lib/sulion/config/ssh/authorized_keys) = 1")
    machine.succeed("sulion-admin-key list | grep -F SHA256:")
    machine.succeed("sudo -u sulion test -r /var/lib/sulion/config/ssh/authorized_keys")
    machine.wait_for_unit("sshd.service")
    machine.succeed(
      "ssh -o BatchMode=yes -o StrictHostKeyChecking=no "
      "-o UserKnownHostsFile=/dev/null -i /tmp/enclave-admin sulion@127.0.0.1 true"
    )

    docker_env = "XDG_RUNTIME_DIR=/run/user/7321 DOCKER_HOST=unix:///run/user/7321/docker.sock"
    as_sulion = f"{docker_env} sudo --preserve-env=XDG_RUNTIME_DIR,DOCKER_HOST -u sulion"
    try:
        machine.wait_until_succeeds(
            f"{as_sulion} systemctl --user is-active docker.service",
            timeout=60,
        )
    except Exception:
        _, user_status = machine.execute("loginctl user-status sulion --no-pager")
        _, service_status = machine.execute(
            f"{as_sulion} systemctl --user status docker.service --no-pager -l"
        )
        _, service_journal = machine.execute(
            f"{as_sulion} journalctl --user -u docker.service --no-pager -n 100"
        )
        print(user_status)
        print(service_status)
        print(service_journal)
        raise
    machine.succeed(f"tar cv --files-from /dev/null | {as_sulion} docker import - scratchimg")
    machine.succeed(f"{as_sulion} docker network create sulion-options")
    machine.succeed(f"{as_sulion} docker volume create sulion-options")
    machine.succeed(
      f"{as_sulion} docker run -d --name=options --memory=128m --cpus=0.5 "
      "--pids-limit=64 --network=sulion-options -v /home/sulion/repos:/repos "
      "-v ${pkgs.pkgsStatic.busybox}/bin/busybox:/bin/busybox:ro "
      "scratchimg /bin/busybox sleep 300"
    )
    machine.succeed(f"{as_sulion} docker ps --format '{{{{.Names}}}}' | grep -Fx options")
    machine.succeed(f"{as_sulion} docker inspect options")
    machine.succeed(f"{as_sulion} docker compose version")
    machine.fail("sudo -u sulion env DOCKER_HOST=unix:///var/run/docker.sock docker version")
    machine.succeed(f"{as_sulion} docker rm --force options")

    machine.wait_for_unit("samba-smbd.service")
    machine.succeed("(printf 'testpass\\ntestpass\\n') | smbpasswd -s -a sulion")
    machine.succeed("testparm -s | grep -F 'server min protocol = SMB3'")
    machine.succeed("testparm -s | grep -F 'vfs objects = acl_xattr fruit streams_xattr'")
    machine.succeed("smbclient //127.0.0.1/repos -U 'sulion%testpass' -c 'mkdir from-smb'")
    machine.succeed(
      "smbclient //127.0.0.1/repos -U 'sulion%testpass' "
      "-c 'put /etc/hostname from-smb/hostname.txt'"
    )
    machine.succeed("test $(stat -c %U:%G /home/sulion/repos/from-smb) = sulion:sulion")
    machine.succeed(
      "test $(stat -c %U:%G /home/sulion/repos/from-smb/hostname.txt) = sulion:sulion"
    )
    machine.succeed("getfattr -n user.DOSATTRIB /home/sulion/repos/from-smb/hostname.txt")

    machine.succeed("systemctl cat sulion-stack.service | grep -F /run/user/7321/docker.sock")
    machine.succeed(
      "systemctl cat sulion-stack.service | grep -F /var/lib/sulion/node/private-key.pk8"
    )
    machine.succeed("systemctl cat sulion-stack.service | grep -F compose.dedicated.yaml")
    machine.succeed(
      "systemctl cat sulion-stack.service | grep -F 'Sulion dedicated development-node application'"
    )
    machine.fail("systemctl is-enabled sulion-stack.service")
    machine.succeed(
      "test ! -e /etc/systemd/system/multi-user.target.wants/sulion-stack.service"
    )
  '';
}
