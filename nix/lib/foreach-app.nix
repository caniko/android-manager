lib: devices:
lib.concatLists (lib.mapAttrsToList (
    deviceName: device:
      lib.concatLists (lib.mapAttrsToList (
          userName: user:
            lib.mapAttrsToList (packageName: app: {
              device = deviceName;
              user = userName;
              package = packageName;
              inherit app;
              loc = "androidManager.devices.${deviceName}.users.${userName}.apps.\"${packageName}\"";
            })
            user.apps
        )
        device.users)
  )
  devices)
