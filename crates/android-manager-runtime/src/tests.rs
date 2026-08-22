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
                                identity: None,
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
        fetcher: Box::new(StaticFetcher {
            index: android_manager_provider_fdroid::IndexV1::default(),
        }),
        releases: Box::new(StaticGithubFetcher {
            release: Default::default(),
        }),
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
        fetcher: Box::new(StaticFetcher {
            index: android_manager_provider_fdroid::IndexV1::default(),
        }),
        releases: Box::new(StaticGithubFetcher {
            release: Default::default(),
        }),
    };
    runtime.lock().unwrap();
    let mut stale = runtime.plan().unwrap();
    stale.snapshot_digest = "nope".into();
    let err = runtime.apply(Some(&stale)).unwrap_err();
    assert!(err.to_string().contains("bindings changed"));
}

#[test]
fn lock_resolves_fdroid_from_signed_index_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = fixture(dir.path()).1;
    let app = manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.app")
        .unwrap();
    app.source = Some(SourceIntent {
        kind: "fdroid".into(),
        path: None,
        url: None,
        sha256: None,
        repository: Some("https://f-droid.org/repo".into()),
        identity: Some("sha256:deadbeef".into()),
        channel: None,
    });
    let inspector = StaticInspector {
        package: InspectedPackage {
            package_name: "org.example.app".into(),
            version_code: 10,
            version_name: Some("1.0".into()),
            signers: vec!["sha256:abcd".into()],
            min_sdk: Some(28),
            abis: vec!["arm64-v8a".into()],
        },
    };
    let mut runtime = Runtime {
        manifest,
        lock: LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        },
        lock_path: dir.path().join("android-manager.lock.json"),
        backend: MemoryDevice::new(empty_snapshot()),
        inspector: Box::new(inspector),
        store: HashingStore::new(dir.path().join("cache")),
        state_dir: dir.path().join("state"),
        confirmer: Box::new(AutoConfirmer),
        fetcher: Box::new(StaticFetcher {
            index: android_manager_provider_fdroid::IndexV1 {
                apps: vec![android_manager_provider_fdroid::IndexApp {
                    package_name: "org.example.app".into(),
                    suggested_version_code: Some("10".into()),
                }],
                packages: BTreeMap::from([(
                    "org.example.app".into(),
                    vec![android_manager_provider_fdroid::IndexPackage {
                        apk_name: "org.example.app_10.apk".into(),
                        hash: "aaa".into(),
                        hash_type: Some("sha256".into()),
                        version_code: 10,
                        version_name: Some("1.0".into()),
                        signer: Some("abcd".into()),
                        nativecode: vec!["arm64-v8a".into()],
                        min_sdk_version: Some(28),
                        size: Some(4),
                    }],
                )]),
                ..Default::default()
            },
        }),
        releases: Box::new(StaticGithubFetcher {
            release: Default::default(),
        }),
    };
    let lock = runtime.lock().unwrap();
    let entry = lock
        .entries
        .get("fdroid:https://f-droid.org/repo:org.example.app")
        .unwrap();
    assert_eq!(entry.version_code, 10);
    assert_eq!(
        entry.artifacts[0].url.as_deref(),
        Some("https://f-droid.org/repo/org.example.app_10.apk")
    );
}

#[test]
fn lock_resolves_github_release_apk() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = apk_bytes();
    let sha = android_manager_model::sha256_hex(&bytes);
    fs::write(std::env::temp_dir().join(&sha), &bytes).unwrap();
    let mut manifest = fixture(dir.path()).1;
    let app = manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.app")
        .unwrap();
    app.source = Some(SourceIntent {
        kind: "github".into(),
        path: None,
        url: None,
        sha256: None,
        repository: Some("owner/app".into()),
        identity: None,
        channel: Some("latest".into()),
    });
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
    let mut runtime = Runtime {
        manifest,
        lock: LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        },
        lock_path: dir.path().join("android-manager.lock.json"),
        backend: MemoryDevice::new(empty_snapshot()),
        inspector: Box::new(inspector),
        store: HashingStore::new(dir.path().join("cache")),
        state_dir: dir.path().join("state"),
        confirmer: Box::new(AutoConfirmer),
        fetcher: Box::new(StaticFetcher {
            index: android_manager_provider_fdroid::IndexV1::default(),
        }),
        releases: Box::new(StaticGithubFetcher {
            release: android_manager_provider_release::GithubRelease {
                tag_name: Some("v1.0.0".into()),
                assets: vec![android_manager_provider_release::GithubAsset {
                    name: "app.apk".into(),
                    browser_download_url: "https://example.test/app.apk".into(),
                    digest: Some(format!("sha256:{sha}")),
                    size: Some(bytes.len() as u64),
                }],
            },
        }),
    };
    let lock = runtime.lock().unwrap();
    let entry = lock
        .entries
        .get("github:owner/app:org.example.app")
        .unwrap();
    assert_eq!(entry.version_code, 10);
    assert_eq!(
        entry.artifacts[0].url.as_deref(),
        Some("https://example.test/app.apk")
    );
}
