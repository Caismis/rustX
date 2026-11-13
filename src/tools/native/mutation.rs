//! Descriptor-owned native whole-file mutations.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{AtFlags, Mode, OFlags, mkdirat, open, openat, renameat, unlinkat};

use super::support::resolve_effective_path;
use crate::tools::managed_output::ManagedToolOutput;

const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Authorization retains the deepest existing directory object. Missing
/// descendants are created only relative to this object, never by full path.
#[derive(Debug, Clone)]
pub struct BoundMutationTarget {
    interpreted: PathBuf,
    effective: PathBuf,
    directory: Arc<File>,
    identity: (u64, u64),
    missing: Vec<OsString>,
    leaf: OsString,
}

impl PartialEq for BoundMutationTarget {
    fn eq(&self, other: &Self) -> bool {
        self.interpreted == other.interpreted
            && self.effective == other.effective
            && self.identity == other.identity
            && self.missing == other.missing
            && self.leaf == other.leaf
    }
}
impl Eq for BoundMutationTarget {}

/// The directory and single leaf used by both reads and atomic replacement.
#[derive(Debug)]
pub struct MutationTarget {
    path: PathBuf,
    directory: Arc<File>,
    leaf: OsString,
}

impl BoundMutationTarget {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.effective
    }

    /// Early refusal remains useful, but commit safety comes from retained
    /// descriptors even if an ancestor changes after this check returns.
    pub fn ensure_still_authorized(&self, output: &ManagedToolOutput) -> Result<(), String> {
        let current = resolve_effective_path(&self.interpreted)?;
        if current != self.effective {
            return Err(format!(
                "mutation target changed since authorization (authorized {}, now {})",
                self.effective.display(),
                current.display()
            ));
        }
        output
            .ensure_model_mutation_allowed(&self.effective)
            .map_err(|e| e.to_string())
    }

    /// Open/create descendants through the already-authorized directory.
    pub fn commit_target(&self, create_parents: bool) -> Result<MutationTarget, String> {
        let mut directory = self.directory.clone();
        for name in &self.missing {
            if create_parents {
                match mkdirat(directory.as_ref(), name, Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(format!("cannot create mutation parent: {error}")),
                }
            }
            directory = Arc::new(File::from(
                openat(directory.as_ref(), name, DIRECTORY, Mode::empty())
                    .map_err(|e| format!("cannot open mutation parent: {e}"))?,
            ));
        }
        Ok(MutationTarget {
            path: self.effective.clone(),
            directory,
            leaf: self.leaf.clone(),
        })
    }
}

impl MutationTarget {
    /// Never follow a leaf substituted after authorization into a new symlink
    /// or block on a substituted FIFO/device.
    pub fn read(&self) -> Result<Vec<u8>, String> {
        let mut file = File::from(
            openat(
                self.directory.as_ref(),
                &self.leaf,
                OFlags::RDONLY
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::NOCTTY
                    | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| format!("cannot read {}: {e}", self.path.display()))?,
        );
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("mutation input must be a regular file".into());
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

/// Resolve links once, then pin the canonical parent chain without following
/// links introduced between resolution and descriptor acquisition.
pub fn bind_mutation_target(
    interpreted: &Path,
    output: &ManagedToolOutput,
) -> Result<BoundMutationTarget, String> {
    let effective = resolve_effective_path(interpreted)?;
    output
        .ensure_model_mutation_allowed(&effective)
        .map_err(|e| e.to_string())?;
    let leaf = effective
        .file_name()
        .ok_or("mutation target has no filename")?
        .to_os_string();
    let parent = effective.parent().ok_or("mutation target has no parent")?;
    let mut directory = File::from(open("/", DIRECTORY, Mode::empty()).map_err(|e| e.to_string())?);
    let mut missing = Vec::new();
    for component in parent.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        if !missing.is_empty() {
            missing.push(name.to_os_string());
            continue;
        }
        match openat(&directory, name, DIRECTORY, Mode::empty()) {
            Ok(fd) => directory = File::from(fd),
            Err(rustix::io::Errno::NOENT) => missing.push(name.to_os_string()),
            Err(error) => {
                return Err(format!(
                    "cannot bind mutation directory {}: {error}",
                    parent.display()
                ));
            }
        }
    }
    let metadata = directory.metadata().map_err(|e| e.to_string())?;
    Ok(BoundMutationTarget {
        interpreted: interpreted.to_path_buf(),
        effective,
        directory: Arc::new(directory),
        identity: (metadata.dev(), metadata.ino()),
        missing,
        leaf,
    })
}

#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_TEMP_CREATE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// A retained temporary File and descriptor-relative rename form the commit.
/// No path-based reopen occurs between creation and replacement.
pub fn atomic_commit(target: &MutationTarget, content: &[u8]) -> Result<(), String> {
    #[cfg(test)]
    BEFORE_TEMP_CREATE.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let temporary = OsString::from(format!(".rustx-tool-tmp-{}", uuid::Uuid::now_v7()));
    let mut file = File::from(
        openat(
            target.directory.as_ref(),
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o666),
        )
        .map_err(|e| format!("cannot create mutation temporary file: {e}"))?,
    );
    let result = file
        .write_all(content)
        .map_err(|e| format!("cannot write mutation temporary file: {e}"))
        .and_then(|()| {
            renameat(
                target.directory.as_ref(),
                &temporary,
                target.directory.as_ref(),
                &target.leaf,
            )
            .map_err(|e| format!("cannot persist {}: {e}", target.path.display()))
        });
    if result.is_err() {
        let _ = unlinkat(target.directory.as_ref(), &temporary, AtFlags::empty());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::runtime::types::CancellationReason;
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext};
    use crate::tools::types::{
        ToolExecutionStatus, ToolInvocation, ToolInvocationId, ToolInvocationMode, ToolProgress,
    };
    use crate::tools::workspace::Workspace;

    struct Quiet;
    impl ProgressReporter for Quiet {
        fn report(&self, _: ToolProgress) {}
    }

    #[test]
    fn write_and_edit_commit_to_held_directory_after_final_check_ancestor_replacement() {
        for (tool, missing) in [("write", false), ("edit", false), ("write", true)] {
            let temporary = tempfile::tempdir().unwrap();
            let ancestor = temporary.path().join("ancestor");
            let root = ancestor.join("workspace");
            std::fs::create_dir_all(&root).unwrap();
            let outside = temporary.path().join("outside");
            std::fs::create_dir_all(outside.join("workspace/nested")).unwrap();
            let relative = if missing { "nested/PLAN.md" } else { "PLAN.md" };
            std::fs::write(outside.join("workspace").join(relative), "outside sentinel").unwrap();
            if !missing {
                std::fs::write(root.join(relative), "old plan").unwrap();
            }
            let conversation = ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
            let output =
                ManagedToolOutput::new(conversation.clone(), temporary.path().join("managed"))
                    .unwrap();
            let artifacts =
                ArtifactStore::new(conversation.clone(), temporary.path().join("artifacts"))
                    .unwrap();
            let workspace = Workspace::new(&root).unwrap();
            let environment = ToolEnvironment::new();
            let mut context = ToolExecutionContext::new(
                &conversation,
                None,
                ExecutionCancellation::detached(
                    CancellationSignal::new(),
                    CancellationReason::UserRequested,
                ),
                &workspace,
                &Quiet,
                &artifacts,
                &output,
                &environment,
            );
            context.bound_mutation =
                Some(bind_mutation_target(&root.join(relative), &output).unwrap());
            let renamed = temporary.path().join("held");
            let original = ancestor.clone();
            let held = renamed.clone();
            let redirected = outside.clone();
            BEFORE_TEMP_CREATE.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    std::fs::rename(&original, held).unwrap();
                    std::os::unix::fs::symlink(redirected, original).unwrap();
                }));
            });
            let invocation = ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-boundary"),
                },
                tool_id: ToolId::new(format!("tool-{tool}")),
                tool_name: tool.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments: if tool == "write" {
                    serde_json::json!({"path": relative, "content": "new plan"})
                } else {
                    serde_json::json!({"path": relative, "edits": [{"oldText": "old plan", "newText": "new plan"}]})
                },
            };
            let result = if tool == "write" {
                super::super::write::run_write(&invocation, &context)
            } else {
                super::super::edit::run_edit(&invocation, &context)
            };
            assert_eq!(
                result.status,
                ToolExecutionStatus::Success,
                "{tool}: {result:?}"
            );
            assert_eq!(
                std::fs::read_to_string(renamed.join("workspace").join(relative)).unwrap(),
                "new plan"
            );
            assert_eq!(
                std::fs::read_to_string(outside.join("workspace").join(relative)).unwrap(),
                "outside sentinel"
            );
            for parent in [outside.join("workspace"), outside.join("workspace/nested")] {
                assert!(!std::fs::read_dir(parent).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".rustx-tool-tmp-")
                }));
            }
        }
    }
}
