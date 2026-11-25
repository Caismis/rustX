//! Bounded Agent and Workflow definitions read through the admitted Client.
use super::{MAX_DIRECTORY_ENTRIES, MAX_RESOURCE_FILES};
use crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint;
use crate::runtime::resources::RuntimeResourceLoadError;
use rustx_execution_protocol::{EntryKind, EntryStamp, MAX_DIRECTORY_PAGE_ENTRIES, PageEntry};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const COLLECTIONS: [(&str, &str); 2] = [(".agents/agents", "toml"), (".agents/workflows", "yaml")];
const CAPTURE_BUDGET: Duration = Duration::from_secs(5);

pub(in crate::local_runtime) struct BoundDefinitions<'a> {
    endpoint: &'a LocalExecutionEndpoint,
    collections: [(PathBuf, Observation); COLLECTIONS.len()],
    deadline: Instant,
}

#[derive(PartialEq, Eq, serde::Serialize)]
enum Observation {
    Missing,
    Invalid {
        entry: EntryStamp,
        reason: &'static str,
    },
    Directory(Collection),
}

#[derive(PartialEq, Eq, serde::Serialize)]
struct Collection {
    identity: String,
    entries: Vec<PageEntry>,
}

impl<'a> BoundDefinitions<'a> {
    pub(in crate::local_runtime) fn capture(
        endpoint: &'a LocalExecutionEndpoint,
    ) -> Result<Self, RuntimeResourceLoadError> {
        let deadline = Instant::now() + CAPTURE_BUDGET;
        let mut collections =
            COLLECTIONS.map(|(name, _)| (endpoint.workspace().join(name), Observation::Missing));
        for ((root, entries), (_, extension)) in collections.iter_mut().zip(COLLECTIONS) {
            *entries = list(endpoint, root, extension, deadline)?;
        }
        Ok(Self {
            endpoint,
            collections,
            deadline,
        })
    }

    pub(in crate::local_runtime) fn files(
        &self,
        root: &Path,
    ) -> Result<Vec<PathBuf>, RuntimeResourceLoadError> {
        match self
            .collections
            .iter()
            .find(|(collection, _)| collection == root)
        {
            Some((_, Observation::Directory(collection))) => Ok(collection
                .entries
                .iter()
                .map(|entry| root.join(&entry.name))
                .collect()),
            Some((_, Observation::Invalid { reason, .. })) => Err(failure(root, reason)),
            Some((_, Observation::Missing)) => Ok(Vec::new()),
            None => Err(failure(
                root,
                "resource collection is outside the bound catalog",
            )),
        }
    }

    pub(in crate::local_runtime) fn read(
        &self,
        path: &Path,
        byte_limit: u64,
    ) -> Result<Vec<u8>, RuntimeResourceLoadError> {
        let stamp = self.collections.iter().find_map(|(root, entries)| {
            let name = path.strip_prefix(root).ok()?.to_str()?;
            let Observation::Directory(collection) = entries else {
                return None;
            };
            collection.entries.iter().find(|entry| entry.name == name)
        });
        let Some(entry) = stamp.filter(|entry| entry.metadata.kind == EntryKind::RegularFile)
        else {
            return Err(failure(
                path,
                "resource must be a regular file, not a symlink",
            ));
        };
        let (bytes, actual) = self
            .endpoint
            .read_workspace_source(path, self.deadline, |connection, relative| {
                connection.read_bounded_file(relative, byte_limit, self.deadline, || Ok(()))
            })
            .map_err(|error| failure(path, error))?;
        if actual != entry.metadata.stamp {
            return Err(failure(
                path,
                "resource changed during capture; rescan to retry",
            ));
        }
        Ok(bytes)
    }

    pub(in crate::local_runtime) fn verify(&self) -> Result<(), RuntimeResourceLoadError> {
        for ((root, original), (_, extension)) in self.collections.iter().zip(COLLECTIONS) {
            if &list(self.endpoint, root, extension, self.deadline)? != original {
                return Err(failure(
                    root,
                    "resource definitions changed during capture; rescan to retry",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::local_runtime) fn revisions(
        &self,
    ) -> impl Iterator<Item = (PathBuf, String)> + '_ {
        self.collections.iter().map(|(root, entries)| {
            let bytes =
                serde_json::to_vec(entries).expect("bounded Client directory records serialize");
            (
                root.clone(),
                crate::local_runtime::settings::revision(Some(&bytes)),
            )
        })
    }

    pub(in crate::local_runtime) fn owns_revision(&self, path: &Path) -> bool {
        self.collections.iter().any(|(root, collection)| {
            path == root
                || (path.parent() == Some(root)
                    && matches!(collection, Observation::Directory(collection) if {
                        path.file_name().is_some_and(|name| {
                            collection
                                .entries
                                .iter()
                                .any(|entry| name == entry.name.as_str())
                        })
                    }))
        })
    }
}

fn list(
    endpoint: &LocalExecutionEndpoint,
    root: &Path,
    extension: &str,
    deadline: Instant,
) -> Result<Observation, RuntimeResourceLoadError> {
    let directory = match endpoint.read_workspace_source(root, deadline, |connection, relative| {
        connection.entry_metadata_until(relative, deadline)
    }) {
        Ok(directory) if directory.kind == EntryKind::Directory => directory,
        Ok(entry) => {
            return Ok(Observation::Invalid {
                entry,
                reason: "canonical resource root must be a directory, not a symlink",
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Observation::Missing),
        Err(error) => return Err(failure(root, error)),
    };
    let mut entries = Vec::new();
    let mut previous = None::<String>;
    let mut offset = 0;
    let mut count = 0;
    loop {
        // Count every authored entry even when its extension is unused. Allow
        // one excluded lock directory and one entry to prove overflow.
        let limit = (MAX_DIRECTORY_ENTRIES + 2 - usize::try_from(offset).unwrap_or(usize::MAX))
            .min(MAX_DIRECTORY_PAGE_ENTRIES);
        let limit = u32::try_from(limit).expect("bounded directory page");
        let page = endpoint.read_workspace_source(root, deadline, |connection, relative| {
            connection.directory_page_until(
                relative,
                offset,
                limit,
                Some(&directory.stamp.revision),
                deadline,
            )
        });
        let page = page.map_err(|error| failure(root, error))?;
        for entry in page.entries {
            if previous.as_ref().is_some_and(|name| name >= &entry.name) {
                return Err(failure(
                    root,
                    "Client resource directory pages are out of order",
                ));
            }
            previous = Some(entry.name.clone());
            if crate::local_runtime::settings::is_lock_directory(entry.name.as_ref()) {
                continue;
            }
            count += 1;
            if count > MAX_DIRECTORY_ENTRIES {
                return Ok(Observation::Invalid {
                    entry: directory,
                    reason: "canonical resource root exceeds 1024 entries",
                });
            }
            if Path::new(&entry.name)
                .extension()
                .is_some_and(|value| value == extension)
            {
                entries.push(entry);
                if entries.len() > MAX_RESOURCE_FILES {
                    return Ok(Observation::Invalid {
                        entry: directory,
                        reason: "canonical catalog exceeds 128 resources",
                    });
                }
            }
        }
        let Some(next) = page.next_offset else {
            return Ok(Observation::Directory(Collection {
                identity: directory.stamp.identity,
                entries,
            }));
        };
        offset = next;
    }
}

fn failure(path: &Path, reason: impl std::fmt::Display) -> RuntimeResourceLoadError {
    let mut error = RuntimeResourceLoadError::new(reason.to_string()).at(path, "resources");
    error.inspection.category = Some("workspace_resources");
    error.inspection.correction = Some("check the bound Workspace definitions, then rescan");
    error.inspection.detail = Some(error.message.clone());
    error
}
