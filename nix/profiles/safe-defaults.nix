{lib, ...}: {
  # Option defaults in policy-type.nix already encode explicit-only / warn / deny-implicit.
  _file = "android-manager.profiles.safe-defaults";
  options = {};
}
