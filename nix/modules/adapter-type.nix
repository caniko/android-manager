{lib}: {config, ...}: {
  options = {
    kind = lib.mkOption {
      type = lib.types.enum ["source" "delegate"];
    };
    executable = lib.mkOption {
      type = lib.types.either lib.types.path lib.types.str;
    };
    capabilities = lib.mkOption {
      type = lib.types.listOf (lib.types.enum ["resolve" "planDelegate"]);
      default = [];
    };
    environment = lib.mkOption {
      type = lib.types.submodule {
        options = {
          allow = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
          };
          files = lib.mkOption {
            type = lib.types.attrsOf lib.types.str;
            default = {};
          };
        };
      };
      default = {};
    };
    config = lib.mkOption {
      type = lib.types.attrs;
      default = {};
    };
    normalized = lib.mkOption {
      type = lib.types.raw;
      readOnly = true;
    };
  };

  config.normalized = {
    inherit (config) kind;
    executable = "${config.executable}";
    capabilities = lib.unique (lib.sort (a: b: a < b) config.capabilities);
    environment = {
      allow = lib.unique (lib.sort (a: b: a < b) config.environment.allow);
      inherit (config.environment) files;
    };
    inherit (config) config;
  };
}
