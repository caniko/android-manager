use super::*;
use android_manager_artifact::{InspectedPackage, StaticInspector};
use android_manager_device_adb::MemoryDevice;
use android_manager_model::{
    AppState, DesiredApp, DesiredDevice, DesiredUser, DeviceCapabilities, DeviceSelector,
    DeviceSnapshot, ExpectedDevice, ManagementMode, Policies, SourceIntent, Transport,
    Verification, VersionPolicy,
};
use std::collections::BTreeMap;
use std::sync::Arc;

fn apk_bytes() -> Vec<u8> {
    b"fake-apk-bytes".to_vec()
}

fn inspected(signer: &str) -> InspectedPackage {
    InspectedPackage {
        package_name: "org.example.app".into(),
        version_code: 10,
        version_name: Some("1.0".into()),
        signers: vec![signer.into()],
        min_sdk: Some(28),
        abis: vec!["arm64-v8a".into()],
    }
}

fn fixture(dir: &Path) -> (Manifest, StaticInspector) {
    let apk = dir.join("app.apk");
    fs::write(&apk, apk_bytes()).unwrap();
    let mut source = SourceIntent::new("local");
    source.path = Some(apk);
    source.sha256 = Some(android_manager_model::sha256_hex(&apk_bytes()));
    let manifest = Manifest {
        schema_version: 1,
        device: DesiredDevice {
            name: "pixel10".into(),
            transport: Transport {
                kind: "adb".into(),
                selector: DeviceSelector {
                    serial: Some("SERIAL".into()),
                },
            },
            expected: ExpectedDevice::default(),
            users: BTreeMap::from([(
                "owner".into(),
                DesiredUser {
                    android_user_id: 0,
                    apps: BTreeMap::from([(
                        "org.example.app".into(),
                        DesiredApp {
                            state: AppState::Present,
                            management: ManagementMode::Host,
                            source: Some(source),
                            version_policy: VersionPolicy::LockedUpgradeOnly,
                            verification: Verification::default(),
                        },
                    )]),
                },
            )]),
            policies: Policies::default(),
        },
        lock_path: Some(dir.join("android-manager.lock.json")),
        realized_artifacts: BTreeMap::new(),
        adapters: BTreeMap::new(),
    };
    (
        manifest,
        StaticInspector {
            package: inspected("sha256:aa"),
        },
    )
}

fn empty_snapshot() -> DeviceSnapshot {
    DeviceSnapshot {
        transport_id: "adb:SERIAL".into(),
        serial: "SERIAL".into(),
        manufacturer: Some("Google".into()),
        sdk: 35,
        abis: vec!["arm64-v8a".into()],
        users: vec![0],
        packages: Vec::new(),
        capabilities: DeviceCapabilities::default(),
    }
}

fn runtime(
    dir: &Path,
    manifest: Manifest,
    inspector: StaticInspector,
    fetcher: StaticFetcher,
    releases: StaticGithubFetcher,
) -> Runtime<MemoryDevice> {
    let inspector: Arc<dyn ArtifactInspector> = Arc::new(inspector);
    let (providers, delegates) = registries(
        inspector.clone(),
        Arc::new(fetcher),
        Arc::new(releases),
        &manifest.adapters,
    )
    .unwrap();
    Runtime {
        lock_path: dir.join("android-manager.lock.json"),
        manifest,
        lock: LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        },
        backend: MemoryDevice::new(empty_snapshot()),
        inspector,
        store: HashingStore::new(dir.join("cache")),
        state_dir: dir.join("state"),
        confirmer: Box::new(AutoConfirmer),
        providers,
        delegates,
    }
}

fn default_runtime(
    dir: &Path,
    manifest: Manifest,
    inspector: StaticInspector,
) -> Runtime<MemoryDevice> {
    runtime(
        dir,
        manifest,
        inspector,
        StaticFetcher {
            index: android_manager_provider_fdroid::IndexV1::default(),
        },
        StaticGithubFetcher {
            release: Default::default(),
        },
    )
}

#[test]
fn lock_plan_apply_verify_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) = fixture(dir.path());
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    runtime.lock().unwrap();
    runtime.realize().unwrap();
    let planned = runtime.plan().unwrap();
    assert_eq!(planned.changes.len(), 1);
    assert_eq!(planned.changes[0].kind, ChangeKind::Create);
    runtime.apply(Some(&planned)).unwrap();
    assert!(runtime.verify().unwrap().automatic_is_empty());
}

#[test]
fn apply_rejects_stale_plan_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) = fixture(dir.path());
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    runtime.lock().unwrap();
    let mut stale = runtime.plan().unwrap();
    stale.snapshot_digest = "nope".into();
    assert!(runtime
        .apply(Some(&stale))
        .unwrap_err()
        .to_string()
        .contains("bindings changed"));
}

#[test]
fn lock_resolves_fdroid_from_signed_index_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, inspector) = fixture(dir.path());
    let app = manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.app")
        .unwrap();
    let mut source = SourceIntent::new("fdroid");
    source.repository = Some("https://f-droid.org/repo".into());
    source.identity = Some("sha256:deadbeef".into());
    app.source = Some(source);
    let fetcher = StaticFetcher {
        index: android_manager_provider_fdroid::IndexV1 {
            apps: vec![android_manager_provider_fdroid::IndexApp {
                package_name: "org.example.app".into(),
                suggested_version_code: Some("10".into()),
            }],
            packages: BTreeMap::from([(
                "org.example.app".into(),
                vec![android_manager_provider_fdroid::IndexPackage {
                    apk_name: "org.example.app_10.apk".into(),
                    hash: "a".repeat(64),
                    hash_type: Some("sha256".into()),
                    version_code: 10,
                    version_name: Some("1.0".into()),
                    signer: Some("aa".into()),
                    nativecode: vec!["arm64-v8a".into()],
                    min_sdk_version: Some(28),
                    size: Some(4),
                }],
            )]),
            ..Default::default()
        },
    };
    let mut runtime = runtime(
        dir.path(),
        manifest,
        inspector,
        fetcher,
        StaticGithubFetcher {
            release: Default::default(),
        },
    );
    let lock = runtime.lock().unwrap();
    assert_eq!(
        lock.entries
            .get("fdroid:https://f-droid.org/repo:org.example.app")
            .unwrap()
            .version_code,
        10
    );
}

fn obtainium_app(valid: bool) -> DesiredApp {
    let mut source = SourceIntent::new("obtainium");
    if valid {
        source.repository = Some("https://github.com/example/app".into());
    }
    DesiredApp {
        state: AppState::Present,
        management: ManagementMode::Delegated,
        source: Some(source),
        version_policy: VersionPolicy::Presence,
        verification: Verification::default(),
    }
}

#[test]
fn obtainium_prerequisites_cover_every_user_and_deduplicate() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, inspector) = fixture(dir.path());
    manifest.device.users = BTreeMap::from([
        (
            "owner".into(),
            DesiredUser {
                android_user_id: 0,
                apps: BTreeMap::from([
                    ("org.example.one".into(), obtainium_app(true)),
                    ("org.example.two".into(), obtainium_app(true)),
                ]),
            },
        ),
        (
            "work".into(),
            DesiredUser {
                android_user_id: 10,
                apps: BTreeMap::from([("org.example.work".into(), obtainium_app(true))]),
            },
        ),
    ]);
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    let mut snapshot = empty_snapshot();
    snapshot.users = vec![0, 10];
    runtime.backend = MemoryDevice::new(snapshot);
    let plan = runtime.plan().unwrap();
    assert_eq!(plan.changes.len(), 5);
    let prerequisites: Vec<_> = plan
        .changes
        .iter()
        .filter(|change| change.summary.contains("Install Obtainium"))
        .collect();
    assert_eq!(prerequisites.len(), 2);
    for action in plan
        .changes
        .iter()
        .filter(|change| change.summary.starts_with("Add org.example"))
    {
        let dependency = action.depends_on.first().unwrap();
        assert!(
            plan.changes
                .iter()
                .position(|change| &change.resource == dependency)
                .unwrap()
                < plan
                    .changes
                    .iter()
                    .position(|change| change.resource == action.resource)
                    .unwrap()
        );
    }
}

#[test]
fn one_installed_user_and_one_malformed_app_do_not_suppress_other_users() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, inspector) = fixture(dir.path());
    manifest.device.users = BTreeMap::from([
        (
            "owner".into(),
            DesiredUser {
                android_user_id: 0,
                apps: BTreeMap::from([("org.example.bad".into(), obtainium_app(false))]),
            },
        ),
        (
            "work".into(),
            DesiredUser {
                android_user_id: 10,
                apps: BTreeMap::from([("org.example.good".into(), obtainium_app(true))]),
            },
        ),
    ]);
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    let mut snapshot = empty_snapshot();
    snapshot.users = vec![0, 10];
    snapshot
        .packages
        .push(android_manager_model::InstalledPackage {
            name: android_manager_delegate_obtainium::OBTAINIUM_PACKAGE.into(),
            user_id: 0,
            version_code: 1,
            version_name: None,
            signers: Vec::new(),
            enabled: true,
            inspected: true,
        });
    runtime.backend = MemoryDevice::new(snapshot);
    let plan = runtime.plan().unwrap();
    assert!(plan
        .changes
        .iter()
        .any(|change| change.kind == ChangeKind::Blocked));
    assert!(plan
        .changes
        .iter()
        .any(|change| change.summary.contains("Install Obtainium")));
    assert!(plan
        .changes
        .iter()
        .any(|change| change.summary.contains("org.example.good")));
}

#[test]
fn manual_actions_remain_after_automatic_convergence() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, inspector) = fixture(dir.path());
    manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .insert("org.example.delegate".into(), obtainium_app(true));
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    runtime.lock().unwrap();
    let plan = runtime.plan().unwrap();
    runtime.apply(Some(&plan)).unwrap();
    let verified = runtime.verify().unwrap();
    assert!(verified.automatic_is_empty());
    assert!(verified
        .changes
        .iter()
        .any(|change| change.kind == ChangeKind::Manual));
}

#[test]
fn verify_rejects_blocked_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) = fixture(dir.path());
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    runtime.lock().unwrap();
    let mut snapshot = empty_snapshot();
    snapshot
        .packages
        .push(android_manager_model::InstalledPackage {
            name: "org.example.app".into(),
            user_id: 0,
            version_code: 9,
            version_name: None,
            signers: vec!["sha256:bb".into()],
            enabled: true,
            inspected: true,
        });
    runtime.backend = MemoryDevice::new(snapshot);
    assert!(runtime
        .verify()
        .unwrap_err()
        .to_string()
        .contains("blocked changes"));
}

#[test]
fn nested_secret_config_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, _) = fixture(dir.path());
    manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.app")
        .unwrap()
        .source
        .as_mut()
        .unwrap()
        .config
        .insert("auth".into(), serde_json::json!({"token": "not-allowed"}));
    assert!(validate_manifest_secrets(&manifest).is_err());
}

#[test]
fn credential_bearing_url_is_rejected_without_echoing_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, _) = fixture(dir.path());
    manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.app")
        .unwrap()
        .source
        .as_mut()
        .unwrap()
        .url = Some("https://user:token@example.test/app".into());
    let error = validate_manifest_secrets(&manifest)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("token"));
}

#[test]
fn direct_manifest_rejects_unknown_and_unnamed_external_sources() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, _) = fixture(dir.path());
    {
        let app = manifest
            .device
            .users
            .get_mut("owner")
            .unwrap()
            .apps
            .get_mut("org.example.app")
            .unwrap();
        app.management = ManagementMode::Manual;
        app.source = Some(SourceIntent::new("unknown"));
    }
    assert!(validate_manifest_sources(&manifest).is_err());

    manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.app")
        .unwrap()
        .source = Some(SourceIntent::new("external"));
    assert!(validate_manifest_sources(&manifest).is_err());
}

#[test]
fn use_lock_path_validates_before_replacing() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) = fixture(dir.path());
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    let missing = dir.path().join("missing.lock.json");
    runtime.use_lock_path(missing.clone()).unwrap();
    assert!(runtime.lock.entries.is_empty());

    let bad = dir.path().join("bad.lock.json");
    fs::write(
        &bad,
        r#"{
          "schemaVersion": 1,
          "entries": {
            "local:org.example.app": {
              "source": {"type": "local", "path": "/tmp/app.apk"},
              "packageName": "org.example.app",
              "versionCode": 1,
              "signers": ["sha256:aa"],
              "artifacts": [{"kind": "apk", "path": "/tmp/app.apk", "sha256": "nope"}]
            }
          }
        }"#,
    )
    .unwrap();
    assert!(runtime.use_lock_path(bad).is_err());
    assert_eq!(runtime.lock_path, missing);

    assert!(runtime
        .plan()
        .unwrap_err()
        .to_string()
        .contains("lock file"));
}

#[test]
fn invalid_device_name_is_rejected() {
    assert!(validate_device_name("../escape").is_err());
    assert!(validate_device_name("pixel10").is_ok());
}

#[test]
fn expected_abi_and_user_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, inspector) = fixture(dir.path());
    manifest.device.expected.supported_abis = vec!["x86_64".into()];
    let runtime = default_runtime(dir.path(), manifest, inspector);
    assert!(runtime.inventory().unwrap_err().to_string().contains("ABI"));
}

#[test]
fn signer_change_requires_explicit_migration() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) = fixture(dir.path());
    let mut runtime = default_runtime(dir.path(), manifest, inspector);
    runtime.lock().unwrap();
    runtime
        .lock
        .entries
        .values_mut()
        .for_each(|entry| entry.signers = vec!["sha256:other".into()]);
    assert!(runtime
        .lock()
        .unwrap_err()
        .to_string()
        .contains("signer change"));
}
