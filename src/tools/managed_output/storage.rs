//! Trusted Conversation-local bindings for disposable output namespaces.
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use nix::fcntl::{Flock, FlockArg, OFlag, open, openat};
use nix::sys::stat::{Mode, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use serde::{Deserialize, Serialize};

use crate::runtime::identity::ConversationId;

const BINDINGS: &str = "output-namespaces.json";
const MARKER: &str = ".owner";
pub(super) const RETRIES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    root: PathBuf,
    device: u64,
    inode: u64,
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ownership {
    conversation: ConversationId,
    owner: PathBuf,
    namespaces: Vec<Binding>,
}

/// OS randomness is independent of all runtime semantic identity generators.
pub(super) fn random_name() -> io::Result<String> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("string write");
            s
        })
}

fn invalid() -> io::Error {
    io::Error::other("invalid temporary output ownership")
}

fn safe_hex(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub(super) fn directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    Ok(File::from(openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?))
}

/// Walk an already canonical absolute spelling, without following any links.
fn directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut current = File::from(open(
        Path::new("/"),
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => current = directory_at(&current, name)?,
            _ => return Err(invalid()),
        }
    }
    Ok(current)
}

fn read(owner: &Path, conversation: &ConversationId) -> io::Result<Ownership> {
    let parent = directory(owner)?;
    let file = match openat(
        &parent,
        BINDINGS,
        OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(nix::errno::Errno::ENOENT) => {
            return Ok(Ownership {
                conversation: conversation.clone(),
                owner: owner.to_owned(),
                namespaces: Vec::new(),
            });
        }
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    let ownership: Ownership = serde_json::from_reader(file).map_err(io::Error::other)?;
    if ownership.owner != owner || ownership.conversation != *conversation {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    for binding in &ownership.namespaces {
        let name = binding
            .root
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|s| s.strip_prefix("rx-"));
        if !binding.root.is_absolute()
            || binding
                .root
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
            || !name.is_some_and(safe_hex)
            || !safe_hex(&binding.token)
            || !seen.insert(&binding.root)
            || owner.starts_with(&binding.root)
            || binding.root.starts_with(owner)
        {
            return Err(invalid());
        }
    }
    Ok(ownership)
}

fn lock(owner: &Path) -> io::Result<Flock<File>> {
    let file = File::from(openat(
        &directory(owner)?,
        "output-namespaces.lock",
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, e)| io::Error::from(e))
}

fn write(ownership: &Ownership) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(&ownership.owner)?;
    serde_json::to_writer(file.as_file_mut(), ownership).map_err(io::Error::other)?;
    file.flush()?;
    // One binding publication per namespace, never one write/fsync per output.
    // Losing this auxiliary metadata at machine failure only forfeits cleanup;
    // it cannot corrupt canonical Conversation data or authorize another root.
    file.persist(ownership.owner.join(BINDINGS))
        .map_err(|e| e.error)?;
    Ok(())
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Marker {
    owner: PathBuf,
    conversation: ConversationId,
    token: String,
}

impl Binding {
    fn marker(&self, owner: &Path, conversation: &ConversationId) -> Marker {
        Marker {
            owner: owner.to_owned(),
            conversation: conversation.clone(),
            token: self.token.clone(),
        }
    }
    fn directory(&self) -> io::Result<File> {
        let dir = directory(&self.root)?;
        let metadata = dir.metadata()?;
        if metadata.dev() != self.device
            || metadata.ino() != self.inode
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.mode() & 0o777 != 0o700
        {
            return Err(invalid());
        }
        Ok(dir)
    }
    fn open(&self, owner: &Path, conversation: &ConversationId) -> io::Result<File> {
        let dir = self.directory()?;
        let mut marker = File::from(
            openat(
                &dir,
                MARKER,
                OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| {
                if e == nix::errno::Errno::ENOENT {
                    invalid()
                } else {
                    io::Error::from(e)
                }
            })?,
        );
        if !marker.metadata()?.is_file() {
            return Err(invalid());
        }
        let marker: Marker =
            serde_json::from_reader((&mut marker).take(8192)).map_err(io::Error::other)?;
        if marker != self.marker(owner, conversation) {
            return Err(invalid());
        }
        Ok(dir)
    }
}

#[derive(Debug)]
pub(super) struct Namespaces {
    owner: PathBuf,
    conversation: ConversationId,
    temp_root: PathBuf,
    bindings: Vec<Binding>,
    current: Option<Binding>,
    workspace: Option<PathBuf>,
    issued: BTreeSet<PathBuf>,
    #[cfg(test)]
    names: std::collections::VecDeque<String>,
}

impl Namespaces {
    pub(super) fn new(
        owner: &Path,
        conversation: &ConversationId,
        temp_root: &Path,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(owner)?;
        // The private metadata root must never be a symlink. The system temp
        // root, on the other hand, may legitimately canonicalize (e.g. macOS).
        if std::fs::symlink_metadata(owner)?.file_type().is_symlink() {
            return Err(invalid());
        }
        let owner = owner.canonicalize()?;
        let ownership = read(&owner, conversation)?;
        Ok(Self {
            owner,
            conversation: conversation.clone(),
            temp_root: temp_root.to_owned(),
            bindings: ownership.namespaces,
            current: None,
            workspace: None,
            issued: BTreeSet::new(),
            #[cfg(test)]
            names: std::collections::VecDeque::default(),
        })
    }

    fn allocate(
        ownership: &mut Ownership,
        temp_root: &Path,
        name: &mut dyn FnMut() -> io::Result<String>,
        workspace: Option<&Path>,
    ) -> io::Result<Binding> {
        let parent = directory(temp_root)?;
        for _ in 0..RETRIES {
            let leaf = format!("rx-{}", name()?);
            let root = temp_root.join(&leaf);
            if ownership.namespaces.iter().any(|b| b.root == root)
                || root.starts_with(&ownership.owner)
                || ownership.owner.starts_with(&root)
                || workspace.is_some_and(|w| root.starts_with(w) || w.starts_with(&root))
            {
                continue;
            }
            match mkdirat(&parent, leaf.as_str(), Mode::from_bits_truncate(0o700)) {
                Ok(()) => {}
                Err(nix::errno::Errno::EEXIST) => continue,
                Err(e) => return Err(e.into()),
            }
            let dir = directory_at(&parent, OsStr::new(&leaf))?;
            let metadata = dir.metadata()?;
            let binding = Binding {
                root,
                device: metadata.dev(),
                inode: metadata.ino(),
                token: random_name()?,
            };
            let mut marker = File::from(openat(
                &dir,
                MARKER,
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            serde_json::to_writer(
                &mut marker,
                &binding.marker(&ownership.owner, &ownership.conversation),
            )
            .map_err(io::Error::other)?;
            ownership.namespaces.push(binding.clone());
            write(ownership)?;
            return Ok(binding);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary output namespace collision budget exhausted",
        ))
    }

    pub(super) fn allocation_location(&self) -> &Path {
        self.current
            .as_ref()
            .map_or(self.temp_root.as_path(), |binding| binding.root.as_path())
    }

    pub(super) fn set_workspace(&mut self, workspace: &Path) {
        self.workspace = Some(workspace.to_owned());
    }

    fn current_directory(&mut self) -> io::Result<File> {
        if let Some(current) = &self.current {
            match current.open(&self.owner, &self.conversation) {
                Ok(dir) => return Ok(dir),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        // Recovery reads trusted bindings without touching the current temp
        // root. Only an actual allocation needs that root to be available.
        let temp_root = self.temp_root.canonicalize()?;
        let _lock = lock(&self.owner)?;
        let mut ownership = read(&self.owner, &self.conversation)?;
        let binding = Self::allocate(
            &mut ownership,
            &temp_root,
            &mut random_name,
            self.workspace.as_deref(),
        )?;
        let dir = binding.open(&self.owner, &self.conversation)?;
        self.current = Some(binding);
        self.bindings = ownership.namespaces;
        Ok(dir)
    }

    pub(super) fn root(&mut self) -> io::Result<PathBuf> {
        self.current_directory()?;
        Ok(self
            .current
            .as_ref()
            .expect("allocated namespace")
            .root
            .clone())
    }

    pub(super) fn roots(&self) -> Vec<PathBuf> {
        self.bindings.iter().map(|b| b.root.clone()).collect()
    }

    pub(super) fn validate(&self, path: &Path) -> io::Result<()> {
        let binding = self
            .bindings
            .iter()
            .find(|b| path.starts_with(&b.root))
            .ok_or_else(invalid)?;
        binding.open(&self.owner, &self.conversation).map(|_| ())
    }

    pub(super) fn create(&mut self, prefix: &str, extension: &str) -> io::Result<(File, PathBuf)> {
        let dir = self.current_directory()?;
        for _ in 0..RETRIES {
            #[cfg(test)]
            let name = self.names.pop_front().map_or_else(random_name, Ok)?;
            #[cfg(not(test))]
            let name = random_name()?;
            let leaf = format!("{prefix}-{name}{extension}");
            let path = self
                .current
                .as_ref()
                .expect("allocated namespace")
                .root
                .join(&leaf);
            if !self.issued.insert(path.clone()) {
                continue;
            }
            match openat(
                &dir,
                leaf.as_str(),
                OFlag::O_WRONLY
                    | OFlag::O_APPEND
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            ) {
                Ok(fd) => return Ok((File::from(fd), path)),
                Err(nix::errno::Errno::EEXIST) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary output file collision budget exhausted",
        ))
    }

    pub(super) fn open_file(&self, path: &Path) -> io::Result<File> {
        let binding = self
            .bindings
            .iter()
            .find(|b| path.parent() == Some(b.root.as_path()))
            .ok_or_else(invalid)?;
        let dir = binding.open(&self.owner, &self.conversation)?;
        Ok(File::from(openat(
            &dir,
            path.file_name().ok_or_else(invalid)?,
            OFlag::O_WRONLY
                | OFlag::O_APPEND
                | OFlag::O_NONBLOCK
                | OFlag::O_NOFOLLOW
                | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?))
    }

    pub(super) fn discard(&self, path: &Path) -> io::Result<()> {
        let binding = self
            .bindings
            .iter()
            .find(|b| path.parent() == Some(b.root.as_path()))
            .ok_or_else(invalid)?;
        let dir = binding.open(&self.owner, &self.conversation)?;
        unlinkat(
            &dir,
            path.file_name().ok_or_else(invalid)?,
            UnlinkatFlags::NoRemoveDir,
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn set_names(&mut self, names: impl IntoIterator<Item = String>) {
        self.names = names.into_iter().collect();
    }
}

/// Validate and fingerprint local ownership, without requiring disposable files.
pub(crate) fn ownership_revision(
    owner: &Path,
    conversation: &ConversationId,
) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&read(owner, conversation)?).map_err(io::Error::other)
}

/// Called only under the Session's committed deletion and allocation exclusion.
pub(crate) fn cleanup(owner: &Path, conversation: &ConversationId) -> io::Result<()> {
    let ownership = match read(owner, conversation) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for binding in ownership.namespaces {
        let dir = match binding.open(&ownership.owner, &ownership.conversation) {
            Ok(dir) => dir,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            // Even an empty directory needs its ownership marker: inode
            // identities may be reused after external removal or restart.
            Err(e) => return Err(e),
        };
        // Managed output is flat. Unexpected subdirectories fail closed rather
        // than granting arbitrary recursive deletion authority.
        let mut entries = nix::dir::Dir::from_fd(dir.try_clone()?.into())?;
        let names = entries
            .iter()
            .map(|e| e.map(|e| e.file_name().to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        for name in names {
            if [b".".as_slice(), b"..", MARKER.as_bytes()].contains(&name.as_bytes()) {
                continue;
            }
            match unlinkat(
                &dir,
                OsStr::from_bytes(name.as_bytes()),
                UnlinkatFlags::NoRemoveDir,
            ) {
                Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
                Err(e) => return Err(e.into()),
            }
        }
        // Leave the marker until all content was removed, so a failed cleanup
        // can revalidate the same namespace on retry.
        match unlinkat(&dir, MARKER, UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
            Err(e) => return Err(e.into()),
        }
        match std::fs::remove_dir(&binding.root) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                let mut marker = File::from(openat(
                    &dir,
                    MARKER,
                    OFlag::O_WRONLY
                        | OFlag::O_CREAT
                        | OFlag::O_EXCL
                        | OFlag::O_NOFOLLOW
                        | OFlag::O_CLOEXEC,
                    Mode::from_bits_truncate(0o600),
                )?);
                serde_json::to_writer(
                    &mut marker,
                    &binding.marker(&ownership.owner, &ownership.conversation),
                )
                .map_err(io::Error::other)?;
                return Err(e);
            }
        }
    }
    Ok(())
}

/// The existing uncommitted child rollback owner retires output before metadata.
/// The Conversation allocation itself remains a burned identity reservation.
pub(crate) fn rollback(owner: &Path, conversation: &ConversationId) -> io::Result<()> {
    cleanup(owner, conversation)?;
    let dir = match directory(owner) {
        Ok(dir) => dir,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for name in [BINDINGS, "output-namespaces.lock"] {
        match unlinkat(&dir, name, UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_collisions_are_bounded_exclusive_and_never_reuse_recorded_names() {
        let root = tempfile::tempdir().unwrap();
        let owner = root.path().join("private");
        std::fs::create_dir(&owner).unwrap();
        let owner = owner.canonicalize().unwrap();
        let temporary = root.path().join("temporary");
        std::fs::create_dir(&temporary).unwrap();
        let temporary = temporary.canonicalize().unwrap();
        let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000001");
        let mut ownership = read(&owner, &conversation).unwrap();
        let name = "0000000000000001";
        let collision = temporary.join(format!("rx-{name}"));
        std::fs::create_dir(&collision).unwrap();
        std::fs::write(collision.join("user"), "keep").unwrap();
        let mut attempts = 0;
        assert_eq!(
            Namespaces::allocate(
                &mut ownership,
                &temporary,
                &mut || {
                    attempts += 1;
                    Ok(name.to_owned())
                },
                None
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(attempts, RETRIES);
        assert_eq!(
            std::fs::read_to_string(collision.join("user")).unwrap(),
            "keep"
        );
        let mut sequence = [name, "0000000000000002"].into_iter();
        let binding = Namespaces::allocate(
            &mut ownership,
            &temporary,
            &mut || Ok(sequence.next().unwrap().to_owned()),
            None,
        )
        .unwrap();
        cleanup(&owner, &conversation).unwrap();
        assert!(!binding.root.exists());
        // Recorded identities stay consumed even after external deletion.
        let mut attempts = 0;
        assert_eq!(
            Namespaces::allocate(
                &mut ownership,
                &temporary,
                &mut || {
                    attempts += 1;
                    Ok("0000000000000002".to_owned())
                },
                None
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(attempts, RETRIES);
    }

    #[test]
    fn foreign_or_traversing_metadata_never_grants_cleanup_authority() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000001");
        let mut first = Namespaces::new(&a, &conversation, root.path()).unwrap();
        let mut second = Namespaces::new(&b, &conversation, root.path()).unwrap();
        let a = a.canonicalize().unwrap();
        first.root().unwrap();
        let user = second.root().unwrap().join("user");
        std::fs::write(&user, "keep").unwrap();
        let mut forged = read(&a, &conversation).unwrap();
        forged.namespaces = second.bindings.clone();
        write(&forged).unwrap();
        assert!(cleanup(&a, &conversation).is_err());
        assert_eq!(std::fs::read_to_string(&user).unwrap(), "keep");
        forged.namespaces[0].root = root.path().join("../rx-0000000000000001");
        write(&forged).unwrap();
        assert!(cleanup(&a, &conversation).is_err());
        assert!(first.root().unwrap().exists());
        assert!(user.exists());
    }
}
