use super::*;
use android_manager_model::{
    Constraints, DelegateOutcome, DelegatePlan, DelegatedAction, DesiredUser, DeviceCapabilities,
    DeviceSelector, DeviceSnapshot, ExpectedDevice, LockedArtifact, LockedSource,
    PrerequisiteResource, SourceIntent, Transport, Verification,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn device(apps: BTreeMap<String, DesiredApp>) -> DesiredDevice {
    DesiredDevice {
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
                apps,
            },
        )]),
        policies: Policies::default(),
    }
}

fn host_app(kind: &str) -> DesiredApp {
    let mut source = SourceIntent::new(kind);
    source.path = Some(PathBuf::from("/tmp/app.apk"));
    source.sha256 = Some("abc".into());
    DesiredApp {
        state: AppState::Present,
        management: ManagementMode::Host,
        source: Some(source),
        version_policy: VersionPolicy::LockedUpgradeOnly,
        verification: Verification::default(),
    }
}

fn lock_file(package: &str, version_code: i64, signers: &[&str], abis: &[&str]) -> LockFile {
    let key = format!("local:{package}");
    LockFile {
        schema_version: 1,
        entries: BTreeMap::from([(
            key,
            LockEntry {
                source: LockedSource {
                    kind: "local".into(),
                    path: Some(PathBuf::from("/tmp/app.apk")),
                    url: None,
                    repository: None,
                    repository_identity: None,
                    adapter: None,
                },
                package_name: package.into(),
                version_name: Some("1.0".into()),
                version_code,
                signers: signers.iter().map(|s| (*s).to_string()).collect(),
                artifacts: vec![LockedArtifact {
                    kind: "apk".into(),
                    url: None,
                    path: Some(PathBuf::from("/tmp/app.apk")),
                    sha256: "abc".into(),
                    size: Some(1),
                    abis: abis.iter().map(|s| (*s).to_string()).collect(),
                }],
                constraints: Constraints { min_sdk: Some(28) },
            },
        )]),
    }
}

fn snapshot(packages: Vec<InstalledPackage>) -> DeviceSnapshot {
    DeviceSnapshot {
        transport_id: "adb:SERIAL".into(),
        serial: "SERIAL".into(),
        manufacturer: Some("Google".into()),
        sdk: 35,
        abis: vec!["arm64-v8a".into()],
        users: vec![0],
        packages,
        capabilities: DeviceCapabilities::default(),
    }
}

fn installed(name: &str, version_code: i64, signer: &str, user_id: u32) -> InstalledPackage {
    InstalledPackage {
        name: name.into(),
        user_id,
        version_code,
        version_name: None,
        signers: vec![signer.into()],
        enabled: true,
    }
}

fn plan(
    apps: BTreeMap<String, DesiredApp>,
    locks: LockFile,
    packages: Vec<InstalledPackage>,
) -> ExecutionPlan {
    plan_device(&device(apps), &locks, &snapshot(packages), &BTreeMap::new()).unwrap()
}

fn delegated_app(kind: &str) -> DesiredApp {
    DesiredApp {
        state: AppState::Present,
        management: ManagementMode::Delegated,
        source: Some(SourceIntent::new(kind)),
        version_policy: VersionPolicy::Presence,
        verification: Verification::default(),
    }
}

fn delegate_outcome(
    package: &str,
    prerequisite: Option<&str>,
) -> BTreeMap<String, DelegateOutcome> {
    let resource = manual_resource_id("pixel10", 0, package);
    let prerequisites = prerequisite
        .map(|dependency| {
            vec![PrerequisiteResource {
                resource: dependency.into(),
                instruction: "Install delegate manager".into(),
            }]
        })
        .unwrap_or_default();
    BTreeMap::from([(
        resource.clone(),
        DelegateOutcome::Planned {
            plan: DelegatePlan {
                action: DelegatedAction {
                    resource,
                    instruction: format!("Delegate {package}"),
                    depends_on: prerequisite.into_iter().map(str::to_string).collect(),
                },
                prerequisites,
            },
        },
    )])
}

#[test]
fn missing_package_installs() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![],
    );
    assert_eq!(result.changes.len(), 1);
    assert_eq!(result.changes[0].kind, ChangeKind::Create);
}

#[test]
fn matching_package_is_noop() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![installed(pkg, 10, "sha256:aa", 0)],
    );
    assert!(result.automatic_is_empty());
    assert!(result.changes.is_empty());
}

#[test]
fn older_package_upgrades() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 11, &["sha256:aa"], &["arm64-v8a"]),
        vec![installed(pkg, 10, "sha256:aa", 0)],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Update);
}

#[test]
fn newer_package_does_not_downgrade() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![installed(pkg, 12, "sha256:aa", 0)],
    );
    assert!(result.automatic_is_empty());
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("newer than the lock")));
}

#[test]
fn signer_mismatch_is_blocked() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![installed(pkg, 9, "sha256:bb", 0)],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Blocked);
    assert!(!result.changes[0].kind.is_automatic());
}

#[test]
fn missing_lock_is_blocked() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        },
        vec![],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Blocked);
    assert!(result.changes[0].summary.contains("missing lock"));
}

#[test]
fn incompatible_abi_is_blocked() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["x86_64"]),
        vec![],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Blocked);
    assert!(result.changes[0].summary.contains("ABI"));
}

#[test]
fn package_on_other_user_is_treated_as_missing() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![installed(pkg, 10, "sha256:aa", 10)],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Create);
}

#[test]
fn unmanaged_extra_is_preserved() {
    let pkg = "org.example.app";
    let result = plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![
            installed(pkg, 10, "sha256:aa", 0),
            installed("com.example.existing", 1, "sha256:zz", 0),
        ],
    );
    assert!(result.automatic_is_empty());
    assert_eq!(result.unmanaged, vec!["com.example.existing".to_string()]);
}

#[test]
fn explicit_removal_requires_confirmation() {
    let pkg = "org.example.app";
    let mut app = host_app("local");
    app.state = AppState::Absent;
    let result = plan(
        BTreeMap::from([(pkg.into(), app)]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![installed(pkg, 10, "sha256:aa", 0)],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Remove);
    assert!(matches!(
        result.changes[0].policy,
        PolicyDecision::Confirm { .. }
    ));
}

#[test]
fn delegated_missing_is_manual() {
    let pkg = "com.revolut.revolut";
    let desired = device(BTreeMap::from([(pkg.into(), delegated_app("delegate"))]));
    let result = plan_device_with_delegates(
        &desired,
        &LockFile::default(),
        &snapshot(Vec::new()),
        &BTreeMap::new(),
        &delegate_outcome(pkg, None),
    )
    .unwrap();
    assert_eq!(result.changes[0].kind, ChangeKind::Manual);
}

#[test]
fn delegated_prerequisite_is_explicitly_ordered() {
    let pkg = "org.example.app";
    let prerequisite = manual_resource_id("pixel10", 0, "delegate.manager");
    let desired = device(BTreeMap::from([(pkg.into(), delegated_app("delegate"))]));
    let result = plan_device_with_delegates(
        &desired,
        &LockFile::default(),
        &snapshot(Vec::new()),
        &BTreeMap::new(),
        &delegate_outcome(pkg, Some(&prerequisite)),
    )
    .unwrap();
    assert_eq!(result.changes.len(), 2);
    assert_eq!(result.changes[0].resource, prerequisite);
    assert_eq!(
        result.changes[1].depends_on,
        [result.changes[0].resource.clone()]
    );
}

#[test]
fn managed_package_satisfies_delegate_prerequisite() {
    let manager = "dev.imranr.obtainium";
    let delegated = "org.example.app";
    let dependency = package_resource_id("pixel10", 0, manager);
    let apps = BTreeMap::from([
        (manager.into(), host_app("local")),
        (delegated.into(), delegated_app("obtainium")),
    ]);
    let result = plan_device_with_delegates(
        &device(apps),
        &lock_file(manager, 10, &["sha256:aa"], &["arm64-v8a"]),
        &snapshot(Vec::new()),
        &BTreeMap::new(),
        &delegate_outcome(delegated, Some(&dependency)),
    )
    .unwrap();

    assert_eq!(result.changes.len(), 2);
    assert_eq!(result.changes[0].resource, dependency);
    assert_eq!(result.changes[0].kind, ChangeKind::Create);
    assert_eq!(result.changes[1].depends_on, [dependency]);
}

#[test]
fn missing_delegate_plan_is_blocked() {
    let pkg = "org.example.app";
    let result = plan_device_with_delegates(
        &device(BTreeMap::from([(pkg.into(), delegated_app("delegate"))])),
        &LockFile::default(),
        &snapshot(Vec::new()),
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(result.changes.len(), 1);
    assert_eq!(result.changes[0].kind, ChangeKind::Blocked);
}

#[test]
fn applying_install_to_model_yields_empty_second_plan() {
    let pkg = "org.example.app";
    let desired = device(BTreeMap::from([(pkg.into(), host_app("local"))]));
    let locks = lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]);
    let mut snap = snapshot(vec![]);
    let first = plan_device(&desired, &locks, &snap, &BTreeMap::new()).unwrap();
    assert_eq!(first.changes[0].kind, ChangeKind::Create);
    snap.packages.push(installed(pkg, 10, "sha256:aa", 0));
    let second = plan_device(&desired, &locks, &snap, &BTreeMap::new()).unwrap();
    assert!(second.automatic_is_empty());
}

#[test]
fn plan_json_is_deterministic() {
    let pkg = "org.example.app";
    let first = android_manager_model::render_plan_json(&plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![],
    ))
    .unwrap();
    let second = android_manager_model::render_plan_json(&plan(
        BTreeMap::from([(pkg.into(), host_app("local"))]),
        lock_file(pkg, 10, &["sha256:aa"], &["arm64-v8a"]),
        vec![],
    ))
    .unwrap();
    assert_eq!(first, second);
}
