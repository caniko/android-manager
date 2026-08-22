use crate::{
    AdapterRequest, AdapterResponse, CapabilitiesResult, HandshakeParams, HandshakeResult,
    MethodName, PlanDelegateParams, PlanDelegateResult, ProtocolError, ProtocolErrorCode,
    RequestMethod, ResolveParams, ResolveResult, PROTOCOL_VERSION,
};
use android_manager_model::{AdapterEnvironment, ExternalAdapter};
use serde::de::DeserializeOwned;
use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_STDERR_BYTES: usize = 8 * 1024;

pub struct ExternalAdapterClient {
    pub id: String,
    pub adapter: ExternalAdapter,
    timeout: Duration,
    max_request_bytes: usize,
    max_response_bytes: usize,
    max_stderr_bytes: usize,
    next_id: AtomicU64,
}

impl std::fmt::Debug for ExternalAdapterClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalAdapterClient")
            .field("id", &self.id)
            .field("adapter", &self.adapter)
            .finish_non_exhaustive()
    }
}

impl ExternalAdapterClient {
    pub fn new(id: impl Into<String>, adapter: ExternalAdapter) -> Result<Self, ProtocolError> {
        if !adapter.executable.is_absolute() {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidExecutable,
                "adapter executable must be an absolute path",
            ));
        }
        let metadata = adapter.executable.metadata().map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::InvalidExecutable,
                "adapter executable is not accessible",
            )
        })?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidExecutable,
                "adapter executable must be an executable file",
            ));
        }
        validate_environment(&adapter.environment)?;
        Ok(Self {
            id: id.into(),
            adapter,
            timeout: DEFAULT_TIMEOUT,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
            next_id: AtomicU64::new(1),
        })
    }

    pub fn with_limits(
        mut self,
        timeout: Duration,
        max_request_bytes: usize,
        max_response_bytes: usize,
        max_stderr_bytes: usize,
    ) -> Self {
        self.timeout = timeout;
        self.max_request_bytes = max_request_bytes;
        self.max_response_bytes = max_response_bytes;
        self.max_stderr_bytes = max_stderr_bytes;
        self
    }

    pub fn handshake(&self) -> Result<HandshakeResult, ProtocolError> {
        self.invoke(RequestMethod::Handshake(self.identity_params()))
    }

    pub fn capabilities(&self) -> Result<CapabilitiesResult, ProtocolError> {
        self.invoke(RequestMethod::Capabilities(self.identity_params()))
    }

    pub fn resolve(&self, params: ResolveParams) -> Result<ResolveResult, ProtocolError> {
        self.invoke(RequestMethod::Resolve(params))
    }

    pub fn plan_delegate(
        &self,
        params: PlanDelegateParams,
    ) -> Result<PlanDelegateResult, ProtocolError> {
        self.invoke(RequestMethod::PlanDelegate(params))
    }

    fn identity_params(&self) -> HandshakeParams {
        HandshakeParams {
            id: self.id.clone(),
            kind: self.adapter.kind,
            config: self.adapter.config.clone(),
        }
    }

    fn invoke<T: DeserializeOwned>(&self, call: RequestMethod) -> Result<T, ProtocolError> {
        if !self.adapter.executable.is_absolute() {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidExecutable,
                "adapter executable must be an absolute path",
            ));
        }
        let method = call.name();
        let request = AdapterRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: self.next_id.fetch_add(1, Ordering::Relaxed).to_string(),
            call,
        };
        let payload = serde_json::to_string(&request).map_err(|_| {
            ProtocolError::new(ProtocolErrorCode::Malformed, "failed to encode request")
        })?;
        if payload.len() > self.max_request_bytes {
            return Err(ProtocolError::new(
                ProtocolErrorCode::Oversized,
                "request exceeds size limit",
            ));
        }

        let mut command = Command::new(&self.adapter.executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .process_group(0);
        for (key, value) in child_environment(&self.adapter.environment) {
            command.env(key, value);
        }

        let mut child = command.spawn().map_err(|_| {
            ProtocolError::new(ProtocolErrorCode::SpawnFailed, "failed to start adapter")
        })?;
        let stdin = child.stdin.take();
        let payload = payload.into_bytes();
        let (writer_tx, writer_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = (|| -> std::io::Result<()> {
                if let Some(mut stdin) = stdin {
                    stdin.write_all(&payload)?;
                    stdin.write_all(b"\n")?;
                }
                Ok(())
            })();
            let _ = writer_tx.send(result);
        });

        let start = Instant::now();
        let output = collect_output(
            &mut child,
            self.timeout,
            self.max_response_bytes,
            self.max_stderr_bytes,
        );
        let (status, stdout, stderr) = output?;
        let write_result = writer_rx
            .recv_timeout(self.timeout.saturating_sub(start.elapsed()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    ProtocolError::new(ProtocolErrorCode::Timeout, "adapter timed out")
                }
                mpsc::RecvTimeoutError::Disconnected => ProtocolError::new(
                    ProtocolErrorCode::SpawnFailed,
                    "adapter stdin writer failed",
                ),
            })?;
        write_result.map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::SpawnFailed,
                "failed to write adapter request",
            )
        })?;
        classify(method, &request.request_id, status, stdout, stderr)
    }
}

fn validate_environment(environment: &AdapterEnvironment) -> Result<(), ProtocolError> {
    for key in environment.allow.iter().chain(environment.files.keys()) {
        let mut bytes = key.bytes();
        if !bytes
            .next()
            .is_some_and(|byte| byte == b'_' || byte.is_ascii_uppercase())
            || !bytes.all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidConfiguration,
                format!("invalid environment variable name {key}"),
            ));
        }
        if is_dangerous_environment(key) {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidConfiguration,
                format!("environment variable {key} is not allowed"),
            ));
        }
    }
    for path in environment.files.values() {
        let canonical = path.canonicalize().map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::InvalidConfiguration,
                "runtime secret file is not accessible",
            )
        })?;
        if !path.is_absolute() || canonical.starts_with("/nix/store") || !canonical.is_file() {
            return Err(ProtocolError::new(
                ProtocolErrorCode::InvalidConfiguration,
                "runtime secret file must be absolute and outside the Nix store",
            ));
        }
    }
    Ok(())
}

fn is_dangerous_environment(key: &str) -> bool {
    key == "PATH"
        || key == "IFS"
        || key == "BASH_ENV"
        || key == "ENV"
        || key == "PERL5OPT"
        || key == "PYTHONPATH"
        || key == "NODE_OPTIONS"
        || key.starts_with("LD_")
        || key.starts_with("DYLD_")
}

pub(crate) fn child_environment(environment: &AdapterEnvironment) -> Vec<(String, OsString)> {
    let mut vars = Vec::new();
    for key in &environment.allow {
        if let Ok(value) = std::env::var(key) {
            vars.push((key.clone(), OsString::from(value)));
        }
    }
    for (key, path) in &environment.files {
        vars.push((key.clone(), path.as_os_str().to_os_string()));
    }
    vars
}

#[cfg(test)]
pub(crate) fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> Result<ExitStatus, ProtocolError> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    terminate(child);
                    return Err(ProtocolError::new(
                        ProtocolErrorCode::Timeout,
                        "adapter timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::SpawnFailed,
                    "failed to wait for adapter",
                ))
            }
        }
    }
}

fn terminate(child: &mut Child) {
    terminate_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

fn terminate_group(child: &Child) {
    let pid = child.id() as i32;
    // The child starts a fresh process group, so this also terminates ordinary descendants.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

fn collect_output(
    child: &mut Child,
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), ProtocolError> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let hit_limit = Arc::new(AtomicBool::new(false));
    let stdout_limit = hit_limit.clone();
    let stderr_limit = hit_limit.clone();
    let (stdout_tx, stdout_rx) = mpsc::sync_channel(1);
    let (stderr_tx, stderr_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let output = match stdout {
            Some(mut pipe) => read_bounded(&mut pipe, max_stdout, &stdout_limit),
            None => Vec::new(),
        };
        let _ = stdout_tx.send(output);
    });
    thread::spawn(move || {
        let output = match stderr {
            Some(mut pipe) => read_bounded(&mut pipe, max_stderr, &stderr_limit),
            None => Vec::new(),
        };
        let _ = stderr_tx.send(output);
    });

    let start = Instant::now();
    let status = loop {
        if hit_limit.load(Ordering::Relaxed) {
            terminate(child);
            return Err(ProtocolError::new(
                ProtocolErrorCode::Oversized,
                "adapter output exceeds size limit",
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_group(child);
                break status;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    terminate(child);
                    return Err(ProtocolError::new(
                        ProtocolErrorCode::Timeout,
                        "adapter timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                terminate(child);
                return Err(ProtocolError::new(
                    ProtocolErrorCode::SpawnFailed,
                    "failed to wait for adapter",
                ));
            }
        }
    };

    let stdout = stdout_rx
        .recv_timeout(timeout.saturating_sub(start.elapsed()))
        .map_err(|_| ProtocolError::new(ProtocolErrorCode::Timeout, "adapter timed out"))?;
    let stderr = stderr_rx
        .recv_timeout(timeout.saturating_sub(start.elapsed()))
        .map_err(|_| ProtocolError::new(ProtocolErrorCode::Timeout, "adapter timed out"))?;
    if hit_limit.load(Ordering::Relaxed) {
        return Err(ProtocolError::new(
            ProtocolErrorCode::Oversized,
            "adapter output exceeds size limit",
        ));
    }
    Ok((status, stdout, stderr))
}

fn read_bounded(reader: &mut impl Read, max: usize, hit_limit: &AtomicBool) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match reader.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len().saturating_add(n) > max {
                    hit_limit.store(true, Ordering::Relaxed);
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    buf
}

fn classify<T: DeserializeOwned>(
    method: MethodName,
    request_id: &str,
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> Result<T, ProtocolError> {
    if !stderr.is_empty() {
        return Err(ProtocolError::new(
            ProtocolErrorCode::StderrNotEmpty,
            "adapter wrote to stderr",
        ));
    }
    if !status.success() {
        return Err(ProtocolError::new(
            ProtocolErrorCode::NonzeroExit,
            "adapter exited nonzero",
        ));
    }
    let text = std::str::from_utf8(&stdout).map_err(|_| {
        ProtocolError::new(
            ProtocolErrorCode::InvalidUtf8,
            "adapter stdout is not valid utf-8",
        )
    })?;
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() > 1 {
        return Err(ProtocolError::new(
            ProtocolErrorCode::MultipleResponses,
            "adapter returned multiple responses",
        ));
    }
    let line = lines.first().ok_or_else(|| {
        ProtocolError::new(ProtocolErrorCode::Malformed, "adapter returned no response")
    })?;
    let response: AdapterResponse = serde_json::from_str(line).map_err(|_| {
        ProtocolError::new(
            ProtocolErrorCode::Malformed,
            "adapter returned malformed json",
        )
    })?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::new(
            ProtocolErrorCode::UnsupportedVersion,
            "adapter protocol version mismatch",
        ));
    }
    if response.request_id != request_id {
        return Err(ProtocolError::new(
            ProtocolErrorCode::RequestIdMismatch,
            "adapter request id mismatch",
        ));
    }
    if response.method != Some(method) {
        return Err(ProtocolError::new(
            ProtocolErrorCode::MethodMismatch,
            "adapter method mismatch",
        ));
    }
    if response.error.is_some() && response.result.is_some() {
        return Err(ProtocolError::new(
            ProtocolErrorCode::InvalidResult,
            "adapter returned both result and error",
        ));
    }
    if let Some(error) = response.error {
        return Err(error);
    }
    let value = response.result.ok_or_else(|| {
        ProtocolError::new(
            ProtocolErrorCode::InvalidResult,
            "adapter returned no result",
        )
    })?;
    serde_json::from_value(value).map_err(|_| {
        ProtocolError::new(
            ProtocolErrorCode::InvalidResult,
            "adapter returned an invalid result",
        )
    })
}
