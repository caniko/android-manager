{
  self,
  nixpkgs,
  rs-harbor,
  rust-overlay,
  treefmt-nix,
  git-hooks,
  nix-manager-core,
  ...
}:
nix-manager-core.lib.mkManagerOutputs {
  inherit self nixpkgs rs-harbor rust-overlay treefmt-nix git-hooks;
  crateName = "android-manager";
  srcDir = ../.;
  extraRuntimePackages = pkgs: [pkgs.android-tools pkgs.curl pkgs.openssl pkgs.unzip];
  extraDevShellPackages = pkgs: [pkgs.android-tools pkgs.curl pkgs.openssl pkgs.unzip];
  extraOutputs = {
    self,
    lib,
    forAllSystems,
    pkgsFor,
    cargoFor,
  }: let
    evalFleet = import ./lib/eval-fleet.nix {inherit lib;};
    mkDevicePackage = import ./lib/mk-device-package.nix {
      inherit nix-manager-core;
    };
    realizeLock = import ./lib/realize-lock.nix;
    mkFleet = import ./lib/mk-fleet.nix {
      inherit nix-manager-core evalFleet mkDevicePackage realizeLock;
    };
    defaultModule = import ./modules/default.nix;
    safeDefaults = import ./profiles/safe-defaults.nix;
  in {
    lib = {
      inherit evalFleet mkFleet mkDevicePackage realizeLock;
    };

    modules.default = defaultModule;
    profiles.safe-defaults = safeDefaults;

    nixosModules.host = {
      pkgs,
      lib,
      ...
    }: {
      environment.systemPackages = [pkgs.android-tools];
    };

    homeManagerModules.default = {
      pkgs,
      lib,
      ...
    }: {
      home.packages = [pkgs.android-tools];
    };

    checks = forAllSystems (system: let
      pkgs = pkgsFor system;
      fixtureApk = pkgs.writeText "fixture.apk" "fake-apk-bytes";
      evaluated = evalFleet {
        modules = [
          defaultModule
          safeDefaults
          ({...}: {
            androidManager.devices.pixel10 = {
              transport = {
                type = "adb";
                selector.serial = "SERIAL";
              };
              expected.supportedAbis = ["arm64-v8a"];
              users.owner = {
                androidUserId = 0;
                apps."org.example.app" = {
                  state = "present";
                  management = "host";
                  source = {
                    type = "local";
                    path = "${fixtureApk}";
                    sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
                  };
                };
                apps."org.fdroid.fdroid" = {
                  source = {
                    type = "fdroid";
                    repository = "https://f-droid.org/repo";
                    identity = "sha256:deadbeef";
                  };
                };
              };
            };
          })
        ];
      };
    in {
      module-evaluation = pkgs.writeText "android-manager-manifest.json" (
        builtins.toJSON evaluated.config.androidManager.manifests.pixel10
      );
    });
  };
}
