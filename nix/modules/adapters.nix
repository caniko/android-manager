{
  lib,
  config,
  ...
}: let
  foreachApp = import ../lib/foreach-app.nix lib;
  builtinNames = ["local" "http" "fdroid" "github" "obtainium" "external"];
  isEnvName = name: builtins.match "[A-Z_][A-Z0-9_]*" name != null;
  isSecretName = name: let
    lower = lib.toLower name;
  in
    lib.any (word: lib.hasInfix word lower) ["secret" "token" "password" "credential"];
  containsSecretKey = value:
    if builtins.isAttrs value
    then lib.any (name: isSecretName name || containsSecretKey value.${name}) (lib.attrNames value)
    else if builtins.isList value
    then lib.any containsSecretKey value
    else false;
  isDangerousEnv = name:
    lib.elem name ["PATH" "IFS" "BASH_ENV" "ENV" "PERL5OPT" "PYTHONPATH" "NODE_OPTIONS"]
    || lib.hasPrefix "LD_" name
    || lib.hasPrefix "DYLD_" name;
  hasUrlCredentials = value:
    value != null && builtins.match ".*://[^/]*@.*" value != null;
  isStorePath = path: path == builtins.storeDir || lib.hasPrefix "${builtins.storeDir}/" path;
  apps = foreachApp config.androidManager.devices;
  adapters = config.androidManager.adapters;

  adapterAssertions =
    lib.mapAttrsToList (name: adapter: let
      exe = "${adapter.executable}";
      expectedCapability =
        if adapter.kind == "source"
        then "resolve"
        else "planDelegate";
      unexpectedCapability =
        if adapter.kind == "source"
        then "planDelegate"
        else "resolve";
    in [
      {
        assertion = !(lib.elem name builtinNames);
        message = "androidManager.adapters.${name}: adapter name must be explicit and must not use built-in provider names (${lib.concatStringsSep ", " builtinNames})";
      }
      {
        assertion = exe != "";
        message = "androidManager.adapters.${name}: executable is required";
      }
      {
        assertion = exe == "" || lib.hasPrefix "/" exe;
        message = "androidManager.adapters.${name}: executable must be an absolute path";
      }
      {
        assertion = lib.elem expectedCapability adapter.capabilities && !(lib.elem unexpectedCapability adapter.capabilities);
        message = "androidManager.adapters.${name}: kind ${adapter.kind} requires capability ${expectedCapability}";
      }
      {
        assertion = lib.all (key: isEnvName key && !(isDangerousEnv key)) adapter.environment.allow;
        message = "androidManager.adapters.${name}: environment.allow contains an invalid environment variable name";
      }
      {
        assertion = lib.all (key: isEnvName key && !(isDangerousEnv key)) (lib.attrNames adapter.environment.files);
        message = "androidManager.adapters.${name}: environment.files contains an invalid environment variable name";
      }
      {
        assertion = lib.all (
          path:
            lib.hasPrefix "/" path && !(isStorePath path)
        ) (lib.attrValues adapter.environment.files);
        message = "androidManager.adapters.${name}: environment.files must be absolute runtime secret paths and must not be Nix store paths";
      }
      {
        assertion = !(containsSecretKey adapter.config);
        message = "androidManager.adapters.${name}: secret values are forbidden in config; use environment.allow or environment.files references";
      }
    ])
    adapters;

  appAssertions =
    map (
      item: let
        source = item.app.source;
        base =
          if source == null
          then []
          else [
            {
              assertion = !(containsSecretKey source.config);
              message = "${item.loc}: secret values are forbidden in source.config; use adapter environment references";
            }
            {
              assertion = !(hasUrlCredentials source.url) && !(hasUrlCredentials source.repository);
              message = "${item.loc}: credential-bearing source URLs are forbidden";
            }
          ];
      in
        if source == null || source.type != "external"
        then base
        else let
          adapterName = source.adapter;
          adapter =
            if adapterName == null || adapterName == ""
            then null
            else adapters.${adapterName} or null;
          hostLike = item.app.management == "host";
          delegated = item.app.management == "delegated";
        in
          base
          ++ [
            {
              assertion = adapterName != null && adapterName != "";
              message = "${item.loc}: external source requires an explicit adapter name";
            }
            {
              assertion = adapterName == null || adapterName == "" || adapters ? ${adapterName};
              message = "${item.loc}: unknown adapter ${toString adapterName}";
            }
            {
              assertion = adapter == null || (adapter.kind == "source" && hostLike) || (adapter.kind == "delegate" && delegated);
              message = "${item.loc}: adapter ${toString adapterName} kind ${
                if adapter == null
                then "unknown"
                else adapter.kind
              } is incompatible with management ${item.app.management}";
            }
          ]
    )
    apps;
in {
  config.androidManager.assertions = lib.flatten (adapterAssertions ++ appAssertions);
}
