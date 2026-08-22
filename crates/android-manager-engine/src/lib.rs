use android_manager_delegate_obtainium::{
    instruction as obtainium_instruction, obtainium_install_instruction, OBTAINIUM_PACKAGE,
};
use android_manager_model::{
    lock_key, manual_resource_id, package_resource_id, AppState, ChangeKind, DesiredApp,
    DesiredDevice, ExecutionPlan, InstalledPackage, LockEntry, LockFile, ManagementMode, Operation,
    PlannedChange, Policies, PolicyDecision, Risk, VersionPolicy,
};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub fn plan_device(
    desired: &DesiredDevice,
    locks: &LockFile,
    snapshot: &android_manager_model::DeviceSnapshot,
    realized: &BTreeMap<String, PathBuf>,
) -> Result<ExecutionPlan> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let declared = declared_packages(desired);
    let mut unmanaged = Vec::new();
    for package in &snapshot.packages {
        if !declared.contains(&package.name) {
            unmanaged.push(package.name.clone());
        }
    }
    unmanaged.sort();
    unmanaged.dedup();

    for (user_name, user) in &desired.users {
        let _ = user_name;
        for (package_name, app) in &user.apps {
            let installed = snapshot
                .packages
                .iter()
                .find(|pkg| pkg.name == *package_name && pkg.user_id == user.android_user_id);
            match plan_app(
                desired,
                locks,
                snapshot,
                realized,
                user.android_user_id,
                package_name,
                app,
                installed,
            )? {
                AppPlan::Change(change) => changes.push(change),
                AppPlan::Warning(message) => warnings.push(message),
                AppPlan::None => {}
            }
        }
    }

    if let Some(user_id) = obtainium_required_user(desired) {
        let installed = snapshot
            .packages
            .iter()
            .any(|pkg| pkg.name == OBTAINIUM_PACKAGE && pkg.user_id == user_id);
        if !installed {
            changes.push(manual(
                desired,
                user_id,
                OBTAINIUM_PACKAGE,
                &desired.policies,
                obtainium_install_instruction(),
            ));
        }
    }

    changes.sort_by(|a, b| {
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| a.resource.cmp(&b.resource))
    });

    Ok(ExecutionPlan {
        schema_version: android_manager_model::SCHEMA_VERSION,
        config_digest: android_manager_model::digest_json(desired)?,
        lock_digest: android_manager_model::digest_json(locks)?,
        device_binding: snapshot.transport_id.clone(),
        snapshot_digest: android_manager_model::digest_json(snapshot)?,
        changes,
        unmanaged,
        warnings,
    })
}

fn kind_rank(kind: ChangeKind) -> u8 {
    match kind {
        ChangeKind::Create | ChangeKind::Update => 0,
        ChangeKind::Remove => 1,
        ChangeKind::Manual | ChangeKind::Blocked => 2,
    }
}

enum AppPlan {
    Change(PlannedChange),
    Warning(String),
    None,
}

#[allow(clippy::too_many_arguments)]
fn plan_app(
    desired: &DesiredDevice,
    locks: &LockFile,
    snapshot: &android_manager_model::DeviceSnapshot,
    realized: &BTreeMap<String, PathBuf>,
    user_id: u32,
    package_name: &str,
    app: &DesiredApp,
    installed: Option<&InstalledPackage>,
) -> Result<AppPlan> {
    if app.state == AppState::Absent {
        return Ok(match installed {
            Some(_) => AppPlan::Change(removal(desired, user_id, package_name, &desired.policies)),
            None => AppPlan::None,
        });
    }

    match app.management {
        ManagementMode::Manual | ManagementMode::Delegated => {
            if installed.is_some() {
                return Ok(AppPlan::None);
            }
            return Ok(AppPlan::Change(manual(
                desired,
                user_id,
                package_name,
                &desired.policies,
                delegated_instruction(package_name, app),
            )));
        }
        ManagementMode::Observe => {
            if installed.is_none() {
                return Ok(AppPlan::Warning(format!(
                    "observed package {package_name} is missing"
                )));
            }
            return Ok(AppPlan::None);
        }
        ManagementMode::Host => {}
    }

    if matches!(
        app.version_policy,
        VersionPolicy::Observe | VersionPolicy::Presence
    ) {
        if installed.is_none() && app.version_policy == VersionPolicy::Presence {
            return host_install(
                desired,
                locks,
                snapshot,
                realized,
                user_id,
                package_name,
                app,
            );
        }
        return Ok(AppPlan::None);
    }

    let Some(source) = &app.source else {
        bail!("host-managed {package_name} has no source");
    };
    let key = lock_key(source, package_name);
    let Some(entry) = locks.entries.get(&key) else {
        return Ok(AppPlan::Change(blocked(
            desired,
            user_id,
            package_name,
            format!("missing lock entry {key}"),
        )));
    };

    if app.verification.require_package_name_match && entry.package_name != package_name {
        return Ok(AppPlan::Change(blocked(
            desired,
            user_id,
            package_name,
            format!(
                "lock package {} does not match declaration {package_name}",
                entry.package_name
            ),
        )));
    }

    if let Some(reason) = compatibility_issue(entry, snapshot) {
        return Ok(AppPlan::Change(blocked(
            desired,
            user_id,
            package_name,
            reason,
        )));
    }

    let paths = artifact_paths(entry, realized, &key);
    match installed {
        None => Ok(AppPlan::Change(install(
            desired,
            user_id,
            package_name,
            entry,
            paths,
        ))),
        Some(current) => {
            compare_installed(desired, user_id, package_name, app, entry, current, paths)
        }
    }
}

fn compare_installed(
    desired: &DesiredDevice,
    user_id: u32,
    package_name: &str,
    app: &DesiredApp,
    entry: &LockEntry,
    current: &InstalledPackage,
    paths: Vec<PathBuf>,
) -> Result<AppPlan> {
    if app.verification.require_signer_pin && !signers_match(&entry.signers, &current.signers) {
        return Ok(AppPlan::Change(blocked(
            desired,
            user_id,
            package_name,
            format!("signer mismatch for {package_name}; source migration required"),
        )));
    }

    if current.version_code == entry.version_code {
        return Ok(AppPlan::None);
    }

    if current.version_code < entry.version_code {
        return Ok(AppPlan::Change(upgrade(
            desired,
            user_id,
            package_name,
            current.version_code,
            entry,
            paths,
        )));
    }

    if app.version_policy == VersionPolicy::Minimum {
        return Ok(AppPlan::None);
    }

    Ok(AppPlan::Warning(format!(
        "{package_name} is newer than the lock ({} > {})",
        current.version_code, entry.version_code
    )))
}

fn host_install(
    desired: &DesiredDevice,
    locks: &LockFile,
    snapshot: &android_manager_model::DeviceSnapshot,
    realized: &BTreeMap<String, PathBuf>,
    user_id: u32,
    package_name: &str,
    app: &DesiredApp,
) -> Result<AppPlan> {
    let Some(source) = &app.source else {
        return Ok(AppPlan::Change(manual(
            desired,
            user_id,
            package_name,
            &desired.policies,
            format!("Install {package_name}; no host source declared"),
        )));
    };
    let key = lock_key(source, package_name);
    let Some(entry) = locks.entries.get(&key) else {
        return Ok(AppPlan::Change(blocked(
            desired,
            user_id,
            package_name,
            format!("missing lock entry {key}"),
        )));
    };
    if let Some(reason) = compatibility_issue(entry, snapshot) {
        return Ok(AppPlan::Change(blocked(
            desired,
            user_id,
            package_name,
            reason,
        )));
    }
    Ok(AppPlan::Change(install(
        desired,
        user_id,
        package_name,
        entry,
        artifact_paths(entry, realized, &key),
    )))
}

fn compatibility_issue(
    entry: &LockEntry,
    snapshot: &android_manager_model::DeviceSnapshot,
) -> Option<String> {
    if let Some(min_sdk) = entry.constraints.min_sdk {
        if snapshot.sdk < min_sdk {
            return Some(format!(
                "device SDK {} is below package minSdk {min_sdk}",
                snapshot.sdk
            ));
        }
    }
    let required: BTreeSet<_> = entry
        .artifacts
        .iter()
        .flat_map(|artifact| artifact.abis.iter())
        .cloned()
        .collect();
    if required.is_empty() {
        return None;
    }
    if required
        .iter()
        .any(|abi| snapshot.abis.iter().any(|have| have == abi))
    {
        return None;
    }
    Some(format!(
        "no compatible ABI: package wants {:?}, device has {:?}",
        required, snapshot.abis
    ))
}

fn artifact_paths(
    entry: &LockEntry,
    realized: &BTreeMap<String, PathBuf>,
    key: &str,
) -> Vec<PathBuf> {
    if let Some(path) = realized.get(key) {
        return vec![path.clone()];
    }
    entry
        .artifacts
        .iter()
        .filter_map(|artifact| artifact.path.clone())
        .collect()
}

fn signers_match(expected: &[String], actual: &[String]) -> bool {
    if expected.is_empty() || actual.is_empty() {
        return false;
    }
    expected.iter().any(|signer| actual.contains(signer))
}

fn install(
    desired: &DesiredDevice,
    user_id: u32,
    package_name: &str,
    entry: &LockEntry,
    artifact_paths: Vec<PathBuf>,
) -> PlannedChange {
    PlannedChange {
        resource: package_resource_id(&desired.name, user_id, package_name),
        kind: ChangeKind::Create,
        summary: format!("Install {package_name} version {}", entry.version_code),
        operation: Operation::Install {
            package_name: package_name.to_string(),
            version_code: entry.version_code,
            artifact_paths,
        },
        risk: Risk::Medium,
        policy: PolicyDecision::Allow,
    }
}

fn upgrade(
    desired: &DesiredDevice,
    user_id: u32,
    package_name: &str,
    from: i64,
    entry: &LockEntry,
    artifact_paths: Vec<PathBuf>,
) -> PlannedChange {
    PlannedChange {
        resource: package_resource_id(&desired.name, user_id, package_name),
        kind: ChangeKind::Update,
        summary: format!("Upgrade {package_name} {from} → {}", entry.version_code),
        operation: Operation::Upgrade {
            package_name: package_name.to_string(),
            from,
            to: entry.version_code,
            artifact_paths,
        },
        risk: Risk::Medium,
        policy: PolicyDecision::Allow,
    }
}

fn removal(
    desired: &DesiredDevice,
    user_id: u32,
    package_name: &str,
    policies: &Policies,
) -> PlannedChange {
    let policy = if policies.removals == "explicit-only" {
        PolicyDecision::Confirm {
            token: package_name.to_string(),
        }
    } else {
        PolicyDecision::Allow
    };
    PlannedChange {
        resource: package_resource_id(&desired.name, user_id, package_name),
        kind: ChangeKind::Remove,
        summary: format!("Uninstall {package_name}"),
        operation: Operation::Uninstall {
            package_name: package_name.to_string(),
        },
        risk: Risk::High,
        policy,
    }
}

fn manual(
    desired: &DesiredDevice,
    user_id: u32,
    package_name: &str,
    policies: &Policies,
    instruction: String,
) -> PlannedChange {
    let policy = if policies.manual_actions == "allow-visible" {
        PolicyDecision::Manual {
            reason: instruction.clone(),
        }
    } else {
        PolicyDecision::Deny {
            reason: instruction.clone(),
        }
    };
    PlannedChange {
        resource: manual_resource_id(&desired.name, user_id, package_name),
        kind: ChangeKind::Manual,
        summary: instruction.clone(),
        operation: Operation::Manual { instruction },
        risk: Risk::Low,
        policy,
    }
}

fn blocked(
    desired: &DesiredDevice,
    user_id: u32,
    package_name: &str,
    reason: String,
) -> PlannedChange {
    PlannedChange {
        resource: package_resource_id(&desired.name, user_id, package_name),
        kind: ChangeKind::Blocked,
        summary: reason.clone(),
        operation: Operation::Blocked { reason },
        risk: Risk::High,
        policy: PolicyDecision::Deny {
            reason: "blocked".into(),
        },
    }
}

fn delegated_instruction(package_name: &str, app: &DesiredApp) -> String {
    match app.source.as_ref() {
        Some(source) if source.kind == "obtainium" => obtainium_instruction(package_name, source),
        _ => format!("Install {package_name} through its delegated or manual source"),
    }
}

fn obtainium_required_user(desired: &DesiredDevice) -> Option<u32> {
    desired.users.values().find_map(|user| {
        user.apps
            .values()
            .any(|app| {
                app.state == AppState::Present
                    && app.management == ManagementMode::Delegated
                    && app
                        .source
                        .as_ref()
                        .is_some_and(|source| source.kind == "obtainium")
            })
            .then_some(user.android_user_id)
    })
}

fn declared_packages(desired: &DesiredDevice) -> BTreeSet<String> {
    desired
        .users
        .values()
        .flat_map(|user| user.apps.keys().cloned())
        .collect()
}

#[cfg(test)]
mod tests;
