//! Native macOS case-insensitive filesystem fixture, with no skip path.
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct CaseInsensitiveVolume {
    temporary: tempfile::TempDir,
    mount: Option<PathBuf>,
}

impl CaseInsensitiveVolume {
    pub fn new() -> Self {
        let temporary = tempfile::tempdir().expect("case-insensitive fixture directory");
        let mut volume = Self {
            temporary,
            mount: None,
        };
        if !is_case_insensitive(volume.path()) {
            let image = volume.temporary.path().join("case-insensitive.dmg");
            hdiutil(&[
                "create".as_ref(),
                "-size".as_ref(),
                "128m".as_ref(),
                "-fs".as_ref(),
                "APFS".as_ref(),
                "-volname".as_ref(),
                "rustx-reserved-name-test".as_ref(),
                "-format".as_ref(),
                "UDRW".as_ref(),
                image.as_os_str(),
            ]);
            let mount = volume.temporary.path().join("mounted");
            std::fs::create_dir(&mount).expect("APFS fixture mountpoint");
            hdiutil(&[
                "attach".as_ref(),
                "-nobrowse".as_ref(),
                "-mountpoint".as_ref(),
                mount.as_os_str(),
                image.as_os_str(),
            ]);
            volume.mount = Some(mount);
        }
        assert!(
            is_case_insensitive(volume.path()),
            "the macOS regression requires a verified case-insensitive filesystem"
        );
        volume
    }

    pub fn path(&self) -> &Path {
        self.mount.as_deref().unwrap_or(self.temporary.path())
    }
}

fn is_case_insensitive(root: &Path) -> bool {
    let upper = root.join("rustx-CASE-probe");
    std::fs::write(&upper, b"case-insensitive fixture probe").expect("write case probe");
    let actual = std::fs::metadata(&upper).expect("stat case probe");
    let equivalent = match std::fs::metadata(root.join("rustx-case-probe")) {
        Ok(alias) => (actual.dev(), actual.ino()) == (alias.dev(), alias.ino()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("cannot verify the fixture filesystem's case behavior: {error}"),
    };
    std::fs::remove_file(upper).expect("remove case probe");
    equivalent
}

fn hdiutil(arguments: &[&std::ffi::OsStr]) {
    let output = Command::new("/usr/bin/hdiutil")
        .args(arguments)
        .output()
        .expect("hdiutil is required to create the case-insensitive APFS test volume");
    assert!(
        output.status.success(),
        "cannot provide the required case-insensitive APFS fixture: hdiutil {arguments:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Drop for CaseInsensitiveVolume {
    fn drop(&mut self) {
        if let Some(mount) = &self.mount {
            let result = Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(mount)
                .output();
            match result {
                Ok(output) if output.status.success() => {}
                failed if std::thread::panicking() => {
                    eprintln!("failed to detach APFS regression fixture: {failed:?}");
                }
                failed => panic!("failed to detach APFS regression fixture: {failed:?}"),
            }
        }
    }
}
