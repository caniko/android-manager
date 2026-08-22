use std::os::unix::process::CommandExt;

#[allow(clippy::zombie_processes)] // The escaped orphan is the behavior under test.
fn main() {
    if std::env::var_os("ANDROID_MANAGER_FIXTURE_SLEEP").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(1));
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().expect("fixture path"));
    child
        .env("ANDROID_MANAGER_FIXTURE_SLEEP", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        child.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    child.spawn().expect("spawn escaped fixture");
}
