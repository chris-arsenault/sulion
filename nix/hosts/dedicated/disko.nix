{
  lib,
  device ? "/dev/disk/by-id/REPLACE_WITH_INSTALL_DISK",
  ...
}:
{
  # The bootstrap command overrides this placeholder with --disk main <by-id>.
  # Runtime mounts continue to use filesystem labels from hardware-configuration.nix,
  # so rebuilding an installed host never depends on this placeholder.
  disko.enableConfig = false;
  disko.devices.disk.main = {
    type = "disk";
    device = lib.mkDefault device;
    content = {
      type = "gpt";
      partitions = {
        ESP = {
          priority = 1;
          size = "1G";
          type = "EF00";
          content = {
            type = "filesystem";
            format = "vfat";
            extraArgs = [
              "-n"
              "boot"
            ];
            mountpoint = "/boot";
            mountOptions = [ "umask=0077" ];
          };
        };
        root = {
          size = "100%";
          content = {
            type = "filesystem";
            format = "ext4";
            extraArgs = [
              "-L"
              "nixos"
              "-m"
              "1"
            ];
            mountpoint = "/";
            mountOptions = [ "acl" ];
          };
        };
      };
    };
  };
}
