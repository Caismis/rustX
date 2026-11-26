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
    MAX_DIGEST_BYTES, Probe, Reply, Request, valid_package_path, valid_probe_name,
};
use sha2::{Digest, Sha256};

pub(super) struct UserSkills {
    root: Option<PathBuf>,
    transfer: super::skill_transfer::Transfer,
}

impl UserSkills {
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
                .map(|home| home.join("rustx")),
        )
    }

    fn from_root(root: Option<PathBuf>) -> Self {
        Self {
            root,
            transfer: super::skill_transfer::Transfer::default(),
        }
    }

    pub(super) fn dispatch(
        &mut self,
        request: &Request,
        arbitration: &super::mutation::Arbitration,
    ) -> io::Result<Reply> {
        match request {
            Request::Root { .. } | Request::Digest { .. } => {
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
                self.transfer.dispatch(root, request, arbitration)
            }
        }
    }

    #[cfg(test)]
    fn inquire(&self, request: &Request) -> io::Result<Reply> {
        self.inquire_checked(request, None)
    }

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
            Request::Digest { package } => {
                if !valid_package_path(package) {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let collection = root.join(".agents/skills");
                let directory = match PackageDirectory::open(&collection) {
                    Ok(directory) => directory,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
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
                Ok(Reply::Digest { digest })
            }
            _ => Err(io::ErrorKind::InvalidInput.into()),
        }
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
        let file = match directory.file(Path::new(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(
            bounded_digest(file, 64, arbitration)?.map(|(metadata, digest)| Probe {
                device: rustx_execution_protocol::DecimalU64::new(metadata.dev()),
                inode: rustx_execution_protocol::DecimalU64::new(metadata.ino()),
                digest,
            }),
        )
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
}
