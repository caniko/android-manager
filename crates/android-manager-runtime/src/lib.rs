pub use android_manager_artifact::{
    ApkSignerInspector, ArtifactInspector, ArtifactStore, HashingStore,
};
pub use android_manager_device_adb::{AdbBackend, DeviceBackend, MemoryDevice, SystemRunner};
pub use android_manager_model::{ExecutionPlan, Manifest};
pub use android_manager_provider_fdroid::{IndexFetcher, LiveJarFetcher, StaticFetcher};
pub use android_manager_provider_release::{
    LiveGithubFetcher, ReleaseFetcher, StaticGithubFetcher,
};

use android_manager_adapter::{
    DelegateRegistry, DelegateRequest, ProviderRegistry, ResolveRequest,
};
use android_manager_delegate_obtainium::ObtainiumProvider;
use android_manager_engine::plan_device_with_delegates;
use android_manager_model::{
    lock_key, manual_resource_id, render_plan_json, AdapterKind, ChangeKind, DelegateOutcome,
    LockEntry, LockFile, ManagementMode, PolicyDecision, SCHEMA_VERSION,
};
use android_manager_plugin_protocol::{ExternalDelegateProvider, ExternalSourceProvider};
use android_manager_provider_fdroid::FdroidProvider;
use android_manager_provider_release::{GithubProvider, HttpProvider, LocalProvider};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Confirmer: Send + Sync {
    fn confirm(&self, prompt: &str, token: &str) -> Result<()>;
}

pub struct TypedConfirmer;

impl Confirmer for TypedConfirmer {
    fn confirm(&self, prompt: &str, token: &str) -> Result<()> {
        eprintln!("{prompt}");
        if nix_manager_core::ui::confirm_typed(&format!("type `{token}` to continue"), token)? {
            Ok(())
        } else {
            bail!("confirmation rejected")
        }
    }
}

pub struct AutoConfirmer;

impl Confirmer for AutoConfirmer {
    fn confirm(&self, _prompt: &str, _token: &str) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    #[serde(default)]
    pub packages: BTreeMap<String, LedgerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub owned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    pub status: String,
    pub completed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

pub struct Runtime<B> {
    pub manifest: Manifest,
    pub lock: LockFile,
    pub lock_path: PathBuf,
    pub backend: B,
    pub inspector: Arc<dyn ArtifactInspector>,
    pub store: HashingStore,
    pub state_dir: PathBuf,
    pub confirmer: Box<dyn Confirmer>,
    pub providers: ProviderRegistry,
    pub delegates: DelegateRegistry,
}

impl<B: DeviceBackend> Runtime<B> {
    pub fn load(
        config_path: &Path,
        backend: B,
        inspector: Box<dyn ArtifactInspector>,
    ) -> Result<Self> {
        let manifest: Manifest = serde_json::from_str(&fs::read_to_string(config_path)?)?;
        validate_manifest_secrets(&manifest)?;
        validate_manifest_sources(&manifest)?;
        let lock_path = manifest
            .lock_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("android-manager.lock.json"));
        let lock = if lock_path.exists() {
            let lock: LockFile = serde_json::from_str(&fs::read_to_string(&lock_path)?)?;
            validate_lock_file(&lock)?;
            lock
        } else {
            LockFile {
                schema_version: SCHEMA_VERSION,
                entries: BTreeMap::new(),
            }
        };
        let state_dir = default_state_dir()?
            .join("devices")
            .join(&manifest.device.name);
        let cache_dir = default_cache_dir()?.join("artifacts");
        let inspector: Arc<dyn ArtifactInspector> = Arc::from(inspector);
        let (providers, delegates) = registries(
            inspector.clone(),
            Arc::new(LiveJarFetcher),
            Arc::new(LiveGithubFetcher),
            &manifest.adapters,
        )?;
        Ok(Self {
            manifest,
            lock,
            lock_path,
            backend,
            inspector,
            store: HashingStore::new(cache_dir),
            state_dir,
            confirmer: Box::new(TypedConfirmer),
            providers,
            delegates,
        })
    }

    pub fn lock(&mut self) -> Result<LockFile> {
        for user in self.manifest.device.users.values() {
            for (package_name, app) in &user.apps {
                if app.management != ManagementMode::Host || app.source.is_none() {
                    continue;
                }
                let source = app.source.as_ref().expect("source checked above");
                let provider = self.providers.resolve(source)?;
                let (key, entry) = provider.resolve(ResolveRequest { package_name, app })?;
                validate_resolved_entry(package_name, app, &key, &entry, self.inspector.as_ref())?;
                self.lock.entries.insert(key, entry);
            }
        }
        self.lock.schema_version = SCHEMA_VERSION;
        write_lock(&self.lock_path, &self.lock)?;
        Ok(self.lock.clone())
    }

    pub fn realize(&mut self) -> Result<()> {
        for (key, entry) in &self.lock.entries {
            validate_lock_entry_shape(entry)?;
            let mut paths = Vec::new();
            for artifact in &entry.artifacts {
                let path = self.store.materialize(artifact)?;
                paths.push(path);
            }
            validate_realized_entry(entry, &paths, self.inspector.as_ref())?;
            if let Some(path) = paths.into_iter().next() {
                self.manifest.realized_artifacts.insert(key.clone(), path);
            }
        }
        Ok(())
    }

    pub fn inventory(&self) -> Result<android_manager_model::DeviceSnapshot> {
        let serial = self
            .backend
            .discover(&self.manifest.device.transport.selector)?;
        let snapshot = self.backend.snapshot(&serial)?;
        if let Some(expected) = &self.manifest.device.expected.manufacturer {
            if snapshot.manufacturer.as_deref() != Some(expected.as_str()) {
                bail!(
                    "device manufacturer {:?} does not match expected {expected}",
                    snapshot.manufacturer
                );
            }
        }
        Ok(snapshot)
    }

    pub fn plan(&self) -> Result<ExecutionPlan> {
        let snapshot = self.inventory()?;
        let delegates = self.delegate_outcomes(&snapshot);
        plan_device_with_delegates(
            &self.manifest.device,
            &self.lock,
            &snapshot,
            &self.manifest.realized_artifacts,
            &delegates,
        )
    }

    pub fn check(&self) -> Result<ExecutionPlan> {
        let plan = self.plan()?;
        let blocked: Vec<_> = plan
            .changes
            .iter()
            .filter(|change| change.kind == ChangeKind::Blocked)
            .map(|change| change.summary.clone())
            .collect();
        if !blocked.is_empty() {
            bail!("check failed: {}", blocked.join("; "));
        }
        Ok(plan)
    }

    pub fn apply(&mut self, expected: Option<&ExecutionPlan>) -> Result<Journal> {
        self.realize()?;
        let plan = self.plan()?;
        if let Some(expected) = expected {
            if expected.config_digest != plan.config_digest
                || expected.lock_digest != plan.lock_digest
                || expected.device_binding != plan.device_binding
                || expected.snapshot_digest != plan.snapshot_digest
            {
                bail!("persisted plan bindings changed; re-run plan");
            }
        }
        if plan
            .changes
            .iter()
            .any(|change| change.kind == ChangeKind::Blocked)
        {
            bail!("refusing to apply a blocked plan");
        }
        for change in &plan.changes {
            if let PolicyDecision::Confirm { token } = &change.policy {
                self.confirmer.confirm(&change.summary, token)?;
            }
            if change.policy.is_denied() {
                bail!("policy denied {}", change.resource);
            }
        }
        let serial = self
            .backend
            .discover(&self.manifest.device.transport.selector)?;
        let mut journal = Journal {
            status: "pending".into(),
            completed: Vec::new(),
            failed: None,
        };
        self.write_journal(&journal)?;
        for change in plan
            .changes
            .iter()
            .filter(|change| change.kind.is_automatic())
        {
            let user_id = user_id_from_resource(&change.resource)?;
            match self.backend.execute(&serial, user_id, &change.operation) {
                Ok(()) => {
                    if let Some(package) = package_from_resource(&change.resource) {
                        self.adopt(&package)?;
                    }
                    journal.completed.push(change.resource.clone());
                    self.write_journal(&journal)?;
                }
                Err(error) => {
                    journal.status = "incomplete".into();
                    journal.failed = Some(error.to_string());
                    self.write_journal(&journal)?;
                    return Err(error);
                }
            }
        }
        journal.status = "ok".into();
        self.write_journal(&journal)?;
        Ok(journal)
    }

    pub fn verify(&self) -> Result<ExecutionPlan> {
        let plan = self.plan()?;
        let blocked = plan
            .changes
            .iter()
            .filter(|change| change.kind == ChangeKind::Blocked)
            .count();
        if blocked > 0 {
            bail!("verify failed: {blocked} blocked changes remain");
        }
        if !plan.automatic_is_empty() {
            bail!(
                "verify failed: {} automatic changes remain",
                plan.automatic_changes().count()
            );
        }
        Ok(plan)
    }

    pub fn render_plan(&self, plan: &ExecutionPlan, json: bool) -> Result<String> {
        if json {
            return Ok(render_plan_json(plan)?);
        }
        Ok(render_plan_human(plan))
    }

    fn adopt(&self, package: &str) -> Result<()> {
        let mut ledger = self.read_ledger()?;
        ledger
            .packages
            .insert(package.to_string(), LedgerEntry { owned: true });
        let path = self.state_dir.join("ledger.json");
        nix_manager_core::fs::atomic_write_0600(
            &path,
            serde_json::to_vec_pretty(&ledger)?.as_slice(),
        )
    }

    fn read_ledger(&self) -> Result<Ledger> {
        let path = self.state_dir.join("ledger.json");
        if !path.exists() {
            return Ok(Ledger::default());
        }
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }

    fn write_journal(&self, journal: &Journal) -> Result<()> {
        let path = self.state_dir.join("journal.json");
        nix_manager_core::fs::atomic_write_0600(
            &path,
            serde_json::to_vec_pretty(journal)?.as_slice(),
        )
    }

    fn delegate_outcomes(
        &self,
        snapshot: &android_manager_model::DeviceSnapshot,
    ) -> BTreeMap<String, DelegateOutcome> {
        let mut outcomes = BTreeMap::new();
        for user in self.manifest.device.users.values() {
            for (package_name, app) in &user.apps {
                if app.management != ManagementMode::Delegated
                    || app.state != android_manager_model::AppState::Present
                    || snapshot.packages.iter().any(|package| {
                        package.name == *package_name && package.user_id == user.android_user_id
                    })
                {
                    continue;
                }
                let resource = manual_resource_id(
                    &self.manifest.device.name,
                    user.android_user_id,
                    package_name,
                );
                let outcome = app
                    .source
                    .as_ref()
                    .ok_or_else(|| anyhow!("delegated app has no source"))
                    .and_then(|source| Ok(self.delegates.resolve(source)?))
                    .and_then(|provider| {
                        let plan = provider.plan(DelegateRequest {
                            device_name: &self.manifest.device.name,
                            user_id: user.android_user_id,
                            package_name,
                            app,
                            snapshot,
                        })?;
                        validate_delegate_plan(&resource, &plan)?;
                        Ok(plan)
                    })
                    .map(|plan| DelegateOutcome::Planned { plan })
                    .unwrap_or_else(|error: anyhow::Error| DelegateOutcome::Blocked {
                        reason: error.to_string(),
                    });
                outcomes.insert(resource, outcome);
            }
        }
        outcomes
    }
}

fn validate_delegate_plan(
    expected_resource: &str,
    plan: &android_manager_model::DelegatePlan,
) -> Result<()> {
    if plan.action.resource != expected_resource || plan.action.instruction.trim().is_empty() {
        bail!("delegate returned an invalid action resource");
    }
    let prerequisites: std::collections::BTreeSet<_> = plan
        .prerequisites
        .iter()
        .map(|prerequisite| prerequisite.resource.as_str())
        .collect();
    if prerequisites.len() != plan.prerequisites.len()
        || plan.prerequisites.iter().any(|prerequisite| {
            prerequisite.resource == expected_resource || prerequisite.instruction.trim().is_empty()
        })
        || plan
            .action
            .depends_on
            .iter()
            .any(|dependency| !prerequisites.contains(dependency.as_str()))
    {
        bail!("delegate returned invalid prerequisite dependencies");
    }
    Ok(())
}

pub fn write_lock(path: &Path, lock: &LockFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    nix_manager_core::fs::atomic_write_0600(
        path,
        (serde_json::to_string_pretty(lock)? + "\n").as_bytes(),
    )
    .with_context(|| format!("write {}", path.display()))
}

fn default_state_dir() -> Result<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|path| path.join("android-manager"))
        .ok_or_else(|| anyhow!("XDG_STATE_HOME or HOME is required"))
}

fn default_cache_dir() -> Result<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|path| path.join("android-manager"))
        .ok_or_else(|| anyhow!("XDG_CACHE_HOME or HOME is required"))
}

pub fn registries(
    inspector: Arc<dyn ArtifactInspector>,
    fetcher: Arc<dyn IndexFetcher>,
    releases: Arc<dyn ReleaseFetcher>,
    external: &BTreeMap<String, android_manager_model::ExternalAdapter>,
) -> Result<(ProviderRegistry, DelegateRegistry)> {
    let mut providers = ProviderRegistry::default();
    providers.register(Arc::new(LocalProvider::new(inspector.clone())))?;
    providers.register(Arc::new(HttpProvider::new(inspector.clone())))?;
    providers.register(Arc::new(GithubProvider::new(inspector, releases)))?;
    providers.register(Arc::new(FdroidProvider::new(fetcher)))?;
    let mut delegates = DelegateRegistry::default();
    delegates.register(Arc::new(ObtainiumProvider))?;
    for (id, adapter) in external {
        match adapter.kind {
            AdapterKind::Source => {
                providers.register(Arc::new(ExternalSourceProvider::new(id, adapter.clone())?))?
            }
            AdapterKind::Delegate => delegates.register(Arc::new(
                ExternalDelegateProvider::new(id, adapter.clone())?,
            ))?,
        }
    }
    Ok((providers, delegates))
}

fn validate_resolved_entry(
    package_name: &str,
    app: &android_manager_model::DesiredApp,
    key: &str,
    entry: &LockEntry,
    inspector: &dyn ArtifactInspector,
) -> Result<()> {
    let source = app
        .source
        .as_ref()
        .ok_or_else(|| anyhow!("missing source"))?;
    if key != lock_key(source, package_name) {
        bail!("provider returned unexpected lock key");
    }
    if entry.package_name != package_name {
        bail!(
            "provider returned package {} for {package_name}",
            entry.package_name
        );
    }
    if entry.source.kind != source.kind
        || (source.kind == "external" && entry.source.adapter != source.adapter)
    {
        bail!("provider returned incompatible locked source identity");
    }
    validate_lock_entry_shape(entry)?;
    if app.verification.require_signer_pin && entry.signers.is_empty() {
        bail!("provider returned no signer pin");
    }
    for artifact in &entry.artifacts {
        if let Some(path) = &artifact.path {
            HashingStore::verify_file(path, &artifact.sha256)?;
        }
    }
    let paths: Vec<_> = entry
        .artifacts
        .iter()
        .filter_map(|artifact| artifact.path.clone())
        .collect();
    if paths.len() == entry.artifacts.len() {
        validate_realized_entry(entry, &paths, inspector)?;
    }
    Ok(())
}

fn validate_lock_file(lock: &LockFile) -> Result<()> {
    if lock.schema_version != SCHEMA_VERSION {
        bail!("unsupported lock schema version {}", lock.schema_version);
    }
    for entry in lock.entries.values() {
        validate_lock_entry_shape(entry)?;
    }
    Ok(())
}

fn validate_manifest_secrets(manifest: &Manifest) -> Result<()> {
    for adapter in manifest.adapters.values() {
        reject_secret_keys(&adapter.config)?;
    }
    for user in manifest.device.users.values() {
        for app in user.apps.values() {
            if let Some(source) = &app.source {
                reject_secret_keys(&source.config)?;
                for value in [&source.url, &source.repository] {
                    if value.as_deref().is_some_and(has_url_credentials) {
                        bail!("credential-bearing source URLs are forbidden");
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_manifest_sources(manifest: &Manifest) -> Result<()> {
    const BUILT_INS: [&str; 5] = ["local", "http", "fdroid", "github", "obtainium"];
    for id in manifest.adapters.keys() {
        if BUILT_INS.contains(&id.as_str()) || id == "external" {
            bail!("external adapter name {id} is reserved");
        }
    }
    for user in manifest.device.users.values() {
        for app in user.apps.values() {
            let Some(source) = &app.source else {
                continue;
            };
            if source.kind != "external" && !BUILT_INS.contains(&source.kind.as_str()) {
                bail!("unknown source type {}", source.kind);
            }
            if source.kind == "obtainium" && app.management != ManagementMode::Delegated {
                bail!("obtainium sources require delegated management");
            }
            if source.kind != "external" {
                if source.adapter.is_some() {
                    bail!("built-in sources must not name an external adapter");
                }
                continue;
            }
            let id = source
                .adapter
                .as_deref()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| anyhow!("external source requires an explicit adapter"))?;
            let adapter = manifest
                .adapters
                .get(id)
                .ok_or_else(|| anyhow!("external source names unknown adapter {id}"))?;
            let compatible = matches!(
                (adapter.kind, app.management),
                (AdapterKind::Source, ManagementMode::Host)
                    | (AdapterKind::Delegate, ManagementMode::Delegated)
            );
            if !compatible {
                bail!("external adapter {id} is incompatible with app management");
            }
        }
    }
    Ok(())
}

fn has_url_credentials(value: &str) -> bool {
    value.split_once("://").is_some_and(|(_, rest)| {
        rest.split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains('@'))
    })
}

fn reject_secret_keys(config: &BTreeMap<String, serde_json::Value>) -> Result<()> {
    fn walk(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
                let key = key.to_ascii_lowercase();
                ["secret", "token", "password", "credential"]
                    .iter()
                    .any(|word| key.contains(word))
                    || walk(value)
            }),
            serde_json::Value::Array(values) => values.iter().any(walk),
            _ => false,
        }
    }

    if config.iter().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        ["secret", "token", "password", "credential"]
            .iter()
            .any(|word| key.contains(word))
            || walk(value)
    }) {
        bail!("secret values are forbidden in adapter configuration");
    }
    Ok(())
}

fn validate_lock_entry_shape(entry: &LockEntry) -> Result<()> {
    if entry.version_code < 0 || entry.package_name.is_empty() || entry.artifacts.is_empty() {
        bail!("lock entry has incomplete version metadata");
    }
    for value in [&entry.source.url, &entry.source.repository] {
        if value.as_deref().is_some_and(has_url_credentials) {
            bail!("credential-bearing lock URLs are forbidden");
        }
    }
    for artifact in &entry.artifacts {
        if !android_manager_model::is_sha256_hex(&artifact.sha256) {
            bail!("lock entry has invalid artifact sha256");
        }
        if artifact.path.is_none() && artifact.url.is_none() {
            bail!("lock artifact has neither path nor url");
        }
        if artifact.url.as_deref().is_some_and(has_url_credentials) {
            bail!("credential-bearing artifact URLs are forbidden");
        }
    }
    Ok(())
}

fn validate_realized_entry(
    entry: &LockEntry,
    paths: &[PathBuf],
    inspector: &dyn ArtifactInspector,
) -> Result<()> {
    if paths.is_empty() {
        bail!("lock entry has no realized artifacts");
    }
    let inspected = inspector.inspect(paths)?;
    if inspected.package_name != entry.package_name
        || inspected.version_code != entry.version_code
        || inspected.signers != entry.signers
    {
        bail!("realized artifact metadata does not match lock entry");
    }
    Ok(())
}

fn user_id_from_resource(resource: &str) -> Result<u32> {
    resource
        .split('/')
        .nth(3)
        .ok_or_else(|| anyhow!("resource {resource} has no user id"))?
        .parse()
        .map_err(|_| anyhow!("resource {resource} has a non-numeric user id"))
}

fn package_from_resource(resource: &str) -> Option<String> {
    let mut parts = resource.split('/');
    (parts.nth(4)? == "package").then(|| parts.next().unwrap_or_default().to_string())
}

pub fn render_plan_human(plan: &ExecutionPlan) -> String {
    let mut out = format!(
        "Device binding: {}\nConfiguration: {}\nLock: {}\n\n",
        plan.device_binding, plan.config_digest, plan.lock_digest
    );
    if plan.changes.is_empty() && plan.unmanaged.is_empty() {
        out.push_str("UNCHANGED\n");
        return out;
    }
    for change in &plan.changes {
        out.push_str(&format!(
            "{:?}\n  {}\n  {}\n\n",
            change.kind, change.resource, change.summary
        ));
    }
    for package in &plan.unmanaged {
        out.push_str(&format!(
            "UNMANAGED\n  {package}\n  Preserved by managed-only policy\n\n"
        ));
    }
    for warning in &plan.warnings {
        out.push_str(&format!("WARN\n  {warning}\n\n"));
    }
    out
}

pub fn lock_key_for_tests(source_kind: &str, package: &str) -> String {
    lock_key(
        &android_manager_model::SourceIntent::new(source_kind),
        package,
    )
}

#[cfg(test)]
mod tests;
