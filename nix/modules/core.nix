{
  lib,
  config,
  ...
}: {
  options.androidManager = {
    devices = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule (import ./device-type.nix {inherit lib;}));
      default = {};
      description = "Named Android devices.";
    };

    manifests = lib.mkOption {
      type = lib.types.attrsOf lib.types.raw;
      readOnly = true;
      description = "Normalized per-device manifests.";
    };
  };

  config.androidManager.manifests =
    lib.mapAttrs (name: device: {
      schemaVersion = 1;
      device =
        device.normalized
        // {
          inherit name;
        };
    })
    config.androidManager.devices;
}
