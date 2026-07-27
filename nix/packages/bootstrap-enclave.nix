{
  pkgs,
  self,
  diskoPackage,
}:
pkgs.writeShellApplication {
  name = "sulion-bootstrap-enclave";
  runtimeInputs = with pkgs; [
    coreutils
    gnugrep
    nixos-install-tools
    openssh
    util-linux
  ];
  text = ''
    export SULION_BOOTSTRAP_FLAKE=${self}
    export SULION_DISKO=${diskoPackage}/bin/disko
    export SULION_KEY_UTILS=${../scripts/key-utils.sh}
    exec ${pkgs.bash}/bin/bash ${../scripts/bootstrap-enclave.sh} "$@"
  '';
}
