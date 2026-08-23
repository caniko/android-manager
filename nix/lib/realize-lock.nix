{
  lib,
  pkgs,
  lock,
}: let
  validSha256 = value:
    builtins.isString value && builtins.match "[0-9a-f]{64}" value != null;
  hasUrlCredentials = value:
    builtins.match ".*://[^/]*@.*" value != null;
  realizeOne = artifact: let
    sha256 = artifact.sha256 or "";
    path = artifact.path or null;
    url = artifact.url or null;
    valid =
      if !validSha256 sha256
      then throw "android-manager lock artifact has an invalid sha256"
      else if path == null && url == null
      then throw "android-manager lock artifact has neither path nor url"
      else if path != null && (!lib.hasPrefix "/" path || builtins.readFileType path != "regular")
      then throw "android-manager lock artifact path must be an absolute regular file"
      else if path == null && (!lib.hasPrefix "https://" url || hasUrlCredentials url)
      then throw "android-manager lock artifact URL must be credential-free HTTPS"
      else true;
  in
    builtins.seq valid (
      if path != null
      then let
        source = builtins.path {
          inherit path;
          name = "android-manager-source-${sha256}";
        };
      in
        pkgs.runCommand "android-manager-artifact-${sha256}" {
          outputHash = "sha256:${sha256}";
          outputHashAlgo = "sha256";
          outputHashMode = "flat";
        } ''
          cp -- ${source} "$out"
        ''
      else
        pkgs.fetchurl {
          inherit url;
          hash = "sha256:${sha256}";
        }
    );
  realize = _: entry: let
    artifacts = entry.artifacts or [];
  in
    if artifacts == []
    then throw "android-manager lock entry has no artifacts"
    else map realizeOne artifacts;
in
  if lock.schemaVersion or null != 1 || !(builtins.isAttrs (lock.entries or null))
  then throw "unsupported android-manager lock schema"
  else lib.mapAttrs realize lock.entries
