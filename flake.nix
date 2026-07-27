{
  description = "Sulion dedicated development node";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      nixosModules = {
        sulion-node = import ./nix/modules/sulion-node.nix;
        sulion-samba = import ./nix/modules/sulion-samba.nix;
        sulion-deployer = import ./nix/modules/sulion-deployer.nix;
        sulion-backup = import ./nix/modules/sulion-backup.nix;
        default = {
          imports = [
            ./nix/modules/sulion-node.nix
            ./nix/modules/sulion-samba.nix
            ./nix/modules/sulion-deployer.nix
            ./nix/modules/sulion-backup.nix
          ];
        };
      };

      nixosConfigurations.sulion-enclave = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [ ./nix/hosts/dedicated/default.nix ];
      };

      checks.${system}.dev-node-vm = import ./nix/tests/dev-node-vm.nix {
        inherit nixpkgs system;
      };

      formatter.${system} = pkgs.nixfmt-tree;
    };
}
