//! Native recovery evidence for one dead activation. A free process lease alone
//! proves nothing about descendants. Only a successful native runtime drain may
//! write the receipt; recovery additionally excludes its still-live writer.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};

use super::ipc::SubagentChildSpec;
use crate::runtime::identity::{ConversationId, SubagentId};

const LEASE: &str = "physical-owner";
const RECEIPT: &str = "physical-settlement.json";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    activation: SubagentId,
    conversation: ConversationId,
}

/// Acquired before composition and retained through dispatcher shutdown. Unlike
/// the optional inspection lease, failure to establish this exclusion is fatal.
pub(crate) struct ChildPhysicalLease {
    _lock: Flock<File>,
    path: PathBuf,
    receipt: Receipt,
}

impl ChildPhysicalLease {
    pub(crate) fn acquire(spec: &SubagentChildSpec) -> std::io::Result<Self> {
        let path = spec.runtime_root()?;
        Self::acquire_at(
            path,
            spec.subagent_id.clone(),
            spec.child_conversation_id.clone(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        path: PathBuf,
        activation: SubagentId,
        conversation: ConversationId,
    ) -> std::io::Result<Self> {
        Self::acquire_at(path, activation, conversation)
    }

    fn acquire_at(
        path: PathBuf,
        activation: SubagentId,
        conversation: ConversationId,
    ) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path.join(LEASE))?;
        let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock)
            .map_err(|(_, error)| std::io::Error::other(error))?;
        Ok(Self {
            _lock: lock,
            path,
            receipt: Receipt {
                activation,
                conversation,
            },
        })
    }

    /// Call only after the runtime's native shutdown returned Quiescent.
    pub(crate) fn publish_quiescent(&self) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.path.join(RECEIPT))?;
        file.write_all(&serde_json::to_vec(&self.receipt)?)?;
        file.sync_all()?;
        File::open(&self.path)?.sync_all()
    }
}

/// The reconciliation owner retains the exact exclusive inode while committing
/// proof. No process is killed, reattached, or resumed by this capability.
pub(crate) struct RecoveredPhysicalProof {
    _lock: Flock<File>,
    _path: PathBuf,
}

pub(crate) fn prove(
    product: &crate::runtime::local_storage::ProductRoot,
    session: &crate::runtime::identity::SessionId,
    conversation: &ConversationId,
    activation: &SubagentId,
) -> std::io::Result<Option<RecoveredPhysicalProof>> {
    let store = super::child_conversation_store_path(product.root(), session, conversation);
    let directory = product.confined(store.parent().expect("conversation parent"))?;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("incarnation-")
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let path = product.confined(&entry.path())?;
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(product.confined(&path.join(LEASE))?)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let Ok(lock) = Flock::lock(file, FlockArg::LockExclusiveNonblock) else {
            continue;
        };
        let receipt = match read_receipt(&product.confined(&path.join(RECEIPT))?) {
            Ok(Some(receipt)) => receipt,
            Ok(None) => continue,
            Err(error) => return Err(error),
        };
        if receipt.activation != *activation || receipt.conversation != *conversation {
            continue;
        }
        return Ok(Some(RecoveredPhysicalProof {
            _lock: lock,
            _path: path,
        }));
    }
    Ok(None)
}

fn read_receipt(path: &Path) -> std::io::Result<Option<Receipt>> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.len() > 4096 => {
            return Err(std::io::Error::other("invalid physical settlement receipt"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
