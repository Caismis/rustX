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
}

/// An inspected resource and the contained paths that can change its resolution.
pub struct ResourceResolution {
    /// Present when resolution found a contained target.
    pub entry: Option<ResolvedEntry>,
    /// Contained links inspected on the way to the resolved entry.
    pub link_paths: Vec<PathBuf>,
    /// The first absent or obstructed component under an existing directory.
    pub unresolved_path: Option<PathBuf>,
}

impl PackageDirectory {
    /// Resolves contained resource links. Each component is physically inspected
    /// relative to a retained descriptor, and the targets of absolute links
    /// must lie under the admitted `root_path`. The resulting path contains
    /// only normal components and can be used with the ordinary no-follow tools.
    ///
    /// # Errors
    /// Returns `InvalidInput` for an oversized or invalid input path and a
    /// conflict if observed links or the resulting entry change during lookup.
    /// Broken, external or cyclic links have no entry and retain their
    /// contained watch paths.
    pub fn resolve_resource(
        &self,
        root_path: &Path,
        relative: &Path,
    ) -> io::Result<ResourceResolution> {
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
        let mut unresolved_path = None;
        let entry = loop {
            let Some(name) = remaining.pop_front() else {
                let metadata = self.entry_metadata(&path).map_err(conflict_if_absent)?;
                if leaf.is_some_and(|before| before != metadata) {
                    return Err(conflict());
                }
                break Some(ResolvedEntry {
                    relative_path: path,
                    metadata,
                });
            };
            if name == "." {
                continue;
            }
            if name == ".." {
                if !path.pop() {
                    break None;
                }
                parent = self.descend(&path).map_err(conflict_if_absent)?;
                continue;
            }
            let component = path.join(&name);
            if component.as_os_str().len() > MAX_PATH_BYTES {
                return Err(invalid("resolved resource path is too long"));
            }
            let metadata = match parent.entry_metadata(Path::new(&name)) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    unresolved_path = Some(component);
                    break None;
                }
                Err(error) => return Err(error),
            };
            if metadata.is_symlink {
                if links.len() == MAX_LINKS {
                    break None;
                }
                let mut buffer = [0; MAX_PATH_BYTES];
                let target = read_target(&parent, &name, &mut buffer)?;
                let retained = parent.child(OsStr::new("."))?;
                links.push((retained, name, metadata, component));
                let target = if target.is_absolute() {
                    let Ok(target) = target.strip_prefix(root_path) else {
                        break None;
                    };
                    path.clear();
                    parent = self.child(OsStr::new("."))?;
                    target
                } else {
                    target
                };
                for part in target.components().rev() {
                    remaining.push_front(part.as_os_str().to_owned());
                }
                continue;
            }
            if !remaining.is_empty() && !metadata.is_directory {
                unresolved_path = Some(component);
                break None;
            }
            path = component;
            if remaining.is_empty() {
                leaf = Some(metadata);
            } else {
                parent = parent.child(&name).map_err(conflict_if_absent)?;
            }
        };
        let mut link_paths = Vec::with_capacity(links.len());
        for (directory, name, before, link_path) in links {
            if before
                != directory
                    .entry_metadata(Path::new(&name))
                    .map_err(conflict_if_absent)?
            {
                return Err(conflict());
            }
            link_paths.push(link_path);
        }
        Ok(ResourceResolution {
            entry,
            link_paths,
            unresolved_path,
        })
    }
}

fn read_target<'a>(
    directory: &PackageDirectory,
    name: &OsStr,
    buffer: &'a mut [u8; MAX_PATH_BYTES],
) -> io::Result<&'a Path> {
    let length = rustix::fs::readlinkat_raw(directory.descriptor(), name, &mut buffer[..])
        .map_err(|error| conflict_if_absent(error.into()))?;
    if length == buffer.len() {
        return Err(invalid("resource link target is too long"));
    }
    Ok(Path::new(OsStr::from_bytes(&buffer[..length])))
}

fn conflict_if_absent(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::NotFound {
        conflict()
    } else {
        error
    }
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
        symlink("aliases/collection/real/SKILL.md", workspace.join("outer")).unwrap();
        for (alias, expected) in [
            (
                "aliases/collection/real/SKILL.md",
                vec!["aliases/collection"],
            ),
            ("marker", vec!["marker"]),
            ("outer", vec!["outer", "aliases/collection"]),
        ] {
            let resolution = root.resolve_resource(&workspace, Path::new(alias)).unwrap();
            assert_eq!(
                resolution.link_paths,
                expected.into_iter().map(PathBuf::from).collect::<Vec<_>>()
            );
            let resolved = resolution.entry.unwrap();
            assert!(resolution.unresolved_path.is_none());
            assert_eq!(resolved.relative_path, Path::new("packages/real/SKILL.md"));
            assert_eq!(
                resolved.metadata,
                root.entry_metadata(&resolved.relative_path).unwrap()
            );
            assert!(root.file(Path::new(alias)).is_err());
        }
        for alias in ["relative-escape", "absolute-escape", "cycle"] {
            let resolution = root.resolve_resource(&workspace, Path::new(alias)).unwrap();
            assert!(resolution.entry.is_none());
            assert!(resolution.unresolved_path.is_none());
            assert_eq!(resolution.link_paths.first().unwrap(), Path::new(alias));
        }
        symlink("../packages/pending", workspace.join("aliases/pending")).unwrap();
        let resolution = root
            .resolve_resource(&workspace, Path::new("aliases/pending/SKILL.md"))
            .unwrap();
        assert!(resolution.entry.is_none());
        assert_eq!(resolution.link_paths, [PathBuf::from("aliases/pending")]);
        assert_eq!(
            resolution.unresolved_path.as_deref(),
            Some(Path::new("packages/pending"))
        );
    }
}
