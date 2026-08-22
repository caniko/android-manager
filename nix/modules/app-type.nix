{lib}: {config, ...}: {
  options = {
    state = lib.mkOption {
      type = lib.types.enum ["present" "absent"];
      default = "present";
    };
    management = lib.mkOption {
      type = lib.types.enum ["host" "delegated" "observe" "manual"];
      default = "host";
    };
    source = lib.mkOption {
      type = lib.types.nullOr (lib.types.submodule {
        options = {
          type = lib.mkOption {
            type = lib.types.str;
          };
          path = lib.mkOption {
            type = lib.types.nullOr lib.types.path;
            default = null;
          };
          url = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
          sha256 = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
          repository = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
          identity = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
          channel = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
        };
      });
      default = null;
    };
    versionPolicy = lib.mkOption {
      type = lib.types.enum ["locked-upgrade-only" "minimum" "presence" "observe"];
      default = "locked-upgrade-only";
    };
    verification = lib.mkOption {
      type = lib.types.submodule {
        options = {
          requireSignerPin = lib.mkOption {
            type = lib.types.bool;
            default = true;
          };
          requirePackageNameMatch = lib.mkOption {
            type = lib.types.bool;
            default = true;
          };
        };
      };
      default = {};
    };
    normalized = lib.mkOption {
      type = lib.types.raw;
      readOnly = true;
    };
  };

  config.normalized = {
    inherit (config) state management versionPolicy;
    source =
      if config.source == null
      then null
      else {
        type = config.source.type;
        path =
          if config.source.path == null
          then null
          else "${config.source.path}";
        inherit (config.source) url sha256 repository identity channel;
      };
    verification = {
      inherit (config.verification) requireSignerPin requirePackageNameMatch;
    };
  };
}
