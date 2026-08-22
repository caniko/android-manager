{
  lib,
  config,
  ...
}: let
  foreachApp = import ../../lib/foreach-app.nix lib;
  httpApps = builtins.filter (item: item.app.source != null && item.app.source.type == "http") (foreachApp config.androidManager.devices);
in {
  imports = [../core.nix];

  config.androidManager = {
    providers.http = true;
    assertions =
      map (item: {
        assertion = item.app.source.url != null && item.app.source.url != "" && item.app.source.sha256 != null && builtins.match "[0-9a-f]{64}" item.app.source.sha256 != null;
        message = "${item.loc}: http source requires url and sha256";
      })
      httpApps
      ++ map (item: {
        assertion = item.app.management != "delegated";
        message = "${item.loc}: http source is incompatible with management delegated";
      })
      httpApps;
  };
}
