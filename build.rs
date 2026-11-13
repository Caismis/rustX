//! Install the default-Python guard beside the built executables.
//!
//! Runtime lookup uses that directory, not the source checkout. `cargo test`
//! runs a binary from `deps/`, and `cargo build` writes `rustx` in the profile
//! directory, so both locations receive the same scripts.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/python-guard/python");
    println!("cargo:rerun-if-changed=assets/python-guard/python3");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest.join("assets/python-guard");
    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    let profile = out
        .ancestors()
        .nth(3)
        .expect("OUT_DIR is <profile>/build/<package>/out")
        .to_path_buf();
    let name = profile
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    assert!(
        name == "debug" || name == "release",
        "refusing to install the Python guard outside the cargo profile directory: {profile:?}"
    );
    install(&source, &profile.join("python-guard"));
    install(&source, &profile.join("deps/python-guard"));
}

fn install(source: &Path, directory: &Path) {
    fs::create_dir_all(directory).expect("python-guard directory");
    for name in ["python", "python3"] {
        let destination = directory.join(name);
        fs::copy(source.join(name), &destination).expect("copy python guard");
        let mut permissions = fs::metadata(&destination)
            .expect("guard metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&destination, permissions).expect("guard mode");
    }
}
