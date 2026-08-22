{
  lib,
  config,
  ...
}: let
  foreachApp = import ../../lib/foreach-app.nix lib;
  obtainiumApps = builtins.filter (item: item.app.source != null && item.app.source.type == "obtainium") (foreachApp config.androidManager.devices);
in {
  imports = [../core.nix];

  config.androidManager = {
    providers.obtainium = true;
    assertions =
      map (item: {
        assertion = (item.app.source.url != null && item.app.source.url != "") || (item.app.source.repository != null && item.app.source.repository != "");
        message = "${item.loc}: obtainium source requires url or repository";
      })
      obtainiumApps
      ++ map (item: {
        assertion = item.app.management == "delegated";
        message = "${item.loc}: obtainium source requires management delegated";
      })
      obtainiumApps;
  };
}
