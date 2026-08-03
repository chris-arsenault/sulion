{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.sulion;
  adminKey = import ../packages/admin-key.nix { inherit pkgs; };
in
{
  options.sulion = {
    enable = lib.mkEnableOption "the Sulion dedicated development host";

    user = lib.mkOption {
      type = lib.types.str;
      default = "sulion";
      description = "Unix identity used by PTYs, repositories, and Docker.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "sulion";
      description = "Primary Unix group for the development identity.";
    };

    uid = lib.mkOption {
      type = lib.types.int;
      default = 7321;
      description = "Stable UID shared by the host and Sulion workbench image.";
    };

    gid = lib.mkOption {
      type = lib.types.int;
      default = 7321;
      description = "Stable GID shared by the host and Sulion workbench image.";
    };

    home = lib.mkOption {
      type = lib.types.str;
      default = "/home/sulion";
      description = "Canonical host home path mounted into the workbench container.";
    };

    reposRoot = lib.mkOption {
      type = lib.types.str;
      default = "${cfg.home}/repos";
      description = "Canonical local repository root and the only SMB-exported tree.";
    };

    workspacesRoot = lib.mkOption {
      type = lib.types.str;
      default = "${cfg.home}/workspaces";
      description = "Canonical local Sulion worktree root.";
    };

    lanCidr = lib.mkOption {
      type = lib.types.str;
      default = "192.168.66.0/24";
      description = "IPv4 subnet this host lives on; Samba binds its address here.";
    };

    clientCidrs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ cfg.lanCidr ];
      description = "IPv4 client networks admitted to SSH, SMB, and development ports.";
    };

    devPortFrom = lib.mkOption {
      type = lib.types.port;
      default = 26000;
      description = "First LAN-published development port.";
    };

    devPortTo = lib.mkOption {
      type = lib.types.port;
      default = 26010;
      description = "Last LAN-published development port.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.devPortFrom <= cfg.devPortTo;
        message = "sulion.devPortFrom must be less than or equal to sulion.devPortTo";
      }
      {
        assertion = cfg.user != "sulion-broker";
        message = "sulion.user must not collide with the broker service identity";
      }
    ];

    users.groups.${cfg.group}.gid = cfg.gid;
    users.users.${cfg.user} = {
      isNormalUser = true;
      uid = cfg.uid;
      group = cfg.group;
      home = cfg.home;
      homeMode = "0750";
      shell = pkgs.bashInteractive;
      extraGroups = [
        "docker"
        "networkmanager"
        "wheel"
      ];
    };

    security.sudo.wheelNeedsPassword = true;

    services.openssh = {
      enable = true;
      openFirewall = false;
      authorizedKeysFiles = lib.mkForce [
        "/var/lib/sulion/config/ssh/authorized_keys"
      ];
      settings = {
        PasswordAuthentication = false;
        PermitRootLogin = "no";
      };
    };

    virtualisation.docker = {
      enable = true;
      daemon.settings."live-restore" = true;
    };

    networking.nftables.enable = true;
    networking.firewall.enable = true;
    networking.networkmanager = {
      enable = true;
      connectionConfig = {
        "ipv4.dhcp-send-hostname" = true;
      };
      unmanaged = [
        "interface-name:br-*"
        "interface-name:docker0"
        "interface-name:sulion0"
        "interface-name:veth*"
      ];
    };

    environment.systemPackages = with pkgs; [
      acl
      adminKey
      attr
      cifs-utils
      docker
      git
      jq
      rsync
    ];

    systemd.tmpfiles.rules = [
      "d ${cfg.home} 0750 ${cfg.user} ${cfg.group} - -"
      "d ${cfg.reposRoot} 2770 ${cfg.user} ${cfg.group} - -"
      "a+ ${cfg.reposRoot} - - - - u::rwx,g::rwx,o::---,d:u::rwx,d:g::rwx,d:o::---"
      "d ${cfg.workspacesRoot} 0700 ${cfg.user} ${cfg.group} - -"
      "d ${cfg.home}/.claude 0700 ${cfg.user} ${cfg.group} - -"
      "d ${cfg.home}/.codex 0700 ${cfg.user} ${cfg.group} - -"
      "d ${cfg.home}/.local 0750 ${cfg.user} ${cfg.group} - -"
      "d ${cfg.home}/.local/share 0750 ${cfg.user} ${cfg.group} - -"
      "d /var/lib/sulion 0710 root ${cfg.group} - -"
      "d /var/lib/sulion/config 0710 root ${cfg.group} - -"
      "d /var/lib/sulion/config/ssh 0710 root ${cfg.group} - -"
      "f /var/lib/sulion/config/ssh/authorized_keys 0640 root ${cfg.group} - -"
      "d /var/lib/sulion/node 0700 root root - -"
      # Present from first boot so Compose can always read it as an env file,
      # and so the path unit watching it has something to watch.
      "f /var/lib/sulion/node/delivered.env 0600 root root - -"
    ];
  };
}
