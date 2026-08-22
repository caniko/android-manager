use android_manager_model::{is_sha256_hex, sha256_hex, LockedArtifact};
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
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
        if !is_sha256_hex(expected) {
            bail!("invalid artifact sha256");
        }
        if !path.is_file() {
            bail!("artifact path is not a regular file: {}", path.display());
        }
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

    fn ensure_cache(&self) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)?;
        let metadata = fs::symlink_metadata(&self.cache_dir)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("artifact cache is not a directory");
        }
        fs::set_permissions(&self.cache_dir, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn cached(&self, expected: &str) -> Result<Option<PathBuf>> {
        let dest = self.cache_dir.join(expected);
        if let Ok(metadata) = fs::symlink_metadata(&dest) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                bail!("artifact cache entry is not a regular file");
            }
            Self::verify_file(&dest, expected)?;
            return Ok(Some(dest));
        }
        Ok(None)
    }

    fn cache_bytes(&self, bytes: &[u8], expected: &str) -> Result<PathBuf> {
        let actual = sha256_hex(bytes);
        if actual != expected {
            bail!("artifact hash mismatch");
        }
        self.ensure_cache()?;
        if let Some(dest) = self.cached(expected)? {
            return Ok(dest);
        }
        let dest = self.cache_dir.join(expected);
        nix_manager_core::fs::atomic_write_0600(&dest, bytes)?;
        Self::verify_file(&dest, expected)?;
        Ok(dest)
    }
}

impl ArtifactStore for HashingStore {
    fn materialize(&self, artifact: &LockedArtifact) -> Result<PathBuf> {
        if !is_sha256_hex(&artifact.sha256) {
            bail!("invalid artifact sha256");
        }
        if let Some(path) = &artifact.path {
            let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
            return self.cache_bytes(&bytes, &artifact.sha256);
        }
        let url = artifact
            .url
            .as_ref()
            .ok_or_else(|| anyhow!("artifact has neither path nor url"))?;
        if !url.starts_with("https://") || url[8..].contains('@') {
            bail!("artifact URL must be credential-free HTTPS");
        }
        self.ensure_cache()?;
        if let Some(dest) = self.cached(&artifact.sha256)? {
            return Ok(dest);
        }
        let temp = tempfile::NamedTempFile::new_in(&self.cache_dir)?;
        let temp_path = temp
            .path()
            .to_str()
            .ok_or_else(|| anyhow!("artifact cache path is not utf-8"))?;
        let captured = nix_manager_core::exec::cap(
            "curl",
            [
                "-fsSL",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "-o",
                temp_path,
                url,
            ],
        );
        if !captured.ok() {
            bail!("artifact download failed");
        }
        let bytes = fs::read(temp.path())?;
        self.cache_bytes(&bytes, &artifact.sha256)
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

    #[test]
    fn path_artifact_is_copied_into_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.apk");
        let bytes = b"apk";
        fs::write(&source, bytes).unwrap();
        let expected = sha256_hex(bytes);
        let cached = HashingStore::new(dir.path().join("cache"))
            .materialize(&LockedArtifact {
                kind: "apk".into(),
                url: None,
                path: Some(source.clone()),
                sha256: expected,
                size: None,
                abis: Vec::new(),
            })
            .unwrap();

        assert_ne!(cached, source);
        fs::write(source, b"changed").unwrap();
        assert_eq!(fs::read(cached).unwrap(), bytes);
    }
}
