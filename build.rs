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
    let profile = profile_dir(&out);
    install(&source, &profile.join("python-guard"));
    install(&source, &profile.join("deps/python-guard"));
}

/// Cargo places build-script output at `<profile>/build/<package>/out`.
/// A custom `--profile` uses its own directory name, so the profile is the
/// parent of `build` rather than a directory named `debug` or `release`.
fn profile_dir(out_dir: &Path) -> PathBuf {
    let package_dir = out_dir
        .parent()
        .unwrap_or_else(|| panic!("OUT_DIR has no package directory: {}", out_dir.display()));
    let build_dir = package_dir
        .parent()
        .unwrap_or_else(|| panic!("OUT_DIR has no build directory: {}", out_dir.display()));
    assert_eq!(
        build_dir.file_name().and_then(|name| name.to_str()),
        Some("build"),
        "refusing to install the Python guard outside <profile>/build/<package>/out: {}",
        out_dir.display()
    );
    build_dir
        .parent()
        .unwrap_or_else(|| {
            panic!(
                "build directory has no profile parent: {}",
                build_dir.display()
            )
        })
        .to_path_buf()
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

#[cfg(test)]
mod tests {
    use super::profile_dir;
    use std::path::Path;

    #[test]
    fn custom_profile_directories_are_parents_of_build() {
        assert_eq!(
            profile_dir(Path::new("/work/target/debug/build/rustx-hash/out")),
            Path::new("/work/target/debug")
        );
        assert_eq!(
            profile_dir(Path::new("/work/target/ci/build/rustx-hash/out")),
            Path::new("/work/target/ci")
        );
        assert_eq!(
            profile_dir(Path::new(
                "/work/target/x86_64-unknown-linux-gnu/ci/build/rustx-hash/out"
            )),
            Path::new("/work/target/x86_64-unknown-linux-gnu/ci")
        );
    }
}
