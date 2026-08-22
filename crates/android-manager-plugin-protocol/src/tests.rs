use super::*;
use crate::client::{child_environment, wait_with_timeout};
use android_manager_adapter::AdapterErrorCode;
use android_manager_model::{AdapterEnvironment, ExternalAdapter};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fixture_executable() -> PathBuf {
    std::env::current_exe().unwrap()
}

#[test]
fn request_json_is_deterministic() {
    let request = AdapterRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "1".into(),
        call: RequestMethod::Handshake(HandshakeParams {
            id: "fixture".into(),
            kind: AdapterKind::Source,
            config: BTreeMap::new(),
        }),
    };
    let first = serde_json::to_string(&request).unwrap();
    let second = serde_json::to_string(&request).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first,
        r#"{"protocolVersion":1,"requestId":"1","method":"handshake","params":{"id":"fixture","kind":"source"}}"#
    );
}

#[test]
fn protocol_error_json_is_deterministic() {
    let error = ProtocolError::new(
        ProtocolErrorCode::Malformed,
        "adapter returned malformed json",
    );
    let first = serde_json::to_string(&error).unwrap();
    let second = serde_json::to_string(&error).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first,
        r#"{"code":"malformed","message":"adapter returned malformed json"}"#
    );
}

#[test]
fn child_environment_is_allowlist_and_files_only() {
    std::env::set_var("AM_PROTOCOL_SECRET", "leak-me");
    std::env::set_var("AM_PROTOCOL_OK", "ok");
    let environment = AdapterEnvironment {
        allow: vec!["AM_PROTOCOL_OK".into()],
        files: BTreeMap::from([("AM_PROTOCOL_FILE".into(), PathBuf::from("/abs/ref"))]),
    };
    let vars = child_environment(&environment);
    assert!(vars
        .iter()
        .any(|(key, value)| key == "AM_PROTOCOL_OK" && value == "ok"));
    assert!(vars
        .iter()
        .any(|(key, value)| key == "AM_PROTOCOL_FILE" && value == "/abs/ref"));
    assert!(vars.iter().all(|(key, _)| key != "AM_PROTOCOL_SECRET"));
}

fn sleep_bin() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let candidate = dir.join("sleep");
                candidate.is_file().then_some(candidate)
            })
        })
        .expect("sleep in PATH")
}

#[test]
fn timeout_kills_the_child() {
    let mut child = Command::new(sleep_bin()).arg("30").spawn().unwrap();
    let pid = child.id();
    let error = wait_with_timeout(&mut child, Duration::from_millis(150)).unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Timeout);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn relative_executable_is_rejected() {
    let error = ExternalAdapterClient::new(
        "fixture",
        ExternalAdapter {
            kind: AdapterKind::Source,
            executable: PathBuf::from("not-absolute"),
            capabilities: BTreeSet::from([AdapterCapability::Resolve]),
            environment: AdapterEnvironment::default(),
            config: BTreeMap::new(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::InvalidExecutable);
}

#[test]
fn invalid_secret_file_reference_is_rejected_without_value() {
    let error = ExternalAdapterClient::new(
        "fixture",
        ExternalAdapter {
            kind: AdapterKind::Source,
            executable: fixture_executable(),
            capabilities: BTreeSet::from([AdapterCapability::Resolve]),
            environment: AdapterEnvironment {
                allow: vec!["token=value".into()],
                files: BTreeMap::from([("TOKEN_FILE".into(), PathBuf::from("/nix/store/secret"))]),
            },
            config: BTreeMap::new(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::InvalidConfiguration);
    assert!(!error.to_string().contains("secret-value"));
}

#[test]
fn oversized_request_is_rejected_before_spawn() {
    let client = ExternalAdapterClient::new(
        "fixture",
        ExternalAdapter {
            kind: AdapterKind::Source,
            executable: fixture_executable(),
            capabilities: BTreeSet::from([AdapterCapability::Resolve]),
            environment: AdapterEnvironment::default(),
            config: BTreeMap::new(),
        },
    )
    .unwrap()
    .with_limits(Duration::from_secs(1), 8, 8, 8);
    let error = client.handshake().unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::Oversized);
}

#[test]
fn wrapper_rejects_missing_configured_capability() {
    let error = ExternalSourceProvider::new(
        "fixture",
        ExternalAdapter {
            kind: AdapterKind::Source,
            executable: fixture_executable(),
            capabilities: BTreeSet::new(),
            environment: AdapterEnvironment::default(),
            config: BTreeMap::new(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::CapabilityMismatch);
}

#[test]
fn supports_requires_external_adapter_id() {
    let provider = ExternalSourceProvider::new(
        "fixture",
        ExternalAdapter {
            kind: AdapterKind::Source,
            executable: fixture_executable(),
            capabilities: BTreeSet::from([AdapterCapability::Resolve]),
            environment: AdapterEnvironment::default(),
            config: BTreeMap::new(),
        },
    )
    .unwrap();
    let mut source = SourceIntent::new("external");
    source.adapter = Some("fixture".into());
    assert!(provider.supports(&source));
    source.adapter = Some("other".into());
    assert!(!provider.supports(&source));
    assert!(!provider.supports(&SourceIntent::new("local")));
}

#[test]
fn adapter_error_mapping_is_stable() {
    let error = adapter_error(
        "fixture",
        ProtocolError::new(
            ProtocolErrorCode::Malformed,
            "adapter returned malformed json",
        ),
    );
    assert_eq!(error.code, AdapterErrorCode::InvalidConfiguration);
    assert_eq!(error.message, "external protocol failure: Malformed");
    assert!(!error.to_string().contains("AKIA"));
}

#[test]
fn fail_mode_reads_source_config() {
    let mut source = SourceIntent::new("external");
    source.adapter = Some("fixture".into());
    source.config.insert("fail".into(), json!("timeout"));
    let request = AdapterRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "1".into(),
        call: RequestMethod::Resolve(ResolveParams {
            package_name: "org.example.app".into(),
            app: DesiredApp {
                state: android_manager_model::AppState::Present,
                management: android_manager_model::ManagementMode::Host,
                source: Some(source),
                version_policy: android_manager_model::VersionPolicy::LockedUpgradeOnly,
                verification: android_manager_model::Verification::default(),
            },
            config: BTreeMap::new(),
        }),
    };
    assert_eq!(fail_mode(&request).as_deref(), Some("timeout"));
}
