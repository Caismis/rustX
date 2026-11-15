//! Profile directory for the Python guard installed beside built executables.
//!
//! The build script includes this file. Cargo does not compile `build.rs`
//! with `cfg(test)`, so the cases below run as part of the library tests.

use std::path::{Path, PathBuf};

/// Cargo places build-script output at `<profile>/build/<package>/out`.
/// A custom `--profile` uses its own directory name, so the profile is the
/// parent of `build` rather than a directory named `debug` or `release`.
pub(crate) fn profile_dir(out_dir: &Path) -> PathBuf {
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
