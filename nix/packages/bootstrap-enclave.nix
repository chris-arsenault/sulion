{
  pkgs,
  self,
  diskoInstall,
}:
pkgs.writeShellApplication {
  name = "sulion-bootstrap-enclave";
  runtimeInputs = with pkgs; [
    coreutils
    gnugrep
    mkpasswd
    openssh
    shadow
    util-linux
  ];
  text = ''
    export SULION_BOOTSTRAP_FLAKE=${self}
    export SULION_DISKO_INSTALL=${diskoInstall}/bin/disko-install
    export SULION_KEY_UTILS=${../scripts/key-utils.sh}
    exec ${pkgs.bash}/bin/bash ${../scripts/bootstrap-enclave.sh} "$@"
  '';
}
