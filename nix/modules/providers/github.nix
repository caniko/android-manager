{
  lib,
  config,
  ...
}: let
  foreachApp = import ../../lib/foreach-app.nix lib;
  githubApps = builtins.filter (item: item.app.source != null && item.app.source.type == "github") (foreachApp config.androidManager.devices);
in {
  imports = [../core.nix];

  config.androidManager = {
    providers.github = true;
    assertions =
      map (item: {
        assertion = item.app.source.repository != null && item.app.source.repository != "";
        message = "${item.loc}: github source requires repository";
      })
      githubApps
      ++ map (item: {
        assertion = item.app.management != "delegated";
        message = "${item.loc}: github source is incompatible with management delegated";
      })
      githubApps;
  };
}
