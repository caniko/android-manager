{
  nix-manager-core,
  evalFleet,
  mkDevicePackage,
  realizeLock,
}: {
  self,
  nixpkgs,
  lockFile ? null,
  modules,
}: let
  inherit (nixpkgs) lib;
  evaluated = evalFleet {inherit modules;};
  lock =
    if lockFile == null
    then null
    else builtins.fromJSON (builtins.readFile lockFile);
in {
  inherit evaluated;
  deviceManifests = evaluated.config.androidManager.manifests;
  realize = pkgs:
    if lock == null
    then {}
    else
      realizeLock {
        inherit lib pkgs lock;
      };
  mkPackages = {
    pkgs,
    managerPackage,
  }:
    lib.mapAttrs (
      name: manifest:
        mkDevicePackage {
          inherit pkgs managerPackage manifest;
        }
    )
    evaluated.config.androidManager.manifests;
}
