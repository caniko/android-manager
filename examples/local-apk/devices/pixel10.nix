{...}: {
  androidManager.devices.pixel10 = {
    transport = {
      type = "adb";
      selector.serial = "replace-with-explicit-adb-serial";
    };
    expected.supportedAbis = ["arm64-v8a"];
    users.owner = {
      androidUserId = 0;
      apps."org.example.app" = {
        state = "present";
        management = "host";
        source = {
          type = "local";
          path = ./app.apk;
        };
      };
    };
  };
}
