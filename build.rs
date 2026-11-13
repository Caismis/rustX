//! Install the default-Python guard beside the built executables.
//!
//! Runtime lookup uses that directory, not the source checkout. `cargo test`
//! runs a binary from `deps/`, and `cargo build` writes `rustx` in the profile
//! directory, so both locations receive the same scripts.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[path = "src/tools/profile_dir.rs"]
mod profile_dir;

fn main() {
    println!("cargo:rerun-if-changed=assets/python-guard/python");
    println!("cargo:rerun-if-changed=assets/python-guard/python3");
    println!("cargo:rerun-if-changed=src/tools/profile_dir.rs");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest.join("assets/python-guard");
    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    let profile = profile_dir::profile_dir(&out);
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
