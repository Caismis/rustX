//! Bounded descriptor-relative sampling, without following workspace symlinks.
use crate::runtime::cancellation::CancellationSignal;
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat, readlinkat},
    sys::stat::{Mode, SFlag, fstatat},
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path},
    time::{Duration, Instant},
};

pub(super) struct Budget<'a> {
    cancellation: &'a CancellationSignal,
    remaining: u64,
    deadline: Instant,
}

impl<'a> Budget<'a> {
    pub fn new(cancellation: &'a CancellationSignal) -> Self {
        Self {
            cancellation,
            remaining: 512 * 1024 * 1024,
            deadline: Instant::now() + Duration::from_secs(30),
        }
    }

    pub(super) fn check(&self, bytes: u64) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err("Raw snapshot cancelled".into());
        }
        if Instant::now() >= self.deadline || bytes > self.remaining {
            return Err("Raw snapshot sampling budget exceeded".into());
        }
        Ok(())
    }
}

pub(super) enum Sample {
    File(String),
    Missing,
    Unsupported,
}

pub(super) fn directory(root: &File, path: &Path) -> Result<Option<File>, String> {
    let mut directory = root.try_clone().map_err(|error| error.to_string())?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err("invalid Git workspace path".into());
        };
        match openat(
            &directory,
            name,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => directory = fd.into(),
            Err(Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(Some(directory))
}

/// Non-following, nonblocking opens keep a replacement from redirecting the
/// sample through a symlink or parking the worker on a FIFO/device.
pub(super) fn sample(
    root: &File,
    path: &Path,
    output: &Path,
    budget: &mut Budget<'_>,
) -> Result<Sample, String> {
    budget.check(0)?;
    let parent = path.parent().ok_or("invalid Git workspace path")?;
    let Some(parent) = directory(root, parent)? else {
        return Ok(Sample::Missing);
    };
    let name = path.file_name().ok_or("invalid Git workspace path")?;
    let metadata = match fstatat(&parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(Errno::ENOENT | Errno::ENOTDIR) => return Ok(Sample::Missing),
        Err(error) => return Err(error.to_string()),
    };
    let kind = SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT;
    if kind == SFlag::S_IFLNK {
        let target = readlinkat(&parent, name).map_err(|error| error.to_string())?;
        budget.check(target.as_bytes().len() as u64)?;
        fs::write(output, target.as_bytes()).map_err(|error| error.to_string())?;
        budget.remaining -= target.as_bytes().len() as u64;
        return Ok(Sample::File("120000".into()));
    }
    if kind == SFlag::S_IFDIR {
        return Ok(Sample::Missing);
    }
    if kind != SFlag::S_IFREG {
        return Ok(Sample::Unsupported);
    }
    let file: File = match openat(
        &parent,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd.into(),
        Err(Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP) => return Ok(Sample::Missing),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Ok(Sample::Unsupported);
    }
    copy(file, output, metadata.len(), budget)?;
    Ok(Sample::File(
        if metadata.mode() & 0o111 == 0 {
            "100644"
        } else {
            "100755"
        }
        .into(),
    ))
}

fn copy(mut file: File, output: &Path, size: u64, budget: &mut Budget<'_>) -> Result<(), String> {
    budget.check(size)?;
    let mut staged = File::create(output).map_err(|error| error.to_string())?;
    let mut buffer = vec![0; 64 * 1024];
    let mut copied = 0;
    loop {
        budget.check(0)?;
        let bytes = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if bytes == 0 {
            if copied != size {
                return Err("Workspace file shrank during snapshot sampling".into());
            }
            return Ok(());
        }
        copied += bytes as u64;
        // Never publish a truncated endpoint or follow an indefinitely growing
        // file. A failed observation leaves the execution result unaffected.
        if copied > size {
            return Err("Workspace file grew during snapshot sampling".into());
        }
        budget.check(bytes as u64)?;
        staged
            .write_all(&buffer[..bytes])
            .map_err(|error| error.to_string())?;
        budget.remaining -= bytes as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_rejects_growing_and_shrinking_endpoints_instead_of_publishing_a_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input");
        fs::write(&input, b"1234").unwrap();
        let cancellation = CancellationSignal::new();
        for expected in [3, 5] {
            let output = directory.path().join(format!("output-{expected}"));
            let mut budget = Budget::new(&cancellation);
            assert!(copy(File::open(&input).unwrap(), &output, expected, &mut budget).is_err());
        }
    }

    #[test]
    fn sampling_budget_rejects_excess_bytes_and_expired_or_cancelled_work() {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        fs::write(directory.path().join("input"), b"1234").unwrap();
        let output = directory.path().join("output");
        let cancellation = CancellationSignal::new();
        let mut budget = Budget::new(&cancellation);
        budget.remaining = 7;
        assert!(matches!(
            sample(&root, Path::new("input"), &output, &mut budget),
            Ok(Sample::File(_))
        ));
        assert_eq!(fs::read(&output).unwrap(), b"1234");
        fs::remove_file(&output).unwrap();
        assert!(sample(&root, Path::new("input"), &output, &mut budget).is_err());
        assert!(!output.exists(), "no truncated file is staged");
        budget.remaining = 7;
        budget.deadline = Instant::now();
        assert!(sample(&root, Path::new("input"), &output, &mut budget).is_err());
        budget.deadline = Instant::now() + Duration::from_secs(30);
        cancellation.cancel();
        assert!(sample(&root, Path::new("input"), &output, &mut budget).is_err());
        assert!(!output.exists());
    }

    #[test]
    fn sampling_never_follows_symlink_parents_or_reads_a_fifo() {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.path().join("link")).unwrap();
        nix::unistd::mkfifo(
            &directory.path().join("fifo"),
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .unwrap();
        let output = directory.path().join("output");
        let cancellation = CancellationSignal::new();
        let mut budget = Budget::new(&cancellation);
        assert!(matches!(
            sample(&root, Path::new("link/secret"), &output, &mut budget),
            Ok(Sample::Missing)
        ));
        assert!(matches!(
            sample(&root, Path::new("fifo"), &output, &mut budget),
            Ok(Sample::Unsupported)
        ));
        assert!(!output.exists());
    }
}
