{
  description = "Sulion dedicated development node";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

    disko = {
      url = "github:nix-community/disko/ff8702b4de27f72b4c78573dfb89ec74e36abdf1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      disko,
      nixpkgs,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      adminKey = import ./nix/packages/admin-key.nix { inherit pkgs; };
      bootstrapEnclave = import ./nix/packages/bootstrap-enclave.nix {
        inherit pkgs self;
        diskoPackage = disko.packages.${system}.disko;
      };
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
        modules = [
          disko.nixosModules.disko
          ./nix/hosts/dedicated/default.nix
        ];
      };

      diskoConfigurations.sulion-enclave =
        {
          lib,
          disk ? "/dev/disk/by-id/REPLACE_WITH_INSTALL_DISK",
          ...
        }:
        import ./nix/hosts/dedicated/disko.nix {
          inherit lib;
          device = disk;
        };

      checks.${system}.dev-node-vm = import ./nix/tests/dev-node-vm.nix {
        inherit nixpkgs system;
      };

      packages.${system} = {
        admin-key = adminKey;
        bootstrap-enclave = bootstrapEnclave;
      };

      apps.${system} = {
        bootstrap-enclave = {
          type = "app";
          program = nixpkgs.lib.getExe bootstrapEnclave;
        };
        install-admin-key = {
          type = "app";
          program = nixpkgs.lib.getExe adminKey;
        };
      };

      formatter.${system} = pkgs.nixfmt-tree;
    };
}
