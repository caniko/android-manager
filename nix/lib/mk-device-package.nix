{nix-manager-core}: {
  pkgs,
  managerPackage,
  manifest,
  extraRuntimePackages ? [pkgs.android-tools],
}:
nix-manager-core.lib.mkDeclarativeManager {
  inherit pkgs managerPackage extraRuntimePackages;
  managerBinary = "android-manager";
  config = manifest;
}
