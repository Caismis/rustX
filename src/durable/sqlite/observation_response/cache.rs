//! Bounded, disposable proofs of completed native response reads. The native
//! cut and current-prefix holes are checked outside this cache on every read.
use super::{
    ObservationReadCut, ResponsePageItem, ResponsePageSelection, ResponsePageSelectionKind,
};
use crate::durable::sqlite::statements::FixedStatements;
use crate::durable::sqlite::{Connection, ConversationStoreError, storage};
use rusqlite::{TransactionState, hooks::PreUpdateCase, types::ValueRef};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const CAPACITY: usize = 32;
const ITEM_BUDGET: usize = 16 * 1024;
use super::super::observation::TRANSCRIPT_ROW_PROOF_BYTES;

#[derive(Clone, Copy, Default)]
pub(super) struct Dependencies {
    pub journal: u64,
    pub ledger: u64,
    pub transcript: u64,
    pub execution_from: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Versions {
    data: u64,
    main: u64,
    temp: u64,
}
impl Versions {
    fn read(connection: &Connection) -> Result<Self, ConversationStoreError> {
        let read = |sql| {
            connection
                .query_row_cached(sql, [], |row| row.get(0))
                .map_err(|error| storage(error.to_string()))
        };
        Ok(Self {
            data: read("PRAGMA main.data_version")?,
            main: read("PRAGMA main.schema_version")?,
            temp: read("PRAGMA temp.schema_version")?,
        })
    }
}

struct Entry {
    selection: ResponsePageSelection,
    cut: ObservationReadCut,
    item: Arc<ResponsePageItem>,
    execution_from: Option<u64>,
}
pub(super) struct Hit {
    pub item: Arc<ResponsePageItem>,
    pub execution_from: Option<u64>,
}
struct Row {
    header: super::super::observation::TranscriptRowProof,
    response: Option<Arc<ResponsePageItem>>,
    prepared: crate::runtime_client::checkpoint::PreparedTranscriptRow,
}
#[derive(Default)]
struct State {
    versions: Option<Versions>,
    epoch: u64,
    entries: VecDeque<Entry>,
    rows: VecDeque<Row>,
}

pub(in crate::durable::sqlite) struct Owner {
    epoch: AtomicU64,
    journal: AtomicU64,
    ledger: AtomicU64,
    transcript: AtomicU64,
    identity_fields: AtomicBool,
    installed_schema: (u64, u64),
    columns: BTreeMap<&'static str, Vec<(i32, Domain)>>,
    state: Mutex<State>,
}
#[derive(Clone, Copy)]
enum Domain {
    Journal,
    Ledger,
    Transcript,
}

impl Owner {
    pub(in crate::durable::sqlite) fn new(
        connection: &Connection,
    ) -> Result<Arc<Self>, ConversationStoreError> {
        let mut columns = BTreeMap::new();
        for (table, wanted) in [
            (
                "events",
                vec![
                    ("sequence", Domain::Journal),
                    ("observation_execution_owner", Domain::Journal),
                    ("observation_member_anchor", Domain::Journal),
                    ("observation_assistant_ledger", Domain::Ledger),
                    ("observation_transcript", Domain::Transcript),
                ],
            ),
            (
                "message_ledger",
                vec![
                    ("position", Domain::Ledger),
                    ("native_transcript", Domain::Transcript),
                ],
            ),
            (
                "observation_headers",
                vec![
                    ("ledger_position", Domain::Ledger),
                    ("transcript_position", Domain::Transcript),
                ],
            ),
            (
                "canonical_tool_calls",
                vec![
                    ("observation_ledger", Domain::Ledger),
                    ("result_ledger_position", Domain::Ledger),
                ],
            ),
            (
                "request_snapshots",
                vec![("started_sequence", Domain::Journal)],
            ),
            ("transcript_order", vec![("position", Domain::Transcript)]),
        ] {
            // xinfo preserves SQLite's logical column ordinals across virtual columns.
            let mut statement = connection
                .prepare(&format!("PRAGMA main.table_xinfo({table})"))
                .map_err(|error| storage(error.to_string()))?;
            let actual = statement
                .query_map([], |row| {
                    Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| storage(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| storage(error.to_string()))?;
            let selected = wanted
                .iter()
                .map(|(name, domain)| {
                    actual
                        .iter()
                        .find(|(_, candidate)| candidate == name)
                        .map(|(index, _)| (*index, *domain))
                        .ok_or_else(|| storage("response dependency column unavailable"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            columns.insert(table, selected);
        }
        let versions = Versions::read(connection)?;
        Ok(Arc::new(Self {
            epoch: AtomicU64::new(0),
            journal: AtomicU64::new(0),
            ledger: AtomicU64::new(0),
            transcript: AtomicU64::new(0),
            identity_fields: AtomicBool::new(false),
            installed_schema: (versions.main, versions.temp),
            columns,
            state: Mutex::new(State::default()),
        }))
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn uninstalled() -> Arc<Self> {
        Arc::new(Self {
            epoch: AtomicU64::new(u64::MAX),
            journal: AtomicU64::new(0),
            ledger: AtomicU64::new(0),
            transcript: AtomicU64::new(0),
            identity_fields: AtomicBool::new(false),
            installed_schema: (0, 0),
            columns: BTreeMap::new(),
            state: Mutex::new(State::default()),
        })
    }
    pub(in crate::durable::sqlite) fn invalidate(&self) {
        let _ = self
            .epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                Some(epoch.saturating_add(1))
            });
    }
    fn relevant(&self, value: &rusqlite::Result<ValueRef<'_>>, domain: Domain) -> bool {
        let frontier = match domain {
            Domain::Journal => &self.journal,
            Domain::Ledger => &self.ledger,
            Domain::Transcript => &self.transcript,
        }
        .load(Ordering::Relaxed);
        match value {
            Ok(ValueRef::Null) => false,
            Ok(ValueRef::Integer(value)) => {
                u64::try_from(*value).map_or(true, |value| value <= frontier)
            }
            _ => true,
        }
    }
    pub(in crate::durable::sqlite) fn observe(&self, table: &str, change: &PreUpdateCase) {
        let invalidates = if let Some(columns) = self.columns.get(table) {
            columns.iter().any(|(column, domain)| match change {
                PreUpdateCase::Insert(new) => {
                    self.relevant(&new.get_new_column_value(*column), *domain)
                }
                PreUpdateCase::Delete(old) => {
                    self.relevant(&old.get_old_column_value(*column), *domain)
                }
                PreUpdateCase::Update {
                    old_value_accessor: old,
                    new_value_accessor: new,
                } => {
                    self.relevant(&old.get_old_column_value(*column), *domain)
                        || self.relevant(&new.get_new_column_value(*column), *domain)
                }
                PreUpdateCase::Unknown => true,
            })
        } else {
            // Inherited proof tables are initialized once. Response identity
            // frontiers are small optional descriptors; do not retain a cached
            // descriptor across any content-frontier rewrite. Ordinary reads
            // do not consult them, so this broad rule only applies when needed.
            table.starts_with("inherited_")
                || (table == "content_frontiers" && self.identity_fields.load(Ordering::Relaxed))
        };
        if invalidates {
            self.invalidate();
        }
    }
    pub(super) fn capture(
        &self,
        validated: &super::super::observation::ValidatedCut<'_, '_>,
    ) -> Result<Option<Snapshot>, ConversationStoreError> {
        let connection = validated.connection()?;
        let cut = validated.expected();
        // No provisional write-transaction evidence enters the cache. A later
        // write/rollback invalidates through preupdate; read-only rollback is harmless.
        if connection
            .transaction_state(Some("main"))
            .map_err(|error| storage(error.to_string()))?
            != TransactionState::Read
        {
            return Ok(None);
        }
        let versions = Versions::read(connection)?;
        let epoch = self.epoch.load(Ordering::Acquire);
        let mut state = self
            .state
            .lock()
            .map_err(|_| storage("response proof lock poisoned"))?;
        if state.versions != Some(versions) || state.epoch != epoch {
            state.entries.clear();
            // Row bodies are computation results, never read authorization.
            // Reuse still requires exact, freshly read descriptor/Tool inputs;
            // changed response epochs cannot authorize a complete old row.
            state.versions = Some(versions);
            state.epoch = epoch;
            self.journal.store(0, Ordering::Relaxed);
            self.ledger.store(0, Ordering::Relaxed);
            self.transcript.store(0, Ordering::Relaxed);
            self.identity_fields.store(false, Ordering::Relaxed);
        }
        drop(state);
        if epoch == u64::MAX || (versions.main, versions.temp) != self.installed_schema {
            return Ok(None);
        }
        let actual = validated.actual();
        // A historical read made after later appends cannot certify their absence.
        Ok(Some(Snapshot {
            epoch,
            admit: (
                actual.read.journal,
                actual.ledger_through,
                actual.read.transcript,
            ) == (cut.read.journal, cut.ledger_through, cut.read.transcript),
        }))
    }
    pub(super) fn get(
        &self,
        snapshot: Snapshot,
        cut: &ObservationReadCut,
        selection: &ResponsePageSelection,
    ) -> Result<Option<Hit>, ConversationStoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| storage("response proof lock poisoned"))?;
        if state.epoch != snapshot.epoch {
            return Ok(None);
        }
        Ok(state
            .entries
            .iter()
            .find(|entry| {
                same_selection(&entry.selection, selection)
                    // Cached seats have native Ledger/terminal authority; no
                    // pending body or count is cached. The caller still validates
                    // the supplied cut, including its pending mutation epoch.
                    && entry.cut.read.conversation_id == cut.read.conversation_id
                    && entry.cut.read.journal <= cut.read.journal
                    && entry.cut.read.transcript <= cut.read.transcript
                    && entry.cut.ledger_through <= cut.ledger_through
                    && entry.cut.surface_revision <= cut.surface_revision
                    && entry.cut.inherited_through == cut.inherited_through
            })
            .map(|entry| Hit {
                item: entry.item.clone(),
                execution_from: entry.execution_from,
            }))
    }
    pub(super) fn insert(
        &self,
        snapshot: Snapshot,
        cut: &ObservationReadCut,
        selection: &ResponsePageSelection,
        item: &ResponsePageItem,
        dependencies: Dependencies,
    ) -> Result<Option<Arc<ResponsePageItem>>, ConversationStoreError> {
        if !snapshot.admit
            || cut.read.conversation_id.as_str().len() > 512
            || !bounded_selection(selection)
            || crate::runtime_client::preflight::bounded_fit(
                &(&item.process, &item.response, &item.identity_fields),
                ITEM_BUDGET,
            ) != Some(true)
        {
            return Ok(None);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| storage("response proof lock poisoned"))?;
        if state.epoch != snapshot.epoch || self.epoch.load(Ordering::Acquire) != snapshot.epoch {
            return Ok(None);
        }
        state
            .entries
            .retain(|entry| !same_selection(&entry.selection, selection));
        if state.entries.len() == CAPACITY {
            state.entries.pop_front();
        }
        self.journal
            .fetch_max(dependencies.journal, Ordering::Relaxed);
        self.ledger
            .fetch_max(dependencies.ledger, Ordering::Relaxed);
        self.transcript
            .fetch_max(dependencies.transcript, Ordering::Relaxed);
        if matches!(selection.kind, ResponsePageSelectionKind::Deferred(_))
            || item
                .response
                .as_ref()
                .is_some_and(|response| !response.deferred_fields.is_empty())
        {
            self.identity_fields.store(true, Ordering::Relaxed);
        }
        // Only admitted, authenticated facts acquire a shared owner. Rejected
        // items remain owned by the caller and move directly into its page.
        #[cfg(test)]
        super::OWNERSHIP_WORK.with(|work| {
            let (owners, clones, moves) = work.get();
            work.set((owners + 1, clones + 1, moves));
        });
        let item = Arc::new(item.clone());
        state.entries.push_back(Entry {
            selection: selection.clone(),
            cut: cut.clone(),
            item: item.clone(),
            execution_from: dependencies.execution_from,
        });
        Ok(Some(item))
    }
    /// Exact freshly read descriptor bytes reuse only the decoded message.
    /// Tool reads and response authorization still run independently afterward;
    /// epoch changes never authorize a prior complete decoration here.
    pub(in crate::durable::sqlite) fn transcript_descriptor(
        &self,
        source: &super::super::observation::TranscriptDescriptorSource<'_>,
    ) -> Result<
        Option<(
            Arc<super::super::observation::TranscriptDescriptor>,
            crate::runtime_client::checkpoint::PreparedTranscriptRow,
        )>,
        ConversationStoreError,
    > {
        let state = self
            .state
            .lock()
            .map_err(|_| storage("response proof lock poisoned"))?;
        Ok(state
            .rows
            .iter()
            .find(|row| row.header.matches_descriptor(source))
            .map(|row| (row.header.descriptor(), row.prepared.clone())))
    }
    /// Exact current descriptor and Tool bytes authorize body reuse only.
    /// Reusing the complete row additionally requires the authenticated response
    /// owner. Running/delegated reads keep current decoration and a fresh row.
    pub(in crate::durable::sqlite) fn transcript_row(
        &self,
        conversation: &crate::runtime::identity::ConversationId,
        value: &crate::runtime_client::snapshot::RuntimeClientTranscriptEntry,
        header: Option<super::super::observation::TranscriptRowProof>,
        response: Option<&Arc<ResponsePageItem>>,
    ) -> Result<crate::runtime_client::checkpoint::PreparedTranscriptRow, ConversationStoreError>
    {
        use crate::runtime_client::checkpoint::PreparedTranscriptRow;
        let Some(header) = header else {
            #[cfg(test)]
            super::record_row_work(false);
            return Ok(PreparedTranscriptRow::new(
                conversation.clone(),
                value.clone(),
            ));
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| storage("response proof lock poisoned"))?;
        let response = response.filter(|response| {
            state.epoch == self.epoch.load(Ordering::Acquire)
                && state
                    .entries
                    .iter()
                    .any(|entry| Arc::ptr_eq(&entry.item, response))
        });
        let prior = state.rows.iter().find(|row| row.header == header);
        if let Some(row) = prior
            && row
                .response
                .as_ref()
                .zip(response)
                .is_some_and(|(old, current)| Arc::ptr_eq(old, current))
        {
            #[cfg(test)]
            super::record_row_work(true);
            return Ok(row.prepared.clone());
        }
        #[cfg(test)]
        super::record_row_work(false);
        let prepared = prior.map_or_else(
            || PreparedTranscriptRow::new(conversation.clone(), value.clone()),
            |row| row.prepared.with_decoration(value),
        );
        let bytes = header.retained_bytes();
        if bytes > TRANSCRIPT_ROW_PROOF_BYTES {
            return Ok(prepared);
        }
        state
            .rows
            .retain(|row| row.header.cursor() != header.cursor());
        let mut retained = state
            .rows
            .iter()
            .map(|row| row.header.retained_bytes())
            .sum::<usize>();
        while state.rows.len() == CAPACITY || retained + bytes > TRANSCRIPT_ROW_PROOF_BYTES {
            let removed = state.rows.pop_front().expect("bounded row proof eviction");
            retained -= removed.header.retained_bytes();
        }
        state.rows.push_back(Row {
            header,
            response: response.cloned(),
            prepared: prepared.clone(),
        });
        Ok(prepared)
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn len(&self) -> usize {
        self.state.lock().unwrap().entries.len()
    }
    #[cfg(test)]
    pub(in crate::durable::sqlite) fn row_retention(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (
            state.rows.len(),
            state
                .rows
                .iter()
                .map(|row| row.header.retained_bytes())
                .sum(),
        )
    }
}

#[derive(Clone, Copy)]
pub(super) struct Snapshot {
    epoch: u64,
    admit: bool,
}

fn same_selection(a: &ResponsePageSelection, b: &ResponsePageSelection) -> bool {
    a.cursor == b.cursor
        && match (&a.kind, &b.kind) {
            (ResponsePageSelectionKind::Process, ResponsePageSelectionKind::Process) => true,
            (ResponsePageSelectionKind::Message(a), ResponsePageSelectionKind::Message(b)) => {
                a == b
            }
            (ResponsePageSelectionKind::Deferred(a), ResponsePageSelectionKind::Deferred(b)) => {
                a == b
            }
            _ => false,
        }
}
fn bounded_selection(selection: &ResponsePageSelection) -> bool {
    match &selection.kind {
        ResponsePageSelectionKind::Process => true,
        ResponsePageSelectionKind::Message(id) => id.as_str().len() <= 256,
        ResponsePageSelectionKind::Deferred(value) => {
            crate::runtime_client::preflight::bounded_fit(value, 512) == Some(true)
        }
    }
}
