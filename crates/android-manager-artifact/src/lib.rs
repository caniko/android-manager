use android_manager_model::{sha256_hex, LockedArtifact};
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedPackage {
    pub package_name: String,
    pub version_code: i64,
    pub version_name: Option<String>,
    pub signers: Vec<String>,
    pub min_sdk: Option<u32>,
    pub abis: Vec<String>,
}

pub trait ArtifactInspector: Send + Sync {
    fn inspect(&self, paths: &[PathBuf]) -> Result<InspectedPackage>;
}

pub trait ArtifactStore: Send + Sync {
    fn materialize(&self, artifact: &LockedArtifact) -> Result<PathBuf>;
}

#[derive(Debug, Clone)]
pub struct StaticInspector {
    pub package: InspectedPackage,
}

impl ArtifactInspector for StaticInspector {
    fn inspect(&self, _paths: &[PathBuf]) -> Result<InspectedPackage> {
        Ok(self.package.clone())
    }
}

pub struct HashingStore {
    pub cache_dir: PathBuf,
}

impl HashingStore {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    pub fn verify_file(path: &Path, expected: &str) -> Result<String> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let actual = sha256_hex(&bytes);
        if actual != expected {
            bail!(
                "hash mismatch for {}: expected {expected}, got {actual}",
                path.display()
            );
        }
        Ok(actual)
    }
}

impl ArtifactStore for HashingStore {
    fn materialize(&self, artifact: &LockedArtifact) -> Result<PathBuf> {
        if let Some(path) = &artifact.path {
            Self::verify_file(path, &artifact.sha256)?;
            return Ok(path.clone());
        }
        let url = artifact
            .url
            .as_ref()
            .ok_or_else(|| anyhow!("artifact has neither path nor url"))?;
        fs::create_dir_all(&self.cache_dir)?;
        let dest = self.cache_dir.join(&artifact.sha256);
        if dest.exists() {
            Self::verify_file(&dest, &artifact.sha256)?;
            return Ok(dest);
        }
        let captured =
            nix_manager_core::exec::cap("curl", ["-fsSL", "-o", dest.to_str().unwrap(), url]);
        if !captured.ok() {
            bail!("curl failed for {url}: {}", captured.stderr);
        }
        Self::verify_file(&dest, &artifact.sha256)?;
        Ok(dest)
    }
}

pub struct ApkSignerInspector;

impl ArtifactInspector for ApkSignerInspector {
    fn inspect(&self, paths: &[PathBuf]) -> Result<InspectedPackage> {
        let path = paths
            .first()
            .ok_or_else(|| anyhow!("no artifact paths to inspect"))?;
        let verify = nix_manager_core::exec::cap(
            "apksigner",
            ["verify", "--print-certs", path.to_str().unwrap()],
        );
        if !verify.ok() {
            bail!("apksigner verify failed: {}", verify.stderr);
        }
        let signers = verify
            .stdout
            .lines()
            .filter_map(parse_signer_line)
            .collect::<Vec<_>>();
        let badging =
            nix_manager_core::exec::cap("aapt", ["dump", "badging", path.to_str().unwrap()]);
        if !badging.ok() {
            bail!("aapt dump badging failed: {}", badging.stderr);
        }
        parse_badging(&badging.stdout, signers)
    }
}

fn parse_signer_line(line: &str) -> Option<String> {
    let marker = "SHA-256 digest:";
    let idx = line.find(marker)?;
    let digest = line[idx + marker.len()..].trim().replace(':', "");
    if digest.is_empty() {
        return None;
    }
    Some(format!("sha256:{digest}"))
}

fn parse_badging(output: &str, signers: Vec<String>) -> Result<InspectedPackage> {
    let mut package_name = None;
    let mut version_code = None;
    let mut version_name = None;
    let mut min_sdk = None;
    let mut abis = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("package: ") {
            for part in rest.split_whitespace() {
                if let Some(value) = named_value(part, "name") {
                    package_name = Some(value);
                } else if let Some(value) = named_value(part, "versionCode") {
                    version_code = value.parse().ok();
                } else if let Some(value) = named_value(part, "versionName") {
                    version_name = Some(value);
                }
            }
        } else if let Some(rest) = line.strip_prefix("sdkVersion:'") {
            min_sdk = rest.trim_end_matches('\'').parse().ok();
        } else if let Some(rest) = line.strip_prefix("native-code: ") {
            abis = rest
                .split_whitespace()
                .filter_map(|part| part.strip_prefix('\'').and_then(|p| p.strip_suffix('\'')))
                .map(ToOwned::to_owned)
                .collect();
        }
    }
    Ok(InspectedPackage {
        package_name: package_name.ok_or_else(|| anyhow!("aapt output missing package name"))?,
        version_code: version_code.ok_or_else(|| anyhow!("aapt output missing versionCode"))?,
        version_name,
        signers,
        min_sdk,
        abis,
    })
}

fn named_value(part: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}='");
    part.strip_prefix(&prefix)?
        .strip_suffix('\'')
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_badging_and_signer() {
        let inspected = parse_badging(
            "package: name='org.example.app' versionCode='12' versionName='1.2'\nsdkVersion:'28'\nnative-code: 'arm64-v8a'\n",
            vec!["sha256:abcd".into()],
        )
        .unwrap();
        assert_eq!(inspected.package_name, "org.example.app");
        assert_eq!(inspected.version_code, 12);
        assert_eq!(inspected.min_sdk, Some(28));
        assert_eq!(inspected.abis, vec!["arm64-v8a"]);
        assert_eq!(
            parse_signer_line("Signer #1 certificate SHA-256 digest: ab:cd"),
            Some("sha256:abcd".into())
        );
    }
}
