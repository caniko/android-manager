# android-manager

Declarative Android device manager. Nix declares desired state; Rust locks, plans, and applies it over ADB.

```text
android-manager --config manifest.json check
android-manager --config manifest.json plan
android-manager --config manifest.json apply
```

Phase 1: local and exact-HTTP APKs, ADB install/upgrade, ownership ledger, empty second plan after apply.

Phase 2: verified F-Droid `index-v1.jar` resolution. Pin `source.identity` to the repository signing-cert SHA-256.

Phase 3: GitHub releases. `source.repository = "owner/repo"`, optional `channel` (tag or `latest`), optional `url` as the asset name when a release has multiple APKs. Uses the GitHub asset digest or a pinned `sha256`.
