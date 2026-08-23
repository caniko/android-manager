use android_manager_model::{
    DeviceCapabilities, DeviceSelector, DeviceSnapshot, InstalledPackage, Operation,
};
use anyhow::{anyhow, bail, Result};
use nix_manager_core::exec::Captured;
use std::collections::{BTreeMap, BTreeSet};

pub trait CommandRunner: Send + Sync {
    fn cap(&self, program: &str, args: &[&str]) -> Captured;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn cap(&self, program: &str, args: &[&str]) -> Captured {
        nix_manager_core::exec::cap(program, args.iter().copied())
    }
}

#[derive(Default)]
pub struct ScriptedRunner {
    pub responses: BTreeMap<String, Captured>,
}

impl ScriptedRunner {
    pub fn insert(&mut self, key: impl Into<String>, stdout: impl Into<String>) {
        self.responses.insert(
            key.into(),
            Captured {
                exit_code: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            },
        );
    }
}

impl CommandRunner for ScriptedRunner {
    fn cap(&self, program: &str, args: &[&str]) -> Captured {
        let key = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.responses.get(&key).cloned().unwrap_or(Captured {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("unexpected command: {key}"),
        })
    }
}

pub trait DeviceBackend {
    fn discover(&self, selector: &DeviceSelector) -> Result<String>;
    fn snapshot(&self, serial: &str, inspect: &[String]) -> Result<DeviceSnapshot>;
    fn execute(&self, serial: &str, user_id: u32, operation: &Operation) -> Result<()>;
}

#[derive(Clone)]
pub struct MemoryDevice {
    pub serial: String,
    pub snapshot: std::sync::Arc<std::sync::Mutex<DeviceSnapshot>>,
}

impl MemoryDevice {
    pub fn new(snapshot: DeviceSnapshot) -> Self {
        Self {
            serial: snapshot.serial.clone(),
            snapshot: std::sync::Arc::new(std::sync::Mutex::new(snapshot)),
        }
    }
}

impl DeviceBackend for MemoryDevice {
    fn discover(&self, selector: &DeviceSelector) -> Result<String> {
        if let Some(serial) = &selector.serial {
            if serial != &self.serial {
                bail!("memory device {serial} is not connected");
            }
        }
        Ok(self.serial.clone())
    }

    fn snapshot(&self, _serial: &str, _inspect: &[String]) -> Result<DeviceSnapshot> {
        Ok(self.snapshot.lock().expect("snapshot lock").clone())
    }

    fn execute(&self, _serial: &str, user_id: u32, operation: &Operation) -> Result<()> {
        let mut snapshot = self.snapshot.lock().expect("snapshot lock");
        match operation {
            Operation::Install {
                package_name,
                version_code,
                signers,
                ..
            }
            | Operation::Upgrade {
                package_name,
                to: version_code,
                signers,
                ..
            } => {
                snapshot
                    .packages
                    .retain(|pkg| !(pkg.name == *package_name && pkg.user_id == user_id));
                snapshot.packages.push(InstalledPackage {
                    name: package_name.clone(),
                    user_id,
                    version_code: *version_code,
                    version_name: None,
                    signers: signers.clone(),
                    enabled: true,
                    inspected: true,
                });
            }
            Operation::EnableExisting { package_name } => {
                let donor = snapshot
                    .packages
                    .iter()
                    .find(|pkg| pkg.name == *package_name)
                    .cloned()
                    .ok_or_else(|| anyhow!("{package_name} is not installed on any user"))?;
                snapshot
                    .packages
                    .retain(|pkg| !(pkg.name == *package_name && pkg.user_id == user_id));
                snapshot
                    .packages
                    .push(InstalledPackage { user_id, ..donor });
            }
            Operation::Uninstall { package_name } => {
                snapshot
                    .packages
                    .retain(|pkg| !(pkg.name == *package_name && pkg.user_id == user_id));
            }
            Operation::Manual { instruction }
            | Operation::Blocked {
                reason: instruction,
            } => {
                bail!("refusing to execute non-automatic operation: {instruction}")
            }
        }
        Ok(())
    }
}

pub struct AdbBackend<R> {
    pub runner: R,
    pub adb: String,
}

impl Default for AdbBackend<SystemRunner> {
    fn default() -> Self {
        Self {
            runner: SystemRunner,
            adb: "adb".into(),
        }
    }
}

impl<R: CommandRunner> DeviceBackend for AdbBackend<R> {
    fn discover(&self, selector: &DeviceSelector) -> Result<String> {
        let captured = self.runner.cap(&self.adb, &["devices"]);
        if !captured.ok() {
            bail!("adb devices failed: {}", captured.stderr);
        }
        let serials = parse_adb_devices(&captured.stdout);
        match (selector.serial.as_deref(), serials.as_slice()) {
            (Some(serial), _) if serials.iter().any(|found| found == serial) => {
                Ok(serial.to_string())
            }
            (Some(serial), _) => bail!("adb device {serial} is not connected"),
            (None, [serial]) => Ok(serial.clone()),
            (None, []) => bail!("no adb devices connected"),
            (None, _) => bail!("multiple adb devices connected; set transport.selector.serial"),
        }
    }

    fn snapshot(&self, serial: &str, inspect: &[String]) -> Result<DeviceSnapshot> {
        let manufacturer = self.prop(serial, "ro.product.manufacturer")?;
        let sdk = self
            .prop(serial, "ro.build.version.sdk")?
            .parse()
            .unwrap_or(0);
        let abi_list = self.prop(serial, "ro.product.cpu.abilist")?;
        let abis = abi_list
            .split(',')
            .filter(|abi| !abi.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let users = parse_users(&self.shell(serial, &["pm", "list", "users"])?);
        let inspect: BTreeSet<&str> = inspect.iter().map(String::as_str).collect();
        let mut dump_cache = BTreeMap::new();
        let mut packages = Vec::new();
        for user in &users {
            let listed = self.shell(
                serial,
                &["pm", "list", "packages", "--user", &user.to_string()],
            )?;
            for name in parse_packages(&listed) {
                if inspect.contains(name.as_str()) {
                    if !dump_cache.contains_key(&name) {
                        dump_cache.insert(
                            name.clone(),
                            self.shell(serial, &["dumpsys", "package", &name])?,
                        );
                    }
                    packages.push(parse_package_dump(&name, *user, &dump_cache[&name]));
                } else {
                    packages.push(InstalledPackage {
                        name,
                        user_id: *user,
                        version_code: 0,
                        version_name: None,
                        signers: Vec::new(),
                        enabled: false,
                        inspected: false,
                    });
                }
            }
        }
        Ok(DeviceSnapshot {
            transport_id: format!("adb:{serial}"),
            serial: serial.to_string(),
            manufacturer: Some(manufacturer),
            sdk,
            abis,
            users,
            packages,
            capabilities: DeviceCapabilities::default(),
        })
    }

    fn execute(&self, serial: &str, user_id: u32, operation: &Operation) -> Result<()> {
        match operation {
            Operation::Install { artifact_paths, .. }
            | Operation::Upgrade { artifact_paths, .. } => {
                if artifact_paths.is_empty() {
                    bail!("install is missing realized artifact paths");
                }
                let replace = matches!(operation, Operation::Upgrade { .. });
                let mut args = vec![
                    "-s".to_string(),
                    serial.to_string(),
                    if artifact_paths.len() == 1 {
                        "install".into()
                    } else {
                        "install-multiple".into()
                    },
                ];
                if replace {
                    args.push("-r".into());
                }
                args.push("--user".into());
                args.push(user_id.to_string());
                args.extend(
                    artifact_paths
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned()),
                );
                self.adb_ok(&args.iter().map(String::as_str).collect::<Vec<_>>())
            }
            Operation::EnableExisting { package_name } => self
                .shell(
                    serial,
                    &[
                        "cmd",
                        "package",
                        "install-existing",
                        "--user",
                        &user_id.to_string(),
                        package_name,
                    ],
                )
                .map(|_| ()),
            Operation::Uninstall { package_name } => self.adb_ok(&[
                "-s",
                serial,
                "uninstall",
                "--user",
                &user_id.to_string(),
                package_name,
            ]),
            Operation::Manual { instruction }
            | Operation::Blocked {
                reason: instruction,
            } => {
                bail!("refusing to execute non-automatic operation: {instruction}")
            }
        }
    }
}

impl<R: CommandRunner> AdbBackend<R> {
    fn prop(&self, serial: &str, name: &str) -> Result<String> {
        self.shell(serial, &["getprop", name])
    }

    fn shell(&self, serial: &str, args: &[&str]) -> Result<String> {
        let mut full = vec!["-s", serial, "shell"];
        full.extend_from_slice(args);
        let captured = self.runner.cap(&self.adb, &full);
        if !captured.ok() {
            bail!("adb {} failed: {}", full.join(" "), captured.stderr);
        }
        Ok(captured.stdout.trim().to_string())
    }

    fn adb_ok(&self, args: &[&str]) -> Result<()> {
        let captured = self.runner.cap(&self.adb, args);
        if !captured.ok() {
            return Err(anyhow!(
                "adb {} failed: {}",
                args.join(" "),
                captured.stderr
            ));
        }
        Ok(())
    }
}

pub fn parse_adb_devices(output: &str) -> Vec<String> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            (state == "device").then(|| serial.to_string())
        })
        .collect()
}

fn parse_users(output: &str) -> Vec<u32> {
    let users: Vec<u32> = output
        .lines()
        .filter_map(|line| {
            let start = line.find("UserInfo{")? + 9;
            let rest = &line[start..];
            let id = rest.split(':').next()?;
            id.parse().ok()
        })
        .collect();
    if users.is_empty() {
        vec![0]
    } else {
        users
    }
}

fn parse_packages(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("package:"))
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_package_dump(name: &str, user_id: u32, dump: &str) -> InstalledPackage {
    let mut version_code = 0;
    let mut version_name = None;
    let mut signers = Vec::new();
    for line in dump.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("versionCode=") {
            version_code = value
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(version_code);
        } else if let Some(value) = trimmed.strip_prefix("versionName=") {
            version_name = Some(value.to_string());
        } else if let Some(value) = trimmed.strip_prefix("SHA-256 digest:") {
            signers.push(format!("sha256:{}", value.trim().replace(':', "")));
        }
    }
    InstalledPackage {
        name: name.to_string(),
        user_id,
        version_code,
        version_name,
        signers,
        enabled: true,
        inspected: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_multiple_devices_without_selector() {
        let mut runner = ScriptedRunner::default();
        runner.insert(
            "adb devices",
            "List of devices attached\nAAA\tdevice\nBBB\tdevice\n",
        );
        let backend = AdbBackend {
            runner,
            adb: "adb".into(),
        };
        let err = backend.discover(&DeviceSelector::default()).unwrap_err();
        assert!(err.to_string().contains("multiple adb devices"));
    }

    #[test]
    fn upgrade_uses_replace_flag() {
        let mut runner = ScriptedRunner::default();
        runner.insert("adb -s SERIAL install -r --user 0 /tmp/app.apk", "");
        let backend = AdbBackend {
            runner,
            adb: "adb".into(),
        };
        backend
            .execute(
                "SERIAL",
                0,
                &Operation::Upgrade {
                    package_name: "org.example.app".into(),
                    from: 1,
                    to: 2,
                    signers: vec![],
                    artifact_paths: vec!["/tmp/app.apk".into()],
                },
            )
            .unwrap();
    }

    #[test]
    fn install_does_not_replace() {
        let mut runner = ScriptedRunner::default();
        runner.insert("adb -s SERIAL install --user 0 /tmp/app.apk", "");
        let backend = AdbBackend {
            runner,
            adb: "adb".into(),
        };
        backend
            .execute(
                "SERIAL",
                0,
                &Operation::Install {
                    package_name: "org.example.app".into(),
                    version_code: 2,
                    signers: vec![],
                    artifact_paths: vec!["/tmp/app.apk".into()],
                },
            )
            .unwrap();
    }

    #[test]
    fn snapshot_dumpsys_only_inspected_packages() {
        let mut runner = ScriptedRunner::default();
        runner.insert(
            "adb -s SERIAL shell getprop ro.product.manufacturer",
            "Google",
        );
        runner.insert("adb -s SERIAL shell getprop ro.build.version.sdk", "35");
        runner.insert(
            "adb -s SERIAL shell getprop ro.product.cpu.abilist",
            "arm64-v8a",
        );
        runner.insert(
            "adb -s SERIAL shell pm list users",
            "Users:\n\tUserInfo{0:Owner:c13} running",
        );
        runner.insert(
            "adb -s SERIAL shell pm list packages --user 0",
            "package:org.example.app\npackage:com.android.systemui\n",
        );
        runner.insert(
            "adb -s SERIAL shell dumpsys package org.example.app",
            "    versionCode=12\n    versionName=1.2\n    SHA-256 digest: ab:cd\n",
        );
        let backend = AdbBackend {
            runner,
            adb: "adb".into(),
        };
        let snapshot = backend
            .snapshot("SERIAL", &["org.example.app".into()])
            .unwrap();
        let app = snapshot
            .packages
            .iter()
            .find(|package| package.name == "org.example.app")
            .unwrap();
        assert_eq!(app.version_code, 12);
        let extra = snapshot
            .packages
            .iter()
            .find(|package| package.name == "com.android.systemui")
            .unwrap();
        assert!(!extra.inspected);
        assert!(extra.signers.is_empty());
    }

    #[test]
    fn enable_existing_uses_install_existing() {
        let mut runner = ScriptedRunner::default();
        runner.insert(
            "adb -s SERIAL shell cmd package install-existing --user 10 org.example.app",
            "Package org.example.app installed for user: 10",
        );
        let backend = AdbBackend {
            runner,
            adb: "adb".into(),
        };
        backend
            .execute(
                "SERIAL",
                10,
                &Operation::EnableExisting {
                    package_name: "org.example.app".into(),
                },
            )
            .unwrap();
    }

    #[test]
    fn parses_inventory() {
        let dump = "    versionCode=12 minSdk=28\n    versionName=1.2\n    SHA-256 digest: ab:cd\n";
        let pkg = parse_package_dump("org.example.app", 0, dump);
        assert_eq!(pkg.version_code, 12);
        assert_eq!(pkg.signers, vec!["sha256:abcd"]);
        assert_eq!(
            parse_users("Users:\n\tUserInfo{0:Owner:c13} running"),
            vec![0]
        );
    }
}
