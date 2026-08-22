{
  lib,
  config,
  ...
}: let
  foreachApp = import ../lib/foreach-app.nix lib;
  failedAssertions = lib.filter (item: !item.assertion) config.androidManager.assertions;
  assertionError =
    if failedAssertions == []
    then null
    else throw (lib.concatMapStringsSep "\n" (item: item.message) failedAssertions);
in {
  imports = [./adapters.nix];

  options.androidManager = {
    devices = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule (import ./device-type.nix {inherit lib;}));
      default = {};
      description = "Named Android devices.";
    };

    adapters = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule (import ./adapter-type.nix {inherit lib;}));
      default = {};
      description = "External source and delegate adapters.";
    };

    providers = lib.mkOption {
      type = lib.types.attrsOf lib.types.bool;
      default = {};
      internal = true;
    };

    assertions = lib.mkOption {
      type = lib.types.listOf (lib.types.submodule {
        options.assertion = lib.mkOption {type = lib.types.bool;};
        options.message = lib.mkOption {type = lib.types.str;};
      });
      default = [];
      internal = true;
    };

    manifests = lib.mkOption {
      type = lib.types.attrsOf lib.types.raw;
      readOnly = true;
      description = "Normalized per-device manifests.";
    };
  };

  config.androidManager = {
    assertions = map (item: let
      source = item.app.source;
    in {
      assertion = source == null || source.type == "external" || config.androidManager.providers.${source.type} or false;
      message =
        if source == null
        then "${item.loc}: source has no provider requirement"
        else "${item.loc}: source type ${source.type} requires its provider module";
    }) (foreachApp config.androidManager.devices);

    manifests = builtins.seq assertionError (
      lib.mapAttrs (name: device: {
        schemaVersion = 1;
        device =
          device.normalized
          // {
            inherit name;
          };
        adapters = lib.mapAttrs (_: adapter: adapter.normalized) config.androidManager.adapters;
      })
      config.androidManager.devices
    );
  };
}
