{
  lib,
  config,
  ...
}: let
  foreachApp = import ../../lib/foreach-app.nix lib;
  fdroidApps = builtins.filter (item: item.app.source != null && item.app.source.type == "fdroid") (foreachApp config.androidManager.devices);
in {
  imports = [../core.nix];

  config.androidManager = {
    providers.fdroid = true;
    assertions =
      map (item: {
        assertion = item.app.source.identity != null && item.app.source.identity != "";
        message = "${item.loc}: fdroid source requires identity";
      })
      fdroidApps
      ++ map (item: {
        assertion = item.app.management != "delegated";
        message = "${item.loc}: fdroid source is incompatible with management delegated";
      })
      fdroidApps;
  };
}
