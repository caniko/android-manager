use android_manager_adapter::{
    AdapterErrorCode, DelegateProvider, DelegateRequest, ResolveRequest, SourceProvider,
};
use android_manager_model::{
    AdapterCapability, AdapterEnvironment, AdapterKind, AppState, DesiredApp, DeviceCapabilities,
    DeviceSnapshot, ExternalAdapter, ManagementMode, SourceIntent, Verification, VersionPolicy,
};
use android_manager_plugin_protocol::{
    ExternalAdapterClient, ExternalDelegateProvider, ExternalSourceProvider, PlanDelegateParams,
    ProtocolErrorCode, ResolveParams,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn fixture_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_android_manager_protocol_fixture") {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("android-manager-protocol-fixture");
    path
}

fn no_read_fixture_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_android_manager_protocol_no_read_fixture") {
        return PathBuf::from(path);
    }
    let mut path = fixture_bin();
    path.set_file_name("android-manager-protocol-no-read-fixture");
    path
}

fn adapter(kind: AdapterKind, fail: Option<&str>) -> ExternalAdapter {
    let mut config = BTreeMap::new();
    if let Some(fail) = fail {
        config.insert("fail".into(), json!(fail));
    }
    ExternalAdapter {
        kind,
        executable: fixture_bin(),
        capabilities: match kind {
            AdapterKind::Source => BTreeSet::from([AdapterCapability::Resolve]),
            AdapterKind::Delegate => BTreeSet::from([AdapterCapability::PlanDelegate]),
        },
        environment: AdapterEnvironment::default(),
        config,
    }
}

fn client(fail: Option<&str>) -> ExternalAdapterClient {
    ExternalAdapterClient::new("fixture", adapter(AdapterKind::Source, fail))
        .unwrap()
        .with_limits(Duration::from_secs(2), 64 * 1024, 4096, 4096)
}

fn app(fail: Option<&str>) -> DesiredApp {
    let mut source = SourceIntent::new("external");
    source.adapter = Some("fixture".into());
    if let Some(fail) = fail {
        source.config.insert("fail".into(), json!(fail));
    }
    DesiredApp {
        state: AppState::Present,
        management: ManagementMode::Host,
        source: Some(source),
        version_policy: VersionPolicy::LockedUpgradeOnly,
        verification: Verification::default(),
    }
}

fn snapshot() -> DeviceSnapshot {
    DeviceSnapshot {
        transport_id: "adb:test".into(),
        serial: "test".into(),
        manufacturer: None,
        sdk: 35,
        abis: Vec::new(),
        users: vec![0],
        packages: Vec::new(),
        capabilities: DeviceCapabilities::default(),
    }
}

fn assert_clean(error: &impl std::fmt::Display) {
    let text = error.to_string();
    assert!(!text.contains("AKIASECRET123"), "{text}");
    assert!(!text.contains("credential="), "{text}");
}

#[test]
fn handshake_and_capabilities_succeed() {
    let client = client(None);
    let handshake = client.handshake().unwrap();
    assert_eq!(handshake.id, "fixture");
    assert_eq!(handshake.kind, AdapterKind::Source);
    assert!(handshake.capabilities.contains(&AdapterCapability::Resolve));
    let capabilities = client.capabilities().unwrap();
    assert_eq!(capabilities.id, handshake.id);
    assert_eq!(capabilities.kind, handshake.kind);
}

#[test]
fn unknown_method_returns_structured_error() {
    let mut child = Command::new(fixture_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"{\"protocolVersion\":1,\"requestId\":\"7\",\"method\":\"unknown\",\"params\":{}}\n",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let response: android_manager_plugin_protocol::AdapterResponse =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response.request_id, "7");
    assert_eq!(
        response.error.unwrap().code,
        ProtocolErrorCode::UnknownMethod
    );
}

#[test]
fn resolve_and_plan_delegate_are_synthetic() {
    let client = client(None);
    let resolved = client
        .resolve(ResolveParams {
            package_name: "org.example.app".into(),
            app: app(None),
            config: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(resolved.lock_key, "external:fixture:org.example.app");
    assert_eq!(resolved.entry.version_code, 1);
    assert_eq!(resolved.entry.signers, ["sha256:00"]);

    let delegate = ExternalAdapterClient::new("fixture", adapter(AdapterKind::Delegate, None))
        .unwrap()
        .plan_delegate(PlanDelegateParams {
            device_name: "phone".into(),
            user_id: 10,
            package_name: "org.example.app".into(),
            app: app(None),
            snapshot: snapshot(),
            config: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(
        delegate.plan.action.resource,
        "device/phone/user/10/manual/org.example.app"
    );
}

#[test]
fn wrappers_handshake_before_invocation() {
    let source =
        ExternalSourceProvider::new("fixture", adapter(AdapterKind::Source, None)).unwrap();
    let app = app(None);
    let (key, entry) = source
        .resolve(ResolveRequest {
            package_name: "org.example.app",
            app: &app,
        })
        .unwrap();
    assert_eq!(key, "external:fixture:org.example.app");
    assert_eq!(entry.package_name, "org.example.app");

    let delegate =
        ExternalDelegateProvider::new("fixture", adapter(AdapterKind::Delegate, None)).unwrap();
    let plan = delegate
        .plan(DelegateRequest {
            device_name: "phone",
            user_id: 0,
            package_name: "org.example.app",
            app: &app,
            snapshot: &snapshot(),
        })
        .unwrap();
    assert!(plan.action.instruction.contains("org.example.app"));
}

#[test]
fn version_mismatch() {
    let error = client(Some("version")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::UnsupportedVersion);
    assert_clean(&error);
}

#[test]
fn malformed_response() {
    let error = client(Some("malformed")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Malformed);
    assert_clean(&error);
}

#[test]
fn request_id_mismatch() {
    let error = client(Some("mismatch-id")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::RequestIdMismatch);
    assert_clean(&error);
}

#[test]
fn multiple_responses() {
    let error = client(Some("multiple")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::MultipleResponses);
    assert_clean(&error);
}

#[test]
fn nonzero_exit() {
    let error = client(Some("nonzero")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::NonzeroExit);
    assert_clean(&error);
}

#[test]
fn timeout_terminates_adapter() {
    let client =
        ExternalAdapterClient::new("fixture", adapter(AdapterKind::Source, Some("timeout")))
            .unwrap()
            .with_limits(Duration::from_millis(200), 64 * 1024, 4096, 4096);
    let error = client.handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Timeout);
    assert_clean(&error);
}

#[test]
fn descendant_cannot_hold_response_pipes_open() {
    let start = std::time::Instant::now();
    client(Some("descendant-holds-pipes")).handshake().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn escaped_descendant_cannot_hang_the_host() {
    let client = ExternalAdapterClient::new(
        "fixture",
        adapter(AdapterKind::Source, Some("escaped-descendant-holds-pipes")),
    )
    .unwrap()
    .with_limits(Duration::from_millis(200), 64 * 1024, 4096, 4096);
    let start = std::time::Instant::now();
    let error = client.handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Timeout);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn escaped_descendant_cannot_hold_request_pipe_open() {
    let mut configured = adapter(AdapterKind::Source, None);
    configured.executable = no_read_fixture_bin();
    configured
        .config
        .insert("padding".into(), json!("x".repeat(512 * 1024)));
    let client = ExternalAdapterClient::new("fixture", configured)
        .unwrap()
        .with_limits(Duration::from_millis(200), 1024 * 1024, 4096, 4096);
    let start = std::time::Instant::now();
    let error = client.handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Timeout);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn oversized_response() {
    let client =
        ExternalAdapterClient::new("fixture", adapter(AdapterKind::Source, Some("oversized")))
            .unwrap()
            .with_limits(Duration::from_secs(2), 64 * 1024, 64, 4096);
    let error = client.handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Oversized);
    assert_clean(&error);
}

#[test]
fn stderr_is_redacted() {
    let error = client(Some("stderr")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::StderrNotEmpty);
    assert_clean(&error);
    assert!(!format!("{error:?}").contains("AKIASECRET123"));
}

#[test]
fn invalid_utf8() {
    let error = client(Some("invalid-utf8")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::InvalidUtf8);
    assert_clean(&error);
}

#[test]
fn invalid_result() {
    let error = client(Some("invalid-result")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::InvalidResult);
    assert_clean(&error);
}

#[test]
fn method_mismatch() {
    let error = client(Some("method-mismatch")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::MethodMismatch);
    assert_clean(&error);
}

#[test]
fn missing_method_is_rejected() {
    let error = client(Some("missing-method")).handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::MethodMismatch);
    assert_clean(&error);
}

#[test]
fn capability_mismatch_on_invoke() {
    let provider = ExternalSourceProvider::new(
        "fixture",
        adapter(AdapterKind::Source, Some("capability-mismatch")),
    )
    .unwrap();
    let app = app(None);
    let error = provider
        .resolve(ResolveRequest {
            package_name: "org.example.app",
            app: &app,
        })
        .unwrap_err();
    assert_eq!(error.code, AdapterErrorCode::CapabilityMismatch);
    assert_clean(&error);
}

#[test]
fn source_config_failure_mode() {
    let error = client(None)
        .resolve(ResolveParams {
            package_name: "org.example.app".into(),
            app: app(Some("malformed")),
            config: BTreeMap::new(),
        })
        .unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Malformed);
}
