fn main() {
    if let Err(error) = android_manager_plugin_protocol::run_fixture() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
