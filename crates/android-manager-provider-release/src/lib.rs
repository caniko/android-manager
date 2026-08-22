use android_manager_adapter::{
    AdapterError, AdapterErrorCode, AdapterResult, ResolveRequest, SourceProvider,
};
use android_manager_artifact::{ArtifactInspector, ArtifactStore, HashingStore};
use android_manager_model::{
    lock_key, AdapterCapability, AdapterDescriptor, AdapterKind, Constraints, DesiredApp,
    LockEntry, LockedArtifact, LockedSource, SourceIntent, SCHEMA_VERSION,
};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

pub struct LocalProvider {
    inspector: Arc<dyn ArtifactInspector>,
}

pub struct HttpProvider {
    inspector: Arc<dyn ArtifactInspector>,
}

pub struct GithubProvider {
    inspector: Arc<dyn ArtifactInspector>,
    fetcher: Arc<dyn ReleaseFetcher>,
}

impl LocalProvider {
    pub fn new(inspector: Arc<dyn ArtifactInspector>) -> Self {
        Self { inspector }
    }
}

impl HttpProvider {
    pub fn new(inspector: Arc<dyn ArtifactInspector>) -> Self {
        Self { inspector }
    }
}

impl GithubProvider {
    pub fn new(inspector: Arc<dyn ArtifactInspector>, fetcher: Arc<dyn ReleaseFetcher>) -> Self {
        Self { inspector, fetcher }
    }
}

macro_rules! source_provider {
    ($provider:ty, $id:literal, $capabilities:expr, $resolve:expr, $validate:expr) => {
        impl SourceProvider for $provider {
            fn descriptor(&self) -> AdapterDescriptor {
                AdapterDescriptor {
                    id: $id.into(),
                    kind: AdapterKind::Source,
                    capabilities: $capabilities,
                }
            }

            fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()> {
                ($validate)(source).map_err(|message| {
                    AdapterError::new(AdapterErrorCode::InvalidConfiguration, $id, message)
                })
            }

            fn supports(&self, source: &SourceIntent) -> bool {
                source.kind == $id
            }

            fn resolve(&self, request: ResolveRequest<'_>) -> AdapterResult<(String, LockEntry)> {
                self.validate_config(request.app.source.as_ref().ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorCode::InvalidConfiguration,
                        $id,
                        format!("{} has no source", request.package_name),
                    )
                })?)?;
                ($resolve)(self, request).map_err(|error: anyhow::Error| {
                    AdapterError::new(AdapterErrorCode::ResolutionFailed, $id, error.to_string())
                })
            }
        }
    };
}

source_provider!(
    LocalProvider,
    "local",
    BTreeSet::from([AdapterCapability::Resolve]),
    |provider: &LocalProvider, request: ResolveRequest<'_>| resolve_local(
        request.package_name,
        request.app,
        request.app.source.as_ref().unwrap(),
        provider.inspector.as_ref(),
    ),
    |source: &SourceIntent| source
        .path
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| "local source requires path".to_string())
);

source_provider!(
    HttpProvider,
    "http",
    BTreeSet::from([AdapterCapability::Resolve]),
    |provider: &HttpProvider, request: ResolveRequest<'_>| resolve_http(
        request.package_name,
        request.app,
        request.app.source.as_ref().unwrap(),
        provider.inspector.as_ref(),
    ),
    |source: &SourceIntent| if source.url.is_some() && source.sha256.is_some() {
        Ok(())
    } else {
        Err("http source requires url and sha256".to_string())
    }
);

source_provider!(
    GithubProvider,
    "github",
    BTreeSet::from([AdapterCapability::Resolve]),
    |provider: &GithubProvider, request: ResolveRequest<'_>| resolve_github(
        request.package_name,
        request.app,
        request.app.source.as_ref().unwrap(),
        provider.inspector.as_ref(),
        provider.fetcher.as_ref(),
    ),
    |source: &SourceIntent| source
        .repository
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| "github source requires repository".to_string())
);

pub fn resolve_host_app(
    package_name: &str,
    app: &DesiredApp,
    inspector: &dyn ArtifactInspector,
    github: Option<&dyn ReleaseFetcher>,
) -> Result<(String, LockEntry)> {
    let source = app
        .source
        .as_ref()
        .ok_or_else(|| anyhow!("{package_name} has no source"))?;
    match source.kind.as_str() {
        "local" => resolve_local(package_name, app, source, inspector),
        "http" => resolve_http(package_name, app, source, inspector),
        "github" => resolve_github(
            package_name,
            app,
            source,
            inspector,
            github.ok_or_else(|| anyhow!("github resolver requires a release fetcher"))?,
        ),
        other => bail!("provider {other} is not implemented"),
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
                adapter: None,
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
    let cache = tempfile::tempdir()?;
    let artifact = LockedArtifact {
        kind: "apk".into(),
        url: Some(url),
        path: None,
        sha256: sha256.clone(),
        size: None,
        abis: Vec::new(),
    };
    let path = HashingStore::new(cache.path().to_path_buf()).materialize(&artifact)?;
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
                adapter: None,
            },
            package_name,
            &inspected,
            LockedArtifact {
                kind: "apk".into(),
                url: artifact.url,
                path: None,
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

pub trait ReleaseFetcher: Send + Sync {
    fn load(&self, repository: &str, channel: &str) -> Result<GithubRelease>;
}

pub struct LiveGithubFetcher;

impl ReleaseFetcher for LiveGithubFetcher {
    fn load(&self, repository: &str, channel: &str) -> Result<GithubRelease> {
        let repo = normalize_repo(repository)?;
        if channel != "latest"
            && (channel.is_empty()
                || !channel
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
        {
            bail!("github channel contains unsupported characters");
        }
        let url = if channel == "latest" {
            format!("https://api.github.com/repos/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{repo}/releases/tags/{channel}")
        };
        let dir = tempfile::tempdir()?;
        let dest = dir.path().join("release.json");
        let dest_str = dest
            .to_str()
            .ok_or_else(|| anyhow!("release path is not utf-8"))?;
        let mut args = vec![
            "-fsSL".into(),
            "--proto".into(),
            "=https".into(),
            "--proto-redir".into(),
            "=https".into(),
            "-A".into(),
            "android-manager".into(),
            "-H".into(),
            "Accept: application/vnd.github+json".into(),
            "-o".into(),
            dest_str.to_string(),
            url.clone(),
        ];
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            if !token.is_empty() {
                if !token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                {
                    bail!("GITHUB_TOKEN contains unsupported characters");
                }
                let header = dir.path().join("github-token-header");
                write_secret_header(&header, &token)?;
                args.insert(args.len() - 1, "-H".into());
                args.insert(args.len() - 1, format!("@{}", header.display()));
            }
        }
        let status = std::process::Command::new("curl")
            .args(args)
            .env_remove("GITHUB_TOKEN")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !status.is_ok_and(|status| status.success()) {
            bail!("github release request failed");
        }
        Ok(serde_json::from_str(&fs::read_to_string(dest)?)?)
    }
}

fn write_secret_header(path: &std::path::Path, token: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "Authorization: Bearer {token}")?;
    Ok(())
}

pub struct StaticGithubFetcher {
    pub release: GithubRelease,
}

impl ReleaseFetcher for StaticGithubFetcher {
    fn load(&self, _repository: &str, _channel: &str) -> Result<GithubRelease> {
        Ok(self.release.clone())
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GithubRelease {
    #[serde(default)]
    pub tag_name: Option<String>,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

pub fn github_artifact(
    package_name: &str,
    source: &SourceIntent,
    fetcher: &dyn ReleaseFetcher,
) -> Result<GithubAsset> {
    let repository = source
        .repository
        .as_deref()
        .ok_or_else(|| anyhow!("github source for {package_name} requires repository"))?;
    let channel = source.channel.as_deref().unwrap_or("latest");
    let release = fetcher.load(repository, channel)?;
    pick_apk(package_name, source, &release.assets)
}

fn resolve_github(
    package_name: &str,
    app: &DesiredApp,
    source: &SourceIntent,
    inspector: &dyn ArtifactInspector,
    fetcher: &dyn ReleaseFetcher,
) -> Result<(String, LockEntry)> {
    let asset = github_artifact(package_name, source, fetcher)?;
    let sha256 = asset_sha256(package_name, source, &asset)?;
    let cache = tempfile::tempdir()?;
    let artifact = LockedArtifact {
        kind: "apk".into(),
        url: Some(asset.browser_download_url.clone()),
        path: None,
        sha256: sha256.clone(),
        size: asset.size,
        abis: Vec::new(),
    };
    let path = HashingStore::new(cache.path().to_path_buf()).materialize(&artifact)?;
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
                kind: "github".into(),
                path: None,
                url: Some(asset.browser_download_url.clone()),
                repository: source.repository.clone(),
                repository_identity: None,
                adapter: None,
            },
            package_name,
            &inspected,
            LockedArtifact {
                kind: "apk".into(),
                url: Some(asset.browser_download_url),
                path: None,
                sha256,
                size: asset.size,
                abis: inspected.abis.clone(),
            },
        ),
    ))
}

fn pick_apk(
    package_name: &str,
    source: &SourceIntent,
    assets: &[GithubAsset],
) -> Result<GithubAsset> {
    let apks: Vec<_> = assets
        .iter()
        .filter(|asset| asset.name.to_ascii_lowercase().ends_with(".apk"))
        .cloned()
        .collect();
    if let Some(name) = source.url.as_deref() {
        return apks
            .into_iter()
            .find(|asset| asset.name == name)
            .ok_or_else(|| anyhow!("{package_name} github release has no apk named {name}"));
    }
    match apks.len() {
        0 => bail!("{package_name} github release has no apk assets"),
        1 => Ok(apks.into_iter().next().unwrap()),
        _ => bail!(
            "{package_name} github release has multiple apks; set source.url to the asset name"
        ),
    }
}

fn asset_sha256(package_name: &str, source: &SourceIntent, asset: &GithubAsset) -> Result<String> {
    if let Some(digest) = asset.digest.as_deref() {
        let hex = digest
            .strip_prefix("sha256:")
            .unwrap_or(digest)
            .to_ascii_lowercase();
        if let Some(expected) = source.sha256.as_deref() {
            if expected != hex {
                bail!("github digest {hex} does not match pinned sha256 {expected}");
            }
        }
        return Ok(hex);
    }
    source.sha256.clone().ok_or_else(|| {
        anyhow!(
            "{package_name} github asset {} has no digest; pin sha256",
            asset.name
        )
    })
}

fn normalize_repo(repository: &str) -> Result<String> {
    let trimmed = repository
        .trim()
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
        .or_else(|| repository.strip_prefix("http://github.com/"))
        .unwrap_or(repository);
    let mut parts = trimmed.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) if valid_repo_part(owner) && valid_repo_part(repo) => {
            Ok(format!("{owner}/{repo}"))
        }
        _ => bail!("github repository must be owner/repo, got {repository}"),
    }
}

fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(assets: Vec<GithubAsset>) -> GithubRelease {
        GithubRelease {
            tag_name: Some("v1.0.0".into()),
            assets,
        }
    }

    fn apk(name: &str, digest: Option<&str>) -> GithubAsset {
        GithubAsset {
            name: name.into(),
            browser_download_url: format!("https://example.test/{name}"),
            digest: digest.map(ToOwned::to_owned),
            size: Some(8),
        }
    }

    fn source(url: Option<&str>, sha256: Option<&str>) -> SourceIntent {
        let mut source = SourceIntent::new("github");
        source.url = url.map(ToOwned::to_owned);
        source.sha256 = sha256.map(ToOwned::to_owned);
        source.repository = Some("owner/app".into());
        source
    }

    #[test]
    fn picks_single_apk() {
        let asset = github_artifact(
            "org.example.app",
            &source(None, None),
            &StaticGithubFetcher {
                release: release(vec![apk("app.apk", Some("sha256:abcd"))]),
            },
        )
        .unwrap();
        assert_eq!(asset.name, "app.apk");
    }

    #[test]
    fn multiple_apks_require_url() {
        let assets = vec![apk("a.apk", None), apk("b.apk", None)];
        let err = github_artifact(
            "org.example.app",
            &source(None, None),
            &StaticGithubFetcher {
                release: release(assets.clone()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("multiple apks"));
        let asset = github_artifact(
            "org.example.app",
            &source(Some("b.apk"), None),
            &StaticGithubFetcher {
                release: release(assets),
            },
        )
        .unwrap();
        assert_eq!(asset.name, "b.apk");
    }

    #[test]
    fn digest_or_pin_required() {
        let source = source(None, None);
        let err = asset_sha256("org.example.app", &source, &apk("app.apk", None)).unwrap_err();
        assert!(err.to_string().contains("pin sha256"));
        assert_eq!(
            asset_sha256(
                "org.example.app",
                &source,
                &apk("app.apk", Some("sha256:AbCd"))
            )
            .unwrap(),
            "abcd"
        );
    }

    #[test]
    fn normalize_github_url() {
        assert_eq!(normalize_repo("caniko/app").unwrap(), "caniko/app");
        assert_eq!(
            normalize_repo("https://github.com/caniko/app/").unwrap(),
            "caniko/app"
        );
        assert!(normalize_repo("nope").is_err());
    }
}
