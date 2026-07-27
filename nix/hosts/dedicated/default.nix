{
  config,
  pkgs,
  ...
}:
{
  imports = [
    ../../modules/sulion-node.nix
    ../../modules/sulion-samba.nix
    ../../modules/sulion-deployer.nix
    ../../modules/sulion-backup.nix
    ./disko.nix
    ./hardware-configuration.nix
  ];

  networking = {
    hostName = "sulion-enclave";
    firewall.extraInputRules = ''
      ip saddr ${config.sulion.lanCidr} tcp dport 22 accept comment "Sulion LAN SSH"
    '';
  };

  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];

  console.keyMap = "us";
  i18n.defaultLocale = "en_US.UTF-8";

  environment.systemPackages = with pkgs; [
    curl
    vim
  ];

  sulion = {
    enable = true;
    user = "sulion";
    group = "sulion";
    home = "/home/sulion";
    lanCidr = "192.168.66.0/24";

    samba.enable = true;

    # The unit is installed now, but remains off until the branch is released
    # as matching OCI images and the root-readable runtime env file exists.
    deployer = {
      enable = true;
      startAtBoot = false;
    };

    # Configure a TrueNAS Restic repository and credential path before
    # enabling. Backups are deliberately asynchronous; repos stay local.
    backup.enable = false;
  };

  time.timeZone = "Etc/UTC";
  system.stateVersion = "26.05";
}
