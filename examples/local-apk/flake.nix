{
  description = "Example android-manager fleet with a local APK";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    android-manager.url = "path:../..";
    android-manager.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    android-manager,
  }:
    android-manager.lib.mkFleet {
      inherit self nixpkgs;
      lockFile = ./android-manager.lock.json;
      modules = [
        android-manager.modules.default
        android-manager.profiles.safe-defaults
        ./devices/pixel10.nix
      ];
    };
}
