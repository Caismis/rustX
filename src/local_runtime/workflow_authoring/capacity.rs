//! Capacity admission of Workflow file additions (`create`, `copy`).
//!
//! Discovery has two bounds, and a file past either breaks discovery for
//! files that did nothing wrong: a canonical resource directory holds at most
//! [`MAX_RESOURCE_FILES`] `.yaml` entries, and one composed catalog (User, or
//! User with one Workspace, after shadowing) at most
//! [`MAX_WORKFLOW_DEFINITIONS`] distinct candidate IDs, counted from valid
//! filename identities before anything is compiled. An addition is refused
//! before publication when it would pass a bound of the scopes it is
//! authorized for: a Global addition counts the User directory and the
//! User-only catalog (it cannot know every Workspace a later session may
//! compose with); a Workspace addition counts that Workspace's directory and
//! its composition with User. Replace and delete never add an identity, so
//! they stay available to repair a directory that is already over a bound.
//!
//! Discovery also reads at most [`MAX_DIRECTORY_ENTRIES`] authored entries
//! of a directory. rustX's own writer locks live in one lock directory, which
//! discovery never counts, however many files were ever written or refused
//! there and whether or not this write is the one that creates it, so only
//! the published file (and its transient staged candidate) is admitted
//! against that bound.
//!
//! Count-changing rustX writes are serialized per physical directory by a
//! persistent advisory capacity lock, taken before the file's own writer
//! lock, in one order: User, then the Workspace. A Workspace addition
//! therefore excludes concurrent User additions as well. A Workspace whose
//! Workflow directory is the User one holds that one lock once. Like every
//! writer lock, it serializes rustX writers only, never external editors or
//! Git.
use super::{Scope, WorkflowDiagnostic};
use crate::local_runtime::configuration::UserConfigManager;
use crate::local_runtime::configuration::settings::SettingsError;
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::local_runtime::resource_directory::{MAX_DIRECTORY_ENTRIES, MAX_RESOURCE_FILES};
use crate::local_runtime::settings::SourceDirectory;
use crate::runtime::workflow::{MAX_WORKFLOW_DEFINITIONS, WorkflowId};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::File;

/// The pseudo document whose writer lock is a scope's capacity lock.
const CAPACITY_DOCUMENT: &str = "workflow-capacity";

/// The capacity locks of one addition's scopes, held until it is published.
pub(in crate::local_runtime) struct Capacity {
    /// The target scope first, then User for a Workspace target.
    scopes: Vec<Scope>,
    _locks: Vec<File>,
}

fn refused(category: &str, reason: &str, correction: &str) -> SettingsError {
    let mut diagnostic = WorkflowDiagnostic::new("$", category, reason);
    diagnostic.correction = Some(correction.into());
    SettingsError::InvalidWorkflow(Box::new(diagnostic))
}

impl Capacity {
    /// Take the capacity locks an addition to `target` needs, User first.
    /// # Errors
    /// `Io` when a scope directory or its lock cannot be opened.
    pub(in crate::local_runtime) fn lock(
        manager: &UserConfigManager,
        target: &SourceTarget,
    ) -> Result<Self, SettingsError> {
        let user = Scope::of(manager, &SourceTarget::User);
        let mut ordered = vec![user];
        if matches!(target, SourceTarget::Workspace { .. }) {
            ordered.push(Scope::of(manager, target));
        }
        let mut locks = Vec::with_capacity(ordered.len());
        let mut held = Vec::with_capacity(ordered.len());
        for scope in &ordered {
            let directory =
                SourceDirectory::create_directory(&scope.directory).map_err(|error| {
                    crate::local_runtime::settings::report_io_failure(
                        "admit_capacity_directory",
                        &error,
                    );
                    target.validate().err().unwrap_or(SettingsError::Io)
                })?;
            // A Workspace whose Workflow directory is the User one (a
            // Workspace at `~/rustx`) shares its one physical capacity lock,
            // which this addition already holds: locking it again would wait
            // for itself.
            if held.contains(&directory.identity()) {
                continue;
            }
            held.push(directory.identity());
            locks.push(
                directory
                    .lock(OsStr::new(CAPACITY_DOCUMENT))
                    .map_err(|error| {
                        crate::local_runtime::settings::report_io_failure(
                            "acquire_capacity_lock",
                            &error,
                        );
                        SettingsError::Io
                    })?,
            );
        }
        ordered.reverse();
        Ok(Self {
            scopes: ordered,
            _locks: locks,
        })
    }

    /// Admit adding the absent file `id` to the target scope, counting under
    /// the held locks exactly what discovery will count.
    /// # Errors
    /// `InvalidWorkflow` with category `capacity` when a bound would pass.
    pub(in crate::local_runtime) fn admit(&self, id: &WorkflowId) -> Result<(), SettingsError> {
        let mut identities = BTreeSet::from([id.clone()]);
        for (index, scope) in self.scopes.iter().enumerate() {
            let entries = crate::local_runtime::resource_directory::entries(
                &scope.boundary,
                &scope.directory,
            )
            .map_err(|_| SettingsError::Invalid)?;
            let files: Vec<_> = entries
                .iter()
                .filter(|path| {
                    path.extension().and_then(|extension| extension.to_str()) == Some("yaml")
                })
                .collect();
            // The physical bounds apply to the directory being added to:
            // publication links the file while its staged candidate still
            // exists, so a discovery read meanwhile counts two more entries;
            // afterwards the file is one more entry and one more YAML file.
            if index == 0 && entries.len() + 2 > MAX_DIRECTORY_ENTRIES {
                return Err(refused(
                    "capacity",
                    "the Workflow directory leaves no room for another file within discovery's bound of 1024 entries",
                    "remove files that are not Workflows from this directory before adding another",
                ));
            }
            if index == 0 && files.len() >= MAX_RESOURCE_FILES {
                return Err(refused(
                    "capacity",
                    "the Workflow directory already holds the maximum of 128 YAML files",
                    "delete a Workflow file of this directory before adding another",
                ));
            }
            if files.len() > MAX_RESOURCE_FILES {
                // An over-full User directory already breaks this composition.
                return Err(refused(
                    "capacity",
                    "the Global Workflow directory holds more than 128 YAML files",
                    "delete Global Workflow files before adding another",
                ));
            }
            identities.extend(files.iter().filter_map(|path| {
                path.file_stem()
                    .and_then(OsStr::to_str)
                    .and_then(|name| WorkflowId::parse(name).ok())
            }));
        }
        if identities.len() > MAX_WORKFLOW_DEFINITIONS {
            return Err(if self.scopes.len() == 1 {
                refused(
                    "capacity",
                    "Global already defines the maximum of 64 Workflow IDs",
                    "delete a Global Workflow before adding another",
                )
            } else {
                refused(
                    "capacity",
                    "Global and this Workspace together already define the maximum of 64 Workflow IDs",
                    "delete a Workflow of this Workspace or of Global, or override an existing ID instead",
                )
            });
        }
        Ok(())
    }
}
