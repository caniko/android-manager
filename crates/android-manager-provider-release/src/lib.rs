use android_manager_artifact::{ArtifactInspector, ArtifactStore, HashingStore};
use android_manager_model::{
    lock_key, Constraints, DesiredApp, LockEntry, LockedArtifact, LockedSource,
    ProviderCapabilities, SourceIntent, SCHEMA_VERSION,
};
use anyhow::{anyhow, bail, Result};
use std::fs;
use std::path::PathBuf;

pub fn capabilities_for(kind: &str) -> ProviderCapabilities {
    match kind {
        "local" | "http" => ProviderCapabilities {
            immutable_resolution: true,
            signed_metadata: false,
            multi_artifact: false,
            requires_authentication: false,
            supports_offline_realization: kind == "local",
        },
        _ => ProviderCapabilities::default(),
    }
}

pub fn resolve_host_app(
    package_name: &str,
    app: &DesiredApp,
    inspector: &dyn ArtifactInspector,
) -> Result<(String, LockEntry)> {
    let source = app
        .source
        .as_ref()
        .ok_or_else(|| anyhow!("{package_name} has no source"))?;
    match source.kind.as_str() {
        "local" => resolve_local(package_name, app, source, inspector),
        "http" => resolve_http(package_name, app, source, inspector),
        other => bail!("provider {other} is not implemented in Phase 1"),
    }
}

fn resolve_local(
    package_name: &str,
    app: &DesiredApp,
    source: &SourceIntent,
    inspector: &dyn ArtifactInspector,
) -> Result<(String, LockEntry)> {
    let path = source
        .path
        .clone()
        .ok_or_else(|| anyhow!("local source for {package_name} has no path"))?;
    let sha256 = match &source.sha256 {
        Some(expected) => {
            HashingStore::verify_file(&path, expected)?;
            expected.clone()
        }
        None => HashingStore::verify_file(&path, &hash_file(&path)?)?,
    };
    let inspected = inspector.inspect(std::slice::from_ref(&path))?;
    if app.verification.require_package_name_match && inspected.package_name != package_name {
        bail!(
            "inspected package {} does not match {package_name}",
            inspected.package_name
        );
    }
    Ok((
        lock_key(source, package_name),
        lock_entry(
            LockedSource {
                kind: "local".into(),
                path: Some(path.clone()),
                url: None,
                repository: None,
                repository_identity: None,
            },
            package_name,
            &inspected,
            LockedArtifact {
                kind: "apk".into(),
                url: None,
                path: Some(path),
                sha256,
                size: None,
                abis: inspected.abis.clone(),
            },
        ),
    ))
}

fn resolve_http(
    package_name: &str,
    app: &DesiredApp,
    source: &SourceIntent,
    inspector: &dyn ArtifactInspector,
) -> Result<(String, LockEntry)> {
    let url = source
        .url
        .clone()
        .ok_or_else(|| anyhow!("http source for {package_name} has no url"))?;
    let sha256 = source
        .sha256
        .clone()
        .ok_or_else(|| anyhow!("http source for {package_name} requires sha256"))?;
    let dest = std::env::temp_dir().join(format!("android-manager-{sha256}.apk"));
    let artifact = LockedArtifact {
        kind: "apk".into(),
        url: Some(url),
        path: None,
        sha256: sha256.clone(),
        size: None,
        abis: Vec::new(),
    };
    let path = HashingStore::new(dest.parent().unwrap().to_path_buf()).materialize(&artifact)?;
    let inspected = inspector.inspect(std::slice::from_ref(&path))?;
    if app.verification.require_package_name_match && inspected.package_name != package_name {
        bail!(
            "inspected package {} does not match {package_name}",
            inspected.package_name
        );
    }
    Ok((
        lock_key(source, package_name),
        lock_entry(
            LockedSource {
                kind: "http".into(),
                path: None,
                url: artifact.url.clone(),
                repository: None,
                repository_identity: None,
            },
            package_name,
            &inspected,
            LockedArtifact {
                kind: "apk".into(),
                url: artifact.url,
                path: Some(path),
                sha256,
                size: None,
                abis: inspected.abis.clone(),
            },
        ),
    ))
}

fn lock_entry(
    source: LockedSource,
    package_name: &str,
    inspected: &android_manager_artifact::InspectedPackage,
    artifact: LockedArtifact,
) -> LockEntry {
    let _ = SCHEMA_VERSION;
    LockEntry {
        source,
        package_name: package_name.to_string(),
        version_name: inspected.version_name.clone(),
        version_code: inspected.version_code,
        signers: inspected.signers.clone(),
        artifacts: vec![artifact],
        constraints: Constraints {
            min_sdk: inspected.min_sdk,
        },
    }
}

fn hash_file(path: &PathBuf) -> Result<String> {
    Ok(android_manager_model::sha256_hex(&fs::read(path)?))
}
