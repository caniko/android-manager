{lib}: {
  name,
  config,
  ...
}: {
  options = {
    transport = lib.mkOption {
      type = lib.types.submodule {
        options = {
          type = lib.mkOption {
            type = lib.types.enum ["adb"];
            default = "adb";
          };
          selector.serial = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
        };
      };
      default = {};
    };

    expected = lib.mkOption {
      type = lib.types.submodule {
        options = {
          manufacturer = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
          };
          supportedAbis = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
          };
        };
      };
      default = {};
    };

    users = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule (import ./user-type.nix {inherit lib;}));
      default = {};
    };

    policies = lib.mkOption {
      type = lib.types.submodule (import ./policy-type.nix {inherit lib;});
      default = {};
    };

    normalized = lib.mkOption {
      type = lib.types.raw;
      readOnly = true;
    };
  };

  config.normalized = {
    inherit name;
    transport = {
      type = config.transport.type;
      selector = {
        serial = config.transport.selector.serial;
      };
    };
    expected = {
      manufacturer = config.expected.manufacturer;
      supportedAbis = config.expected.supportedAbis;
    };
    users = lib.mapAttrs (_: user: user.normalized) config.users;
    policies = config.policies.normalized;
  };
}
