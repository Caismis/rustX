//! Physical readers for the shared Skill collection scanner.

use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustx_execution_protocol::{FileStamp, MAX_DIRECTORY_PAGE_ENTRIES};

use crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint;
use crate::runtime::CancellationSignal;

pub(super) use rustx_types::skills::tree::{Children, Entry, Native, SkillTree};

pub(crate) type ObservedSkillDirectories =
    std::collections::BTreeMap<PathBuf, Option<std::collections::BTreeSet<std::ffi::OsString>>>;

#[derive(PartialEq, Eq)]
pub(crate) struct SkillWatchObservation {
    pub(crate) paths: ObservedSkillDirectories,
    pub(crate) complete: bool,
}

pub(super) struct Client<'a> {
    observations: std::cell::RefCell<ObservedSkillDirectories>,
    resolved_links: std::cell::RefCell<std::collections::BTreeMap<PathBuf, PathBuf>>,
    endpoint: &'a LocalExecutionEndpoint,
    deadline: Instant,
    signal: &'a CancellationSignal,
}

impl<'a> Client<'a> {
    pub(super) fn new(
        endpoint: &'a LocalExecutionEndpoint,
        signal: &'a CancellationSignal,
    ) -> Self {
        Self {
            observations: std::cell::RefCell::default(),
            resolved_links: std::cell::RefCell::default(),
            endpoint,
            deadline: Instant::now() + Duration::from_secs(5),
            signal,
        }
    }
}

impl Client<'_> {
    pub(super) fn into_observations(self) -> ObservedSkillDirectories {
        self.observations.into_inner()
    }
}

impl SkillTree for Client<'_> {
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(self
            .resolved_links
            .borrow()
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_path_buf()))
    }

    fn entry(&self, path: &Path) -> io::Result<Option<Entry>> {
        crate::package_io::check_cancelled(self.signal)?;
        match self
            .endpoint
            .read_workspace_source(path, self.deadline, |channel, relative| {
                channel.resolve_resource_entry_until(relative, self.deadline)
            }) {
            Ok(resolution) => {
                let link = !resolution.dependencies.is_empty();
                for dependency in resolution.dependencies {
                    self.observe(&self.endpoint.workspace().join(dependency));
                }
                let Some((relative, metadata)) = resolution.entry else {
                    return Ok(None);
                };
                if link {
                    self.resolved_links
                        .borrow_mut()
                        .insert(path.to_path_buf(), self.endpoint.workspace().join(relative));
                }
                Ok(Some(Entry {
                    kind: metadata.kind,
                    link,
                }))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn children(&self, path: &Path, limit: usize) -> io::Result<Children> {
        let physical = self.canonical(path)?;
        self.observations
            .borrow_mut()
            .insert(physical.clone(), None);
        let mut children = Vec::new();
        let mut offset = 0;
        let mut revision = None;
        let mut scanned = 0;
        while scanned < limit {
            crate::package_io::check_cancelled(self.signal)?;
            let count = u32::try_from((limit - scanned).min(MAX_DIRECTORY_PAGE_ENTRIES))
                .expect("bounded directory page");
            let page = self.endpoint.read_workspace_source(
                &physical,
                self.deadline,
                |channel, relative| {
                    channel.skill_directory_page_until(
                        relative,
                        offset,
                        count,
                        revision.as_deref(),
                        self.deadline,
                    )
                },
            )?;
            if let (Some(previous), Some(first)) = (children.last(), page.entries.first())
                && previous >= &path.join(&first.name)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Client Skill directory pages are out of order",
                ));
            }
            scanned +=
                page.entries.len() + usize::try_from(page.skipped).expect("bounded directory page");
            children.extend(page.entries.into_iter().map(|entry| path.join(entry.name)));
            let Some(next) = page.next_offset else {
                break;
            };
            offset = next;
            revision = Some(page.revision);
        }
        Ok(Children {
            paths: children,
            scanned,
        })
    }

    fn read_file(&self, path: &Path, bound: u64) -> io::Result<Vec<u8>> {
        self.endpoint
            .read_workspace_source(path, self.deadline, |channel, relative| {
                channel
                    .read_bounded_file(relative, bound, self.deadline, || {
                        crate::package_io::check_cancelled(self.signal)
                    })
                    .map(|(bytes, _stamp)| bytes)
            })
    }

    fn frontmatter(&self, path: &Path, signal: &CancellationSignal) -> io::Result<Vec<u8>> {
        let reader = ClientFile {
            source: self,
            path,
            stamp: None,
            offset: Some(0),
        };
        super::frontmatter::prefix(
            &mut BufReader::with_capacity(super::frontmatter::BUFFER_BYTES, reader),
            signal,
        )
        .map(|(bytes, _)| bytes)
    }

    fn observe(&self, path: &Path) {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if !parent.starts_with(self.endpoint.workspace()) {
            return;
        }
        let physical = self.canonical(parent).expect("cached Client path");
        if let Some(names) = self
            .observations
            .borrow_mut()
            .entry(physical)
            .or_insert_with(|| Some(std::collections::BTreeSet::new()))
        {
            names.insert(name.to_owned());
        }
    }
}

struct ClientFile<'a> {
    source: &'a Client<'a>,
    path: &'a Path,
    stamp: Option<FileStamp>,
    offset: Option<u64>,
}

impl Read for ClientFile<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let Some(offset) = self.offset else {
            return Ok(0);
        };
        let count = u32::try_from(buffer.len().min(super::frontmatter::BUFFER_BYTES))
            .expect("bounded read-ahead");
        if count == 0 {
            return Ok(0);
        }
        let revision = self.stamp.as_ref().map(|stamp| stamp.revision.as_str());
        let page = self
            .source
            .endpoint
            .read_workspace_source(self.path, self.source.deadline, |channel, relative| {
                channel.read_range_admitted(
                    relative,
                    offset,
                    count,
                    revision,
                    self.source.deadline,
                    || crate::package_io::check_cancelled(self.source.signal),
                    || (),
                )
            })
            .map_err(|error| {
                // A wire failure is a failed observation, distinct from invalid YAML.
                if error.kind() == io::ErrorKind::InvalidData {
                    io::Error::other(error)
                } else {
                    error
                }
            })?;
        if self
            .stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &page.metadata)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Client Skill changed while reading its definition",
            ));
        }
        self.stamp = Some(page.metadata);
        self.offset = page.next_offset;
        buffer[..page.bytes.len()].copy_from_slice(&page.bytes);
        Ok(page.bytes.len())
    }
}
