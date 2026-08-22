use android_manager_adapter::{
    AdapterError, AdapterErrorCode, AdapterResult, DelegateProvider, DelegateRequest,
    ResolveRequest, SourceProvider,
};
use android_manager_model::{
    AdapterCapability, AdapterDescriptor, AdapterKind, DelegatePlan, DesiredApp, DeviceSnapshot,
    ExternalAdapter, LockEntry, SourceIntent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{self, BufRead, Write};

mod client;

pub use client::{
    ExternalAdapterClient, DEFAULT_MAX_REQUEST_BYTES, DEFAULT_MAX_RESPONSE_BYTES,
    DEFAULT_MAX_STDERR_BYTES, DEFAULT_TIMEOUT,
};

pub const PROTOCOL_VERSION: u32 = 1;

pub fn kind() -> &'static str {
    "external"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MethodName {
    Handshake,
    Capabilities,
    Resolve,
    PlanDelegate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterRequest {
    pub protocol_version: u32,
    pub request_id: String,
    #[serde(flatten)]
    pub call: RequestMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
pub enum RequestMethod {
    Handshake(HandshakeParams),
    Capabilities(HandshakeParams),
    Resolve(ResolveParams),
    PlanDelegate(PlanDelegateParams),
}

impl RequestMethod {
    pub fn name(&self) -> MethodName {
        match self {
            Self::Handshake(_) => MethodName::Handshake,
            Self::Capabilities(_) => MethodName::Capabilities,
            Self::Resolve(_) => MethodName::Resolve,
            Self::PlanDelegate(_) => MethodName::PlanDelegate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandshakeParams {
    pub id: String,
    pub kind: AdapterKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterIdentity {
    pub id: String,
    pub kind: AdapterKind,
    pub capabilities: BTreeSet<AdapterCapability>,
}

pub type HandshakeResult = AdapterIdentity;
pub type CapabilitiesResult = AdapterIdentity;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveParams {
    pub package_name: String,
    pub app: DesiredApp,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveResult {
    pub lock_key: String,
    pub entry: LockEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDelegateParams {
    pub device_name: String,
    pub user_id: u32,
    pub package_name: String,
    pub app: DesiredApp,
    pub snapshot: DeviceSnapshot,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDelegateResult {
    pub plan: DelegatePlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtocolErrorCode {
    UnsupportedVersion,
    UnknownMethod,
    Malformed,
    InvalidUtf8,
    MultipleResponses,
    RequestIdMismatch,
    NonzeroExit,
    Timeout,
    Oversized,
    StderrNotEmpty,
    MethodMismatch,
    InvalidResult,
    CapabilityMismatch,
    InvalidExecutable,
    SpawnFailed,
    InvalidConfiguration,
    ResolutionFailed,
    DelegatePlanningFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
}

impl ProtocolError {
    pub fn new(code: ProtocolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl Error for ProtocolError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterResponse {
    pub protocol_version: u32,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<MethodName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

pub fn adapter_error(adapter: &str, error: ProtocolError) -> AdapterError {
    let code = match error.code {
        ProtocolErrorCode::CapabilityMismatch => AdapterErrorCode::CapabilityMismatch,
        ProtocolErrorCode::ResolutionFailed => AdapterErrorCode::ResolutionFailed,
        ProtocolErrorCode::DelegatePlanningFailed => AdapterErrorCode::DelegatePlanningFailed,
        _ => AdapterErrorCode::InvalidConfiguration,
    };
    AdapterError::new(
        code,
        adapter,
        format!("external protocol failure: {:?}", error.code),
    )
}

#[derive(Debug)]
pub struct ExternalSourceProvider {
    client: ExternalAdapterClient,
}

impl ExternalSourceProvider {
    pub fn new(id: impl Into<String>, adapter: ExternalAdapter) -> Result<Self, ProtocolError> {
        if adapter.kind != AdapterKind::Source {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidConfiguration,
                "source wrapper requires source adapter",
            ));
        }
        if !adapter.capabilities.contains(&AdapterCapability::Resolve) {
            return Err(ProtocolError::new(
                ProtocolErrorCode::CapabilityMismatch,
                "missing resolve capability",
            ));
        }
        Ok(Self {
            client: ExternalAdapterClient::new(id, adapter)?,
        })
    }
}

impl SourceProvider for ExternalSourceProvider {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            id: self.client.id.clone(),
            kind: AdapterKind::Source,
            capabilities: self.client.adapter.capabilities.clone(),
        }
    }

    fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()> {
        if !self.supports(source) {
            return Err(AdapterError::new(
                AdapterErrorCode::IncompatibleSource,
                &self.client.id,
                "source type is incompatible",
            ));
        }
        Ok(())
    }

    fn supports(&self, source: &SourceIntent) -> bool {
        source.kind == kind() && source.adapter.as_deref() == Some(self.client.id.as_str())
    }

    fn resolve(&self, request: ResolveRequest<'_>) -> AdapterResult<(String, LockEntry)> {
        ensure_ready(&self.client, AdapterCapability::Resolve)?;
        let mut config = self.client.adapter.config.clone();
        if let Some(source) = &request.app.source {
            config.extend(source.config.clone());
        }
        let result = self
            .client
            .resolve(ResolveParams {
                package_name: request.package_name.to_string(),
                app: request.app.clone(),
                config,
            })
            .map_err(|error| adapter_error(&self.client.id, error))?;
        Ok((result.lock_key, result.entry))
    }
}

#[derive(Debug)]
pub struct ExternalDelegateProvider {
    client: ExternalAdapterClient,
}

impl ExternalDelegateProvider {
    pub fn new(id: impl Into<String>, adapter: ExternalAdapter) -> Result<Self, ProtocolError> {
        if adapter.kind != AdapterKind::Delegate {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidConfiguration,
                "delegate wrapper requires delegate adapter",
            ));
        }
        if !adapter
            .capabilities
            .contains(&AdapterCapability::PlanDelegate)
        {
            return Err(ProtocolError::new(
                ProtocolErrorCode::CapabilityMismatch,
                "missing planDelegate capability",
            ));
        }
        Ok(Self {
            client: ExternalAdapterClient::new(id, adapter)?,
        })
    }
}

impl DelegateProvider for ExternalDelegateProvider {
    fn descriptor(&self) -> AdapterDescriptor {
        AdapterDescriptor {
            id: self.client.id.clone(),
            kind: AdapterKind::Delegate,
            capabilities: self.client.adapter.capabilities.clone(),
        }
    }

    fn validate_config(&self, source: &SourceIntent) -> AdapterResult<()> {
        if !self.supports(source) {
            return Err(AdapterError::new(
                AdapterErrorCode::IncompatibleSource,
                &self.client.id,
                "source type is incompatible",
            ));
        }
        Ok(())
    }

    fn supports(&self, source: &SourceIntent) -> bool {
        source.kind == kind() && source.adapter.as_deref() == Some(self.client.id.as_str())
    }

    fn plan(&self, request: DelegateRequest<'_>) -> AdapterResult<DelegatePlan> {
        ensure_ready(&self.client, AdapterCapability::PlanDelegate)?;
        let mut config = self.client.adapter.config.clone();
        if let Some(source) = &request.app.source {
            config.extend(source.config.clone());
        }
        self.client
            .plan_delegate(PlanDelegateParams {
                device_name: request.device_name.to_string(),
                user_id: request.user_id,
                package_name: request.package_name.to_string(),
                app: request.app.clone(),
                snapshot: request.snapshot.clone(),
                config,
            })
            .map(|result| result.plan)
            .map_err(|error| adapter_error(&self.client.id, error))
    }
}

fn ensure_ready(
    client: &ExternalAdapterClient,
    required: AdapterCapability,
) -> AdapterResult<HandshakeResult> {
    let result = client
        .handshake()
        .map_err(|error| adapter_error(&client.id, error))?;
    let capabilities = client
        .capabilities()
        .map_err(|error| adapter_error(&client.id, error))?;
    if result.id != client.id {
        return Err(AdapterError::new(
            AdapterErrorCode::InvalidConfiguration,
            &client.id,
            "adapter identity mismatch",
        ));
    }
    if result.kind != client.adapter.kind {
        return Err(AdapterError::new(
            AdapterErrorCode::WrongAdapterKind,
            &client.id,
            "adapter kind mismatch",
        ));
    }
    if capabilities != result {
        return Err(AdapterError::new(
            AdapterErrorCode::CapabilityMismatch,
            &client.id,
            "handshake and capabilities responses differ",
        ));
    }
    if !result.capabilities.contains(&required)
        || !client
            .adapter
            .capabilities
            .iter()
            .all(|capability| result.capabilities.contains(capability))
    {
        return Err(AdapterError::new(
            AdapterErrorCode::CapabilityMismatch,
            &client.id,
            "adapter capability mismatch",
        ));
    }
    Ok(result)
}

pub fn run_fixture() -> io::Result<()> {
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let raw: Value = match serde_json::from_str(line.trim_end()) {
        Ok(value) => value,
        Err(_) => {
            io::stdout().write_all(b"{not-json\n")?;
            return Ok(());
        }
    };
    let request_id = raw
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    if raw.get("protocolVersion").and_then(Value::as_u64) != Some(PROTOCOL_VERSION.into()) {
        write_raw(AdapterResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            method: None,
            result: None,
            error: Some(ProtocolError::new(
                ProtocolErrorCode::UnsupportedVersion,
                "unsupported protocol version",
            )),
        })?;
        return Ok(());
    }
    let request: AdapterRequest = match serde_json::from_value(raw) {
        Ok(request) => request,
        Err(_) => {
            write_raw(AdapterResponse {
                protocol_version: PROTOCOL_VERSION,
                request_id,
                method: None,
                result: None,
                error: Some(ProtocolError::new(
                    ProtocolErrorCode::UnknownMethod,
                    "unknown method or invalid parameters",
                )),
            })?;
            return Ok(());
        }
    };
    if let Some(mode) = fail_mode(&request) {
        apply_fail_mode(&request, &mode)?;
        return Ok(());
    }
    write_success(&request, fixture_result(&request))?;
    Ok(())
}

fn fail_mode(request: &AdapterRequest) -> Option<String> {
    let from_config = |config: &BTreeMap<String, Value>| {
        config
            .get("fail")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    match &request.call {
        RequestMethod::Handshake(params) | RequestMethod::Capabilities(params) => {
            from_config(&params.config)
        }
        RequestMethod::Resolve(params) => from_config(&params.config).or_else(|| {
            params
                .app
                .source
                .as_ref()
                .and_then(|source| from_config(&source.config))
        }),
        RequestMethod::PlanDelegate(params) => from_config(&params.config).or_else(|| {
            params
                .app
                .source
                .as_ref()
                .and_then(|source| from_config(&source.config))
        }),
    }
}

fn apply_fail_mode(request: &AdapterRequest, mode: &str) -> io::Result<()> {
    match mode {
        "malformed" => io::stdout().write_all(b"{not-json\n"),
        "mismatch-id" => write_raw(AdapterResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: "mismatch".into(),
            method: Some(request.call.name()),
            result: Some(fixture_result(request)),
            error: None,
        }),
        "nonzero" => std::process::exit(2),
        "timeout" => {
            std::thread::sleep(std::time::Duration::from_secs(30));
            Ok(())
        }
        "oversized" => {
            let body = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(256 * 1024));
            io::stdout().write_all(body.as_bytes())
        }
        "stderr" => {
            eprint!("credential=AKIASECRET123");
            write_success(request, fixture_result(request))
        }
        "invalid-result" => write_raw(AdapterResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            method: Some(request.call.name()),
            result: Some(serde_json::json!({"nope": true})),
            error: None,
        }),
        "version" => write_raw(AdapterResponse {
            protocol_version: PROTOCOL_VERSION + 1,
            request_id: request.request_id.clone(),
            method: Some(request.call.name()),
            result: Some(fixture_result(request)),
            error: None,
        }),
        "multiple" => {
            write_success(request, fixture_result(request))?;
            write_success(request, fixture_result(request))
        }
        "capability-mismatch" => {
            let identity = match &request.call {
                RequestMethod::Handshake(params) | RequestMethod::Capabilities(params) => {
                    AdapterIdentity {
                        id: params.id.clone(),
                        kind: params.kind,
                        capabilities: BTreeSet::new(),
                    }
                }
                _ => AdapterIdentity {
                    id: "fixture".into(),
                    kind: AdapterKind::Source,
                    capabilities: BTreeSet::new(),
                },
            };
            write_success(request, serde_json::to_value(identity).unwrap())
        }
        "invalid-utf8" => io::stdout().write_all(&[0xff, 0xfe, b'\n']),
        "method-mismatch" => write_raw(AdapterResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            method: Some(match request.call.name() {
                MethodName::Handshake => MethodName::Resolve,
                _ => MethodName::Handshake,
            }),
            result: Some(fixture_result(request)),
            error: None,
        }),
        "missing-method" => write_raw(AdapterResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            method: None,
            result: Some(fixture_result(request)),
            error: None,
        }),
        "descendant-holds-pipes" | "escaped-descendant-holds-pipes" => {
            let mut child = std::process::Command::new(std::env::current_exe()?);
            child
                .env("ANDROID_MANAGER_FIXTURE_SLEEP_MS", "1000")
                .stdin(std::process::Stdio::null());
            if fail_mode(request).as_deref() == Some("escaped-descendant-holds-pipes") {
                use std::os::unix::process::CommandExt;
                unsafe {
                    child.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            child.spawn()?;
            write_success(request, fixture_result(request))
        }
        _ => write_success(request, fixture_result(request)),
    }
}

fn fixture_result(request: &AdapterRequest) -> Value {
    match &request.call {
        RequestMethod::Handshake(params) | RequestMethod::Capabilities(params) => {
            serde_json::to_value(fixture_identity(params)).expect("identity")
        }
        RequestMethod::Resolve(params) => {
            serde_json::to_value(synthetic_resolve(params)).expect("resolve")
        }
        RequestMethod::PlanDelegate(params) => {
            serde_json::to_value(synthetic_delegate(params)).expect("delegate")
        }
    }
}

fn fixture_identity(params: &HandshakeParams) -> AdapterIdentity {
    let capabilities = params
        .config
        .get("capabilities")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_else(|| {
            BTreeSet::from([AdapterCapability::Resolve, AdapterCapability::PlanDelegate])
        });
    AdapterIdentity {
        id: params.id.clone(),
        kind: params.kind,
        capabilities,
    }
}

fn synthetic_resolve(params: &ResolveParams) -> ResolveResult {
    let adapter = params
        .app
        .source
        .as_ref()
        .and_then(|source| source.adapter.clone())
        .unwrap_or_else(|| "fixture".into());
    let path = params
        .config
        .get("artifactPath")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from);
    let sha256 = params
        .config
        .get("sha256")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "0".repeat(64));
    let signer = params
        .config
        .get("signer")
        .and_then(Value::as_str)
        .unwrap_or("sha256:00")
        .to_string();
    let package_name = params
        .config
        .get("packageName")
        .and_then(Value::as_str)
        .unwrap_or(&params.package_name)
        .to_string();
    ResolveResult {
        lock_key: format!("external:{adapter}:{}", params.package_name),
        entry: LockEntry {
            source: android_manager_model::LockedSource {
                kind: kind().into(),
                path: None,
                url: None,
                repository: None,
                repository_identity: None,
                adapter: Some(adapter),
            },
            package_name,
            version_name: Some("fixture".into()),
            version_code: 1,
            signers: vec![signer],
            artifacts: vec![android_manager_model::LockedArtifact {
                kind: "apk".into(),
                url: None,
                path,
                sha256,
                size: None,
                abis: vec!["arm64-v8a".into()],
            }],
            constraints: android_manager_model::Constraints::default(),
        },
    }
}

fn synthetic_delegate(params: &PlanDelegateParams) -> PlanDelegateResult {
    PlanDelegateResult {
        plan: DelegatePlan {
            action: android_manager_model::DelegatedAction {
                resource: android_manager_model::manual_resource_id(
                    &params.device_name,
                    params.user_id,
                    &params.package_name,
                ),
                instruction: format!("fixture delegate {}", params.package_name),
                depends_on: Vec::new(),
            },
            prerequisites: Vec::new(),
        },
    }
}

fn write_success(request: &AdapterRequest, result: Value) -> io::Result<()> {
    write_raw(AdapterResponse {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        method: Some(request.call.name()),
        result: Some(result),
        error: None,
    })
}

fn write_raw(response: AdapterResponse) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

#[cfg(test)]
mod tests;
