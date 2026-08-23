use std::time::{SystemTime, UNIX_EPOCH};

// Bakes the compile time into the binary as BUILD_EPOCH (read by
// utility_core::AppInfo::build_stamp for the header and `--version`).
fn main() {
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=BUILD_EPOCH={epoch}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
