# android-manager

Declarative Android device manager. Nix declares desired state; Rust locks, plans, and applies it over ADB.

```text
android-manager --config manifest.json check
android-manager --config manifest.json plan
android-manager --config manifest.json apply
```

Phase 1: local and exact-HTTP APKs, ADB install/upgrade, ownership ledger, empty second plan after apply.
