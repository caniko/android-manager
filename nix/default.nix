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
    providerModules = {
      local = ./modules/providers/local.nix;
      http = ./modules/providers/http.nix;
      fdroid = ./modules/providers/fdroid.nix;
      github = ./modules/providers/github.nix;
      obtainium = ./modules/providers/obtainium.nix;
    };
    evalManifests = extra:
      (evalFleet {
        modules = [defaultModule safeDefaults extra];
      }).config.androidManager.manifests;
    evalFails = extra: !(builtins.tryEval (builtins.deepSeq (evalManifests extra) true)).success;
    providerEvalFails = provider: extra:
      !(builtins.tryEval (builtins.deepSeq ((evalFleet {
          modules = [provider safeDefaults extra];
        }).config.androidManager.manifests)
        true)).success;
  in {
    lib = {
      inherit evalFleet mkFleet mkDevicePackage realizeLock;
    };

    modules.default = defaultModule;
    modules.providers = providerModules;
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
      fixtureApk = ../examples/local-apk/app.apk;
      adapterBin = pkgs.writeShellScript "external-adapter" "true";
      evaluated = evalFleet {
        modules = [
          defaultModule
          safeDefaults
          ({...}: {
            androidManager.adapters.custom-index = {
              kind = "source";
              executable = adapterBin;
              capabilities = ["resolve"];
              environment.allow = ["HOME"];
              environment.files.TOKEN = "/run/secrets/token";
              config.repo = "https://example.test/index";
            };
            androidManager.adapters.custom-delegate = {
              kind = "delegate";
              executable = adapterBin;
              capabilities = ["planDelegate"];
            };
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
                apps."org.example.http" = {
                  source = {
                    type = "http";
                    url = "https://example.test/app.apk";
                    sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
                  };
                };
                apps."org.fdroid.fdroid" = {
                  source = {
                    type = "fdroid";
                    repository = "https://f-droid.org/repo";
                    identity = "sha256:deadbeef";
                  };
                };
                apps."org.example.github" = {
                  source = {
                    type = "github";
                    repository = "owner/app";
                    channel = "latest";
                  };
                };
                apps."org.example.obtainium" = {
                  management = "delegated";
                  source = {
                    type = "obtainium";
                    url = "https://github.com/example/app";
                  };
                };
                apps."org.example.external" = {
                  source = {
                    type = "external";
                    adapter = "custom-index";
                    config.channel = "stable";
                  };
                };
                apps."org.example.external-delegate" = {
                  management = "delegated";
                  source = {
                    type = "external";
                    adapter = "custom-delegate";
                  };
                };
              };
            };
          })
        ];
      };
      invalidCases = {
        unknown-adapter = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source = {
            type = "external";
            adapter = "missing";
          };
        });
        wrong-kind = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
          };
          androidManager.devices.d.users.u.apps.p = {
            management = "delegated";
            source = {
              type = "external";
              adapter = "custom-index";
            };
          };
        });
        missing-executable = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            capabilities = ["resolve"];
          };
        });
        non-absolute-executable = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = "adapter";
            capabilities = ["resolve"];
          };
        });
        capability-mismatch = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["planDelegate"];
          };
        });
        invalid-env-name = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
            environment.allow = ["1BAD"];
          };
        });
        store-secret-file = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
            environment.files.TOKEN = "${pkgs.writeText "secret" "nope"}";
          };
        });
        relative-secret-file = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
            environment.files.TOKEN = "secrets/token";
          };
        });
        inline-adapter-token = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
            config.apiToken = "must-not-enter-manifest";
          };
        });
        nested-adapter-token = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
            config.auth.token = "must-not-enter-manifest";
          };
        });
        inline-source-secret = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
          };
          androidManager.devices.d.users.u.apps.p.source = {
            type = "external";
            adapter = "custom-index";
            config.secret = "must-not-enter-manifest";
          };
        });
        nested-source-secret = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
          };
          androidManager.devices.d.users.u.apps.p.source = {
            type = "external";
            adapter = "custom-index";
            config.auth.password = "must-not-enter-manifest";
          };
        });
        builtin-source-secret = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source = {
            type = "github";
            repository = "owner/app";
            config.auth.token = "must-not-enter-manifest";
          };
        });
        credential-bearing-url = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p = {
            management = "delegated";
            source = {
              type = "obtainium";
              url = "https://user:token@example.test/app";
            };
          };
        });
        dangerous-environment = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
            environment.allow = ["LD_PRELOAD"];
          };
        });
        external-source-manual = evalFails ({...}: {
          androidManager.adapters.custom-index = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
          };
          androidManager.devices.d.users.u.apps.p = {
            management = "manual";
            source = {
              type = "external";
              adapter = "custom-index";
            };
          };
        });
        incompatible-local-delegated = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p = {
            management = "delegated";
            source = {
              type = "local";
              path = "${fixtureApk}";
            };
          };
        });
        obtainium-host = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source = {
            type = "obtainium";
            url = "https://example.test/app";
          };
        });
        local-missing-path = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source.type = "local";
        });
        http-missing-sha256 = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source = {
            type = "http";
            url = "https://example.test/app.apk";
          };
        });
        http-empty-values = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source = {
            type = "http";
            url = "";
            sha256 = "";
          };
        });
        fdroid-missing-identity = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source.type = "fdroid";
        });
        github-missing-repository = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source.type = "github";
        });
        external-unnamed = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source.type = "external";
        });
        builtin-adapter-name = evalFails ({...}: {
          androidManager.adapters.local = {
            kind = "source";
            executable = adapterBin;
            capabilities = ["resolve"];
          };
        });
        unknown-source-type = evalFails ({...}: {
          androidManager.devices.d.users.u.apps.p.source.type = "apk";
        });
        unimported-provider = providerEvalFails providerModules.local ({...}: {
          androidManager.devices.d.users.u.apps.p.source = {
            type = "github";
            repository = "owner/app";
          };
        });
      };
      invalidEvalNames =
        lib.mapAttrsToList (
          name: failed:
            if failed
            then name
            else throw "expected ${name} to fail evaluation"
        )
        invalidCases;
      realizedFixture = realizeLock {
        inherit lib pkgs;
        lock = {
          schemaVersion = 1;
          entries.fixture = {
            artifacts = [
              {
                path = "${fixtureApk}";
                sha256 = builtins.hashFile "sha256" fixtureApk;
              }
            ];
          };
        };
      };
    in {
      module-evaluation = pkgs.writeText "android-manager-manifest.json" (
        builtins.toJSON evaluated.config.androidManager.manifests.pixel10
      );
      module-evaluation-invalid = pkgs.writeText "android-manager-invalid-evals.json" (
        builtins.toJSON invalidEvalNames
      );
      lock-realization = builtins.head realizedFixture.fixture;
    });
  };
}
