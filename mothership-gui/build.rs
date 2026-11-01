use std::path::Path;
use std::process::Command;

fn main() {
    // Only the embedded-frontend build needs dist/. Dev builds (--no-default-features) load devUrl.
    if std::env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_some() {
        build_frontend();
    }
    tauri_build::build()
}

fn build_frontend() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for input in ["ui", "hub.html", "vite.config.js", "package.json", "package-lock.json"] {
        println!("cargo:rerun-if-changed={}", dir.join(input).display());
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let status = Command::new(npm)
        .args(["run", "build"])
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to start `{npm} run build` in {}: {e}", dir.display()));
    if !status.success() {
        panic!("`{npm} run build` failed in {}", dir.display());
    }
}