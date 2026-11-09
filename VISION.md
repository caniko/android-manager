# android-manager vision

android-manager applies declarative Android app state to real devices. A Nix
module tree describes the desired packages per device and user; Rust resolves
each app to an immutable artifact, builds a typed plan, and applies only
automatic changes over ADB. Users are people who already manage their machines
declaratively and want the same answer for a phone or tablet: what is on the
device should be reproducible from the repository, not remembered from manual
taps.

## Problem

Android app installation is imperative and opaque. Sideloaded APKs come from
mutable URLs, package names, signers and versions are checked by hand or not at
all, and two devices drift silently. Reproducing a device means remembering
which APK came from where. Declaring apps in Nix helps only if something turns
the declaration into verified artifacts and a safe sequence of ADB operations.

## Principles

- Mechanism is separated from policy. Nix modules produce a normalized manifest;
  providers resolve it; a pure planner consumes provider-independent model
  types and does not import F-Droid, GitHub, Obtainium or protocol crates.
- Trust is verified, never inferred. F-Droid indexes are checked against a
  repository certificate identity; `http` sources require an SHA-256; GitHub,
  local and external locks are verified for package name, version, signer,
  target compatibility, realized bytes and inspected APK metadata before apply.
- External adapters are processes with an explicit contract, not libraries.
  Protocol version 1 is one JSON line in, one JSON line out, with stable
  envelope semantics; new optional methods may grow without a version bump.
- Adapters are declared, never discovered. No shell, no `$PATH` scan, absolute
  executable paths only, cleared environment, bounded request/stdout/stderr,
  timeouts, and rejection of malformed JSON, capability drift and stray output.
- Secrets are references. Public adapter and app `config` is normalized data;
  credential-looking keys are rejected by the Nix interface and tokens are read
  at runtime from declared environment names or files.
- Plans are explicit graphs. Missing resources, duplicates and cycles are
  rejected; lexical naming is not used to infer dependency order.
- Delegation stays honest. Obtainium-managed apps appear as manual actions with
  their prerequisite resources; android-manager does not modify Obtainium's
  database or claim an importable helper it does not have.

## Scope

- Built-in sources: local APK, exact HTTP URL, verified F-Droid repository,
  GitHub release channel, and Obtainium delegation.
- A capability-checked external adapter protocol (`handshake`, `capabilities`,
  `resolve`, `planDelegate`) with a fixture adapter used by protocol tests.
- A dependency-aware planner, schemas for lock/plan/protocol v1, an ADB device
  backend, and a memory backend for testing.
- Nix fleet/device/user modules and profiles built on `nix-manager-core`.

## Non-goals

- A general Android device manager beyond package state; no claim to manage app
  data, permissions or settings.
- Modifying or migrating another manager's database (Obtainium's schema-v2
  backup).
- Dynamic-library plugins or adapter discovery from the filesystem.
- Future device-backend and artifact-inspector adapter kinds: reserved design
  space, not implemented protocol methods.

## Direction

Phases 1-5 delivered local/HTTP, F-Droid, GitHub and delegated Obtainium support,
then external adapters and a hardened F-Droid trust, lock and apply path. Recent
history is trust and safety work plus environment maintenance rather than new
source types. Anything further must keep the pure planner free of provider
imports and every new source behind the same verification chain.

## Place in the fleet

Canix pins it in `root/hosts/atlas/simit-updater.nix` for dependency updates.
The repository is built on `nix-manager-core`, sharing the manager output shape
with sibling managers, and uses `harbor-rs` for its Rust/Nix build.
