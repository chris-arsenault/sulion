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
      description = "Start the split Sulion control and development-node stack automatically at boot.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = host.enable;
        message = "sulion.deployer.enable requires sulion.enable";
      }
    ];

    systemd.services.sulion-stack = {
      description = "Sulion dedicated-host control and development-node application";
      wantedBy = lib.optional cfg.startAtBoot "multi-user.target";
      requires = [ "docker.service" ];
      wants = [
        "network-online.target"
        "user@${toString host.uid}.service"
      ];
      after = [
        "docker.service"
        "network-online.target"
        "user@${toString host.uid}.service"
      ];
      unitConfig.ConditionPathExists = cfg.envFile;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        WorkingDirectory = cfg.source;
        EnvironmentFile = cfg.envFile;
        ExecStartPre = [
          "${pkgs.coreutils}/bin/test -S ${host.rootlessSocket}"
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

    networking.firewall.extraInputRules = ''
      ip saddr ${host.lanCidr} tcp dport { ${toString host.frontendPort}, ${toString host.devPortFrom}-${toString host.devPortTo} } accept comment "Sulion UI and dev ports"
      iifname "${host.bridgeName}" tcp dport ${toString host.backendPort} accept comment "Sulion frontend to host backend"
    '';
  };
}
