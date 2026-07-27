{
  config,
  lib,
  pkgs,
  ...
}:
let
  host = config.sulion;
  cfg = host.deployer;
  compose = "${pkgs.docker-compose}/bin/docker-compose";
  composeArgs = "--env-file ${cfg.envFile} -f ${cfg.source}/compose.yaml -f ${cfg.source}/deploy/compose.dedicated.yaml";
  nodeDeploy = import ../packages/node-deploy.nix {
    inherit pkgs;
    source = cfg.source;
    envFile = cfg.envFile;
  };
  nodeUpdate = pkgs.writeShellApplication {
    name = "sulion-node-update";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.git
      pkgs.gnused
      nodeDeploy
    ];
    text = ''
      env_file=${lib.escapeShellArg cfg.envFile}
      release="$(
        git ls-remote --exit-code \
          https://github.com/chris-arsenault/sulion.git \
          refs/heads/node-release |
          cut -f1
      )"

      if [[ ! "$release" =~ ^[0-9a-f]{40}$ ]]; then
        echo "node-release did not resolve to one full Git commit SHA" >&2
        exit 65
      fi

      current="$(sed -n 's/^IMAGE_TAG=//p' "$env_file")"
      if [[ "$current" == "$release" ]]; then
        exit 0
      fi

      exec sulion-node-deploy "$release"
    '';
  };
in
{
  options.sulion.deployer = {
    enable = lib.mkEnableOption "the root-owned Sulion Compose deployment unit";

    source = lib.mkOption {
      type = lib.types.path;
      default = ../..;
      description = "Immutable source tree containing the common Compose graph and dedicated overlay.";
    };

    envFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/config/runtime.env";
      description = "Root-readable runtime and secret environment file outside the Nix store.";
    };

    startAtBoot = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Start the Sulion development-node stack automatically at boot.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = host.enable;
        message = "sulion.deployer.enable requires sulion.enable";
      }
    ];

    environment.systemPackages = [
      nodeDeploy
      nodeUpdate
    ];

    systemd.services.sulion-stack = {
      description = "Sulion dedicated development-node application";
      wantedBy = lib.optional cfg.startAtBoot "multi-user.target";
      requires = [ "docker.service" ];
      wants = [ "network-online.target" ];
      after = [
        "docker.service"
        "network-online.target"
      ];
      unitConfig.ConditionPathExists = [
        cfg.envFile
        "/var/lib/sulion/node/private-key.pk8"
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        WorkingDirectory = cfg.source;
        EnvironmentFile = cfg.envFile;
        ExecStartPre = [
          "${pkgs.coreutils}/bin/test -S /var/run/docker.sock"
          "${pkgs.coreutils}/bin/test -f /var/lib/sulion/node/private-key.pk8"
        ];
        ExecStart = "${compose} ${composeArgs} up -d --remove-orphans";
        ExecReload = "${compose} ${composeArgs} up -d --remove-orphans";
        ExecStop = "${compose} ${composeArgs} stop";
        Restart = "on-failure";
        RestartSec = "5s";
        TimeoutStartSec = "20min";
        TimeoutStopSec = "5min";
      };
    };

    systemd.services.sulion-node-update = {
      description = "Poll and deploy the current Sulion node release";
      requires = [ "docker.service" ];
      wants = [ "network-online.target" ];
      after = [
        "docker.service"
        "network-online.target"
      ];
      unitConfig.ConditionPathExists = [
        cfg.envFile
        "/var/lib/sulion/node/private-key.pk8"
      ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${nodeUpdate}/bin/sulion-node-update";
      };
    };

    systemd.timers.sulion-node-update = {
      description = "Poll for Sulion node releases";
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = "2min";
        Unit = "sulion-node-update.service";
      };
    };

    networking.firewall.extraInputRules = ''
      ip saddr ${host.lanCidr} tcp dport ${toString host.devPortFrom}-${toString host.devPortTo} accept comment "Sulion development ports"
    '';
  };
}
