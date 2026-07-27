{ pkgs }:
pkgs.writeShellApplication {
  name = "sulion-admin-key";
  runtimeInputs = with pkgs; [
    coreutils
    gnugrep
    openssh
    util-linux
  ];
  text = ''
    export SULION_KEY_UTILS=${../scripts/key-utils.sh}
    exec ${pkgs.bash}/bin/bash ${../scripts/sulion-admin-key.sh} "$@"
  '';
}
