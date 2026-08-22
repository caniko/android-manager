{lib}: {config, ...}: {
  options = {
    removals = lib.mkOption {
      type = lib.types.str;
      default = "explicit-only";
    };
    sourceMigration = lib.mkOption {
      type = lib.types.str;
      default = "deny-implicit";
    };
    signerChanges = lib.mkOption {
      type = lib.types.str;
      default = "require-explicit-migration";
    };
    newerThanLock = lib.mkOption {
      type = lib.types.str;
      default = "warn";
    };
    manualActions = lib.mkOption {
      type = lib.types.str;
      default = "allow-visible";
    };
    normalized = lib.mkOption {
      type = lib.types.raw;
      readOnly = true;
    };
  };

  config.normalized = {
    inherit
      (config)
      removals
      sourceMigration
      signerChanges
      newerThanLock
      manualActions
      ;
  };
}
