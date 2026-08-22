{lib}: {modules}: let
  evaluated = lib.evalModules {
    inherit modules;
  };
in
  builtins.seq (evaluated.config.androidManager.manifests or null) evaluated
