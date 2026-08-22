fn main() {
    if let Some(milliseconds) = std::env::var_os("ANDROID_MANAGER_FIXTURE_SLEEP_MS") {
        std::thread::sleep(std::time::Duration::from_millis(
            milliseconds.to_string_lossy().parse().unwrap_or(1000),
        ));
        return;
    }
    if let Err(error) = android_manager_plugin_protocol::run_fixture() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
