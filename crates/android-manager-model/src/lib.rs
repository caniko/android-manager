use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

pub const SCHEMA_VERSION: u32 = 1;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    serde_json::to_string(&value)
}

pub fn digest_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(sha256_hex(canonical_json(value)?.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u32,
    pub device: DesiredDevice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub realized_artifacts: BTreeMap<String, Vec<PathBuf>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adapters: BTreeMap<String, ExternalAdapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredDevice {
    pub name: String,
    pub transport: Transport,
    #[serde(default)]
    pub expected: ExpectedDevice,
    pub users: BTreeMap<String, DesiredUser>,
    #[serde(default)]
    pub policies: Policies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub selector: DeviceSelector,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedDevice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_abis: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredUser {
    pub android_user_id: u32,
    #[serde(default)]
    pub apps: BTreeMap<String, DesiredApp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredApp {
    #[serde(default)]
    pub state: AppState,
    #[serde(default)]
    pub management: ManagementMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceIntent>,
    #[serde(default)]
    pub version_policy: VersionPolicy,
    #[serde(default)]
    pub verification: Verification,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppState {
    #[default]
    Present,
    Absent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagementMode {
    #[default]
    Host,
    Delegated,
    Observe,
    Manual,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionPolicy {
    #[default]
    LockedUpgradeOnly,
    Minimum,
    Presence,
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceIntent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, serde_json::Value>,
}

impl SourceIntent {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            path: None,
            url: None,
            sha256: None,
            repository: None,
            identity: None,
            channel: None,
            adapter: None,
            config: BTreeMap::new(),
        }
    }

    pub fn provider_id(&self) -> Option<&str> {
        if self.kind == "external" {
            self.adapter.as_deref()
        } else {
            Some(&self.kind)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
    Source,
    Delegate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdapterCapability {
    Resolve,
    PlanDelegate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterDescriptor {
    pub id: String,
    pub kind: AdapterKind,
    pub capabilities: BTreeSet<AdapterCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAdapter {
    pub kind: AdapterKind,
    pub executable: PathBuf,
    pub capabilities: BTreeSet<AdapterCapability>,
    #[serde(default)]
    pub environment: AdapterEnvironment,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterEnvironment {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verification {
    #[serde(default = "default_true")]
    pub require_signer_pin: bool,
    #[serde(default = "default_true")]
    pub require_package_name_match: bool,
}

impl Default for Verification {
    fn default() -> Self {
        Self {
            require_signer_pin: true,
            require_package_name_match: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policies {
    #[serde(default = "explicit_only")]
    pub removals: String,
    #[serde(default = "deny_implicit")]
    pub source_migration: String,
    #[serde(default = "require_explicit_migration")]
    pub signer_changes: String,
    #[serde(default = "warn_policy")]
    pub newer_than_lock: String,
    #[serde(default = "allow_visible")]
    pub manual_actions: String,
}

impl Default for Policies {
    fn default() -> Self {
        Self {
            removals: explicit_only(),
            source_migration: deny_implicit(),
            signer_changes: require_explicit_migration(),
            newer_than_lock: warn_policy(),
            manual_actions: allow_visible(),
        }
    }
}

fn explicit_only() -> String {
    "explicit-only".into()
}
fn deny_implicit() -> String {
    "deny-implicit".into()
}
fn require_explicit_migration() -> String {
    "require-explicit-migration".into()
}
fn warn_policy() -> String {
    "warn".into()
}
fn allow_visible() -> String {
    "allow-visible".into()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockFile {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockEntry {
    pub source: LockedSource,
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    pub version_code: i64,
    pub signers: Vec<String>,
    pub artifacts: Vec<LockedArtifact>,
    #[serde(default)]
    pub constraints: Constraints,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedSource {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedArtifact {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub abis: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Constraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_sdk: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub transport_id: String,
    pub serial: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    pub sdk: u32,
    pub abis: Vec<String>,
    #[serde(default)]
    pub users: Vec<u32>,
    #[serde(default)]
    pub packages: Vec<InstalledPackage>,
    #[serde(default)]
    pub capabilities: DeviceCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub name: String,
    pub user_id: u32,
    pub version_code: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_name: Option<String>,
    #[serde(default)]
    pub signers: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub inspected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCapabilities {
    pub install_apk: bool,
    pub install_multiple: bool,
    pub uninstall: bool,
    pub multi_user: bool,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            install_apk: true,
            install_multiple: true,
            uninstall: true,
            multi_user: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPlan {
    pub schema_version: u32,
    pub config_digest: String,
    pub lock_digest: String,
    pub device_binding: String,
    pub snapshot_digest: String,
    pub changes: Vec<PlannedChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmanaged: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ExecutionPlan {
    pub fn automatic_changes(&self) -> impl Iterator<Item = &PlannedChange> {
        self.changes
            .iter()
            .filter(|change| change.kind.is_automatic())
    }

    pub fn automatic_is_empty(&self) -> bool {
        self.automatic_changes().next().is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedChange {
    pub resource: String,
    pub kind: ChangeKind,
    pub summary: String,
    pub operation: Operation,
    pub risk: Risk,
    pub policy: PolicyDecision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatePlan {
    pub action: DelegatedAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<PrerequisiteResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum DelegateOutcome {
    Planned { plan: DelegatePlan },
    Blocked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedAction {
    pub resource: String,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrerequisiteResource {
    pub resource: String,
    pub instruction: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    Create,
    Update,
    Remove,
    Manual,
    Blocked,
}

impl ChangeKind {
    pub fn is_automatic(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Remove)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Operation {
    Install {
        package_name: String,
        version_code: i64,
        #[serde(default)]
        signers: Vec<String>,
        artifact_paths: Vec<PathBuf>,
    },
    Upgrade {
        package_name: String,
        from: i64,
        to: i64,
        #[serde(default)]
        signers: Vec<String>,
        artifact_paths: Vec<PathBuf>,
    },
    Uninstall {
        package_name: String,
    },
    EnableExisting {
        package_name: String,
    },
    Manual {
        instruction: String,
    },
    Blocked {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    Confirm { token: String },
    Manual { reason: String },
}

impl PolicyDecision {
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }
}

pub fn lock_key(source: &SourceIntent, package: &str) -> String {
    match source.kind.as_str() {
        "fdroid" => format!(
            "fdroid:{}:{package}",
            source
                .repository
                .as_deref()
                .unwrap_or("https://f-droid.org/repo")
        ),
        "github" => format!(
            "github:{}:{package}",
            source.repository.as_deref().unwrap_or("unknown")
        ),
        "external" => format!(
            "external:{}:{package}",
            source.adapter.as_deref().unwrap_or("unknown")
        ),
        other => format!("{other}:{package}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDependencyError {
    DuplicateResource(String),
    MissingDependency {
        resource: String,
        dependency: String,
    },
    Cycle(Vec<String>),
}

impl Display for PlanDependencyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateResource(resource) => {
                write!(formatter, "duplicate plan resource {resource}")
            }
            Self::MissingDependency {
                resource,
                dependency,
            } => {
                write!(
                    formatter,
                    "plan resource {resource} depends on missing {dependency}"
                )
            }
            Self::Cycle(resources) => {
                write!(formatter, "plan dependency cycle: {}", resources.join(", "))
            }
        }
    }
}

impl Error for PlanDependencyError {}

pub fn order_changes(
    changes: Vec<PlannedChange>,
) -> Result<Vec<PlannedChange>, PlanDependencyError> {
    let mut by_resource = BTreeMap::new();
    for mut change in changes {
        change.depends_on.sort();
        change.depends_on.dedup();
        let resource = change.resource.clone();
        if by_resource.insert(resource.clone(), change).is_some() {
            return Err(PlanDependencyError::DuplicateResource(resource));
        }
    }
    let mut indegree = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (resource, change) in &by_resource {
        indegree.insert(resource.clone(), change.depends_on.len());
        for dependency in &change.depends_on {
            if !by_resource.contains_key(dependency) {
                return Err(PlanDependencyError::MissingDependency {
                    resource: resource.clone(),
                    dependency: dependency.clone(),
                });
            }
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(resource.clone());
        }
    }
    let mut ready: BTreeSet<(u8, String)> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(resource, _)| (change_rank(by_resource[resource].kind), resource.clone()))
        .collect();
    let mut ordered = Vec::with_capacity(by_resource.len());
    while let Some((_, resource)) = ready.pop_first() {
        let change = by_resource
            .remove(&resource)
            .expect("ready resource exists");
        ordered.push(change);
        for dependent in dependents.get(&resource).into_iter().flatten() {
            let count = indegree
                .get_mut(dependent)
                .expect("dependent indegree exists");
            *count -= 1;
            if *count == 0 {
                ready.insert((change_rank(by_resource[dependent].kind), dependent.clone()));
            }
        }
    }
    if !by_resource.is_empty() {
        return Err(PlanDependencyError::Cycle(
            by_resource.into_keys().collect(),
        ));
    }
    Ok(ordered)
}

fn change_rank(kind: ChangeKind) -> u8 {
    match kind {
        ChangeKind::Create | ChangeKind::Update => 0,
        ChangeKind::Remove => 1,
        ChangeKind::Manual | ChangeKind::Blocked => 2,
    }
}

pub fn package_resource_id(device: &str, user_id: u32, package: &str) -> String {
    format!("device/{device}/user/{user_id}/package/{package}")
}

pub fn manual_resource_id(device: &str, user_id: u32, package: &str) -> String {
    format!("device/{device}/user/{user_id}/manual/{package}")
}

pub fn render_plan_json(plan: &ExecutionPlan) -> Result<String, serde_json::Error> {
    Ok(serde_json::to_string_pretty(plan)? + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable() {
        let first = digest_json(&LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        })
        .unwrap();
        let second = digest_json(&LockFile {
            schema_version: 1,
            entries: BTreeMap::new(),
        })
        .unwrap();
        assert_eq!(first, second);
    }

    fn manual(resource: &str, depends_on: &[&str]) -> PlannedChange {
        PlannedChange {
            resource: resource.into(),
            kind: ChangeKind::Manual,
            summary: resource.into(),
            operation: Operation::Manual {
                instruction: resource.into(),
            },
            risk: Risk::Low,
            policy: PolicyDecision::Manual {
                reason: resource.into(),
            },
            depends_on: depends_on.iter().map(|value| (*value).into()).collect(),
        }
    }

    #[test]
    fn dependencies_have_stable_topological_order() {
        let first =
            order_changes(vec![manual("app", &["manager"]), manual("manager", &[])]).unwrap();
        let second =
            order_changes(vec![manual("manager", &[]), manual("app", &["manager"])]).unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].resource, "manager");
    }

    #[test]
    fn dependency_cycle_is_rejected() {
        let error = order_changes(vec![manual("a", &["b"]), manual("b", &["a"])]).unwrap_err();
        assert_eq!(
            error,
            PlanDependencyError::Cycle(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn phase_one_json_remains_readable() {
        let manifest = r#"{
          "schemaVersion": 1,
          "device": {
            "name": "phone",
            "transport": {"type": "adb", "selector": {}},
            "users": {"owner": {"androidUserId": 0, "apps": {}}}
          }
        }"#;
        let parsed: Manifest = serde_json::from_str(manifest).unwrap();
        assert!(parsed.adapters.is_empty());

        let lock = r#"{
          "schemaVersion": 1,
          "entries": {
            "local:org.example.app": {
              "source": {"type": "local", "path": "/tmp/app.apk"},
              "packageName": "org.example.app",
              "versionCode": 1,
              "signers": ["sha256:aa"],
              "artifacts": [{"kind": "apk", "path": "/tmp/app.apk", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]
            }
          }
        }"#;
        let parsed: LockFile = serde_json::from_str(lock).unwrap();
        assert!(parsed
            .entries
            .values()
            .next()
            .unwrap()
            .source
            .adapter
            .is_none());

        let plan = r#"{
          "schemaVersion": 1,
          "configDigest": "a",
          "lockDigest": "b",
          "deviceBinding": "adb:test",
          "snapshotDigest": "c",
          "changes": [{
            "resource": "device/phone/user/0/package/org.example.app",
            "kind": "create",
            "summary": "install",
            "operation": {"type": "install", "package_name": "org.example.app", "version_code": 1, "artifact_paths": ["/tmp/app.apk"]},
            "risk": "medium",
            "policy": {"decision": "allow"}
          }]
        }"#;
        let parsed: ExecutionPlan = serde_json::from_str(plan).unwrap();
        assert!(parsed.changes[0].depends_on.is_empty());
    }
}
