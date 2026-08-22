pub use android_manager_artifact::{
    ApkSignerInspector, ArtifactInspector, ArtifactStore, HashingStore,
};
pub use android_manager_device_adb::{AdbBackend, DeviceBackend, MemoryDevice, SystemRunner};
pub use android_manager_model::{ExecutionPlan, Manifest};
pub use android_manager_provider_fdroid::{IndexFetcher, LiveJarFetcher, StaticFetcher};
pub use android_manager_provider_release::{
    LiveGithubFetcher, ReleaseFetcher, StaticGithubFetcher,
};

use android_manager_engine::plan_device;
use android_manager_model::{
    lock_key, render_plan_json, ChangeKind, LockFile, ManagementMode, PolicyDecision,
    SCHEMA_VERSION,
};
use android_manager_provider_fdroid::resolve as resolve_fdroid;
use android_manager_provider_release::resolve_host_app;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    pub inspector: Box<dyn ArtifactInspector>,
    pub store: HashingStore,
    pub state_dir: PathBuf,
    pub confirmer: Box<dyn Confirmer>,
    pub fetcher: Box<dyn IndexFetcher>,
    pub releases: Box<dyn ReleaseFetcher>,
}

impl<B: DeviceBackend> Runtime<B> {
    pub fn load(
        config_path: &Path,
        backend: B,
        inspector: Box<dyn ArtifactInspector>,
    ) -> Result<Self> {
        let manifest: Manifest = serde_json::from_str(&fs::read_to_string(config_path)?)?;
        let lock_path = manifest
            .lock_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("android-manager.lock.json"));
        let lock = if lock_path.exists() {
            serde_json::from_str(&fs::read_to_string(&lock_path)?)?
        } else {
            LockFile {
                schema_version: SCHEMA_VERSION,
                entries: BTreeMap::new(),
            }
        };
        let state_dir = default_state_dir()
            .join("devices")
            .join(&manifest.device.name);
        let cache_dir = default_cache_dir().join("artifacts");
        Ok(Self {
            manifest,
            lock,
            lock_path,
            backend,
            inspector,
            store: HashingStore::new(cache_dir),
            state_dir,
            confirmer: Box::new(TypedConfirmer),
            fetcher: Box::new(LiveJarFetcher),
            releases: Box::new(LiveGithubFetcher),
        })
    }

    pub fn lock(&mut self) -> Result<LockFile> {
        for user in self.manifest.device.users.values() {
            for (package_name, app) in &user.apps {
                if app.management != ManagementMode::Host || app.source.is_none() {
                    continue;
                }
                let (key, entry) = resolve_app(
                    package_name,
                    app,
                    self.inspector.as_ref(),
                    self.fetcher.as_ref(),
                    self.releases.as_ref(),
                )?;
                self.lock.entries.insert(key, entry);
            }
        }
        self.lock.schema_version = SCHEMA_VERSION;
        write_lock(&self.lock_path, &self.lock)?;
        Ok(self.lock.clone())
    }

    pub fn realize(&mut self) -> Result<()> {
        for (key, entry) in &self.lock.entries {
            for artifact in &entry.artifacts {
                let path = self.store.materialize(artifact)?;
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
        plan_device(
            &self.manifest.device,
            &self.lock,
            &snapshot,
            &self.manifest.realized_artifacts,
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
}

pub fn write_lock(path: &Path, lock: &LockFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(lock)? + "\n")
        .with_context(|| format!("write {}", path.display()))
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
                .join(".local/state")
        })
        .join("android-manager")
}

fn default_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache")
        })
        .join("android-manager")
}

fn resolve_app(
    package_name: &str,
    app: &android_manager_model::DesiredApp,
    inspector: &dyn ArtifactInspector,
    fetcher: &dyn IndexFetcher,
    releases: &dyn ReleaseFetcher,
) -> Result<(String, android_manager_model::LockEntry)> {
    let kind = app
        .source
        .as_ref()
        .map(|source| source.kind.as_str())
        .unwrap_or_default();
    match kind {
        "local" | "http" | "github" => {
            resolve_host_app(package_name, app, inspector, Some(releases))
        }
        "fdroid" => resolve_fdroid(package_name, app, fetcher),
        other => bail!("provider {other} is not implemented"),
    }
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
        &android_manager_model::SourceIntent {
            kind: source_kind.into(),
            path: None,
            url: None,
            sha256: None,
            repository: None,
            identity: None,
            channel: None,
        },
        package,
    )
}

#[cfg(test)]
mod tests;
