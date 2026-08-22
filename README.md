# android-manager

Declarative Android package management. Nix defines desired state; Rust resolves immutable artifacts, builds a typed plan, and applies automatic changes over ADB.

```text
android-manager --config manifest.json check
android-manager --config manifest.json plan
android-manager --config manifest.json apply
```

## Architecture

The system separates mechanisms from policy:

```text
Nix modules
  -> normalized manifest
  -> source/delegate registries
  -> immutable lock entries or delegated actions
  -> artifact realization and inspection
  -> pure dependency-aware planner
  -> ADB or MemoryDevice apply
```

`SourceProvider` resolves host-managed apps into normalized `LockEntry` values. `DelegateProvider` produces manual actions and prerequisite resources. The pure engine depends only on provider-independent model types; it does not import F-Droid, GitHub, Obtainium, or protocol crates.

Official built-ins are statically linked Rust implementations:

- `local`: local APK with optional SHA-256 pin;
- `http`: exact URL plus required SHA-256;
- `fdroid`: verified `index-v1.jar` plus repository certificate identity;
- `github`: one release APK selected from a pinned tag or `latest` release;
- `obtainium`: delegated manual action with a per-user Obtainium prerequisite.

External adapters use the same registries and capability checks. They do not bypass lock, package-name, signer, version, target-compatibility, hash, realization, or artifact-inspection checks.

## Built-In Sources

```nix
apps."org.example.local" = {
  management = "host";
  source = {
    type = "local";
    path = ./app.apk;
    sha256 = "...";
  };
};

apps."org.fdroid.fdroid" = {
  management = "host";
  source = {
    type = "fdroid";
    repository = "https://f-droid.org/repo";
    identity = "sha256:...";
  };
};

apps."org.example.github" = {
  management = "host";
  source = {
    type = "github";
    repository = "owner/repository";
    channel = "v1.2.3";
    sha256 = "...";
  };
};
```

GitHub authentication is read from `GITHUB_TOKEN` at runtime. The token is written to a mode-0600 temporary curl header file; it is never placed in command arguments, locks, plans, or manifests.

## Delegated Apps

```nix
apps."org.example.app" = {
  management = "delegated";
  source = {
    type = "obtainium";
    repository = "https://github.com/example/app";
  };
};
```

Each Android user gets an independent prerequisite when Obtainium is missing. Several apps for the same user share one prerequisite. Plan ordering follows explicit `dependsOn` edges; lexical naming is not used to infer dependencies.

Obtainium actions are currently internal manual-plan instructions. android-manager does not modify Obtainium's database or claim that its old `{apps: [{id, url}]}` helper is importable. Obtainium's current schema-v2 backup contains full app records with required fields and double-encoded settings, so the incomplete helper was removed.

## External Adapters

External adapters must be declared and selected explicitly:

```nix
androidManager.adapters.myResolver = {
  kind = "source";
  executable = "${pkgs.my-resolver}/bin/my-resolver";
  capabilities = ["resolve"];

  environment.allow = ["MY_RESOLVER_TOKEN"];
  environment.files.MY_RESOLVER_TOKEN_FILE = "/run/secrets/my-resolver-token";

  config.endpoint = "https://example.invalid";
};

androidManager.devices.phone.users.owner.apps."org.example.app" = {
  management = "host";
  source = {
    type = "external";
    adapter = "myResolver";
    config.package = "org.example.app";
  };
};
```

An external delegate uses `kind = "delegate"`, capability `planDelegate`, and `management = "delegated"`.

Unknown source types are rejected. `source.type = "external"` never falls back to `$PATH` discovery and always requires a named adapter. Executables must be absolute paths. Runtime secret files must be absolute paths outside `/nix/store`.

Adapter `config` and app `source.config` are public, normalized data. Keys that look like credentials are rejected by the Nix interface. Secrets may only be referenced through `environment.allow` or `environment.files`; values are read at runtime and are not serialized.

## Protocol Version 1

External adapters are processes, not Rust dynamic libraries. The host starts the configured executable directly, sends one JSON Line on stdin, reads one JSON Line from stdout, and waits for exit.

Methods:

- `handshake`: confirm adapter identity, kind, protocol version, and capabilities;
- `capabilities`: repeat the declared capability contract independently;
- `resolve`: return one normalized source lock result;
- `planDelegate`: return one delegated action and its prerequisite resources.

Request envelope:

```json
{
  "protocolVersion": 1,
  "requestId": "1",
  "method": "handshake",
  "params": { "id": "myResolver", "kind": "source" }
}
```

Success response:

```json
{
  "protocolVersion": 1,
  "requestId": "1",
  "method": "handshake",
  "result": {
    "id": "myResolver",
    "kind": "source",
    "capabilities": ["resolve"]
  }
}
```

Error response:

```json
{
  "protocolVersion": 1,
  "requestId": "1",
  "error": {
    "code": "unsupported-version",
    "message": "unsupported protocol version"
  }
}
```

Version 1 fields and method semantics are stable. New optional capabilities or methods may be added without changing version 1. Incompatible envelope or existing-method changes require a new protocol version. Future device-backend and artifact-inspector adapter kinds are reserved design space, not implemented protocol methods.

### Authoring An Adapter

An adapter should:

1. Read exactly one UTF-8 JSON request line.
2. Reject unsupported versions and unknown methods with a structured error.
3. Echo the request ID exactly once.
4. Write exactly one deterministic JSON response line to stdout.
5. Keep stderr empty on success.
6. Exit zero only after writing a valid response.

The workspace's `android-manager-fixture-adapter` is a complete deterministic example with source and delegate responses plus failure modes used by protocol tests.

## Process Security

The host runner:

- never invokes a shell;
- never scans `$PATH` or plugin directories;
- clears the inherited environment;
- passes only declared environment names and runtime file references;
- bounds request, stdout, and stderr sizes;
- enforces a timeout and kills timed-out children;
- rejects malformed JSON, invalid UTF-8, duplicate responses, mismatched IDs, method/result mismatches, capability drift, stderr output, and nonzero exits;
- renders deterministic errors without including adapter output or environment values.

Adapter output is untrusted. The host verifies lock keys, source identity, package names, version metadata, signer pins, SHA-256 digests, realized bytes, inspected APK metadata, SDK constraints, and ABI compatibility.

## Plan Semantics

Plans contain explicit dependency edges. Stable topological sorting rejects missing resources, duplicate resources, and cycles. Automatic changes are `create`, `update`, and `remove`; delegated and manual changes remain visible after automatic convergence but do not make `verify` report an automatic failure.

## Migration

Phase 5 makes source selection explicit:

- `source.type` is now one of `local`, `http`, `fdroid`, `github`, `obtainium`, or `external`;
- external sources require `source.adapter`;
- adapter-specific public options belong in `source.config`;
- external adapter declarations live in `androidManager.adapters`;
- plan changes may include `dependsOn`;
- external locked sources retain their adapter identity.

Existing Phase 1-4 built-in declarations remain valid. Replace any custom unknown `source.type` string with an explicit external adapter declaration.
