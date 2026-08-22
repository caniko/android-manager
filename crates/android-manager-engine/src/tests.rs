use super::*;
use android_manager_model::{
    Constraints, DesiredUser, DeviceCapabilities, DeviceSelector, DeviceSnapshot, ExpectedDevice,
    LockedArtifact, LockedSource, SourceIntent, Transport, Verification,
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
    DesiredApp {
        state: AppState::Present,
        management: ManagementMode::Host,
        source: Some(SourceIntent {
            kind: kind.into(),
            path: Some(PathBuf::from("/tmp/app.apk")),
            url: None,
            sha256: Some("abc".into()),
            repository: None,
            identity: None,
            channel: None,
        }),
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
    let app = DesiredApp {
        state: AppState::Present,
        management: ManagementMode::Delegated,
        source: Some(SourceIntent {
            kind: "play".into(),
            path: None,
            url: None,
            sha256: None,
            repository: None,
            identity: None,
            channel: None,
        }),
        version_policy: VersionPolicy::Minimum,
        verification: Verification {
            require_signer_pin: false,
            require_package_name_match: true,
        },
    };
    let result = plan(
        BTreeMap::from([(pkg.into(), app)]),
        LockFile::default(),
        vec![],
    );
    assert_eq!(result.changes[0].kind, ChangeKind::Manual);
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
