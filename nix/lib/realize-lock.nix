{
  lib,
  pkgs,
  lock,
}:
lib.mapAttrs (
  _: entry: let
    artifact = builtins.head entry.artifacts;
  in
    if artifact.path or null != null
    then artifact.path
    else
      pkgs.fetchurl {
        url = artifact.url;
        hash = "sha256:${artifact.sha256}";
      }
)
lock.entries
