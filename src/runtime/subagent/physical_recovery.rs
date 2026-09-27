//! Activation identities are consumed privately before executable publication.
//! A namespace lock excludes recovery throughout initialization; atomic rename
//! publishes the complete authority. Private abandoned identities remain consumed.
//! Published authority is installed before admission and always needs exact proof.
//!
//! The parent acquires an exclusive open-file-description lock before Reserved
//! and passes a duplicate to the child on fd 2. Closing the parent's descriptor
//! never unlocks a surviving child's copy. The child takes its CLOEXEC copy
//! before composition and redirects stderr back to diagnostics. Recovery must
//! acquire that same inode, then consume explicit Unstarted or Quiescent state;
//! neither a missing file nor process disappearance is evidence.
//!
//! Evidence lives outside the disposable incarnation directory and is retained
//! until Session deletion. In particular physical cleanup cannot destroy it
//! before terminal/rollback durability acknowledges the proof.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ipc::SubagentChildSpec;
use crate::runtime::identity::{ConversationId, SessionId, SubagentId};
use crate::runtime::local_storage::ProductRoot;

const LEASE: &str = "physical-owner";
const RECEIPT: &str = "physical-settlement.json";
const EVIDENCE: &str = "physical-settlement";
const CONTINUATIONS: &str = "continuations";
const ALLOCATION_OWNER: &str = ".allocation-owner";
const PRIVATE: &str = ".pending-";
const ABANDONED: &str = ".abandoned-";

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AllocationBoundary {
    Created,
    LeaseCreated,
    Initialized,
    Published,
}

fn allocation_lock(directory: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(ALLOCATION_OWNER))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    /// No child has acquired executable resource authority. A free inherited
    /// lease proves that neither the reserving parent nor any spawned prelude
    /// remains capable of crossing this boundary.
    Unstarted,
    Running,
    Quiescent,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    activation: SubagentId,
    conversation: ConversationId,
    phase: Phase,
}

#[derive(Debug)]
struct PhysicalLease {
    // Do not use an RAII wrapper that calls LOCK_UN in Drop: that would unlock
    // all inherited duplicates. File close releases flock only on the last
    // descriptor referring to this open file description (Unix flock).
    lock: File,
    path: PathBuf,
    activation: SubagentId,
    conversation: ConversationId,
}

impl PhysicalLease {
    fn publish(&self, phase: Phase) -> std::io::Result<()> {
        write_receipt(
            &self.path,
            &Receipt {
                activation: self.activation.clone(),
                conversation: self.conversation.clone(),
                phase,
            },
        )
    }
}

/// The pre-admission owner remains alive through spawn, rollback, or the
/// committed driver. Even a child that has not read Hello inherits its lock.
#[derive(Debug)]
pub(crate) struct ParentPhysicalLease(PhysicalLease);

#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct AllocationError {
    source: std::io::Error,
    /// A positive namespace allocation exists and must enter settlement even
    /// when initialization could not return an executable authority handle.
    pub(crate) consumed: bool,
}

impl ParentPhysicalLease {
    pub(crate) fn reserve(
        product: &ProductRoot,
        session: &SessionId,
        conversation: &ConversationId,
        activation: &SubagentId,
    ) -> Result<Self, AllocationError> {
        Self::reserve_inner(
            product,
            session,
            conversation,
            activation,
            #[cfg(test)]
            |_| Ok(()),
        )
    }

    /// The namespace lock covers identity consumption through publication. No
    /// authority descriptor escapes this scope before the final directory barrier.
    fn reserve_inner(
        product: &ProductRoot,
        session: &SessionId,
        conversation: &ConversationId,
        activation: &SubagentId,
        #[cfg(test)] boundary: impl Fn(AllocationBoundary) -> std::io::Result<()>,
    ) -> Result<Self, AllocationError> {
        let mut consumed = false;
        Self::initialize(
            product,
            session,
            conversation,
            activation,
            &mut consumed,
            #[cfg(test)]
            boundary,
        )
        .map_err(|source| AllocationError { source, consumed })
    }

    fn initialize(
        product: &ProductRoot,
        session: &SessionId,
        conversation: &ConversationId,
        activation: &SubagentId,
        consumed: &mut bool,
        #[cfg(test)] boundary: impl Fn(AllocationBoundary) -> std::io::Result<()>,
    ) -> std::io::Result<Self> {
        let path = evidence_path(product, session, conversation, activation)?;
        let parent = path.parent().expect("activation evidence parent");
        std::fs::create_dir_all(parent)?;
        let allocation = allocation_lock(parent)?;
        allocation.lock()?;
        let pending = parent.join(format!("{PRIVATE}{}", activation.as_str()));
        let abandoned = parent.join(format!("{ABANDONED}{}", activation.as_str()));
        if path.try_exists()? || abandoned.try_exists()? || pending.try_exists()? {
            *consumed = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "activation identity consumed",
            ));
        }
        std::fs::create_dir(&pending)?;
        *consumed = true;
        // This barrier durably consumes the ordinal, independently of whether
        // authority initialization ever finishes. Persist all new ancestors.
        File::open(&pending)?.sync_all()?;
        for directory in parent.ancestors() {
            File::open(directory)?.sync_all()?;
            if directory == product.root() {
                break;
            }
        }
        #[cfg(test)]
        boundary(AllocationBoundary::Created)?;
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(pending.join(LEASE))?;
        file.try_lock()?;
        file.sync_all()?;
        #[cfg(test)]
        boundary(AllocationBoundary::LeaseCreated)?;
        let mut owner = Self(PhysicalLease {
            lock: file,
            path: pending.clone(),
            activation: activation.clone(),
            conversation: conversation.clone(),
        });
        owner.0.publish(Phase::Unstarted)?;
        #[cfg(test)]
        boundary(AllocationBoundary::Initialized)?;
        std::fs::rename(&pending, &path)?;
        owner.0.path.clone_from(&path);
        File::open(parent)?.sync_all()?;
        // Errors here leave published authority, which recovery must validate
        // exactly like any other published allocation. Err never means harmless.
        #[cfg(test)]
        boundary(AllocationBoundary::Published)?;
        Ok(owner)
    }

    #[cfg(test)]
    pub(crate) fn reserve_at_boundaries(
        product: &ProductRoot,
        session: &SessionId,
        conversation: &ConversationId,
        activation: &SubagentId,
        boundary: impl Fn(AllocationBoundary) -> std::io::Result<()>,
    ) -> Result<Self, AllocationError> {
        Self::reserve_inner(product, session, conversation, activation, boundary)
    }

    pub(crate) fn inherited_file(&self) -> std::io::Result<File> {
        self.0.lock.try_clone()
    }

    /// Parent-owned workspace commands have independent settlement owners.
    /// A child receipt cannot acknowledge work that starts after child drain.
    pub(crate) fn reserve_continuation(&self) -> std::io::Result<ParentPhysicalContinuation> {
        let directory = self.0.path.join(CONTINUATIONS);
        std::fs::create_dir_all(&directory)?;
        File::open(&self.0.path)?.sync_all()?;
        let identity = uuid::Uuid::now_v7().to_string();
        let pending = directory.join(format!(".pending-{identity}"));
        let published = directory.join(&identity);
        std::fs::create_dir(&pending)?;
        let lock = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(pending.join(LEASE))?;
        lock.try_lock()?;
        let mut lease = PhysicalLease {
            lock,
            path: pending.clone(),
            activation: self.0.activation.clone(),
            conversation: self.0.conversation.clone(),
        };
        lease.publish(Phase::Unstarted)?;
        // Only a returned, published authority can be passed to a supervisor.
        // An interrupted .pending allocation durably denotes no spawn permit.
        std::fs::rename(&pending, &published)?;
        File::open(&directory)?.sync_all()?;
        lease.path = published;
        Ok(ParentPhysicalContinuation(lease))
    }

    /// The parent lock excludes new allocation while the caller proves all
    /// helpers started during pre-admission verification. Retain the returned
    /// locks through the durable admission or rollback decision.
    pub(crate) fn prove_continuations(&self) -> std::io::Result<Option<RecoveredPhysicalProof>> {
        prove_continuations_at(
            &self.0.path,
            &self.0.conversation,
            &self.0.activation,
            ProofLock::AfterSupervision,
            RecoveredPhysicalProof { locks: Vec::new() },
        )
    }

    #[cfg(test)]
    pub(crate) fn duplicate_lock_for_test(&self) -> std::io::Result<File> {
        self.0.lock.try_clone()
    }

    /// Only the staging/driver owner after its explicit full resource drain
    /// may assert this. An error or absent child alone never calls it.
    pub(crate) fn publish_quiescent(&self) -> std::io::Result<()> {
        self.0.publish(Phase::Quiescent)
    }
}

/// A parent-side command is a separate physical lifetime from the child.
/// The trusted command supervisor inherits this authority before it can spawn.
#[derive(Debug)]
pub(crate) struct ParentPhysicalContinuation(PhysicalLease);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhysicalContinuationSpec {
    pub(crate) path: PathBuf,
    pub(crate) activation: SubagentId,
    pub(crate) conversation: ConversationId,
}

impl ParentPhysicalContinuation {
    /// The parent may consume the supervisor's explicit complete containment
    /// result when emergency settlement proved the entire owned group empty.
    pub(crate) fn publish_quiescent(&self) -> std::io::Result<()> {
        self.0.publish(Phase::Quiescent)
    }

    pub(crate) fn spec(&self) -> PhysicalContinuationSpec {
        PhysicalContinuationSpec {
            path: self.0.path.clone(),
            activation: self.0.activation.clone(),
            conversation: self.0.conversation.clone(),
        }
    }

    pub(crate) fn inherited_file(&self) -> std::io::Result<File> {
        self.0.lock.try_clone()
    }
}

/// Lives only in the trusted outer supervisor, outside its killable group.
/// An abnormal supervisor exit leaves Running, never fabricated quiescence.
pub(crate) struct ChildPhysicalContinuation(PhysicalLease);

impl ChildPhysicalContinuation {
    pub(crate) fn acquire(spec: &PhysicalContinuationSpec) -> std::io::Result<Self> {
        let inherited = File::from(std::io::stderr().as_fd().try_clone_to_owned()?);
        let expected = std::fs::metadata(spec.path.join(LEASE))?;
        let actual = inherited.metadata()?;
        if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
            return Err(std::io::Error::other(
                "continuation authority descriptor mismatch",
            ));
        }
        let diagnostics = OpenOptions::new()
            .create(true)
            .append(true)
            .open(spec.path.join("diagnostics.log"))?;
        nix::unistd::dup2_stderr(&diagnostics).map_err(std::io::Error::other)?;
        let child = ChildPhysicalLease::acquire_inherited(
            spec.path.clone(),
            inherited,
            spec.activation.clone(),
            spec.conversation.clone(),
        )?;
        Ok(Self(child.0))
    }

    pub(crate) fn publish_quiescent(&self) -> std::io::Result<()> {
        self.0.publish(Phase::Quiescent)
    }
}

/// Acquired before composition and retained through dispatcher/native drain.
pub(crate) struct ChildPhysicalLease(PhysicalLease);

impl ChildPhysicalLease {
    pub(crate) fn acquire(spec: &SubagentChildSpec) -> std::io::Result<Self> {
        let product = ProductRoot::existing(&spec.product_root)?;
        let path = evidence_path(
            &product,
            &spec.session_id,
            &spec.child_conversation_id,
            &spec.subagent_id,
        )?;
        let inherited = File::from(std::io::stderr().as_fd().try_clone_to_owned()?);
        let expected = std::fs::metadata(path.join(LEASE))?;
        let actual = inherited.metadata()?;
        if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
            return Err(std::io::Error::other(
                "child physical authority descriptor mismatch",
            ));
        }
        // stderr is no longer the private lease after this point; the sole
        // retained child copy is CLOEXEC and cannot leak into native tools.
        let diagnostics = OpenOptions::new()
            .create(true)
            .append(true)
            .open(spec.runtime_root()?.join("diagnostics.log"))?;
        nix::unistd::dup2_stderr(&diagnostics).map_err(std::io::Error::other)?;
        Self::acquire_inherited(
            path,
            inherited,
            spec.subagent_id.clone(),
            spec.child_conversation_id.clone(),
        )
    }

    fn acquire_inherited(
        path: PathBuf,
        lock: File,
        activation: SubagentId,
        conversation: ConversationId,
    ) -> std::io::Result<Self> {
        let receipt = read_receipt(&path.join(RECEIPT))?
            .ok_or_else(|| std::io::Error::other("missing physical authority"))?;
        if receipt.activation != activation
            || receipt.conversation != conversation
            || receipt.phase != Phase::Unstarted
        {
            return Err(std::io::Error::other("physical authority is not unstarted"));
        }
        let owner = Self(PhysicalLease {
            lock,
            path,
            activation,
            conversation,
        });
        owner.0.publish(Phase::Running)?;
        Ok(owner)
    }

    #[cfg(test)]
    pub(crate) fn acquire_from_parent(
        spec: &SubagentChildSpec,
        parent: &ParentPhysicalLease,
    ) -> std::io::Result<Self> {
        Self::acquire_inherited(
            parent.0.path.clone(),
            parent.inherited_file()?,
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
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path.join(LEASE))?;
        file.try_lock()?;
        let owner = Self(PhysicalLease {
            lock: file,
            path,
            activation,
            conversation,
        });
        owner.0.publish(Phase::Running)?;
        Ok(owner)
    }

    /// Call only after the explicit native physical lifetime epilogue drained.
    pub(crate) fn publish_quiescent(&self) -> std::io::Result<()> {
        self.0.publish(Phase::Quiescent)
    }
}

/// Retains the exclusive inode until the durable fact consumes the proof.
#[derive(Debug)]
pub(crate) struct RecoveredPhysicalProof {
    locks: Vec<File>,
}

#[cfg(test)]
impl RecoveredPhysicalProof {
    /// Retain only this proof's open-file descriptions after its original owner drops.
    pub(crate) fn duplicate_for_test(&self) -> std::io::Result<Self> {
        Ok(Self {
            locks: self
                .locks
                .iter()
                .map(File::try_clone)
                .collect::<std::io::Result<Vec<_>>>()?,
        })
    }
}

pub(crate) fn prove(
    product: &ProductRoot,
    session: &SessionId,
    conversation: &ConversationId,
    activation: &SubagentId,
) -> std::io::Result<Option<RecoveredPhysicalProof>> {
    prove_with_lock(product, session, conversation, activation, ProofLock::Try)
}

#[derive(Clone, Copy)]
enum ProofLock {
    Try,
    AfterSupervision,
    #[cfg(test)]
    AwaitRelease,
}

/// Positive-proof assertions synchronize on the kernel ownership boundary.
/// Concurrent fork/exec in the test harness can temporarily inherit even a
/// CLOEXEC proof descriptor. Dropping this thread's copy need not make a
/// nonblocking reacquisition succeed immediately. Waiting changes only lock
/// acquisition (including the allocation namespace): exact identity, receipt
/// phase, and every continuation still
/// pass through the production validator, with all proof locks retained.
#[cfg(test)]
pub(crate) fn prove_after_release(
    product: &ProductRoot,
    session: &SessionId,
    conversation: &ConversationId,
    activation: &SubagentId,
) -> std::io::Result<Option<RecoveredPhysicalProof>> {
    prove_with_lock(
        product,
        session,
        conversation,
        activation,
        ProofLock::AwaitRelease,
    )
}

fn prove_with_lock(
    product: &ProductRoot,
    session: &SessionId,
    conversation: &ConversationId,
    activation: &SubagentId,
    locking: ProofLock,
) -> std::io::Result<Option<RecoveredPhysicalProof>> {
    let path = evidence_path(product, session, conversation, activation)?;
    let parent = path.parent().expect("activation evidence parent");
    if !parent.try_exists()? {
        return Ok(None);
    }
    let allocation = allocation_lock(parent)?;
    match locking {
        #[cfg(test)]
        ProofLock::AwaitRelease => allocation.lock()?,
        ProofLock::Try | ProofLock::AfterSupervision => match allocation.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        },
    }
    if !path.try_exists()? {
        let pending = parent.join(format!("{PRIVATE}{}", activation.as_str()));
        let abandoned = parent.join(format!("{ABANDONED}{}", activation.as_str()));
        if pending.try_exists()? {
            // Acquiring the initializer's lock excludes a surviving publisher.
            // Seal its consumed identity without manufacturing physical receipts.
            std::fs::rename(pending, &abandoned)?;
        } else if !abandoned.try_exists()? {
            return Ok(None);
        }
        // Repeat the barrier after uncertain acknowledgement of an earlier seal.
        File::open(parent)?.sync_all()?;
        return Ok(Some(RecoveredPhysicalProof {
            locks: vec![allocation],
        }));
    }
    // Published names always require the exact lease and receipt. Private
    // evidence cannot excuse missing/corrupt evidence in this namespace.
    let Some(proof) = prove_at(&path, conversation, activation, locking)? else {
        return Ok(None);
    };
    prove_continuations_at(&path, conversation, activation, locking, proof)
}

fn prove_continuations_at(
    path: &Path,
    conversation: &ConversationId,
    activation: &SubagentId,
    locking: ProofLock,
    mut proof: RecoveredPhysicalProof,
) -> std::io::Result<Option<RecoveredPhysicalProof>> {
    // Holding the parent authority excludes any further continuation allocation
    // or spawn. Each published helper has its own explicit durable lifetime;
    // the already-quiescent child does not prove these later commands settled.
    let entries = match std::fs::read_dir(path.join(CONTINUATIONS)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(proof)),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".pending-") {
            // reserve_continuation never exposes an unpublished spawn permit.
            continue;
        }
        if !entry.file_type()?.is_dir() {
            return Err(std::io::Error::other(
                "invalid physical continuation authority",
            ));
        }
        let Some(continuation) = prove_at(&entry.path(), conversation, activation, locking)? else {
            return Ok(None);
        };
        proof.locks.extend(continuation.locks);
    }
    Ok(Some(proof))
}

fn prove_at(
    path: &Path,
    conversation: &ConversationId,
    activation: &SubagentId,
    locking: ProofLock,
) -> std::io::Result<Option<RecoveredPhysicalProof>> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join(LEASE))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // After the caller joined native supervision, a Quiescent receipt proves
    // the helper finished. A concurrent fork can still temporarily retain a
    // CLOEXEC duplicate until exec. Await that exact descriptor release, then
    // re-read and validate under the lock; no PID/elapsed-time inference is used.
    let await_release = match locking {
        ProofLock::Try => false,
        ProofLock::AfterSupervision => read_receipt(&path.join(RECEIPT))?.is_some_and(|receipt| {
            receipt.activation == *activation
                && receipt.conversation == *conversation
                && receipt.phase == Phase::Quiescent
        }),
        #[cfg(test)]
        ProofLock::AwaitRelease => true,
    };
    if await_release {
        file.lock()?;
    } else {
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    let Some(mut receipt) = read_receipt(&path.join(RECEIPT))? else {
        return Ok(None);
    };
    if receipt.activation != *activation || receipt.conversation != *conversation {
        return Err(std::io::Error::other(
            "physical authority identity mismatch",
        ));
    }
    match receipt.phase {
        Phase::Running => return Ok(None),
        Phase::Unstarted => {
            // The inherited lease excludes *all* potential writers. Seal this
            // exact activation before publishing the no-work proof.
            receipt.phase = Phase::Quiescent;
            write_receipt(path, &receipt)?;
        }
        Phase::Quiescent => {}
    }
    Ok(Some(RecoveredPhysicalProof { locks: vec![file] }))
}

fn evidence_path(
    product: &ProductRoot,
    session: &SessionId,
    conversation: &ConversationId,
    activation: &SubagentId,
) -> std::io::Result<PathBuf> {
    let store = super::child_conversation_store_path(product.root(), session, conversation);
    product.confined(
        &store
            .parent()
            .expect("conversation parent")
            .join(EVIDENCE)
            .join(activation.as_str()),
    )
}

/// All positive activation allocations in the owning Session participate in
/// identity consumption, including a fresh child Conversation whose parent died
/// before ownership was committed. No missing receipt implies settlement.
pub(crate) fn consumed_session_allocations(
    product: &ProductRoot,
    session: &SessionId,
) -> std::io::Result<Vec<(SubagentId, ConversationId)>> {
    let directory = product.confined(
        &product
            .root()
            .join("sessions")
            .join(session.as_str())
            .join("conversations"),
    )?;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let conversation =
                ConversationId::new(entry.file_name().to_string_lossy().into_owned());
            ids.extend(
                consumed_activation_ids(product, session, &conversation)?
                    .into_iter()
                    .map(|activation| (activation, conversation.clone())),
            );
        }
    }
    Ok(ids)
}

/// Activation names in this durable allocation namespace are consumed even if
/// the parent died before Reserved reached `SQLite`. The registry reseeds from
/// these positive facts, never from an absent process or receipt.
pub(crate) fn consumed_activation_ids(
    product: &ProductRoot,
    session: &SessionId,
    conversation: &ConversationId,
) -> std::io::Result<Vec<SubagentId>> {
    let store = super::child_conversation_store_path(product.root(), session, conversation);
    let directory =
        product.confined(&store.parent().expect("conversation parent").join(EVIDENCE))?;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let identity = name
                .strip_prefix(PRIVATE)
                .or_else(|| name.strip_prefix(ABANDONED))
                .unwrap_or(&name);
            ids.push(SubagentId::new(identity));
        }
    }
    Ok(ids)
}

fn write_receipt(path: &Path, receipt: &Receipt) -> std::io::Result<()> {
    let temporary = path.join("physical-settlement.pending");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(receipt)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path.join(RECEIPT))?;
    File::open(path)?.sync_all()
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

#[cfg(test)]
mod inheritance_tests {
    use super::*;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    fn identities() -> (SessionId, ConversationId, SubagentId) {
        (
            SessionId::new("ses_01900000-0000-7000-8000-000000000001"),
            ConversationId::new("conv_01900000-0000-7000-8000-000000000002"),
            SubagentId::new("activation:1"),
        )
    }

    #[test]
    fn live_private_initializer_excludes_recovery_at_every_publication_boundary() {
        for boundary in [
            AllocationBoundary::Created,
            AllocationBoundary::LeaseCreated,
            AllocationBoundary::Initialized,
            AllocationBoundary::Published,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let product = ProductRoot::create(directory.path()).unwrap();
            let (session, conversation, activation) = identities();
            let owner = ParentPhysicalLease::reserve_at_boundaries(
                &product,
                &session,
                &conversation,
                &activation,
                |at| {
                    if at == boundary {
                        // Synchronous injection runs while the actual initializer
                        // retains its lock, before it may return any spawn handle.
                        assert!(prove(&product, &session, &conversation, &activation)?.is_none());
                        assert_eq!(
                            consumed_activation_ids(&product, &session, &conversation)?,
                            vec![activation.clone()]
                        );
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert!(
                prove(&product, &session, &conversation, &activation)
                    .unwrap()
                    .is_none()
            );
            drop(owner);
            assert!(
                prove_after_release(&product, &session, &conversation, &activation)
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn published_initialization_error_never_excuses_missing_or_corrupt_evidence() {
        for damage in [LEASE, RECEIPT, "corrupt", "mismatched"] {
            let directory = tempfile::tempdir().unwrap();
            let product = ProductRoot::create(directory.path()).unwrap();
            let (session, conversation, activation) = identities();
            let path = evidence_path(&product, &session, &conversation, &activation).unwrap();
            assert!(
                ParentPhysicalLease::reserve_at_boundaries(
                    &product,
                    &session,
                    &conversation,
                    &activation,
                    |at| {
                        if at != AllocationBoundary::Published {
                            return Ok(());
                        }
                        match damage {
                            LEASE | RECEIPT => std::fs::remove_file(path.join(damage))?,
                            "corrupt" => std::fs::write(path.join(RECEIPT), b"invalid")?,
                            _ => write_receipt(
                                &path,
                                &Receipt {
                                    activation: SubagentId::new("wrong"),
                                    conversation: conversation.clone(),
                                    phase: Phase::Unstarted,
                                },
                            )?,
                        }
                        Err(std::io::Error::other("error after publication"))
                    }
                )
                .is_err()
            );
            assert!(!matches!(
                prove_after_release(&product, &session, &conversation, &activation),
                Ok(Some(_))
            ));
            assert!(
                ParentPhysicalLease::reserve(&product, &session, &conversation, &activation)
                    .is_err()
            );
        }
    }

    #[test]
    fn child_quiescence_does_not_acknowledge_a_parent_continuation() {
        let directory = tempfile::tempdir().unwrap();
        let product = ProductRoot::create(directory.path()).unwrap();
        let (session, conversation, activation) = identities();
        let parent =
            ParentPhysicalLease::reserve(&product, &session, &conversation, &activation).unwrap();
        parent.publish_quiescent().unwrap();
        let continuation = parent.reserve_continuation().unwrap();
        let spec = continuation.spec();
        let child = ChildPhysicalLease::acquire_inherited(
            spec.path,
            continuation.inherited_file().unwrap(),
            activation.clone(),
            conversation.clone(),
        )
        .unwrap();
        drop(continuation);
        drop(parent);
        assert!(
            prove(&product, &session, &conversation, &activation)
                .unwrap()
                .is_none()
        );
        child.publish_quiescent().unwrap();
        // Even a durable helper receipt is not consumable until its inherited
        // authority has closed: no next resource can race the snapshot.
        assert!(
            prove(&product, &session, &conversation, &activation)
                .unwrap()
                .is_none()
        );
        drop(child);
        assert!(
            prove_after_release(&product, &session, &conversation, &activation)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn abandoned_running_continuation_cannot_borrow_child_quiescence() {
        let directory = tempfile::tempdir().unwrap();
        let product = ProductRoot::create(directory.path()).unwrap();
        let (session, conversation, activation) = identities();
        let parent =
            ParentPhysicalLease::reserve(&product, &session, &conversation, &activation).unwrap();
        parent.publish_quiescent().unwrap();
        let continuation = parent.reserve_continuation().unwrap();
        let spec = continuation.spec();
        let child = ChildPhysicalLease::acquire_inherited(
            spec.path,
            continuation.inherited_file().unwrap(),
            activation.clone(),
            conversation.clone(),
        )
        .unwrap();
        drop(child);
        drop(continuation);
        drop(parent);
        assert!(
            prove(&product, &session, &conversation, &activation)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn unstarted_authority_excludes_recovery_until_inherited_child_descriptor_closes() {
        let directory = tempfile::tempdir().unwrap();
        let product = ProductRoot::create(directory.path()).unwrap();
        let (session, conversation, activation) = identities();
        let parent =
            ParentPhysicalLease::reserve(&product, &session, &conversation, &activation).unwrap();
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", "printf 'inherited\\n'; read release"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(parent.inherited_file().unwrap()))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(
            lines.next_line().await.unwrap().as_deref(),
            Some("inherited")
        );
        // The real child has exec'd and is parked reading its stdin. The
        // parent's final descriptor can close, but the inherited description
        // still excludes recovery even before the child enters Running.
        drop(parent);
        assert!(
            prove(&product, &session, &conversation, &activation)
                .unwrap()
                .is_none()
        );
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        assert!(child.wait().await.unwrap().success());
        let proof = prove_after_release(&product, &session, &conversation, &activation)
            .unwrap()
            .unwrap();
        let path = evidence_path(&product, &session, &conversation, &activation).unwrap();
        assert_eq!(
            read_receipt(&path.join(RECEIPT)).unwrap().unwrap().phase,
            Phase::Quiescent
        );
        drop(proof);
        // Cleanup acts only on the mutable incarnation; the explicit sealed
        // evidence and consumed activation identity remain durable authority.
        let conversation_root = path.parent().unwrap().parent().unwrap();
        let incarnation = conversation_root.join("incarnation-disposable");
        std::fs::create_dir(&incarnation).unwrap();
        std::fs::remove_dir(&incarnation).unwrap();
        assert!(
            prove_after_release(&product, &session, &conversation, &activation)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            consumed_activation_ids(&product, &session, &conversation).unwrap(),
            vec![activation.clone()]
        );
        assert!(
            ParentPhysicalLease::reserve(&product, &session, &conversation, &activation).is_err()
        );
    }

    #[test]
    fn released_running_authority_is_never_mistaken_for_a_quiescent_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let product = ProductRoot::create(directory.path()).unwrap();
        let (session, conversation, activation) = identities();
        assert!(
            prove(&product, &session, &conversation, &activation)
                .unwrap()
                .is_none()
        );
        let parent =
            ParentPhysicalLease::reserve(&product, &session, &conversation, &activation).unwrap();
        let path = evidence_path(&product, &session, &conversation, &activation).unwrap();
        let child = ChildPhysicalLease::acquire_inherited(
            path.clone(),
            parent.inherited_file().unwrap(),
            activation.clone(),
            conversation.clone(),
        )
        .unwrap();
        drop(parent);
        drop(child); // abrupt owner loss; deliberately no native settlement
        assert!(
            prove(&product, &session, &conversation, &activation)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            read_receipt(&path.join(RECEIPT)).unwrap().unwrap().phase,
            Phase::Running
        );
    }
}
