{lib}: {config, ...}: {
  options = {
    androidUserId = lib.mkOption {
      type = lib.types.ints.unsigned;
      default = 0;
    };
    apps = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule (import ./app-type.nix {inherit lib;}));
      default = {};
    };
    normalized = lib.mkOption {
      type = lib.types.raw;
      readOnly = true;
    };
  };

  config.normalized = {
    androidUserId = config.androidUserId;
    apps = lib.mapAttrs (_: app: app.normalized) config.apps;
  };
}
