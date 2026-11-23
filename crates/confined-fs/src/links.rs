//! Read-only resolution of resource links within an admitted directory.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use super::{FileMetadata, PackageDirectory, conflict, invalid, io};

const MAX_LINKS: usize = 32;
const MAX_PATH_BYTES: usize = 4096;

/// Entry metadata and the no-follow path that reaches the same physical entry.
pub struct ResolvedEntry {
    /// The path that ordinary Workspace tools can open.
    pub relative_path: PathBuf,
    /// Physical metadata verified during resolution.
    pub metadata: FileMetadata,
    /// Whether any component of the original path was a link.
    pub through_link: bool,
}

impl PackageDirectory {
    /// Resolves contained resource links. Each component is physically inspected
    /// relative to a retained descriptor, and the targets of absolute links
    /// must lie under the admitted `root_path`. The resulting path contains
    /// only normal components and can be used with the ordinary no-follow tools.
    ///
    /// # Errors
    /// Returns `NotFound` for broken, external or cyclic links, `InvalidInput` for
    /// an oversized or invalid input path, and a conflict if observed links or
    /// the resulting entry change during lookup.
    pub fn resolve_resource(&self, root_path: &Path, relative: &Path) -> io::Result<ResolvedEntry> {
        if relative.as_os_str().len() > MAX_PATH_BYTES
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(invalid("invalid relative resource path"));
        }
        let mut remaining: VecDeque<OsString> = relative
            .components()
            .map(|part| part.as_os_str().to_owned())
            .collect();
        let mut path = PathBuf::new();
        let mut parent = self.child(OsStr::new("."))?;
        let mut links = Vec::new();
        let mut leaf = None;
        while let Some(name) = remaining.pop_front() {
            if name == "." {
                continue;
            }
            if name == ".." {
                if !path.pop() {
                    return Err(unavailable_link());
                }
                parent = self.descend(&path)?;
                continue;
            }
            let metadata = parent.entry_metadata(Path::new(&name))?;
            if metadata.is_symlink {
                if links.len() == MAX_LINKS {
                    return Err(unavailable_link());
                }
                let mut buffer = [0; MAX_PATH_BYTES];
                let length =
                    rustix::fs::readlinkat_raw(parent.descriptor(), &name, &mut buffer[..])?;
                if length == MAX_PATH_BYTES {
                    return Err(invalid("resource link target is too long"));
                }
                let target = Path::new(OsStr::from_bytes(&buffer[..length]));
                let target = if target.is_absolute() {
                    let target = target
                        .strip_prefix(root_path)
                        .map_err(|_| unavailable_link())?;
                    path.clear();
                    let directory = self.child(OsStr::new("."))?;
                    links.push((std::mem::replace(&mut parent, directory), name, metadata));
                    target
                } else {
                    links.push((self.descend(&path)?, name, metadata));
                    target
                };
                for part in target.components().rev() {
                    remaining.push_front(part.as_os_str().to_owned());
                }
                continue;
            }
            path.push(&name);
            if path.as_os_str().len() > MAX_PATH_BYTES {
                return Err(invalid("resolved resource path is too long"));
            }
            if remaining.is_empty() {
                leaf = Some(metadata);
            } else {
                parent = parent.child(&name)?;
            }
        }
        let metadata = self.entry_metadata(&path)?;
        if leaf.is_some_and(|before| before != metadata) {
            return Err(conflict());
        }
        for (directory, name, before) in &links {
            if *before != directory.entry_metadata(Path::new(name))? {
                return Err(conflict());
            }
        }
        Ok(ResolvedEntry {
            relative_path: path,
            metadata,
            through_link: !links.is_empty(),
        })
    }
}

fn unavailable_link() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "resource link has no target in the admitted Workspace",
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn a_hidden_non_utf8_name_counts_at_the_page_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        std::fs::write(workspace.join(OsStr::from_bytes(b".\xff")), []).unwrap();
        std::fs::create_dir(workspace.join("node_modules")).unwrap();
        std::fs::write(workspace.join("z.md"), b"visible").unwrap();
        let root = PackageDirectory::open(&workspace).unwrap();
        let (page, skipped) = root
            .skill_directory_page(Path::new(""), 0, 2, None, 3, 2)
            .unwrap();
        assert_eq!(skipped, 2);
        assert_eq!(page.entries.len(), 0);
        assert_eq!(page.next_offset, Some(2));
        let (last, skipped) = root
            .skill_directory_page(Path::new(""), 2, 2, Some(&page.revision), 3, 2)
            .unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(last.entries[0].file_name, "z.md");
        assert_eq!(last.next_offset, None);
        assert!(
            root.skill_directory_page(Path::new(""), 0, 1, None, 2, 2)
                .is_err()
        );
        assert!(
            root.directory_page(Path::new(""), 0, 2, None, 3, 2)
                .is_err()
        );
    }

    #[test]
    fn resolves_only_contained_links_and_preserves_no_follow_operations() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let workspace = base.join("workspace");
        std::fs::create_dir_all(workspace.join("packages/real")).unwrap();
        std::fs::create_dir(workspace.join("aliases")).unwrap();
        std::fs::write(workspace.join("packages/real/SKILL.md"), "marker").unwrap();
        std::fs::write(base.join("secret"), "external").unwrap();
        symlink("../packages", workspace.join("aliases/collection")).unwrap();
        symlink(
            workspace.join("packages/real/SKILL.md"),
            workspace.join("marker"),
        )
        .unwrap();
        symlink("../secret", workspace.join("relative-escape")).unwrap();
        symlink(base.join("secret"), workspace.join("absolute-escape")).unwrap();
        symlink("cycle", workspace.join("cycle")).unwrap();

        let root = PackageDirectory::open(&workspace).unwrap();
        for alias in ["aliases/collection/real/SKILL.md", "marker"] {
            let resolved = root.resolve_resource(&workspace, Path::new(alias)).unwrap();
            assert!(resolved.through_link);
            assert_eq!(resolved.relative_path, Path::new("packages/real/SKILL.md"));
            assert_eq!(
                resolved.metadata,
                root.entry_metadata(&resolved.relative_path).unwrap()
            );
            assert!(root.file(Path::new(alias)).is_err());
        }
        for alias in ["relative-escape", "absolute-escape", "cycle"] {
            assert_eq!(
                root.resolve_resource(&workspace, Path::new(alias))
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::NotFound
            );
        }
    }
}
