//! Coarse invalidation hints for configuration watches. Exact resource
//! capture remains authoritative, including while a workspace stays dirty.
use super::{command, git};
use crate::process::Operations;
use rustx_process_supervisor::{process_runner::SupervisedCommandSpec, signal::ProcessSignal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::oneshot;

const PROBE_INTERVAL: Duration = Duration::from_secs(5);
const GIT_BUDGET: Duration = Duration::from_millis(750);

#[derive(Debug, PartialEq, Eq)]
enum Fingerprint {
    Git { head: String, dirty: bool },
    Directory { path: PathBuf, modified: SystemTime },
}

/// Owned by the binding's data lane. All configuration watchers share its
/// one pending worker, including across same-instance connection recovery.
#[derive(Default)]
pub(crate) struct WorkspaceObservation {
    baseline: Option<Fingerprint>,
    pending: Option<oneshot::Receiver<Option<Fingerprint>>>,
    next: Option<Instant>,
}

impl WorkspaceObservation {
    /// Return immediately. A completed change becomes one native resync hint.
    pub(crate) fn poll(&mut self, operations: &Operations) -> bool {
        if let Some(receiver) = &mut self.pending {
            match receiver.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => return false,
                Ok(Some(current)) => {
                    self.pending = None;
                    return self.replace(current);
                }
                _ => self.pending = None,
            }
        }
        let now = Instant::now();
        if self.next.is_some_and(|next| next > now) {
            return false;
        }
        self.next = Some(now + PROBE_INTERVAL);
        let (send, receive) = oneshot::channel();
        let workspace = operations.workspace().to_owned();
        let signal = operations.auxiliary_signal();
        if operations.start_auxiliary(async move {
            let _ = send.send(observe(&workspace, signal).await);
        }) {
            self.pending = Some(receive);
        }
        false
    }

    fn replace(&mut self, current: Fingerprint) -> bool {
        let changed = self.baseline.as_ref().is_some_and(|old| old != &current);
        self.baseline = Some(current);
        changed
    }
}

async fn observe(workspace: &Path, cancellation: ProcessSignal) -> Option<Fingerprint> {
    let directory = || {
        Some(Fingerprint::Directory {
            modified: workspace.metadata().ok()?.modified().ok()?,
            path: workspace.to_owned(),
        })
    };
    if !git::may_have_repository(workspace) {
        return directory();
    }
    let deadline = Instant::now() + GIT_BUDGET;
    let run = |args: &[&str]| {
        command::run(
            SupervisedCommandSpec {
                command: format!("exec {}", git::invocation(args)),
                cwd: workspace.to_owned(),
                command_environment: git::environment(),
                timeout: Some(deadline.saturating_duration_since(Instant::now())),
                cancellation: cancellation.child(),
            },
            128,
            (),
        )
    };
    let (status, head, _) = run(&["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .ok()?;
    if matches!(status, 1 | 128) {
        return directory();
    }
    let head = std::str::from_utf8(&head).ok()?.trim();
    if status != 0 || !matches!(head.len(), 40 | 64) || !head.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    // Plumbing compares the tracked index/worktree stat information. It
    // neither enumerates untracked directories nor produces a content diff.
    let (status, _, _) = run(&[
        "diff-index",
        "--quiet",
        "--no-ext-diff",
        "--no-renames",
        "--ignore-submodules=all",
        "HEAD",
        "--",
        ".",
    ])
    .await
    .ok()?;
    matches!(status, 0 | 1).then(|| Fingerprint::Git {
        head: head.to_owned(),
        dirty: status == 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[tokio::test(flavor = "multi_thread")]
    async fn git_tracks_head_and_dirty_state_and_an_unborn_head_has_a_stable_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut observed = WorkspaceObservation::default();
        let sample = || observe(root, ProcessSignal::new());
        assert!(!observed.replace(sample().await.unwrap()));
        std::fs::create_dir(root.join("nested")).unwrap();
        observed.replace(sample().await.unwrap());
        std::fs::write(root.join("nested/file"), "initial").unwrap();
        assert!(!observed.replace(sample().await.unwrap()));
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .current_dir(root)
                    .args([
                        "-c",
                        "user.name=Fixture",
                        "-c",
                        "user.email=fixture@example.test"
                    ])
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        git(&["init", "-q"]);
        observed.replace(sample().await.unwrap());
        assert!(!observed.replace(sample().await.unwrap()));
        git(&["add", "nested/file"]);
        git(&["commit", "-qm", "Initial"]);
        assert!(matches!(
            sample().await,
            Some(Fingerprint::Git { dirty: false, .. })
        ));
        std::fs::write(root.join("nested/file"), "updated file").unwrap();
        assert!(matches!(
            sample().await,
            Some(Fingerprint::Git { dirty: true, .. })
        ));
    }
}
