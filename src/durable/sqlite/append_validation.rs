//! Disposable connection-local evidence for the existing canonical validator.
//! Never an authority: reads, duplicate checks and head publication checks stay
//! native. A batch owns provisional evidence; only successful COMMIT seats it.
use super::statements::FixedStatements;
use super::{
    Arc, AtomicU64, Connection, ConversationStoreError, MessageBlock, MessageId, Mutex, Ordering,
    SurfaceRevision, Transaction, TranscriptCursor, head_frontier, materialize_head, nonnegative,
    storage,
};
use rusqlite::hooks::PreUpdateCase;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Versions {
    data: u64,
    main_schema: u64,
    temp_schema: u64,
}
impl Versions {
    fn read(connection: &Connection) -> Result<Self, ConversationStoreError> {
        let read = |sql| {
            connection
                .query_row_cached(sql, [], |row| row.get(0))
                .map_err(|error| storage(format!("append validation version: {error}")))
        };
        Ok(Self {
            data: read("PRAGMA main.data_version")?,
            main_schema: read("PRAGMA main.schema_version")?,
            temp_schema: read("PRAGMA temp.schema_version")?,
        })
    }
}
#[derive(Clone, Copy)]
struct Certificate {
    versions: Versions,
    epoch: u64,
    revision: SurfaceRevision,
    generation: u64,
    count: usize,
}
pub(super) struct Owner {
    epoch: AtomicU64,
    installed_schema: u64,
    identity_columns: Option<[i32; 2]>,
    committed: Mutex<Option<Certificate>>,
}
impl Owner {
    pub(super) fn new(connection: &Connection) -> Result<Arc<Self>, ConversationStoreError> {
        let mut statement = connection.prepare("SELECT cid,name FROM pragma_table_xinfo('message_ledger') WHERE name IN ('position','message_id') ORDER BY name")
            .map_err(|error| storage(error.to_string()))?;
        let columns = statement
            .query_map([], |row| row.get::<_, i32>(0))
            .map_err(|error| storage(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(error.to_string()))?;
        let identity_columns = columns
            .try_into()
            .map_err(|_| storage("append identity columns unavailable"))?;
        Ok(Arc::new(Self {
            epoch: AtomicU64::new(0),
            installed_schema: Versions::read(connection)?.main_schema,
            identity_columns: Some(identity_columns),
            committed: Mutex::new(None),
        }))
    }
    #[cfg(test)]
    pub(super) fn uninstalled() -> Arc<Self> {
        Arc::new(Self {
            epoch: AtomicU64::new(0),
            installed_schema: 0,
            identity_columns: None,
            committed: Mutex::new(None),
        })
    }
    #[cfg(test)]
    pub(super) fn exhaust_epoch(&self) {
        self.epoch.store(u64::MAX, Ordering::Release);
    }
    #[cfg(test)]
    pub(super) fn has_certificate(&self) -> bool {
        self.committed.lock().unwrap().is_some()
    }
    pub(super) fn invalidate(&self) {
        self.invalidate_by(1);
    }
    fn invalidate_by(&self, increment: u64) {
        // Saturation permanently disables reuse; never wrap to an old proof.
        let _ = self
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                Some(n.saturating_add(increment))
            });
    }
    pub(super) fn observe(&self, table: &str, change: &PreUpdateCase) {
        let invalidates = match (table, change) {
            (
                "message_ledger",
                PreUpdateCase::Update {
                    old_value_accessor: old,
                    new_value_accessor: new,
                },
            ) => self.identity_columns.is_none_or(|columns| {
                columns.iter().any(|column| {
                    match (
                        old.get_old_column_value(*column),
                        new.get_new_column_value(*column),
                    ) {
                        (Ok(a), Ok(b)) => a != b,
                        _ => true,
                    }
                })
            }),
            (_, PreUpdateCase::Unknown)
            | (
                "message_ledger"
                | "surface_head"
                | "projection_head"
                | "projection_checkpoints"
                | "projection_activations",
                _,
            ) => true,
            _ => false,
        };
        if invalidates {
            let direct = match change {
                PreUpdateCase::Delete(old) => old.get_query_depth() == 0,
                PreUpdateCase::Insert(new)
                | PreUpdateCase::Update {
                    new_value_accessor: new,
                    ..
                } => new.get_query_depth() == 0,
                PreUpdateCase::Unknown => false,
            };
            // A nested/unknown mutation must exceed the native writer's two
            // expected mutations even if a trigger suppresses a native write.
            self.invalidate_by(if direct { 1 } else { 3 });
        }
    }
}

pub(super) struct Batch<'a> {
    owner: &'a Owner,
    proof: Option<Certificate>,
}
impl<'a> Batch<'a> {
    pub(super) fn begin(owner: &'a Owner) -> Self {
        // Removing the seat means every early return/failed commit drops it.
        let proof = owner.committed.lock().ok().and_then(|mut seat| seat.take());
        Self { owner, proof }
    }
    fn head(
        &mut self,
        transaction: &Transaction<'_>,
    ) -> Result<(SurfaceRevision, u64), ConversationStoreError> {
        // Both version checks and validation refer to this pinned transaction.
        // Recheck for DDL or unrecognized mutations between batch appends too.
        let versions = Versions::read(transaction)?;
        let epoch = self.owner.epoch.load(Ordering::Acquire);
        let (ledger, checkpoint) = head_frontier(transaction)?;
        let revision = SurfaceRevision::new(nonnegative(ledger, "canonical frontier")?);
        let generation = checkpoint
            .as_ref()
            .map_or(0, |head| head.activation.activation.0);
        let reusable = self.owner.identity_columns.is_some()
            && versions.main_schema == self.owner.installed_schema
            && epoch != u64::MAX;
        let count = if let Some(proof) = self.proof.filter(|proof| {
            reusable
                && proof.versions == versions
                && proof.epoch == epoch
                && proof.revision == revision
                && proof.generation == generation
        }) {
            if proof.count > 8192 {
                return Err(crate::context::resources::bound(
                    "canonical working set exceeds bounded projection size",
                ));
            }
            #[cfg(test)]
            WORK.with(|work| {
                let (cold, warm) = work.get();
                work.set((cold, warm + 1));
            });
            proof.count
        } else {
            #[cfg(test)]
            WORK.with(|work| {
                let (cold, warm) = work.get();
                work.set((cold + 1, warm));
            });
            if checkpoint.is_some() {
                materialize_head(transaction, ledger, checkpoint)?
                    .active_message_ids
                    .len()
            } else {
                super::validate_append_identities(transaction, ledger)?
            }
        };
        self.proof = reusable.then_some(Certificate {
            versions,
            epoch,
            revision,
            generation,
            count,
        });
        Ok((revision, generation))
    }
    pub(super) fn append(
        &mut self,
        transaction: &Transaction<'_>,
        message: &MessageBlock,
        allowed_pending: Option<&MessageId>,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        self.append_with_capture(transaction, message, allowed_pending, None)
    }
    pub(super) fn append_publication(
        &mut self,
        transaction: &Transaction<'_>,
        message: &MessageBlock,
        header: &mut Option<super::observation::PublicationHeader>,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        self.append_with_capture(transaction, message, None, Some(header))
    }
    fn append_with_capture(
        &mut self,
        transaction: &Transaction<'_>,
        message: &MessageBlock,
        allowed_pending: Option<&MessageId>,
        publication_header: Option<&mut Option<super::observation::PublicationHeader>>,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let head = self.head(transaction)?;
        let cursor = super::append_message_with_head(
            transaction,
            message,
            allowed_pending,
            head,
            publication_header,
        )?;
        let epoch = self.owner.epoch.load(Ordering::Acquire);
        // The shared native writer inserts exactly one Ledger identity and
        // updates one Surface head. Other relevant writes (including triggers)
        // make this proof unusable. Ledger/Pending uniqueness checks still run.
        self.proof = self.proof.and_then(|mut proof| {
            if proof.epoch.checked_add(2) != Some(epoch)
                || epoch == u64::MAX
                || message.id().as_str().len() > 4096
            {
                return None;
            }
            // Preserve native admission timing for a new oversized identity:
            // it receives no certificate; the next append uses the old validator.
            proof.epoch = epoch;
            proof.revision = head.0.next();
            proof.count += 1;
            Some(proof)
        });
        Ok(cursor)
    }
    pub(super) fn committed(self, connection: &Connection) {
        // Call only after Transaction::commit succeeded. No fallible SQL after
        // acceptance, and no cached count can escape an open transaction.
        if !connection.is_autocommit() {
            return;
        }
        if let Some(proof) = self.proof
            && proof.epoch == self.owner.epoch.load(Ordering::Acquire)
            && let Ok(mut seat) = self.owner.committed.lock()
        {
            *seat = Some(proof);
        }
    }
}
#[cfg(test)]
thread_local! { static WORK: std::cell::Cell<(usize,usize)> = const {std::cell::Cell::new((0,0))}; }
#[cfg(test)]
pub(super) fn take_work() -> (usize, usize) {
    WORK.with(|work| work.replace((0, 0)))
}
