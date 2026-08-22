use android_manager_artifact::{InspectedPackage, StaticInspector};
use android_manager_model::{
    AdapterCapability, AdapterEnvironment, AdapterKind, AppState, DesiredApp, DesiredDevice,
    DesiredUser, DeviceCapabilities, DeviceSelector, DeviceSnapshot, ExpectedDevice,
    ExternalAdapter, LockFile, ManagementMode, Policies, SourceIntent, Transport, Verification,
    VersionPolicy,
};
use android_manager_provider_fdroid::StaticFetcher;
use android_manager_provider_release::StaticGithubFetcher;
use android_manager_runtime::{
    registries, ArtifactInspector, AutoConfirmer, HashingStore, Manifest, MemoryDevice, Runtime,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

fn fixture_manifest(
    dir: &Path,
    package_override: Option<&str>,
    signer: &str,
) -> (Manifest, StaticInspector) {
    let bytes = b"external-fixture-apk";
    let apk = dir.join("fixture.apk");
    std::fs::write(&apk, bytes).unwrap();
    let sha256 = android_manager_model::sha256_hex(bytes);
    let mut source = SourceIntent::new("external");
    source.adapter = Some("fixture".into());
    source.config = BTreeMap::from([
        ("artifactPath".into(), serde_json::json!(apk)),
        ("sha256".into(), serde_json::json!(sha256)),
        ("signer".into(), serde_json::json!(signer)),
    ]);
    if let Some(package) = package_override {
        source
            .config
            .insert("packageName".into(), serde_json::json!(package));
    }
    let manifest = Manifest {
        schema_version: 1,
        device: DesiredDevice {
            name: "fixture-device".into(),
            transport: Transport {
                kind: "adb".into(),
                selector: DeviceSelector {
                    serial: Some("FIXTURE".into()),
                },
            },
            expected: ExpectedDevice::default(),
            users: BTreeMap::from([(
                "owner".into(),
                DesiredUser {
                    android_user_id: 0,
                    apps: BTreeMap::from([(
                        "org.example.external".into(),
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
        adapters: BTreeMap::from([(
            "fixture".into(),
            ExternalAdapter {
                kind: AdapterKind::Source,
                executable: env!("CARGO_BIN_EXE_android-manager-fixture-adapter").into(),
                capabilities: BTreeSet::from([AdapterCapability::Resolve]),
                environment: AdapterEnvironment::default(),
                config: BTreeMap::new(),
            },
        )]),
    };
    let inspector = StaticInspector {
        package: InspectedPackage {
            package_name: "org.example.external".into(),
            version_code: 1,
            version_name: Some("fixture".into()),
            signers: vec!["sha256:aa".into()],
            min_sdk: None,
            abis: vec!["arm64-v8a".into()],
        },
    };
    (manifest, inspector)
}

fn runtime(dir: &Path, manifest: Manifest, inspector: StaticInspector) -> Runtime<MemoryDevice> {
    let manifest: Manifest =
        serde_json::from_str(&serde_json::to_string(&manifest).unwrap()).unwrap();
    let inspector: Arc<dyn ArtifactInspector> = Arc::new(inspector);
    let (providers, delegates) = registries(
        inspector.clone(),
        Arc::new(StaticFetcher {
            index: Default::default(),
        }),
        Arc::new(StaticGithubFetcher {
            release: Default::default(),
        }),
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
        backend: MemoryDevice::new(DeviceSnapshot {
            transport_id: "adb:FIXTURE".into(),
            serial: "FIXTURE".into(),
            manufacturer: None,
            sdk: 35,
            abis: vec!["arm64-v8a".into()],
            users: vec![0],
            packages: Vec::new(),
            capabilities: DeviceCapabilities::default(),
        }),
        inspector,
        store: HashingStore::new(dir.join("cache")),
        state_dir: dir.join("state"),
        confirmer: Box::new(AutoConfirmer),
        providers,
        delegates,
    }
}

#[test]
fn external_source_lock_to_idempotent_apply() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) = fixture_manifest(dir.path(), None, "sha256:aa");
    let mut runtime = runtime(dir.path(), manifest, inspector);
    runtime.lock().unwrap();
    runtime.realize().unwrap();
    let plan = runtime.plan().unwrap();
    runtime.apply(Some(&plan)).unwrap();
    assert!(runtime.verify().unwrap().automatic_is_empty());
}

#[test]
fn external_package_and_signer_mismatches_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, inspector) =
        fixture_manifest(dir.path(), Some("org.example.wrong"), "sha256:aa");
    assert!(runtime(dir.path(), manifest, inspector).lock().is_err());

    let (manifest, inspector) = fixture_manifest(dir.path(), None, "sha256:bb");
    assert!(runtime(dir.path(), manifest, inspector).lock().is_err());
}

#[test]
fn external_delegate_plans_through_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let (mut manifest, inspector) = fixture_manifest(dir.path(), None, "sha256:aa");
    let app = manifest
        .device
        .users
        .get_mut("owner")
        .unwrap()
        .apps
        .get_mut("org.example.external")
        .unwrap();
    app.management = ManagementMode::Delegated;
    manifest.adapters.get_mut("fixture").unwrap().kind = AdapterKind::Delegate;
    manifest.adapters.get_mut("fixture").unwrap().capabilities =
        BTreeSet::from([AdapterCapability::PlanDelegate]);

    let runtime = runtime(dir.path(), manifest, inspector);
    let plan = runtime.plan().unwrap();
    assert_eq!(plan.changes.len(), 1);
    assert_eq!(
        plan.changes[0].kind,
        android_manager_model::ChangeKind::Manual
    );
    assert_eq!(
        plan.changes[0].summary,
        "fixture delegate org.example.external"
    );
}
