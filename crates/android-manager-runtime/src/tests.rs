use super::*;
use android_manager_artifact::{InspectedPackage, StaticInspector};
use android_manager_device_adb::MemoryDevice;
use android_manager_model::{
    AppState, DesiredApp, DesiredDevice, DesiredUser, DeviceCapabilities, DeviceSelector,
    DeviceSnapshot, ExpectedDevice, ManagementMode, Policies, SourceIntent, Transport,
    Verification, VersionPolicy,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn apk_bytes() -> Vec<u8> {
    b"fake-apk-bytes".to_vec()
}

fn fixture(dir: &Path) -> (PathBuf, Manifest, StaticInspector) {
    let apk = dir.join("app.apk");
    fs::write(&apk, apk_bytes()).unwrap();
    let sha = android_manager_model::sha256_hex(&apk_bytes());
    let inspector = StaticInspector {
        package: InspectedPackage {
            package_name: "org.example.app".into(),
            version_code: 10,
            version_name: Some("1.0".into()),
            signers: vec!["sha256:aa".into()],
            min_sdk: Some(28),
            abis: vec!["arm64-v8a".into()],
        },
    };
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
                            source: Some(SourceIntent {
                                kind: "local".into(),
                                path: Some(apk.clone()),
                                url: None,
                                sha256: Some(sha),
                                repository: None,
                                channel: None,
                            }),
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
    };
    (apk, manifest, inspector)
}

fn empty_snapshot() -> DeviceSnapshot {
    DeviceSnapshot {
        transport_id: "adb:SERIAL".into(),
        serial: "SERIAL".into(),
        manufacturer: Some("Google".into()),
        sdk: 35,
        abis: vec!["arm64-v8a".into()],
        users: vec![0],
        packages: vec![],
        capabilities: DeviceCapabilities::default(),
    }
}

#[test]
fn lock_plan_apply_verify_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let (_apk, manifest, inspector) = fixture(dir.path());
    let device = MemoryDevice::new(empty_snapshot());
    let mut runtime = Runtime {
        manifest,
        lock: LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        },
        lock_path: dir.path().join("android-manager.lock.json"),
        backend: device,
        inspector: Box::new(inspector),
        store: HashingStore::new(dir.path().join("cache")),
        state_dir: dir.path().join("state"),
        confirmer: Box::new(AutoConfirmer),
    };
    runtime.lock().unwrap();
    runtime.realize().unwrap();
    let planned = runtime.plan().unwrap();
    assert_eq!(planned.changes.len(), 1);
    assert_eq!(planned.changes[0].kind, ChangeKind::Create);
    runtime.apply(Some(&planned)).unwrap();
    let verified = runtime.verify().unwrap();
    assert!(verified.automatic_is_empty());
}

#[test]
fn apply_rejects_stale_plan_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let (_apk, manifest, inspector) = fixture(dir.path());
    let device = MemoryDevice::new(empty_snapshot());
    let mut runtime = Runtime {
        manifest,
        lock: LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        },
        lock_path: dir.path().join("android-manager.lock.json"),
        backend: device,
        inspector: Box::new(inspector),
        store: HashingStore::new(dir.path().join("cache")),
        state_dir: dir.path().join("state"),
        confirmer: Box::new(AutoConfirmer),
    };
    runtime.lock().unwrap();
    let mut stale = runtime.plan().unwrap();
    stale.snapshot_digest = "nope".into();
    let err = runtime.apply(Some(&stale)).unwrap_err();
    assert!(err.to_string().contains("bindings changed"));
}
