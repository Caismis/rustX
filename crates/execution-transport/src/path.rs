//! No-follow filesystem pinning shared by the Server and Client ends.
//!
//! Both sides open credential-bearing artifacts — the published
//! bootstrap bundle, a private CA policy file — that must provably be
//! the object the owner placed there, never whatever a swapped or
//! symlinked path component happens to reach at open time. Pathname
//! resolution cannot promise that; descriptor pinning can: walk every
//! ancestor with `openat(O_DIRECTORY|O_NOFOLLOW)` and every later
//! `openat`/`fstatat`/`unlinkat` against the pinned descriptor is
//! immune to renames and symlink swaps at the original path.

use std::fs::File;
use std::io;
use std::path::{Component, Path};

const OPEN_DIR_FLAGS: nix::fcntl::OFlag = nix::fcntl::OFlag::O_DIRECTORY
    .union(nix::fcntl::OFlag::O_NOFOLLOW)
    .union(nix::fcntl::OFlag::O_CLOEXEC);

/// Opens the absolute directory `dir` by walking every component from
/// `/` with `openat(O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)` — no component
/// is ever a symlink, and the returned descriptor is provably the
/// directory itself: a later rename or symlink swap at any ancestor of
/// the pathname cannot redirect a lookup done through it.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] when `dir` is not an
/// absolute path of normal components, and the underlying `openat`
/// error when a component is missing, not a directory, or a symlink.
pub fn open_dir_nofollow(dir: &Path) -> io::Result<File> {
    if !dir.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no-follow path must be absolute",
        ));
    }
    let mut fd = nix::fcntl::open("/", OPEN_DIR_FLAGS, nix::sys::stat::Mode::empty())
        .map_err(io::Error::from)?;
    for component in dir.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                fd = nix::fcntl::openat(&fd, name, OPEN_DIR_FLAGS, nix::sys::stat::Mode::empty())
                    .map_err(io::Error::from)?;
            }
            Component::ParentDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "no-follow path must carry normal components only",
                ));
            }
        }
    }
    Ok(File::from(fd))
}
