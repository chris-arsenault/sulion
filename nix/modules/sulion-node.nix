{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.sulion;
in
{
  options.sulion = {
    enable = lib.mkEnableOption "the Sulion dedicated development host";

    user = lib.mkOption {
      type = lib.types.str;
      default = "dev";
      description = "Unix identity used by PTYs, repositories, and rootless Docker.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "dev";
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
      default = "/home/dev";
      description = "Canonical local home path, identical inside the workbench container.";
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
      description = "IPv4 LAN allowed to reach SSH, SMB, discovery, UI, and development ports.";
    };

    frontendPort = lib.mkOption {
      type = lib.types.port;
      default = 30080;
      description = "LAN port for the transitional dedicated Sulion frontend.";
    };

    backendPort = lib.mkOption {
      type = lib.types.port;
      default = 8080;
      description = "Host-network backend port reachable only from the Sulion system bridge.";
    };

    devPortFrom = lib.mkOption {
      type = lib.types.port;
      default = 26000;
      description = "First LAN-published rootless development port.";
    };

    devPortTo = lib.mkOption {
      type = lib.types.port;
      default = 26010;
      description = "Last LAN-published rootless development port.";
    };

    bridgeName = lib.mkOption {
      type = lib.types.str;
      default = "sulion0";
      description = "Stable system-Docker bridge used by the dedicated Compose role.";
    };

    rootlessSocket = lib.mkOption {
      type = lib.types.str;
      default = "/run/user/${toString cfg.uid}/docker.sock";
      description = "Rootless Docker socket mounted into the Sulion workbench.";
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
      linger = true;
      extraGroups = [ "wheel" ];
    };

    users.groups.sulion-broker.gid = 7322;
    users.users.sulion-broker = {
      isSystemUser = true;
      uid = 7322;
      group = "sulion-broker";
      home = "/var/lib/sulion/broker";
      createHome = false;
    };

    security.sudo.wheelNeedsPassword = true;

    virtualisation.docker = {
      enable = true;
      daemon.settings."live-restore" = true;
      rootless = {
        enable = true;
        setSocketVariable = true;
      };
    };

    networking.nftables.enable = true;
    networking.firewall.enable = true;

    environment.systemPackages = with pkgs; [
      acl
      attr
      cifs-utils
      docker
      git
      jq
      restic
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
      "d ${cfg.home}/.local/share/docker 0710 ${cfg.user} ${cfg.group} - -"
      "d /var/lib/sulion 0750 root root - -"
      "d /var/lib/sulion/config 0700 root root - -"
      "d /var/lib/sulion/secrets 0700 root root - -"
      "d /var/lib/sulion/broker 0750 sulion-broker sulion-broker - -"
    ];
  };
}
