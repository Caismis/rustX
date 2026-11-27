//! Fixed-root inquiries for global User Skills.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustx_confined_fs::PackageDirectory;
#[cfg(test)]
use rustx_execution_protocol::is_normalized_absolute_path;
use rustx_execution_protocol::save::Sha256Hex;
use rustx_execution_protocol::skills::{
    COLLECTION_PROOF_DIRECTORY, COLLECTION_PROOF_MARKER, COLLECTION_PROOF_MARKER_CONTENT,
    EntryIdentity, MAX_DIGEST_BYTES, PackageIdentity, Probe, Reply, Request,
    valid_identity_packages, valid_package_path, valid_probe_name, valid_standalone_names,
};
use sha2::{Digest, Sha256};

pub(super) struct UserSkills {
    root: Option<PathBuf>,
    selected_collection: Option<PathBuf>,
    transfer: super::skill_transfer::Transfer,
}

impl UserSkills {
    pub(super) fn lane_lost(&mut self) -> io::Result<()> {
        self.transfer.lane_lost()
    }

    pub(super) fn reply(
        &mut self,
        request: &Request,
        arbitration: &super::mutation::Arbitration,
    ) -> rustx_execution_protocol::ClientReply {
        match self.dispatch(request, arbitration) {
            Ok(reply) => rustx_execution_protocol::ClientReply::UserSkills(reply),
            Err(error) => rustx_execution_protocol::ClientReply::Error {
                kind: if error.kind() == io::ErrorKind::FileTooLarge {
                    rustx_execution_protocol::ReadError::SkillTooLarge
                } else {
                    super::read_error(&error)
                },
            },
        }
    }
    pub(super) fn from_environment() -> Self {
        Self::from_root(crate::client_user_root())
    }

    #[cfg(test)]
    pub(super) fn from_home(home: Option<PathBuf>) -> Self {
        Self::from_root(
            home.filter(|home| is_normalized_absolute_path(home))
                .and_then(|home| crate::resolve_user_root(&home).ok()),
        )
    }

    fn from_root(root: Option<PathBuf>) -> Self {
        Self {
            root,
            selected_collection: None,
            transfer: super::skill_transfer::Transfer::default(),
        }
    }

    pub(super) fn dispatch(
        &mut self,
        request: &Request,
        arbitration: &super::mutation::Arbitration,
    ) -> io::Result<Reply> {
        match request {
            Request::Root { .. } | Request::RootIdentity => {
                self.inquire_checked(request, Some(arbitration))
            }
            Request::Collection { .. } => {
                let reply = self.inquire_checked(request, Some(arbitration))?;
                let Reply::Collection { path, .. } = &reply else {
                    return Err(io::ErrorKind::InvalidData.into());
                };
                if self
                    .selected_collection
                    .as_ref()
                    .is_some_and(|selected| selected != path)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Client Skill collection changed",
                    ));
                }
                self.selected_collection = Some(path.clone());
                Ok(reply)
            }
            Request::Packages { .. } | Request::Standalone { .. } | Request::Digest { .. } => {
                self.check_selected_collection()?;
                self.inquire_checked(request, Some(arbitration))
            }
            _ => {
                arbitration.check_grant()?;
                let root = self.root.as_ref().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Client home is unavailable",
                    )
                })?;
                let cleanup = matches!(request, Request::AbortPackage | Request::EndSync);
                let collection = match (&self.selected_collection, cleanup) {
                    (Some(selected), true) => selected.clone(),
                    _ => Self::collection_path(root)?,
                };
                if !cleanup
                    && self
                        .selected_collection
                        .as_ref()
                        .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Client Skill collection changed",
                    ));
                }
                let selected = self.selected_collection.get_or_insert(collection);
                self.transfer.dispatch(root, selected, request, arbitration)
            }
        }
    }

    fn check_selected_collection(&self) -> io::Result<()> {
        if let (Some(root), Some(selected)) = (&self.root, &self.selected_collection)
            && &Self::collection_path(root)? != selected
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Client Skill collection changed",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn inquire(&self, request: &Request) -> io::Result<Reply> {
        self.inquire_checked(request, None)
    }

    #[allow(clippy::too_many_lines, reason = "one fixed-root inquiry dispatcher")]
    fn inquire_checked(
        &self,
        request: &Request,
        arbitration: Option<&super::mutation::Arbitration>,
    ) -> io::Result<Reply> {
        let root = self.root.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Client home is unavailable",
            )
        })?;
        match request {
            Request::Root { probe } => {
                let probe = match probe {
                    Some(name) if valid_probe_name(name) => Self::probe(root, name, arbitration)?,
                    Some(_) => return Err(io::ErrorKind::InvalidInput.into()),
                    None => None,
                };
                Ok(Reply::Root {
                    path: root.clone(),
                    probe,
                })
            }
            Request::RootIdentity => {
                let identity = match PackageDirectory::open(root) {
                    Ok(directory) => Some(Self::directory_identity(&directory)?),
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound
                                | io::ErrorKind::NotADirectory
                                | io::ErrorKind::PermissionDenied
                        ) =>
                    {
                        None
                    }
                    Err(error) => return Err(error),
                };
                Ok(Reply::RootIdentity {
                    path: root.clone(),
                    identity,
                })
            }
            Request::Collection { probe } => {
                let collection = Self::collection_path(root)?;
                let directory = match PackageDirectory::open(&collection) {
                    Ok(directory) => Some(directory),
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        None
                    }
                    Err(error) => return Err(error),
                };
                let identity = directory
                    .as_ref()
                    .map(Self::directory_identity)
                    .transpose()?;
                let probe = match probe {
                    Some(name) if valid_probe_name(name) => {
                        if let Some(directory) = &directory {
                            Self::collection_probe(directory, name, arbitration)?
                        } else {
                            None
                        }
                    }
                    Some(_) => return Err(io::ErrorKind::InvalidInput.into()),
                    None => None,
                };
                if let Some(directory) = &directory {
                    directory.verify()?;
                }
                if Self::collection_path(root)? != collection {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                Ok(Reply::Collection {
                    path: collection,
                    identity,
                    probe,
                })
            }
            Request::Packages { packages } => {
                if !valid_identity_packages(packages) {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let collection = Self::collection_path(root)?;
                if self
                    .selected_collection
                    .as_ref()
                    .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let directory = match PackageDirectory::open(&collection) {
                    Ok(directory) => directory,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound
                                | io::ErrorKind::PermissionDenied
                                | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        return Ok(Reply::Packages {
                            packages: vec![None; packages.len()],
                        });
                    }
                    Err(error) => return Err(error),
                };
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut identities = Vec::with_capacity(packages.len());
                for package in packages {
                    if let Some(arbitration) = arbitration {
                        arbitration.check_grant()?;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    identities.push(Self::package_identity(&directory, package)?);
                }
                directory.verify()?;
                if Self::collection_path(root)? != collection
                    || self
                        .selected_collection
                        .as_ref()
                        .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                Ok(Reply::Packages {
                    packages: identities,
                })
            }
            Request::Standalone { names } => {
                if !valid_standalone_names(names) {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let collection = Self::collection_path(root)?;
                if self
                    .selected_collection
                    .as_ref()
                    .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let directory = match PackageDirectory::open(&collection) {
                    Ok(directory) => directory,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound
                                | io::ErrorKind::NotADirectory
                                | io::ErrorKind::PermissionDenied
                        ) =>
                    {
                        return Ok(Reply::Standalone {
                            identities: vec![None; names.len()],
                        });
                    }
                    Err(error) => return Err(error),
                };
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut identities = Vec::with_capacity(names.len());
                for name in names {
                    if let Some(arbitration) = arbitration {
                        arbitration.check_grant()?;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    identities.push(Self::standalone_identity(&directory, name)?);
                }
                directory.verify()?;
                if Self::collection_path(root)? != collection
                    || self
                        .selected_collection
                        .as_ref()
                        .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                Ok(Reply::Standalone { identities })
            }
            Request::Digest { package } => {
                if !valid_package_path(package) {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let collection = Self::collection_path(root)?;
                if self
                    .selected_collection
                    .as_ref()
                    .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let directory = match PackageDirectory::open(&collection) {
                    Ok(directory) => directory,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        return Ok(Reply::Digest { digest: None });
                    }
                    Err(error) => return Err(error),
                };
                let path = package.join("SKILL.md");
                let digest = match directory.file(&path).and_then(|file| {
                    let opened = rustx_confined_fs::FileMetadata::of(&file)?;
                    let hash = bounded_digest(file, MAX_DIGEST_BYTES, arbitration)?;
                    let named = directory.entry_metadata(&path)?;
                    Ok(hash.filter(|_| named == opened).map(|(_, digest)| digest))
                }) {
                    Ok(digest) => digest,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
                directory.verify()?;
                if Self::collection_path(root)? != collection
                    || self
                        .selected_collection
                        .as_ref()
                        .is_some_and(|selected| selected != &collection)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                Ok(Reply::Digest { digest })
            }
            _ => Err(io::ErrorKind::InvalidInput.into()),
        }
    }

    /// Resolve only the Client-selected collection and its existing local
    /// ancestors. Missing suffixes are returned without creating anything.
    pub(super) fn collection_path(root: &Path) -> io::Result<PathBuf> {
        let requested = root.join(".agents/skills");
        let mut ancestor = requested.as_path();
        let mut missing = Vec::new();
        loop {
            match ancestor.canonicalize() {
                Ok(mut physical) => {
                    if std::fs::metadata(&physical)?.is_dir() {
                        PackageDirectory::open(&physical)?.verify()?;
                    }
                    for component in missing.iter().rev() {
                        physical.push(component);
                    }
                    return Ok(physical);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    match std::fs::symlink_metadata(ancestor) {
                        Ok(_) => return Err(io::ErrorKind::NotFound.into()),
                        Err(missing_error)
                            if matches!(
                                missing_error.kind(),
                                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                            ) => {}
                        Err(other) => return Err(other),
                    }
                    missing.push(
                        ancestor
                            .file_name()
                            .ok_or(io::ErrorKind::InvalidInput)?
                            .to_owned(),
                    );
                    ancestor = ancestor.parent().ok_or(io::ErrorKind::InvalidInput)?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn package_identity(
        root: &PackageDirectory,
        relative: &Path,
    ) -> io::Result<Option<PackageIdentity>> {
        let package = match root.descend(relative) {
            Ok(package) => package,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let named = match package.entry_metadata(Path::new("SKILL.md")) {
            Ok(metadata) if metadata.is_file => metadata,
            Ok(_) => return Ok(None),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let marker = match package.file(Path::new("SKILL.md")) {
            Ok(marker) => marker,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let opened = rustx_confined_fs::FileMetadata::of(&marker)?;
        if named != opened || package.entry_metadata(Path::new("SKILL.md"))? != opened {
            return Ok(None);
        }
        let directory = Self::directory_identity(&package)?;
        let marker = marker.metadata()?;
        package.verify()?;
        Ok(Some(PackageIdentity {
            directory,
            marker: EntryIdentity {
                device: rustx_execution_protocol::DecimalU64::new(marker.dev()),
                inode: rustx_execution_protocol::DecimalU64::new(marker.ino()),
            },
        }))
    }

    fn standalone_identity(
        root: &PackageDirectory,
        name: &str,
    ) -> io::Result<Option<EntryIdentity>> {
        let path = Path::new(name);
        let named = match root.entry_metadata(path) {
            Ok(metadata) if metadata.is_file => metadata,
            Ok(_) => return Ok(None),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let file = match root.file(path) {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if rustx_confined_fs::FileMetadata::of(&file)? != named
            || root.entry_metadata(path)? != named
        {
            return Ok(None);
        }
        let metadata = file.metadata()?;
        root.verify()?;
        Ok(Some(EntryIdentity {
            device: rustx_execution_protocol::DecimalU64::new(metadata.dev()),
            inode: rustx_execution_protocol::DecimalU64::new(metadata.ino()),
        }))
    }

    fn directory_identity(directory: &PackageDirectory) -> io::Result<EntryIdentity> {
        directory.verify()?;
        let metadata = File::from(directory.descriptor().try_clone()?).metadata()?;
        Ok(EntryIdentity {
            device: rustx_execution_protocol::DecimalU64::new(metadata.dev()),
            inode: rustx_execution_protocol::DecimalU64::new(metadata.ino()),
        })
    }

    fn probe(
        root: &Path,
        name: &str,
        arbitration: Option<&super::mutation::Arbitration>,
    ) -> io::Result<Option<Probe>> {
        let directory = match PackageDirectory::open(root) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Self::probe_opened(&directory, name, arbitration)
    }

    fn collection_probe(
        collection: &PackageDirectory,
        name: &str,
        arbitration: Option<&super::mutation::Arbitration>,
    ) -> io::Result<Option<Probe>> {
        let directory = match collection.child(std::ffi::OsStr::new(COLLECTION_PROOF_DIRECTORY)) {
            Ok(directory) => directory,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut marker = match directory.file(Path::new(COLLECTION_PROOF_MARKER)) {
            Ok(file) => file,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::NotADirectory
                        | io::ErrorKind::PermissionDenied
                ) || error.raw_os_error() == Some(libc::ELOOP) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let opened = rustx_confined_fs::FileMetadata::of(&marker)?;
        let mut content = Vec::new();
        Read::by_ref(&mut marker)
            .take(128)
            .read_to_end(&mut content)?;
        if content != COLLECTION_PROOF_MARKER_CONTENT
            || rustx_confined_fs::FileMetadata::of(&marker)? != opened
            || directory.entry_metadata(Path::new(COLLECTION_PROOF_MARKER))? != opened
        {
            return Ok(None);
        }
        Self::probe_opened(&directory, name, arbitration)
    }

    fn probe_opened(
        directory: &PackageDirectory,
        name: &str,
        arbitration: Option<&super::mutation::Arbitration>,
    ) -> io::Result<Option<Probe>> {
        let file = match directory.file(Path::new(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let opened = rustx_confined_fs::FileMetadata::of(&file)?;
        let digest = bounded_digest(file, 64, arbitration)?;
        if directory.entry_metadata(Path::new(name))? != opened {
            return Ok(None);
        }
        Ok(digest.map(|(metadata, digest)| Probe {
            device: rustx_execution_protocol::DecimalU64::new(metadata.dev()),
            inode: rustx_execution_protocol::DecimalU64::new(metadata.ino()),
            digest,
        }))
    }
}

/// Hashes a single open regular file and refuses a changing observation.
/// `None` means over budget, never a digest of a prefix.
fn bounded_digest(
    mut file: File,
    limit: u64,
    arbitration: Option<&super::mutation::Arbitration>,
) -> io::Result<Option<(Metadata, Sha256Hex)>> {
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if before.len() > limit {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    loop {
        if let Some(arbitration) = arbitration {
            arbitration.check_grant()?;
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count += u64::try_from(read).expect("bounded read");
        if count > limit {
            return Ok(None);
        }
        hash.update(&buffer[..read]);
    }
    let after = file.metadata()?;
    let stamp = |m: &Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if stamp(&before) != stamp(&after) || count != after.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "skill instruction changed during inquiry",
        ));
    }
    Ok(Some((
        after,
        Sha256Hex::from_digest(hash.finalize().into()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn digest_only_reads_the_fixed_skill_marker() {
        let home = tempfile::tempdir().unwrap();
        let package = home.path().join("rustx/.agents/skills/nested/example");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("SKILL.md"), b"instructions").unwrap();
        fs::write(package.join("attachment"), b"first").unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let request = Request::Digest {
            package: "nested/example".into(),
        };
        let first = skills.inquire(&request).unwrap();
        fs::write(package.join("attachment"), b"changed").unwrap();
        assert_eq!(first, skills.inquire(&request).unwrap());
        fs::write(package.join("SKILL.md"), b"changed").unwrap();
        assert_ne!(first, skills.inquire(&request).unwrap());
        fs::remove_file(package.join("SKILL.md")).unwrap();
        assert_eq!(
            skills.inquire(&request).unwrap(),
            Reply::Digest { digest: None }
        );
        assert!(
            skills
                .inquire(&Request::Digest {
                    package: "../../outside".into()
                })
                .is_err()
        );
    }

    #[test]
    fn shared_root_proof_uses_a_fresh_reserved_file_and_identity() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        fs::create_dir(&root).unwrap();
        let name = ".rustx-skill-probe-0123456789abcdef0123456789abcdef";
        fs::write(root.join(name), b"fresh challenge").unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let Reply::Root {
            path,
            probe: Some(probe),
        } = skills
            .inquire(&Request::Root {
                probe: Some(name.into()),
            })
            .unwrap()
        else {
            panic!("probe is visible")
        };
        let metadata = fs::metadata(root.join(name)).unwrap();
        assert_eq!(path, root);
        assert_eq!(
            (probe.device.get(), probe.inode.get()),
            (metadata.dev(), metadata.ino())
        );
        assert!(
            skills
                .inquire(&Request::Root {
                    probe: Some("SKILL.md".into())
                })
                .is_err()
        );
    }

    #[test]
    fn oversized_instruction_is_a_cache_miss() {
        let home = tempfile::tempdir().unwrap();
        let package = home.path().join("rustx/.agents/skills/large");
        fs::create_dir_all(&package).unwrap();
        let file = File::create(package.join("SKILL.md")).unwrap();
        file.set_len(MAX_DIGEST_BYTES + 1).unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        assert_eq!(
            skills
                .inquire(&Request::Digest {
                    package: "large".into()
                })
                .unwrap(),
            Reply::Digest { digest: None }
        );
    }

    #[test]
    fn instruction_digest_is_not_limited_to_one_mebibyte() {
        let home = tempfile::tempdir().unwrap();
        let package = home.path().join("rustx/.agents/skills/large-marker");
        fs::create_dir_all(&package).unwrap();
        let marker = vec![b'x'; 1024 * 1024 + 1];
        fs::write(package.join("SKILL.md"), &marker).unwrap();
        fs::write(package.join("attachment"), b"first").unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let request = Request::Digest {
            package: "large-marker".into(),
        };
        let expected = Reply::Digest {
            digest: Some(Sha256Hex::of(&marker)),
        };
        assert_eq!(skills.inquire(&request).unwrap(), expected);
        fs::write(package.join("attachment"), b"different").unwrap();
        assert_eq!(skills.inquire(&request).unwrap(), expected);
    }

    #[test]
    fn revoked_client_stops_instruction_hashing() {
        let home = tempfile::tempdir().unwrap();
        let package = home.path().join("rustx/.agents/skills/revoked");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("SKILL.md"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        let mut skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let arbitration = super::super::mutation::Arbitration::new();
        arbitration.revoke();
        assert!(
            skills
                .dispatch(
                    &Request::Digest {
                        package: "revoked".into()
                    },
                    &arbitration
                )
                .is_err()
        );
    }

    #[test]
    fn collection_path_resolves_local_aliases_without_creating_missing_components() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let physical_home = temporary.path().join("physical-home");
        fs::create_dir(&physical_home).unwrap();
        std::os::unix::fs::symlink(&physical_home, &home).unwrap();
        let product = physical_home.join("actual-product");
        fs::create_dir(&product).unwrap();
        std::os::unix::fs::symlink(&product, physical_home.join("rustx")).unwrap();
        let agents = temporary.path().join("actual-agents");
        fs::create_dir(&agents).unwrap();
        std::os::unix::fs::symlink(&agents, product.join(".agents")).unwrap();
        let root = crate::resolve_user_root(&home).unwrap();
        assert_eq!(root, product);
        assert_eq!(
            UserSkills::collection_path(&root).unwrap(),
            agents.join("skills")
        );
        assert!(!agents.join("skills").exists());
        std::os::unix::fs::symlink(temporary.path().join("missing"), agents.join("skills"))
            .unwrap();
        assert!(UserSkills::collection_path(&root).is_err());
    }

    #[test]
    fn non_directory_collection_is_unavailable_without_mutation() {
        for entry in [".agents", ".agents/skills"] {
            let home = tempfile::tempdir().unwrap();
            let product = home.path().join("rustx");
            fs::create_dir_all(product.join(entry).parent().unwrap()).unwrap();
            let file = product.join(entry);
            fs::write(&file, b"personal file").unwrap();
            let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
            assert!(matches!(
                skills
                    .inquire(&Request::Collection { probe: None })
                    .unwrap(),
                Reply::Collection {
                    identity: None,
                    probe: None,
                    ..
                }
            ));
            assert_eq!(fs::read(file).unwrap(), b"personal file");
        }
    }

    #[test]
    fn package_identity_is_fixed_to_skill_marker_and_refuses_linked_packages() {
        let home = tempfile::tempdir().unwrap();
        let collection = home.path().join("rustx/.agents/skills");
        let package = collection.join("guide");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("SKILL.md"), b"instructions").unwrap();
        std::os::unix::fs::symlink(&package, collection.join("alias")).unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let Reply::Packages { packages } = skills
            .inquire(&Request::Packages {
                packages: vec!["guide".into(), "alias".into()],
            })
            .unwrap()
        else {
            panic!("identity reply")
        };
        assert!(packages[0].is_some());
        assert!(packages[1].is_none());
        assert!(
            skills
                .inquire(&Request::Packages {
                    packages: vec!["../outside".into()]
                })
                .is_err()
        );
    }

    #[test]
    fn standalone_inquiry_is_bounded_and_refuses_links() {
        let home = tempfile::tempdir().unwrap();
        let collection = home.path().join("rustx/.agents/skills");
        fs::create_dir_all(&collection).unwrap();
        fs::write(collection.join("guide.md"), b"instructions").unwrap();
        std::os::unix::fs::symlink("guide.md", collection.join("linked.md")).unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let Reply::Standalone { identities } = skills
            .inquire(&Request::Standalone {
                names: vec!["guide.md".into(), "linked.md".into(), "absent.md".into()],
            })
            .unwrap()
        else {
            panic!("standalone identities")
        };
        let metadata = fs::metadata(collection.join("guide.md")).unwrap();
        assert_eq!(
            identities[0]
                .as_ref()
                .map(|identity| (identity.device.get(), identity.inode.get())),
            Some((metadata.dev(), metadata.ino()))
        );
        assert_eq!(identities[1..], [None, None]);
        for name in [
            "../secret.md",
            "subdir/secret.md",
            ".hidden.md",
            "secret.txt",
        ] {
            assert!(
                skills
                    .inquire(&Request::Standalone {
                        names: vec![name.into()]
                    })
                    .is_err()
            );
        }
    }

    #[test]
    fn root_identity_reports_only_the_configured_directory() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        fs::create_dir(&root).unwrap();
        let skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let Reply::RootIdentity {
            path,
            identity: Some(identity),
        } = skills.inquire(&Request::RootIdentity).unwrap()
        else {
            panic!("root identity")
        };
        let metadata = fs::metadata(&root).unwrap();
        assert_eq!(path, root);
        assert_eq!(
            (identity.device.get(), identity.inode.get()),
            (metadata.dev(), metadata.ino())
        );
    }

    #[test]
    fn retargeted_collection_is_rejected_before_transfer() {
        let home = tempfile::tempdir().unwrap();
        let product = home.path().join("rustx");
        let first = home.path().join("first");
        let second = home.path().join("second");
        fs::create_dir(&product).unwrap();
        fs::create_dir_all(first.join("skills")).unwrap();
        fs::create_dir_all(second.join("skills")).unwrap();
        std::os::unix::fs::symlink(&first, product.join(".agents")).unwrap();
        let mut skills = UserSkills::from_home(Some(home.path().to_path_buf()));
        let arbitration = super::super::mutation::Arbitration::new();
        let Reply::Collection { path, .. } = skills
            .dispatch(&Request::Collection { probe: None }, &arbitration)
            .unwrap()
        else {
            panic!("collection")
        };
        assert_eq!(path, first.join("skills"));
        fs::remove_file(product.join(".agents")).unwrap();
        std::os::unix::fs::symlink(&second, product.join(".agents")).unwrap();
        assert!(
            skills
                .dispatch(
                    &Request::BeginSync {
                        authority: "a".repeat(64)
                    },
                    &arbitration
                )
                .is_err()
        );
        assert!(!first.join("skills/.rustx-sync").exists());
        assert!(!second.join("skills/.rustx-sync").exists());
    }
}
