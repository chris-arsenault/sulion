{
  config,
  lib,
  pkgs,
  ...
}:
let
  host = config.sulion;
  cfg = host.backup;
in
{
  options.sulion.backup = {
    enable = lib.mkEnableOption "asynchronous Restic backup of Sulion host state";

    repository = lib.mkOption {
      type = lib.types.str;
      default = "";
      example = "sftp:backup@truenas:/mnt/backups/sulion-enclave";
      description = "Restic repository URL; the primary repository filesystem remains local.";
    };

    passwordFile = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/sulion/secrets/restic-password";
      description = "Root-readable Restic password file outside the Nix store.";
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Optional root-readable environment file for remote repository credentials.";
    };

    onCalendar = lib.mkOption {
      type = lib.types.str;
      default = "daily";
      description = "systemd calendar expression for asynchronous backups.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = host.enable;
        message = "sulion.backup.enable requires sulion.enable";
      }
      {
        assertion = cfg.repository != "";
        message = "sulion.backup.repository must be set when backups are enabled";
      }
    ];

    systemd.services.sulion-backup = {
      description = "Asynchronous Sulion repository and identity backup";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      environment = {
        RESTIC_REPOSITORY = cfg.repository;
        RESTIC_PASSWORD_FILE = cfg.passwordFile;
      };
      serviceConfig = {
        Type = "oneshot";
        EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;
        ExecStart = "${pkgs.restic}/bin/restic backup --one-file-system ${host.reposRoot} /var/lib/samba /var/lib/sulion/node";
        ExecStartPost = "${pkgs.restic}/bin/restic forget --keep-daily 7 --keep-weekly 5 --keep-monthly 12 --prune";
        Nice = 10;
        IOSchedulingClass = "best-effort";
        IOSchedulingPriority = 7;
        CPUSchedulingPolicy = "batch";
      };
    };

    systemd.timers.sulion-backup = {
      description = "Schedule asynchronous Sulion backups";
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.onCalendar;
        Persistent = true;
        RandomizedDelaySec = "30min";
      };
    };
  };
}
