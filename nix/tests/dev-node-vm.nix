{ nixpkgs, system }:
let
  pkgs = nixpkgs.legacyPackages.${system};
in
pkgs.testers.runNixOSTest {
  name = "sulion-dev-node";
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
      networking.hostName = "sulion-node-test";
      system.stateVersion = "26.05";
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")

    machine.succeed("test $(id -u dev) = 7321")
    machine.succeed("test $(id -g dev) = 7321")
    machine.succeed("test $(stat -c %U:%G /home/dev/repos) = dev:dev")
    machine.succeed("getfacl -cp /home/dev/repos | grep -F 'default:user::rwx'")
    machine.succeed("getfacl -cp /home/dev/repos | grep -F 'default:group::rwx'")
    machine.succeed("getfacl -cp /home/dev/repos | grep -F 'default:other::---'")
    machine.succeed("test $(stat -c %U:%G /home/dev/.local) = dev:dev")
    machine.succeed("test $(stat -c %U:%G /home/dev/.local/share) = dev:dev")
    machine.succeed("test $(stat -c %U:%G /home/dev/.local/share/docker) = dev:dev")

    docker_env = "XDG_RUNTIME_DIR=/run/user/7321 DOCKER_HOST=unix:///run/user/7321/docker.sock"
    as_dev = f"{docker_env} sudo --preserve-env=XDG_RUNTIME_DIR,DOCKER_HOST -u dev"
    try:
        machine.wait_until_succeeds(
            f"{as_dev} systemctl --user is-active docker.service",
            timeout=60,
        )
    except Exception:
        _, user_status = machine.execute("loginctl user-status dev --no-pager")
        _, service_status = machine.execute(
            f"{as_dev} systemctl --user status docker.service --no-pager -l"
        )
        _, service_journal = machine.execute(
            f"{as_dev} journalctl --user -u docker.service --no-pager -n 100"
        )
        print(user_status)
        print(service_status)
        print(service_journal)
        raise
    machine.succeed(f"tar cv --files-from /dev/null | {as_dev} docker import - scratchimg")
    machine.succeed(f"{as_dev} docker network create sulion-options")
    machine.succeed(f"{as_dev} docker volume create sulion-options")
    machine.succeed(
      f"{as_dev} docker run -d --name=options --memory=128m --cpus=0.5 "
      "--pids-limit=64 --network=sulion-options -v /home/dev/repos:/repos "
      "-v ${pkgs.pkgsStatic.busybox}/bin/busybox:/bin/busybox:ro "
      "scratchimg /bin/busybox sleep 300"
    )
    machine.succeed(f"{as_dev} docker ps --format '{{{{.Names}}}}' | grep -Fx options")
    machine.succeed(f"{as_dev} docker inspect options")
    machine.succeed(f"{as_dev} docker compose version")
    machine.fail("sudo -u dev env DOCKER_HOST=unix:///var/run/docker.sock docker version")
    machine.succeed(f"{as_dev} docker rm --force options")

    machine.wait_for_unit("samba-smbd.service")
    machine.succeed("(printf 'testpass\\ntestpass\\n') | smbpasswd -s -a dev")
    machine.succeed("testparm -s | grep -F 'server min protocol = SMB3'")
    machine.succeed("testparm -s | grep -F 'vfs objects = acl_xattr fruit streams_xattr'")
    machine.succeed("smbclient //127.0.0.1/repos -U 'dev%testpass' -c 'mkdir from-smb'")
    machine.succeed(
      "smbclient //127.0.0.1/repos -U 'dev%testpass' "
      "-c 'put /etc/hostname from-smb/hostname.txt'"
    )
    machine.succeed("test $(stat -c %U:%G /home/dev/repos/from-smb) = dev:dev")
    machine.succeed("test $(stat -c %U:%G /home/dev/repos/from-smb/hostname.txt) = dev:dev")
    machine.succeed("getfattr -n user.DOSATTRIB /home/dev/repos/from-smb/hostname.txt")

    machine.succeed("systemctl cat sulion-stack.service | grep -F /run/user/7321/docker.sock")
    machine.succeed("systemctl cat sulion-stack.service | grep -F compose.dedicated.yaml")
    machine.fail("systemctl is-enabled sulion-stack.service")
    machine.succeed(
      "test ! -e /etc/systemd/system/multi-user.target.wants/sulion-stack.service"
    )
  '';
}
