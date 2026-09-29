//! Process-wide coordination for one repository's shared worktree/ref metadata.
//!
//! Managers are composed independently for Conversations, so a manager-local
//! mutex cannot own this boundary. Canonical Git common-directory identity also
//! unifies linked worktrees and logical subdirectories. Only metadata commands
//! (including registration reads) hold the guard; overlay/model work does not.
//! Git remains authoritative against external processes and ref changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use super::{CancellationSignal, WorkspaceAcquireError};

type RepositoryLocks = BTreeMap<PathBuf, Weak<AsyncMutex<()>>>;

fn repository_lock(common_directory: &Path) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<RepositoryLocks>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Mutex::default).lock().unwrap();
    locks.retain(|_, lock| lock.strong_count() != 0);
    if let Some(lock) = locks.get(common_directory).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(common_directory.to_path_buf(), Arc::downgrade(&lock));
    lock
}

pub(super) async fn acquire(
    common_directory: &Path,
    cancellation: Option<&CancellationSignal>,
) -> Result<OwnedMutexGuard<()>, WorkspaceAcquireError> {
    let lock = repository_lock(common_directory);
    if let Some(cancellation) = cancellation {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(WorkspaceAcquireError::Cancelled),
            guard = lock.lock_owned() => Ok(guard),
        }
    } else {
        Ok(lock.lock_owned().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    #[tokio::test]
    async fn same_repository_waits_other_repository_proceeds_and_cancelled_waiter_exits() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let other = root.path().join("other");
        let held = acquire(&first, None).await.unwrap();
        // Independently resolved owners share the same gate, including the
        // owning Arc retained by a guard after acquire's local Arc is gone.
        let cancellation = CancellationSignal::new();
        let mut waiting = Box::pin(acquire(&first, Some(&cancellation)));
        let mut context = Context::from_waker(Waker::noop());
        assert!(waiting.as_mut().poll(&mut context).is_pending());
        let mut independent = Box::pin(acquire(&other, None));
        assert!(matches!(
            independent.as_mut().poll(&mut context),
            Poll::Ready(Ok(_))
        ));
        cancellation.cancel();
        assert!(matches!(
            waiting.as_mut().poll(&mut context),
            Poll::Ready(Err(WorkspaceAcquireError::Cancelled))
        ));
        let mut next = Box::pin(acquire(&first, None));
        assert!(next.as_mut().poll(&mut context).is_pending());
        drop(held);
        assert!(matches!(
            next.as_mut().poll(&mut context),
            Poll::Ready(Ok(_))
        ));
    }
}
