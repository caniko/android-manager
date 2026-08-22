{
  imports = [
    ./core.nix
    ./devices.nix
    ./users.nix
    ./policies.nix
    ./transports/adb.nix
    ./providers/local.nix
    ./providers/http.nix
    ./providers/fdroid.nix
    ./providers/github.nix
    ./providers/obtainium.nix
    ./resources/packages.nix
  ];
}
