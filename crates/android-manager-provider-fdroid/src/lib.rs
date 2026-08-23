use android_manager_adapter::{
    AdapterError, AdapterErrorCode, AdapterResult, ResolveRequest, SourceProvider,
};
use android_manager_model::{
    lock_key, AdapterCapability, AdapterDescriptor, AdapterKind, Constraints, DesiredApp,
    LockEntry, LockedArtifact, LockedSource, SourceIntent,
};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct FdroidProvider {
    fetcher: Arc<dyn IndexFetcher>,
}

impl FdroidProvider {
    pub fn new(fetcher: Arc<dyn IndexFetcher>) -> Self {
        Self { fetcher }
    }
}

impl SourceProvider for FdroidProvider {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            id: "fdroid".into(),
            kind: AdapterKind::Source,
            capabilities: BTreeSet::from([AdapterCapability::Resolve]),
        }
    }

    fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()> {
        if source.identity.is_none() {
            return Err(AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "fdroid",
                "fdroid source requires identity",
            ));
        }
        Ok(())
    }

    fn supports(&self, source: &SourceIntent) -> bool {
        source.kind == "fdroid"
    }

    fn resolve(&self, request: ResolveRequest<'_>) -> AdapterResult<(String, LockEntry)> {
        let source = request.app.source.as_ref().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorCode::InvalidConfiguration,
                "fdroid",
                format!("{} has no source", request.package_name),
            )
        })?;
        self.validate_config(source)?;
        resolve(request.package_name, request.app, self.fetcher.as_ref()).map_err(|error| {
            AdapterError::new(
                AdapterErrorCode::ResolutionFailed,
                "fdroid",
                error.to_string(),
            )
        })
    }
}

pub trait IndexFetcher: Send + Sync {
    fn load(&self, repository: &str, identity: &str) -> Result<IndexV1>;
}

pub struct LiveJarFetcher;

impl IndexFetcher for LiveJarFetcher {
    fn load(&self, repository: &str, identity: &str) -> Result<IndexV1> {
        let repo = repository.trim_end_matches('/');
        require_https(repo)?;
        let url = format!("{repo}/index-v1.jar");
        let dir = tempfile::tempdir().context("fdroid jar tempdir")?;
        let jar = dir.path().join("index-v1.jar");
        let captured = nix_manager_core::exec::cap(
            "curl",
            [
                "--proto",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                "-o",
                jar.to_str()
                    .ok_or_else(|| anyhow!("jar path is not utf-8"))?,
                &url,
            ],
        );
        if !captured.ok() {
            bail!("curl failed for {url}: {}", captured.stderr);
        }
        load_signed_index(&jar, identity)
    }
}

pub struct StaticFetcher {
    pub index: IndexV1,
}

impl IndexFetcher for StaticFetcher {
    fn load(&self, _repository: &str, _identity: &str) -> Result<IndexV1> {
        Ok(self.index.clone())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexV1 {
    #[serde(default)]
    pub repo: IndexRepo,
    #[serde(default)]
    pub apps: Vec<IndexApp>,
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<IndexPackage>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IndexRepo {
    #[serde(default)]
    pub address: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexApp {
    pub package_name: String,
    #[serde(default)]
    pub suggested_version_code: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexPackage {
    pub apk_name: String,
    pub hash: String,
    #[serde(default)]
    pub hash_type: Option<String>,
    pub version_code: i64,
    #[serde(default)]
    pub version_name: Option<String>,
    #[serde(default)]
    pub signer: Option<String>,
    #[serde(default)]
    pub nativecode: Vec<String>,
    #[serde(default)]
    pub min_sdk_version: Option<u32>,
    #[serde(default)]
    pub size: Option<u64>,
}

pub fn resolve(
    package_name: &str,
    app: &DesiredApp,
    fetcher: &dyn IndexFetcher,
) -> Result<(String, LockEntry)> {
    let source = app
        .source
        .as_ref()
        .ok_or_else(|| anyhow!("{package_name} has no source"))?;
    if source.kind != "fdroid" {
        bail!("provider {} is not fdroid", source.kind);
    }
    let repository = source
        .repository
        .as_deref()
        .unwrap_or("https://f-droid.org/repo")
        .trim_end_matches('/');
    require_https(repository)?;
    let identity = source
        .identity
        .as_deref()
        .ok_or_else(|| anyhow!("fdroid source for {package_name} requires identity"))?;
    let index = fetcher.load(repository, identity)?;
    let entry = resolve_from_index(package_name, app, source, repository, identity, &index)?;
    Ok((lock_key(source, package_name), entry))
}

pub fn resolve_from_index(
    package_name: &str,
    app: &DesiredApp,
    source: &SourceIntent,
    repository: &str,
    identity: &str,
    index: &IndexV1,
) -> Result<LockEntry> {
    let packages = index
        .packages
        .get(package_name)
        .ok_or_else(|| anyhow!("{package_name} is not in the F-Droid index"))?;
    let version_code = selected_version(package_name, source.channel.as_deref(), index, packages)?;
    let apk = packages
        .iter()
        .filter(|pkg| pkg.version_code == version_code)
        .min_by_key(|pkg| if pkg.nativecode.is_empty() { 0 } else { 1 })
        .ok_or_else(|| anyhow!("{package_name} has no apk for versionCode {version_code}"))?;
    if apk
        .hash_type
        .as_deref()
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("sha256"))
    {
        bail!("{package_name} apk {} is not sha256-hashed", apk.apk_name);
    }
    if app.verification.require_package_name_match && apk.apk_name.is_empty() {
        bail!("{package_name} index entry is missing apkName");
    }
    let signer = apk
        .signer
        .as_deref()
        .map(normalize_signer)
        .ok_or_else(|| anyhow!("{package_name} index entry is missing signer"))?;
    Ok(LockEntry {
        source: LockedSource {
            kind: "fdroid".into(),
            path: None,
            url: Some(repository.to_string()),
            repository: Some(repository.to_string()),
            repository_identity: Some(identity.to_string()),
            adapter: None,
        },
        package_name: package_name.to_string(),
        version_name: apk.version_name.clone(),
        version_code: apk.version_code,
        signers: vec![signer],
        artifacts: vec![LockedArtifact {
            kind: "apk".into(),
            url: Some(format!("{repository}/{}", apk.apk_name)),
            path: None,
            sha256: apk.hash.clone(),
            size: apk.size,
            abis: apk.nativecode.clone(),
        }],
        constraints: Constraints {
            min_sdk: apk.min_sdk_version,
        },
    })
}

fn selected_version(
    package_name: &str,
    channel: Option<&str>,
    index: &IndexV1,
    packages: &[IndexPackage],
) -> Result<i64> {
    let latest = packages
        .iter()
        .map(|pkg| pkg.version_code)
        .max()
        .ok_or_else(|| anyhow!("{package_name} has no packages in the index"))?;
    match channel.unwrap_or("stable") {
        "latest" => Ok(latest),
        "stable" | "" => {
            let suggested = index
                .apps
                .iter()
                .find(|app| app.package_name == package_name)
                .and_then(|app| app.suggested_version_code.as_deref())
                .and_then(|code| code.parse::<i64>().ok());
            Ok(suggested.unwrap_or(latest))
        }
        other => bail!("unsupported fdroid channel {other}"),
    }
}

fn require_https(url: &str) -> Result<()> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        bail!("fdroid repository must be https")
    }
}

fn normalize_signer(signer: &str) -> String {
    let digest = signer
        .strip_prefix("sha256:")
        .unwrap_or(signer)
        .replace(':', "")
        .to_ascii_lowercase();
    format!("sha256:{digest}")
}

pub fn load_signed_index(jar: &Path, identity: &str) -> Result<IndexV1> {
    let dir = tempfile::tempdir().context("fdroid extract tempdir")?;
    let dest = dir.path();
    let dest_str = dest
        .to_str()
        .ok_or_else(|| anyhow!("extract path is not utf-8"))?;
    let jar_str = jar
        .to_str()
        .ok_or_else(|| anyhow!("jar path is not utf-8"))?;
    let members = jar_members(jar)?;
    for member in &members {
        reject_unsafe_jar_entry(member)?;
    }
    let extract: Vec<&str> = members
        .iter()
        .filter(|member| allowed_jar_entry(member))
        .map(String::as_str)
        .collect();
    if !extract.contains(&"index-v1.json") {
        bail!("jar missing index-v1.json");
    }
    let mut args = vec!["-o", "-qq", jar_str, "-d", dest_str];
    args.extend(extract);
    let unzip = nix_manager_core::exec::cap("unzip", args);
    if !unzip.ok() {
        bail!("unzip failed for {}: {}", jar.display(), unzip.stderr);
    }
    let meta = dest.join("META-INF");
    let (sf, block) = signature_files(&meta)?;
    let actual = signer_identity(&block, &sf)?;
    if actual != normalize_signer(identity) {
        bail!("fdroid repository identity {actual} does not match {identity}");
    }
    let manifest = fs::read_to_string(meta.join("MANIFEST.MF"))?;
    let signed = fs::read_to_string(&sf)?;
    let manifest_digest = header_value(&signed, "SHA-256-Digest-Manifest")
        .ok_or_else(|| anyhow!("signature file missing SHA-256-Digest-Manifest"))?;
    if sha256_hex(fs::read(meta.join("MANIFEST.MF"))?.as_slice())
        != decode_b64_hex(&manifest_digest)?
    {
        bail!("MANIFEST.MF digest does not match the signed .SF");
    }
    let json_name = named_digest(&manifest, "index-v1.json")
        .ok_or_else(|| anyhow!("MANIFEST.MF missing SHA-256-Digest for index-v1.json"))?;
    let index_path = dest.join("index-v1.json");
    if sha256_hex(&fs::read(&index_path)?) != decode_b64_hex(&json_name)? {
        bail!("index-v1.json digest does not match MANIFEST.MF");
    }
    Ok(serde_json::from_slice(&fs::read(index_path)?)?)
}

fn signature_files(meta: &Path) -> Result<(PathBuf, PathBuf)> {
    for entry in fs::read_dir(meta).with_context(|| format!("read {}", meta.display()))? {
        let path = entry?.path();
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "rsa" | "dsa" | "ec") {
            let sf = path.with_extension("SF");
            if sf.exists() {
                return Ok((sf, path));
            }
        }
    }
    bail!("no JAR signature block in {}", meta.display())
}

fn jar_members(jar: &Path) -> Result<Vec<String>> {
    let jar_str = jar
        .to_str()
        .ok_or_else(|| anyhow!("jar path is not utf-8"))?;
    let listed = nix_manager_core::exec::cap("unzip", ["-Z1", jar_str]);
    if !listed.ok() {
        bail!("unzip list failed for {}: {}", jar.display(), listed.stderr);
    }
    Ok(listed
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn reject_unsafe_jar_entry(name: &str) -> Result<()> {
    if name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || name.split('/').any(|part| part == "..")
    {
        bail!("unsafe jar entry {name}");
    }
    Ok(())
}

fn allowed_jar_entry(name: &str) -> bool {
    if name == "index-v1.json" || name == "META-INF/MANIFEST.MF" {
        return true;
    }
    let Some(file) = name.strip_prefix("META-INF/") else {
        return false;
    };
    if file.contains('/') {
        return false;
    }
    matches!(
        Path::new(file)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "sf" | "rsa" | "dsa" | "ec"
    )
}

fn signer_identity(block: &Path, sf: &Path) -> Result<String> {
    let pem = nix_manager_core::exec::cap(
        "openssl",
        [
            "pkcs7",
            "-inform",
            "DER",
            "-in",
            block
                .to_str()
                .ok_or_else(|| anyhow!("signature block path is not utf-8"))?,
            "-print_certs",
        ],
    );
    if !pem.ok() {
        bail!("openssl pkcs7 failed: {}", pem.stderr);
    }
    let certs = pem_certificates(&pem.stdout);
    if certs.is_empty() {
        bail!("pkcs7 output has no certificate");
    }
    let dir = tempfile::tempdir()?;
    let mut matched = Vec::new();
    for (index, cert) in certs.iter().enumerate() {
        let pem_path = dir.path().join(format!("cert-{index}.pem"));
        fs::write(&pem_path, cert)?;
        if cms_verifies(block, sf, &pem_path)? {
            matched.push(cert_sha256(&pem_path)?);
        }
    }
    match matched.as_slice() {
        [identity] => Ok(identity.clone()),
        [] => bail!("openssl cms verify failed: no embedded certificate verified the signature"),
        _ => bail!("fdroid signature has multiple verifying certificates"),
    }
}

fn cms_verifies(block: &Path, sf: &Path, cert: &Path) -> Result<bool> {
    let captured = nix_manager_core::exec::cap(
        "openssl",
        [
            "cms",
            "-verify",
            "-inform",
            "DER",
            "-binary",
            "-in",
            block
                .to_str()
                .ok_or_else(|| anyhow!("signature block path is not utf-8"))?,
            "-content",
            sf.to_str()
                .ok_or_else(|| anyhow!("signature file path is not utf-8"))?,
            "-nointern",
            "-certfile",
            cert.to_str()
                .ok_or_else(|| anyhow!("certificate path is not utf-8"))?,
            "-noverify",
            "-out",
            "/dev/null",
        ],
    );
    Ok(captured.ok())
}

fn cert_sha256(pem_path: &Path) -> Result<String> {
    let dir = tempfile::tempdir()?;
    let der_path = dir.path().join("cert.der");
    let der = nix_manager_core::exec::cap(
        "openssl",
        [
            "x509",
            "-in",
            pem_path
                .to_str()
                .ok_or_else(|| anyhow!("certificate path is not utf-8"))?,
            "-outform",
            "DER",
            "-out",
            der_path
                .to_str()
                .ok_or_else(|| anyhow!("der path is not utf-8"))?,
        ],
    );
    if !der.ok() {
        bail!("openssl x509 failed: {}", der.stderr);
    }
    Ok(normalize_signer(&sha256_hex(&fs::read(der_path)?)))
}

fn pem_certificates(text: &str) -> Vec<String> {
    let mut certs = Vec::new();
    let mut rest = text;
    while let Some(begin) = rest.find("-----BEGIN CERTIFICATE-----") {
        let slice = &rest[begin..];
        let Some(end) = slice.find("-----END CERTIFICATE-----") else {
            break;
        };
        certs.push(slice[..end + "-----END CERTIFICATE-----".len()].to_string());
        rest = &slice[end + "-----END CERTIFICATE-----".len()..];
    }
    certs
}

fn named_digest(manifest: &str, name: &str) -> Option<String> {
    let mut take = false;
    for line in manifest.lines() {
        if let Some(value) = line.strip_prefix("Name: ") {
            take = value.trim() == name;
        } else if take {
            if let Some(value) = line.strip_prefix("SHA-256-Digest: ") {
                return Some(value.trim().into());
            }
        }
    }
    None
}

fn header_value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix).map(|value| value.trim().into()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn decode_b64_hex(input: &str) -> Result<String> {
    Ok(decode_b64(input.trim())?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn decode_b64(input: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Result<u8> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => bail!("invalid base64"),
        }
    }
    let filtered: Vec<u8> = input
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .collect();
    let mut out = Vec::with_capacity(filtered.len() * 3 / 4);
    for chunk in filtered.chunks(4) {
        let a = val(chunk[0])?;
        let b = val(*chunk.get(1).ok_or_else(|| anyhow!("truncated base64"))?)?;
        out.push((a << 2) | (b >> 4));
        if chunk.len() > 2 {
            let c = val(chunk[2])?;
            out.push((b << 4) | (c >> 2));
            if chunk.len() > 3 {
                let d = val(chunk[3])?;
                out.push((c << 6) | d);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use android_manager_model::{AppState, ManagementMode, Verification, VersionPolicy};

    fn index() -> IndexV1 {
        IndexV1 {
            repo: IndexRepo {
                address: Some("https://f-droid.org/repo".into()),
            },
            apps: vec![IndexApp {
                package_name: "org.example.app".into(),
                suggested_version_code: Some("10".into()),
            }],
            packages: BTreeMap::from([(
                "org.example.app".into(),
                vec![
                    IndexPackage {
                        apk_name: "org.example.app_10.apk".into(),
                        hash: "aaa".into(),
                        hash_type: Some("sha256".into()),
                        version_code: 10,
                        version_name: Some("1.0".into()),
                        signer: Some("abcd".into()),
                        nativecode: vec!["arm64-v8a".into()],
                        min_sdk_version: Some(28),
                        size: Some(4),
                    },
                    IndexPackage {
                        apk_name: "org.example.app_11.apk".into(),
                        hash: "bbb".into(),
                        hash_type: Some("sha256".into()),
                        version_code: 11,
                        version_name: Some("1.1".into()),
                        signer: Some("abcd".into()),
                        nativecode: vec![],
                        min_sdk_version: Some(28),
                        size: Some(5),
                    },
                ],
            )]),
        }
    }

    fn app(channel: Option<&str>) -> DesiredApp {
        let mut source = SourceIntent::new("fdroid");
        source.repository = Some("https://f-droid.org/repo".into());
        source.identity = Some("sha256:deadbeef".into());
        source.channel = channel.map(ToOwned::to_owned);
        DesiredApp {
            state: AppState::Present,
            management: ManagementMode::Host,
            source: Some(source),
            version_policy: VersionPolicy::LockedUpgradeOnly,
            verification: Verification::default(),
        }
    }

    #[test]
    fn stable_uses_suggested_version() {
        let app = app(None);
        let (key, entry) =
            resolve("org.example.app", &app, &StaticFetcher { index: index() }).unwrap();
        assert_eq!(key, "fdroid:https://f-droid.org/repo:org.example.app");
        assert_eq!(entry.version_code, 10);
        assert_eq!(entry.signers, ["sha256:abcd"]);
        assert_eq!(
            entry.artifacts[0].url.as_deref(),
            Some("https://f-droid.org/repo/org.example.app_10.apk")
        );
    }

    #[test]
    fn latest_uses_highest_version() {
        let app = app(Some("latest"));
        let (_, entry) =
            resolve("org.example.app", &app, &StaticFetcher { index: index() }).unwrap();
        assert_eq!(entry.version_code, 11);
    }

    #[test]
    fn missing_package_fails() {
        let err =
            resolve("org.missing", &app(None), &StaticFetcher { index: index() }).unwrap_err();
        assert!(err.to_string().contains("not in the F-Droid index"));
    }

    #[test]
    fn identity_is_required() {
        let mut app = app(None);
        if let Some(source) = app.source.as_mut() {
            source.identity = None;
        }
        let err = resolve("org.example.app", &app, &StaticFetcher { index: index() }).unwrap_err();
        assert!(err.to_string().contains("requires identity"));
    }

    #[test]
    fn decodes_manifest_digest() {
        let bytes = b"index-bytes";
        let hex = sha256_hex(bytes);
        let b64 = "aW5kZXgtYnl0ZXM=";
        assert_eq!(decode_b64(b64).unwrap(), bytes);
        assert_eq!(hex.len(), 64);
    }

    #[test]
    fn http_repository_is_rejected() {
        let mut app = app(None);
        if let Some(source) = app.source.as_mut() {
            source.repository = Some("http://f-droid.org/repo".into());
        }
        let err = resolve("org.example.app", &app, &StaticFetcher { index: index() }).unwrap_err();
        assert!(err.to_string().contains("https"));
    }

    #[test]
    fn rejects_zip_slip_names() {
        assert!(reject_unsafe_jar_entry("../evil").is_err());
        assert!(reject_unsafe_jar_entry("/abs").is_err());
        assert!(reject_unsafe_jar_entry("META-INF\\x").is_err());
        assert!(reject_unsafe_jar_entry("index-v1.json").is_ok());
        assert!(allowed_jar_entry("index-v1.json"));
        assert!(allowed_jar_entry("META-INF/CERT.RSA"));
        assert!(!allowed_jar_entry("evil.sh"));
        assert!(!allowed_jar_entry("META-INF/nested/CERT.RSA"));
    }

    #[test]
    fn splits_pem_certificates() {
        let certs = pem_certificates(
            "ignored\n-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n\
             -----BEGIN CERTIFICATE-----\nBBB\n-----END CERTIFICATE-----\n",
        );
        assert_eq!(certs.len(), 2);
        assert!(certs[0].contains("AAA"));
        assert!(certs[1].contains("BBB"));
    }
}
