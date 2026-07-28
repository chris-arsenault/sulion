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
  # The delivered file is listed last so control-plane values win over the
  # host-generated defaults beside them.
  envFileArgs = "--env-file ${cfg.bootstrapEnvFile} --env-file ${cfg.deliveredEnvFile}";
  composeArgs = "${envFileArgs} -f ${cfg.source}/compose.yaml -f ${cfg.source}/deploy/compose.dedicated.yaml";
  nodeDeploy = import ../packages/node-deploy.nix {
    inherit pkgs;
    source = cfg.source;
    envFile = cfg.bootstrapEnvFile;
    deliveredEnvFile = cfg.deliveredEnvFile;
  };
  nodeBootstrap = pkgs.writeShellApplication {
    name = "sulion-node-bootstrap";
    runtimeInputs = with pkgs; [
      coreutils
      diffutils
      git
      gnused
    ];
    text = ''
      export SULION_BOOTSTRAP_ENV_FILE=${lib.escapeShellArg cfg.bootstrapEnvFile}
      export SULION_DELIVERED_ENV_FILE=${lib.escapeShellArg cfg.deliveredEnvFile}
      export SULION_CODE_INTEL_TOKEN_FILE=${lib.escapeShellArg cfg.codeIntelTokenFile}
      export SULION_RELEASE_REPOSITORY=${lib.escapeShellArg cfg.releaseRepository}
      export SULION_RELEASE_BRANCH=${lib.escapeShellArg cfg.releaseBranch}
      export SULION_IMAGE_REGISTRY=${lib.escapeShellArg cfg.imageRegistry}
      export SULION_HOME_HOST_PATH=${lib.escapeShellArg host.home}
      export SULION_REPOS_HOST_PATH=${lib.escapeShellArg host.reposRoot}
      export SULION_WORKSPACES_HOST_PATH=${lib.escapeShellArg host.workspacesRoot}
      export SULION_NODE_STATE_HOST_PATH=${lib.escapeShellArg cfg.nodeStateDirectory}
      export SULION_NODE_ID=${lib.escapeShellArg cfg.nodeId}
      export SULION_NODE_CONTROL_URL=${lib.escapeShellArg cfg.controlUrl}
      export SULION_NODE_ALLOW_INSECURE_WS=${if cfg.allowInsecureControlUrl then "1" else "0"}
      export SULION_SECRET_BROKER_URL=${lib.escapeShellArg cfg.secretBrokerUrl}
      export SULION_RETRIEVAL_URL=${lib.escapeShellArg cfg.retrievalUrl}
      export SULION_CODE_INTEL_URL=${lib.escapeShellArg cfg.codeIntelUrl}
      export SULION_DEV_PORT_RANGE=${toString host.devPortFrom}-${toString host.devPortTo}
      exec ${pkgs.bash}/bin/bash ${../scripts/sulion-node-bootstrap.sh}
    '';
  };
  nodeTunnel = pkgs.writeShellApplication {
    name = "sulion-node-tunnel";
    runtimeInputs = with pkgs; [
      coreutils
      diffutils
      gnugrep
      iproute2
      util-linux
      wireguard-tools
    ];
    text = ''
      export SULION_TUNNEL_CONF_SOURCE=${lib.escapeShellArg cfg.tunnelConfigFile}
      export SULION_TUNNEL_INTERFACE=${lib.escapeShellArg cfg.tunnelInterface}
      exec ${pkgs.bash}/bin/bash ${../scripts/sulion-node-tunnel.sh}
    '';
  };
  nodeActivate = pkgs.writeShellApplication {
    name = "sulion-node-activate";
    runtimeInputs = with pkgs; [
      coreutils
      gnugrep
      systemd
    ];
    text = ''
      export SULION_DELIVERED_ENV_FILE=${lib.escapeShellArg cfg.deliveredEnvFile}
      exec ${pkgs.bash}/bin/bash ${../scripts/sulion-node-activate.sh}
    '';
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
      env_file=${lib.escapeShellArg cfg.bootstrapEnvFile}
      release="$(
        git ls-remote --exit-code \
          ${lib.escapeShellArg cfg.releaseRepository} \
          refs/heads/${cfg.releaseBranch} |
          cut -f1
      )"

      if [[ ! "$release" =~ ^[0-9a-f]{40}$ ]]; then
        echo "${cfg.releaseBranch} did not resolve to one full Git commit SHA" >&2
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

    bootstrapEnvFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/config/bootstrap.env";
      description = ''
        Host-generated runtime environment. Holds only values this repository
        already knows plus the machine-local code intelligence token; shared
        credentials never appear here.
      '';
    };

    deliveredEnvFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/node/delivered.env";
      description = ''
        Runtime environment the node receives from the control plane after an
        operator approves its identity key, written by `sulion-node` itself.
      '';
    };

    codeIntelTokenFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/config/code-intel.token";
      description = ''
        Machine-local shared secret between the node and code intelligence.
        Both ends are on this host's loopback, so it is generated here rather
        than shared with the control plane.
      '';
    };

    tunnelConfigFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/node/wg0.conf";
      description = ''
        WireGuard configuration `sulion-node` renders from the peering the
        control plane granted it. Written root-only; a path unit applies it.
      '';
    };

    tunnelInterface = lib.mkOption {
      type = lib.types.str;
      default = "wg0";
      description = "Interface name for the control-plane tunnel.";
    };

    nodeStateDirectory = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/node";
      description = "Root-owned node identity and delivered-configuration directory.";
    };

    nodeId = lib.mkOption {
      type = lib.types.str;
      default = "019d4f28-88ac-7a80-932c-b0f53a0708f4";
      description = "Dedicated node identity. The node key, not this value, authenticates the machine.";
    };

    controlUrl = lib.mkOption {
      type = lib.types.str;
      default = "ws://192.168.66.3:30081/ws/nodes";
      description = ''
        Node control channel. Points at the control plane's LAN-bound node port
        rather than its public hostname, so node traffic never leaves the LAN
        and never traverses the public reverse proxy.
      '';
    };

    allowInsecureControlUrl = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Permit a `ws://` control URL. The default control URL is a LAN address
        on the same segment the node already reaches Postgres over, so it
        carries the same exposure rather than a new one.
      '';
    };

    secretBrokerUrl = lib.mkOption {
      type = lib.types.str;
      default = "https://sulion.services.ahara.io/broker";
      description = "Outbound secret broker endpoint.";
    };

    retrievalUrl = lib.mkOption {
      type = lib.types.str;
      default = "https://sulion.services.ahara.io/retrieval";
      description = "Outbound retrieval endpoint.";
    };

    codeIntelUrl = lib.mkOption {
      type = lib.types.str;
      default = "http://127.0.0.1:8084";
      description = "Node-local code intelligence endpoint.";
    };

    imageRegistry = lib.mkOption {
      type = lib.types.str;
      default = "ghcr.io/chris-arsenault/sulion";
      description = "Registry holding the published application images.";
    };

    releaseRepository = lib.mkOption {
      type = lib.types.str;
      default = "https://github.com/chris-arsenault/sulion.git";
      description = "Repository whose release branch names the deployable commit.";
    };

    releaseBranch = lib.mkOption {
      type = lib.types.str;
      default = "node-release";
      description = "Branch CI advances once images and the control plane are live.";
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
      {
        assertion = cfg.allowInsecureControlUrl || lib.hasPrefix "wss://" cfg.controlUrl;
        message = "sulion.deployer.controlUrl must use wss:// unless allowInsecureControlUrl is set";
      }
    ];

    environment.systemPackages = [
      nodeActivate
      nodeBootstrap
      nodeDeploy
      nodeTunnel
      nodeUpdate
      pkgs.wireguard-tools
    ];

    # Generates the host half of the runtime environment. Without this the node
    # could not start at all, which is why the stack requires it rather than
    # silently skipping on a missing file.
    systemd.services.sulion-node-bootstrap = {
      description = "Generate the Sulion development-node bootstrap environment";
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${nodeBootstrap}/bin/sulion-node-bootstrap";
      };
    };

    systemd.services.sulion-stack = {
      description = "Sulion dedicated development-node application";
      wantedBy = lib.optional cfg.startAtBoot "multi-user.target";
      requires = [
        "docker.service"
        "sulion-node-bootstrap.service"
      ];
      wants = [ "network-online.target" ];
      after = [
        "docker.service"
        "network-online.target"
        "sulion-node-bootstrap.service"
      ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        WorkingDirectory = cfg.source;
        EnvironmentFile = [
          cfg.bootstrapEnvFile
          cfg.deliveredEnvFile
        ];
        ExecStartPre = [
          "${pkgs.coreutils}/bin/test -S /var/run/docker.sock"
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

    # The node writes delivered configuration as root once it is approved; this
    # is what turns that write into a running stack without anyone logging in.
    systemd.paths.sulion-node-activate = {
      description = "Watch for configuration delivered to the Sulion node";
      wantedBy = [ "paths.target" ];
      pathConfig = {
        PathChanged = cfg.deliveredEnvFile;
        Unit = "sulion-node-activate.service";
      };
    };

    systemd.services.sulion-node-activate = {
      description = "Apply configuration delivered to the Sulion node";
      after = [ "sulion-stack.service" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${nodeActivate}/bin/sulion-node-activate";
      };
    };

    # The node writes its peering after it is approved; this is what turns that
    # write into a live interface without anyone logging in. Credentials are
    # withheld by the control plane until they can cross it.
    systemd.paths.sulion-node-tunnel = {
      description = "Watch for tunnel peering delivered to the Sulion node";
      wantedBy = [ "paths.target" ];
      pathConfig = {
        PathChanged = cfg.tunnelConfigFile;
        Unit = "sulion-node-tunnel.service";
      };
    };

    systemd.services.sulion-node-tunnel = {
      description = "Apply tunnel peering delivered to the Sulion node";
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${nodeTunnel}/bin/sulion-node-tunnel";
      };
    };

    systemd.services.sulion-node-update = {
      description = "Poll and deploy the current Sulion node release";
      requires = [
        "docker.service"
        "sulion-node-bootstrap.service"
      ];
      wants = [ "network-online.target" ];
      after = [
        "docker.service"
        "network-online.target"
        "sulion-node-bootstrap.service"
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
