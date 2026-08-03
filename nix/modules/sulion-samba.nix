{
  config,
  lib,
  pkgs,
  ...
}:
let
  host = config.sulion;
  cfg = host.samba;
in
{
  options.sulion.samba = {
    enable = lib.mkEnableOption "the authenticated Sulion repository SMB share";

    shareName = lib.mkOption {
      type = lib.types.str;
      default = "repos";
      description = "SMB share name for the canonical repository root.";
    };

    workgroup = lib.mkOption {
      type = lib.types.str;
      default = "WORKGROUP";
      description = "SMB workgroup advertised to LAN clients.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = host.enable;
        message = "sulion.samba.enable requires sulion.enable";
      }
    ];

    services.samba = {
      enable = true;
      package = pkgs.samba4Full;
      openFirewall = false;
      nmbd.enable = false;
      winbindd.enable = false;
      settings = {
        global = {
          "server role" = "standalone server";
          "security" = "user";
          "workgroup" = cfg.workgroup;
          "server min protocol" = "SMB3";
          "server max protocol" = "SMB3";
          "map to guest" = "never";
          "interfaces" = "127.0.0.1 ${host.lanCidr}";
          "bind interfaces only" = "yes";
          "ea support" = "yes";
          "store dos attributes" = "yes";
          "map archive" = "no";
          "map hidden" = "no";
          "map system" = "no";
          "map readonly" = "no";
          "fruit:aapl" = "yes";
        };

        ${cfg.shareName} = {
          "path" = host.reposRoot;
          "comment" = "Sulion canonical repositories";
          "browseable" = "yes";
          "read only" = "no";
          "guest ok" = "no";
          "valid users" = host.user;
          "inherit owner" = "yes";
          "inherit permissions" = "yes";
          "inherit acls" = "yes";
          "map acl inherit" = "yes";
          "create mask" = "0660";
          "force create mode" = "0660";
          "directory mask" = "0770";
          "force directory mode" = "02770";
          "vfs objects" = "acl_xattr fruit streams_xattr";
          "acl_xattr:ignore system acls" = "no";
          "fruit:metadata" = "stream";
          "fruit:resource" = "stream";
          "fruit:posix_rename" = "yes";
        };
      };
    };

    networking.firewall.extraInputRules = ''
      ip saddr { ${lib.concatStringsSep ", " host.clientCidrs} } tcp dport 445 accept comment "Sulion SMB"
    '';
  };
}
