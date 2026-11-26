//! Inquiries scoped to the Client's official global User Skill collection.
//!
//! A package is named relative to `<client-home>/rustx/.agents/skills`.
//! The digest operation always appends `SKILL.md` itself; it cannot name an
//! arbitrary file. Instruction files exceeding the package cap return no
//! digest; transfer then enforces the same package cap.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::save::Sha256Hex;

/// Maximum package bytes transferred in one synchronization.
pub const MAX_SKILL_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum instruction bytes inspected for the deduplication inquiry.
pub const MAX_DIGEST_BYTES: u64 = MAX_SKILL_BYTES;
/// Maximum package bytes transferred in one bind.
pub const MAX_SYNC_BYTES: u64 = 1024 * 1024 * 1024;
/// Maximum UTF-8 byte length of a collection-relative package path.
pub const MAX_PACKAGE_PATH_BYTES: usize = 4096;
/// Maximum number of components in a collection-relative package path.
pub const MAX_PACKAGE_DEPTH: usize = 128;
/// Maximum files and directories in a transferred package.
pub const MAX_PACKAGE_ENTRIES: usize = 16_384;
/// Maximum package identities checked in one shared-root inquiry.
pub const MAX_IDENTITY_PACKAGES: usize = 256;
/// Maximum aggregate UTF-8 path bytes and components in one identity inquiry.
pub const MAX_IDENTITY_PATH_BYTES: usize = 64 * 1024;
/// Maximum aggregate normal path components in one identity inquiry.
pub const MAX_IDENTITY_COMPONENTS: usize = 4096;
/// Reserved hidden directory used only for fresh collection identity probes.
pub const COLLECTION_PROOF_DIRECTORY: &str = ".rustx-sync-proof";
/// Ownership marker within the reserved proof directory.
pub const COLLECTION_PROOF_MARKER: &str = ".rustx-owned";
/// Exact marker contents, written only when the proof directory is created.
pub const COLLECTION_PROOF_MARKER_CONTENT: &[u8] = b"rustx skill collection proof v1\n";

/// A SHA-256-shaped lower-case cache identifier, never a path.
#[must_use]
pub fn valid_cache_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether a path is a bounded, strictly relative package name.
#[must_use]
pub fn valid_package_path(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    if text.is_empty()
        || text.len() > MAX_PACKAGE_PATH_BYTES
        || text.bytes().any(|byte| byte.is_ascii_control())
        || text
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return false;
    }
    let mut count = 0;
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return false;
        }
        count += 1;
    }
    (1..=MAX_PACKAGE_DEPTH).contains(&count)
}

/// A name used only to prove that both sides can see a fresh Host-created
/// file at their respective `~/rustx/` roots.
#[must_use]
pub fn valid_probe_name(name: &str) -> bool {
    name.strip_prefix(".rustx-skill-probe-")
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
}

/// Validate a bounded list of collection-relative package directories. An
/// empty path names the collection itself, never an arbitrary filesystem path.
#[must_use]
pub fn valid_identity_packages(packages: &[PathBuf]) -> bool {
    if packages.len() > MAX_IDENTITY_PACKAGES {
        return false;
    }
    let mut bytes = 0usize;
    let mut components = 0usize;
    for package in packages {
        let Some(text) = package.to_str() else {
            return false;
        };
        if !text.is_empty() && !valid_package_path(package) {
            return false;
        }
        bytes = bytes.saturating_add(text.len());
        components = components.saturating_add(package.components().count());
        if bytes > MAX_IDENTITY_PATH_BYTES || components > MAX_IDENTITY_COMPONENTS {
            return false;
        }
    }
    true
}

/// An inquiry addressed to the Client's fixed User root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// Return the Client's local `~/rustx` and, if requested, inspect one
    /// freshly generated root-identity probe. Only the reserved probe name
    /// form is accepted; this is not an arbitrary file reader.
    Root { probe: Option<String> },
    /// Return the locally resolved official collection and inspect only a
    /// reserved, fresh probe directly beneath it when requested.
    Collection { probe: Option<String> },
    /// Inspect package directory identities and their fixed `SKILL.md` files.
    /// No arbitrary leaf path or file contents can be requested.
    Packages { packages: Vec<PathBuf> },
    /// Hash only the named package's `SKILL.md`, with a fixed byte budget.
    Digest { package: PathBuf },
    /// Enter one authority's owned cache and acquire its nonblocking sync lock.
    BeginSync { authority: String },
    /// Start a fresh private package staging directory, named by an opaque key.
    BeginPackage { version: String },
    /// Pin an existing immutable version for the lifetime of this Client.
    Retain { version: String },
    /// Add one package-relative directory to the private stage.
    Directory { path: PathBuf },
    /// Open one new package-relative file in the private stage.
    BeginFile {
        path: PathBuf,
        length: crate::DecimalU64,
        executable: bool,
    },
    /// Append one bounded chunk to the currently open file at the exact offset.
    Chunk {
        offset: crate::DecimalU64,
        data_base64: String,
    },
    /// Close the current file after exactly its declared bytes were received.
    EndFile,
    /// Publish the complete stage under its immutable version key.
    Publish,
    /// Discard a private stage, without deleting a published package.
    AbortPackage,
    /// Release the authority lock. No published package is removed.
    EndSync,
}

/// Evidence from opening the fresh identity probe as a regular file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    /// Device number of the opened file.
    pub device: crate::DecimalU64,
    /// Inode number of the opened file.
    pub inode: crate::DecimalU64,
    /// Hash of its bounded content.
    pub digest: Sha256Hex,
}

/// Device/inode identity of a locally opened filesystem object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryIdentity {
    pub device: crate::DecimalU64,
    pub inode: crate::DecimalU64,
}

/// A package directory and its fixed regular `SKILL.md` marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub directory: EntryIdentity,
    pub marker: EntryIdentity,
}

/// The result of an inquiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Reply {
    /// The Client-local `~/rustx` and optional proof for the requested probe.
    Root { path: PathBuf, probe: Option<Probe> },
    /// The Client-local collection and optional fresh-probe evidence.
    Collection {
        path: PathBuf,
        identity: Option<EntryIdentity>,
        probe: Option<Probe>,
    },
    /// One result per requested package, in order; missing or unsafe entries
    /// have no identity and cannot establish a shared-root skip.
    Packages {
        packages: Vec<Option<PackageIdentity>>,
    },
    /// The digest, or `None` when the instruction file is missing or too big.
    Digest { digest: Option<Sha256Hex> },
    /// The authority lock is held by this binding.
    SyncStarted,
    /// Another binding is synchronizing the same authority; no work started.
    Busy,
    /// The staged operation completed without publishing a package.
    Accepted,
    /// A cached version is pinned; path names its package directory.
    Retained {
        package: PathBuf,
        bytes: crate::DecimalU64,
    },
    /// The immutable package was published; path is relative to the Client's
    /// official User Skill collection and names the package directory.
    Published { package: PathBuf },
    /// An immutable version already exists; no published bytes were changed.
    VersionExists,
    /// The authority lock and private stage were released.
    SyncEnded,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_inquiry_cannot_escape_its_collection() {
        for bad in [
            "",
            ".",
            "..",
            "../other",
            "a/../../etc",
            "/etc",
            "a/./b",
            "a\nb",
        ] {
            assert!(!valid_package_path(Path::new(bad)), "{bad:?}");
        }
        assert!(valid_package_path(Path::new("nested/package")));
        assert!(valid_probe_name(
            ".rustx-skill-probe-0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_probe_name("../../etc/passwd"));
        assert!(valid_identity_packages(&[
            PathBuf::new(),
            PathBuf::from("nested/package")
        ]));
        assert!(!valid_identity_packages(&[PathBuf::from("../outside")]));
        assert!(!valid_identity_packages(&vec![
            PathBuf::new();
            MAX_IDENTITY_PACKAGES + 1
        ]));
    }
}
