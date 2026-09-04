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
    ./disko.nix
    ./hardware-configuration.nix
  ];

  networking = {
    hostName = "sulion-enclave";
    # The second 10G port sits in "connecting (getting IP configuration)"
    # forever — no DHCP answers there — which kept NetworkManager from
    # reaching startup-complete, failed NetworkManager-wait-online on
    # every activation, and wedged node releases with switch status 4.
    # eno1 is the only uplink; leave this port out of NM's hands.
    networkmanager.unmanaged = [ "interface-name:enp1s0f1" ];
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
    # The management terminal on the trust appliance is the only SSH source.
    sshAdminSource = "192.168.67.2/32";
    # Server subnet (this host's own) plus the home LAN retain SMB and
    # development-port access independently of the SSH administration path.
    clientCidrs = [
      "192.168.66.0/24"
      "192.168.65.0/24"
    ];

    samba.enable = true;

    deployer = {
      enable = true;
      startAtBoot = true;
    };
  };

  time.timeZone = "Etc/UTC";
  system.stateVersion = "26.05";
}
