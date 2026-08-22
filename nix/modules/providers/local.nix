{
  lib,
  config,
  ...
}: let
  foreachApp = import ../../lib/foreach-app.nix lib;
  localApps = builtins.filter (item: item.app.source != null && item.app.source.type == "local") (foreachApp config.androidManager.devices);
in {
  imports = [../core.nix];

  config.androidManager = {
    providers.local = true;
    assertions =
      map (item: {
        assertion = item.app.source.path != null;
        message = "${item.loc}: local source requires path";
      })
      localApps
      ++ map (item: {
        assertion = item.app.management != "delegated";
        message = "${item.loc}: local source is incompatible with management delegated";
      })
      localApps;
  };
}
