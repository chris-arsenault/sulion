{
  pkgs,
  source,
  envFile,
}:
pkgs.writeShellApplication {
  name = "sulion-node-deploy";
  runtimeInputs = with pkgs; [
    coreutils
    docker
    docker-compose
    gnugrep
    gnused
  ];
  text = ''
    export SULION_DEPLOY_SOURCE=${source}
    export SULION_DEPLOY_ENV_FILE=${envFile}
    export SULION_COMPOSE_BIN=${pkgs.docker-compose}/bin/docker-compose
    exec ${pkgs.bash}/bin/bash ${../scripts/sulion-node-deploy.sh} "$@"
  '';
}
