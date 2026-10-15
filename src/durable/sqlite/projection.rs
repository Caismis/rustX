//! The checkpoint transaction is the sole historical projection mutation boundary.
use super::{
    Connection, ConversationStoreError, OptionalExtension, Transaction, decode, encode,
    nonnegative, params, seq_to_i64, storage,
};
use crate::context::checkpoint::{
    CheckpointCandidate, CheckpointMessage, OriginActivation, ProjectionActivation,
    ProjectionActivationId, compile_checkpoint,
};
use crate::context::resources::{JournalFrontier, LedgerFrontier, ResourceLimits};
use crate::durable::projection::{
    ProjectionCheckpoint, ProjectionCommitInput, ProjectionCommitReceipt, ProjectionOwner,
};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::runtime::identity::{
    AttemptId, CheckpointId, CompactionOperationId, ConversationId, EventId,
};
use sha2::{Digest, Sha256};

fn invalid(message: impl Into<String>) -> ConversationStoreError {
    ConversationStoreError::InvalidReference(message.into())
}
fn digest(bytes: &str) -> String {
    format!("{:x}", Sha256::digest(bytes.as_bytes()))
}

pub(super) fn owner(
    connection: &Connection,
    expected: Option<&AttemptId>,
) -> Result<ProjectionOwner, ConversationStoreError> {
    let last: Option<(i64, String)> = connection.query_row(
        "SELECT sequence,event_json FROM events WHERE json_extract(event_json,'$.event.type') IN ('attempt_started','attempt_completed','attempt_failed','attempt_cancelled','attempt_timed_out','attempt_limit_exceeded') ORDER BY sequence DESC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional().map_err(|e| storage(e.to_string()))?;
    let (transition_sequence, active) = match last {
        None => (0, None),
        Some((sequence, json)) => {
            let event: RuntimeEventEnvelope = decode(&json, "projection execution fence")?;
            let active = match event.event {
                RuntimeEvent::AttemptStarted { attempt_id } => Some(attempt_id),
                _ => None,
            };
            (nonnegative(sequence, "execution fence")?, active)
        }
    };
    if active.as_ref() != expected {
        return Err(invalid("projection execution owner changed"));
    }
    Ok(ProjectionOwner {
        attempt_id: active,
        transition_sequence,
    })
}

#[cfg(test)]
thread_local! { static CANDIDATE_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn take_candidate_decodes() -> usize {
    CANDIDATE_DECODES.with(|n| n.replace(0))
}

#[cfg(test)]
thread_local! { static METADATA_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
fn take_metadata_decodes() -> usize {
    METADATA_DECODES.with(|n| n.replace(0))
}

fn candidate(
    connection: &Connection,
    id: &CheckpointId,
) -> Result<
    (
        CheckpointCandidate,
        String,
        Option<crate::context::occupancy::ContextOccupancy>,
    ),
    ConversationStoreError,
> {
    let row: Option<(String, Option<String>, Option<String>)> = connection.query_row(
        "SELECT candidate_fingerprint,CASE WHEN octet_length(candidate_json)<=8388608 THEN candidate_json END,CASE WHEN octet_length(occupancy_json)<=1048576 THEN occupancy_json END FROM projection_checkpoints WHERE checkpoint_id=?1", [id.as_str()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional().map_err(|e| storage(e.to_string()))?;
    let (fingerprint, json, occupancy) =
        row.ok_or_else(|| invalid(format!("missing checkpoint {id}")))?;
    let json = json.ok_or_else(|| invalid("oversized checkpoint"))?;
    let occupancy = occupancy.ok_or_else(|| invalid("oversized checkpoint occupancy"))?;
    #[cfg(test)]
    CANDIDATE_DECODES.with(|n| n.set(n.get() + 1));
    let value: CheckpointCandidate = decode(&json, "checkpoint content")?;
    value.validate_budget().map_err(invalid)?;
    if value.checkpoint_id != *id || value.fingerprint() != fingerprint {
        return Err(invalid("corrupted checkpoint content"));
    }
    Ok((
        value,
        fingerprint,
        decode(&occupancy, "checkpoint occupancy")?,
    ))
}

/// Small immutable publication coordinates, committed with the activation.
/// Integrity covers these coordinates independently of activation payload bytes.
struct EligibilityHeader {
    id: u64,
    checkpoint: String,
    ledger: u64,
    journal: u64,
    installation: Option<String>,
}
fn header_digest(header: &EligibilityHeader) -> Result<String, ConversationStoreError> {
    Ok(digest(&encode(
        &(
            header.id,
            &header.checkpoint,
            header.ledger,
            header.journal,
            &header.installation,
        ),
        "projection eligibility header",
    )?))
}
#[cfg(test)]
thread_local! {
    static HEADER_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ACTIVATION_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn take_header_work() -> (usize, usize) {
    (
        HEADER_READS.with(|n| n.replace(0)),
        ACTIVATION_DECODES.with(|n| n.replace(0)),
    )
}
fn eligibility_header(
    connection: &Connection,
    id: u64,
) -> Result<EligibilityHeader, ConversationStoreError> {
    type Row = (
        Option<String>,
        u64,
        u64,
        Option<String>,
        bool,
        Option<String>,
    );
    #[cfg(test)]
    HEADER_READS.with(|n| n.set(n.get() + 1));
    let row: Option<Row> = connection.query_row(
        "SELECT CASE WHEN octet_length(checkpoint_id)<=1024 THEN checkpoint_id END,ledger_frontier,visible_journal,CASE WHEN octet_length(installation_id)<=1024 THEN installation_id END,installation_id IS NOT NULL,CASE WHEN octet_length(header_digest)=64 THEN header_digest END FROM projection_activations WHERE activation=?1",
        [seq_to_i64(id)?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
    ).optional().map_err(|e|storage(e.to_string()))?;
    let (checkpoint, ledger, journal, installation, installed, integrity) =
        row.ok_or_else(|| invalid("missing historical projection predecessor"))?;
    let header = EligibilityHeader {
        id,
        checkpoint: checkpoint
            .ok_or_else(|| invalid("oversized projection checkpoint identity"))?,
        ledger,
        journal,
        installation,
    };
    if id == 0
        || header.checkpoint.is_empty()
        || installed != header.installation.is_some()
        || (installed && journal != 0)
        || (!installed && journal == 0)
        || Some(header_digest(&header)?) != integrity
    {
        return Err(invalid("corrupted projection eligibility header"));
    }
    Ok(header)
}
fn persist_activation(
    connection: &Connection,
    activation: &ProjectionActivation,
) -> Result<(), ConversationStoreError> {
    if activation.checkpoint_id.as_str().len() > 1024
        || activation
            .destination_installation
            .as_ref()
            .is_some_and(|id| id.as_str().len() > 1024)
    {
        return Err(invalid("projection header identity exceeds bound"));
    }
    let json = encode(activation, "projection activation")?;
    let header = EligibilityHeader {
        id: activation.activation.0,
        checkpoint: activation.checkpoint_id.to_string(),
        ledger: activation.ledger.0,
        journal: if activation.destination_installation.is_some() {
            0
        } else {
            activation.origin.completed_journal.0
        },
        installation: activation
            .destination_installation
            .as_ref()
            .map(ToString::to_string),
    };
    connection.execute(
        "INSERT INTO projection_activations(activation,checkpoint_id,ledger_frontier,visible_journal,installation_id,activation_json,activation_digest,header_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![seq_to_i64(header.id)?,header.checkpoint,seq_to_i64(header.ledger)?,seq_to_i64(header.journal)?,header.installation,json,digest(&json),header_digest(&header)?],
    ).map_err(|e|storage(e.to_string()))?;
    Ok(())
}
fn inherited_count_digest(history: &str, count: u64) -> String {
    digest(&format!(
        "rustx-installed-projection-prefix\n{history}\n{count}"
    ))
}
fn inherited_count(connection: &Connection) -> Result<u64, ConversationStoreError> {
    let row: Option<(Option<String>,u64,Option<String>)> = connection.query_row(
        "SELECT CASE WHEN octet_length(history_digest)=64 THEN history_digest END,projection_count,CASE WHEN octet_length(projection_digest)=64 THEN projection_digest END FROM bootstrap_identity WHERE id=1", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional().map_err(|e|storage(e.to_string()))?;
    let Some((history, count, integrity)) = row else {
        return Ok(0);
    };
    let history = history.ok_or_else(|| invalid("invalid installed history digest"))?;
    if integrity.as_deref() != Some(inherited_count_digest(&history, count).as_str()) {
        return Err(invalid("corrupted installed projection boundary"));
    }
    Ok(count)
}
fn completion_at(
    connection: &Connection,
    through: u64,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let json: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN octet_length(event_json)<=1048576 THEN event_json END FROM events INDEXED BY events_kind_idx WHERE json_extract(event_json,'$.event.type')='compaction_completed' AND sequence<=?1 ORDER BY sequence DESC LIMIT 1",
        [seq_to_i64(through)?], |r|r.get(0),
    ).optional().map_err(|e|storage(e.to_string()))?;
    json.map(|body| {
        let event: RuntimeEventEnvelope = decode(
            &body.ok_or_else(|| invalid("oversized projection completion fact"))?,
            "projection completion fact",
        )?;
        let local: bool = connection
            .query_row(
                "SELECT conversation_id=?1 FROM rustx_store WHERE id=1",
                [event.conversation_id.as_str()],
                |r| r.get(0),
            )
            .map_err(|e| storage(e.to_string()))?;
        if !local || event.schema_version != EVENT_SCHEMA_VERSION {
            return Err(invalid(
                "foreign or incompatible projection completion fact",
            ));
        }
        Ok(event)
    })
    .transpose()
}
/// The Journal and immutable installation count anchor visibility independently
/// of selector columns. Neither query reads future activation payloads.
fn journal_frontier(connection: &Connection, through: u64) -> Result<u64, ConversationStoreError> {
    let inherited = inherited_count(connection)?;
    let id = if let Some(event) = completion_at(connection, through)? {
        let RuntimeEvent::CompactionCompleted {
            activation_id,
            checkpoint_id,
            ..
        } = event.event
        else {
            return Err(invalid("invalid projection completion authority"));
        };
        if activation_id.0 <= inherited || event.sequence > through {
            return Err(invalid("inconsistent local projection publication"));
        }
        let header = eligibility_header(connection, activation_id.0)?;
        if header.journal != event.sequence
            || header.installation.is_some()
            || header.checkpoint != checkpoint_id.as_str()
            || event.schema_version != EVENT_SCHEMA_VERSION
        {
            return Err(invalid(
                "projection selector disagrees with committed completion",
            ));
        }
        activation_id.0
    } else {
        inherited
    };
    if id > 0 {
        let header = eligibility_header(connection, id)?;
        if id <= inherited && (header.journal != 0 || header.installation.is_none()) {
            return Err(invalid(
                "projection selector disagrees with inherited installation",
            ));
        }
    }
    let last: u64 = connection
        .query_row(
            "SELECT coalesce(max(activation),0) FROM projection_activations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if last > id {
        let next = eligibility_header(connection, id + 1)?;
        if next.journal <= through || next.installation.is_some() {
            return Err(invalid(
                "missing eligible projection completion or installation",
            ));
        }
    }
    Ok(id)
}
/// Monotone Ledger publication makes the eligible set one contiguous prefix.
/// Validate the indexed boundary and its successor, never the entire timeline.
fn selected_frontier(
    connection: &Connection,
    selection: Option<(LedgerFrontier, JournalFrontier, ProjectionActivationId)>,
) -> Result<u64, ConversationStoreError> {
    let (ledger, journal, activation) = selection.unwrap_or((
        LedgerFrontier(i64::MAX as u64),
        JournalFrontier(i64::MAX as u64),
        ProjectionActivationId(i64::MAX as u64),
    ));
    let journal_head = journal_frontier(connection, journal.0)?;
    let through = journal_head.min(activation.0);
    if through == 0 {
        return Ok(0);
    }
    let head = eligibility_header(connection, through)?;
    if head.ledger <= ledger.0 {
        return Ok(through);
    }
    let id: Option<u64> = connection.query_row(
        "SELECT activation FROM projection_activations INDEXED BY projection_ledger_idx WHERE ledger_frontier<=?1 ORDER BY ledger_frontier DESC,activation DESC LIMIT 1",
        [seq_to_i64(ledger.0)?],|r|r.get(0),
    ).optional().map_err(|e|storage(e.to_string()))?;
    let id = id.unwrap_or(0);
    if id >= through {
        return Err(invalid("nonmonotone projection Ledger eligibility"));
    }
    if id > 0 {
        eligibility_header(connection, id)?;
    }
    let next = eligibility_header(connection, id + 1)?;
    if next.ledger <= ledger.0 {
        return Err(invalid("hidden eligible projection predecessor"));
    }
    Ok(id)
}

pub(super) fn validate_activation_boundary(
    connection: &Connection,
    id: ProjectionActivationId,
    ledger: LedgerFrontier,
    journal: JournalFrontier,
) -> Result<(), ConversationStoreError> {
    if id.0 > 0 && selected_frontier(connection, Some((ledger, journal, id)))? != id.0 {
        return Err(invalid("incompatible historical projection boundary"));
    }
    Ok(())
}

fn activation_content(
    connection: &Connection,
    id: ProjectionActivationId,
) -> Result<ProjectionActivation, ConversationStoreError> {
    let header = eligibility_header(connection, id.0)?;
    let (json,payload_digest): (Option<String>,Option<String>) = connection.query_row(
        "SELECT CASE WHEN octet_length(activation_json)<=1048576 THEN activation_json END,CASE WHEN octet_length(activation_digest)=64 THEN activation_digest END FROM projection_activations WHERE activation=?1",[seq_to_i64(id.0)?],|r|Ok((r.get(0)?,r.get(1)?)),
    ).map_err(|e|storage(e.to_string()))?;
    let json = json.ok_or_else(|| invalid("oversized expected projection activation"))?;
    #[cfg(test)]
    ACTIVATION_DECODES.with(|n| n.set(n.get() + 1));
    let activation: ProjectionActivation = decode(&json, "projection activation")?;
    if Some(digest(&json)) != payload_digest
        || activation.activation != id
        || activation.checkpoint_id.as_str() != header.checkpoint
        || activation.ledger.0 != header.ledger
        || activation
            .destination_installation
            .as_ref()
            .map(ToString::to_string)
            != header.installation
        || (header.installation.is_none()
            && activation.origin.completed_journal.0 != header.journal)
    {
        return Err(invalid("corrupted projection activation coordinates"));
    }
    Ok(activation)
}
pub(super) fn activation_at(
    connection: &Connection,
    id: ProjectionActivationId,
) -> Result<ProjectionActivation, ConversationStoreError> {
    let activation = activation_content(connection, id)?;
    if activation.destination_installation.is_none() {
        let journal = activation.origin.completed_journal.0;
        let event = completion_at(connection, journal)?
            .ok_or_else(|| invalid("missing projection completion fact"))?;
        if event.sequence != journal
            || event.schema_version != EVENT_SCHEMA_VERSION
            || event.conversation_id != activation.origin.conversation_id
            || !matches!(event.event,RuntimeEvent::CompactionCompleted {activation_id,checkpoint_id,..} if activation_id==id && checkpoint_id==activation.checkpoint_id)
        {
            return Err(invalid(
                "activation disagrees with committed completion fact",
            ));
        }
    } else if id.0 > inherited_count(connection)? {
        return Err(invalid("activation is outside immutable inherited prefix"));
    }
    Ok(activation)
}
/// Write-side reference validation precedes insertion of this very completion
/// fact. It checks the same payload plus the transaction's allocated Journal slot;
/// read-side validation instead requires the already committed fact.
pub(super) fn completion_activation(
    connection: &Connection,
    id: &CheckpointId,
) -> Result<ProjectionActivation, ConversationStoreError> {
    let activation = activation_content(
        connection,
        ProjectionActivationId(activation_number(connection, id.as_str())?),
    )?;
    candidate(connection, id)?;
    let (next, scope): (u64, String) = connection
        .query_row(
            "SELECT next_event_sequence+1,conversation_id FROM rustx_store WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| storage(e.to_string()))?;
    if activation.destination_installation.is_some()
        || activation.origin.completed_journal.0 != next
        || activation.origin.conversation_id.as_str() != scope
    {
        return Err(invalid(
            "activation does not own the pending completion slot",
        ));
    }
    Ok(activation)
}

fn activated(
    connection: &Connection,
    activation: u64,
) -> Result<ProjectionCheckpoint, ConversationStoreError> {
    let activation = activation_at(connection, ProjectionActivationId(activation))?;
    let (candidate, fingerprint, occupancy) = candidate(connection, &activation.checkpoint_id)?;
    Ok(ProjectionCheckpoint {
        candidate,
        fingerprint,
        activation,
        occupancy,
    })
}
fn activation_number(connection: &Connection, id: &str) -> Result<u64, ConversationStoreError> {
    let n: i64 = connection
        .query_row(
            "SELECT activation FROM projection_activations WHERE checkpoint_id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    nonnegative(n, "projection activation")
}

pub(super) fn head(
    connection: &Connection,
) -> Result<Option<ProjectionCheckpoint>, ConversationStoreError> {
    let n: Option<i64> = connection
        .query_row(
            "SELECT activation FROM projection_head WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| storage(e.to_string()))?;
    if n.is_none() {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM projection_activations)",
                [],
                |r| r.get(0),
            )
            .map_err(|e| storage(e.to_string()))?;
        if exists {
            return Err(invalid("missing expected Projection Head"));
        }
    }
    n.map(|n| activated(connection, nonnegative(n, "projection head")?))
        .transpose()
}
pub(super) fn checkpoint(
    connection: &Connection,
    id: &CheckpointId,
) -> Result<ProjectionCheckpoint, ConversationStoreError> {
    activated(connection, activation_number(connection, id.as_str())?)
}
pub(super) fn timeline(
    connection: &Connection,
    after: ProjectionActivationId,
    limit: usize,
) -> Result<Vec<ProjectionCheckpoint>, ConversationStoreError> {
    timeline_page(
        connection,
        after,
        limit,
        None,
        ResourceLimits::default().total_bytes,
    )
}

pub(super) fn timeline_at_journal_cut(
    connection: &Connection,
    through: u64,
    after: ProjectionActivationId,
    limit: usize,
) -> Result<crate::durable::projection::ProjectionTimelinePage, ConversationStoreError> {
    let frontier = selected_frontier(
        connection,
        Some((
            LedgerFrontier(i64::MAX as u64),
            JournalFrontier(through),
            ProjectionActivationId(i64::MAX as u64),
        )),
    )?;
    let records = timeline_prefix(
        connection,
        after,
        limit,
        frontier,
        ResourceLimits::default().total_bytes,
    )?;
    let next_activation = records
        .last()
        .map(|p| p.activation.activation)
        .filter(|last| last.0 < frontier);
    Ok(crate::durable::projection::ProjectionTimelinePage {
        records,
        next_activation,
    })
}

pub(super) fn checkpoint_at_journal_cut(
    connection: &Connection,
    through: u64,
    id: &CheckpointId,
) -> Result<ProjectionCheckpoint, ConversationStoreError> {
    let frontier = journal_frontier(connection, through)?;
    let activation: Option<u64> = connection.query_row("SELECT activation FROM projection_activations WHERE checkpoint_id=?1 AND activation<=?2",params![id.as_str(),seq_to_i64(frontier)?],|r|r.get(0)).optional().map_err(|e|storage(e.to_string()))?;
    activated(
        connection,
        activation.ok_or_else(|| invalid("projection is not installed at the published cut"))?,
    )
}

pub(super) fn timeline_page(
    connection: &Connection,
    after: ProjectionActivationId,
    limit: usize,
    selection: Option<(LedgerFrontier, JournalFrontier, ProjectionActivationId)>,
    budget: usize,
) -> Result<Vec<ProjectionCheckpoint>, ConversationStoreError> {
    let frontier = selected_frontier(connection, selection)?;
    timeline_prefix(connection, after, limit, frontier, budget)
}
fn timeline_prefix(
    connection: &Connection,
    after: ProjectionActivationId,
    limit: usize,
    frontier: u64,
    budget: usize,
) -> Result<Vec<ProjectionCheckpoint>, ConversationStoreError> {
    if limit == 0 || limit > 64 {
        return Err(invalid("projection timeline page exceeds bound"));
    }
    let mut page = Vec::new();
    let mut remaining = budget;
    let end = after.0.saturating_add(limit as u64).min(frontier);
    for id in after.0.saturating_add(1)..=end {
        let header = eligibility_header(connection, id)?;
        let predecessor = if id > 1 {
            Some(eligibility_header(connection, id - 1)?)
        } else {
            None
        };
        if predecessor
            .as_ref()
            .is_some_and(|p| p.ledger > header.ledger || p.journal > header.journal)
        {
            return Err(invalid("nonmonotone projection predecessor coordinates"));
        }
        let bytes:usize = connection.query_row("SELECT octet_length(candidate_json)+octet_length(activation_json)+octet_length(occupancy_json) FROM projection_activations a JOIN projection_checkpoints p ON a.checkpoint_id=p.checkpoint_id WHERE activation=?1",[seq_to_i64(id)?],|r|r.get(0)).map_err(|e|storage(e.to_string()))?;
        if bytes > remaining && !page.is_empty() {
            break;
        }
        if bytes > remaining {
            return Err(invalid(
                "projection timeline record exceeds materialization budget",
            ));
        }
        let value = activated(connection, id)?;
        if value
            .candidate
            .expected_head
            .as_ref()
            .map(CheckpointId::as_str)
            != predecessor.as_ref().map(|p| p.checkpoint.as_str())
        {
            return Err(invalid("missing historical projection predecessor"));
        }
        remaining -= crate::context::resources::bounded_encoded_len(&value, remaining)?;
        page.push(value);
    }
    Ok(page)
}

pub(super) fn receipt(
    connection: &Connection,
    id: &CompactionOperationId,
    fingerprint: &str,
) -> Result<Option<ProjectionCommitReceipt>, ConversationStoreError> {
    let row: Option<(String,Option<String>,String)> = connection.query_row("SELECT candidate_fingerprint,CASE WHEN octet_length(receipt_json)<=16777216 THEN receipt_json END,receipt_digest FROM compaction_operations WHERE operation_id=?1",[id.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|storage(e.to_string()))?;
    let Some((stored, json, integrity)) = row else {
        return Ok(None);
    };
    if stored != fingerprint {
        return Err(invalid(
            "CompactionOperationId reused with different candidate",
        ));
    }
    let json = json.ok_or_else(|| invalid("oversized compaction receipt"))?;
    if digest(&json) != integrity {
        return Err(invalid("corrupted compaction receipt"));
    }
    let receipt: ProjectionCommitReceipt = decode(&json, "compaction receipt")?;
    if receipt.operation_id != *id
        || receipt.candidate_fingerprint != fingerprint
        || receipt.checkpoint.candidate.fingerprint() != fingerprint
    {
        return Err(invalid("compaction receipt identity mismatch"));
    }
    Ok(Some(receipt))
}

pub(super) fn validate_dependencies(
    connection: &Connection,
    scope: &ConversationId,
    value: &CheckpointCandidate,
) -> Result<(), ConversationStoreError> {
    super::compaction_resources::validate(
        connection,
        scope,
        &value.read_cut,
        ResourceLimits::default(),
    )?;
    // Historical authority and current-head CAS are independent checks. The
    // candidate must name precisely the predecessor observed at its cut.
    let observed = activation_at_journal_cut(connection, value.read_cut.journal.0)?;
    if observed
        .as_ref()
        .map_or(ProjectionActivationId(0), |a| a.activation)
        != value.read_cut.projection_activation
        || observed.as_ref().map(|a| &a.checkpoint_id) != value.expected_head.as_ref()
    {
        return Err(invalid(
            "checkpoint predecessor disagrees with Resource Read Cut head",
        ));
    }
    validate_current_dependencies(connection, scope, value)?;
    compile_checkpoint(value, |source, body_required| {
        super::compaction_resources::dependency(
            connection,
            scope,
            &value.read_cut,
            source,
            body_required,
            ResourceLimits::default(),
        )
        .map_err(|e| e.to_string())
    })
    .map_err(invalid)?;
    if let Some(metadata) = &value.summary_metadata {
        validate_summary_metadata(connection, scope, value, metadata)?;
    }
    Ok(())
}

fn validate_summary_metadata(
    connection: &Connection,
    scope: &ConversationId,
    value: &CheckpointCandidate,
    metadata: &crate::message::types::CompactionSummaryMetadata,
) -> Result<(), ConversationStoreError> {
    let mut original_sources = std::collections::BTreeMap::new();
    let mut inherited = Vec::new();
    let mut parent_ids = std::collections::BTreeSet::new();
    let limits = ResourceLimits::default();
    let mut remaining_bytes = limits.total_bytes;
    let mut remaining_items = limits.total_items;
    for item in &value.items {
        if let CheckpointMessage::Derived(derived) = &item.message
            && derived
                .transformations
                .contains(&crate::model::projected::SemanticTransformation::Summary)
        {
            for source in &derived.sources {
                original_sources.insert(source.message_id.clone(), source);
            }
            for source in &derived.checkpoint_sources {
                if !parent_ids.insert(source.checkpoint_id.clone()) {
                    continue;
                }
                let bytes: usize = connection.query_row("SELECT octet_length(candidate_json) FROM projection_checkpoints WHERE checkpoint_id=?1", [source.checkpoint_id.as_str()], |r| r.get(0)).map_err(|e| storage(e.to_string()))?;
                remaining_bytes = remaining_bytes
                    .checked_sub(bytes)
                    .ok_or_else(|| invalid("trusted metadata cumulative byte budget exceeded"))?;
                remaining_items = remaining_items
                    .checked_sub(1)
                    .ok_or_else(|| invalid("trusted metadata cumulative item budget exceeded"))?;
                // The immutable content digest was checked above. Read only
                // its trusted metadata, not retired Derived bodies or occupancy.
                let json: Option<String> = connection.query_row("SELECT json_extract(candidate_json,'$.summary_metadata') FROM projection_checkpoints WHERE checkpoint_id=?1", [source.checkpoint_id.as_str()], |r| r.get(0)).map_err(|e| storage(e.to_string()))?;
                if let Some(json) = json {
                    #[cfg(test)]
                    METADATA_DECODES.with(|n| n.set(n.get() + 1));
                    inherited.push(decode::<crate::message::types::CompactionSummaryMetadata>(
                        &json,
                        "trusted inherited metadata",
                    )?);
                }
            }
        }
    }
    let mut read_files = Vec::new();
    let mut modified_files = Vec::new();
    for source in original_sources.into_values() {
        if remaining_bytes == 0 || remaining_items == 0 {
            return Err(invalid(
                "trusted metadata cumulative resource budget exceeded",
            ));
        }
        let page = super::compaction_resources::messages(
            connection,
            scope,
            &value.read_cut,
            LedgerFrontier(0),
            Some(std::slice::from_ref(&source.message_id)),
            ResourceLimits {
                page_bytes: limits.page_bytes.min(remaining_bytes),
                page_items: 1,
                ..limits
            },
        )?;
        remaining_bytes -= page.bytes;
        remaining_items -= page.rows.len();
        let (_, message) = page
            .rows
            .into_iter()
            .next()
            .ok_or_else(|| invalid("missing trusted metadata source"))?;
        let physical = crate::context::compaction_metadata::retired_span_metadata(
            std::slice::from_ref(&message),
        );
        read_files.extend_from_slice(physical.read_files());
        modified_files.extend_from_slice(physical.modified_files());
    }
    let expected = crate::message::types::CompactionSummaryMetadata::accumulate(
        inherited.iter(),
        read_files,
        modified_files,
    );
    if metadata != &expected {
        return Err(invalid("untrusted cumulative file-operation metadata"));
    }
    Ok(())
}

pub(super) fn commit(
    transaction: &Transaction<'_>,
    scope: &ConversationId,
    input: ProjectionCommitInput,
    mut stage: impl FnMut(super::CompactionFaultOperation) -> Result<(), ConversationStoreError>,
) -> Result<ProjectionCommitReceipt, ConversationStoreError> {
    if input.grant.operation_id() != &input.operation_id {
        return Err(invalid("foreign compaction commit grant"));
    }
    input.candidate.validate_budget().map_err(invalid)?;
    let fingerprint = input.candidate.fingerprint();
    if let Some(receipt) = receipt(transaction, &input.operation_id, &fingerprint)? {
        return Ok(receipt);
    }
    let actual = head(transaction)?;
    if actual.as_ref().map(|p| &p.candidate.checkpoint_id) != input.candidate.expected_head.as_ref()
    {
        return Err(invalid("stale Projection Head"));
    }
    if owner(transaction, input.owner.attempt_id.as_ref())? != input.owner {
        return Err(invalid("lost execution fencing"));
    }
    validate_dependencies(transaction, scope, &input.candidate)?;
    stage(super::CompactionFaultOperation::BeforeContentInsert)?;
    let candidate_json = encode(&input.candidate, "checkpoint")?;
    transaction.execute("INSERT INTO projection_checkpoints(checkpoint_id,candidate_fingerprint,candidate_json,occupancy_json) VALUES(?1,?2,?3,?4)",params![input.candidate.checkpoint_id.as_str(),fingerprint,candidate_json,encode(&input.occupancy,"checkpoint occupancy")?]).map_err(|e|storage(e.to_string()))?;
    stage(super::CompactionFaultOperation::AfterContentInsert)?;
    let next: i64 = transaction
        .query_row(
            "SELECT coalesce(max(activation),0)+1 FROM projection_activations",
            [],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    let activation_id = ProjectionActivationId(nonnegative(next, "projection activation")?);
    let event = RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(""),
        sequence: 0,
        conversation_id: scope.clone(),
        attempt_id: input.owner.attempt_id.clone(),
        turn_id: input.turn_id,
        timestamp: input.timestamp,
        event: RuntimeEvent::CompactionCompleted {
            operation_id: input.operation_id.clone(),
            checkpoint_id: input.candidate.checkpoint_id.clone(),
            activation_id,
            generation: activation_id.0,
            tokens_before: input.tokens_before,
            estimated_tokens_after: input.estimated_tokens_after,
            occupancy: input.occupancy.clone(),
        },
    };
    // Allocate the actual Journal coordinate within this transaction. No inherited fact is emitted.
    let sequence: i64 = transaction
        .query_row(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    let activated_ledger: u64 = transaction
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    let activation = ProjectionActivation {
        activation: activation_id,
        ledger: LedgerFrontier(activated_ledger),
        checkpoint_id: input.candidate.checkpoint_id.clone(),
        origin: OriginActivation {
            conversation_id: scope.clone(),
            activation: activation_id,
            completed_journal: JournalFrontier(nonnegative(sequence, "completion sequence")? + 1),
        },
        destination_installation: None,
    };
    persist_activation(transaction, &activation)?;
    stage(super::CompactionFaultOperation::AfterActivationInsert)?;
    transaction.execute("INSERT INTO projection_head(id,activation) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET activation=excluded.activation",[next]).map_err(|e|storage(e.to_string()))?;
    stage(super::CompactionFaultOperation::AfterHeadTransition)?;
    stage(super::CompactionFaultOperation::BeforeCompletionInsert)?;
    let completed = super::persist_event_tx(transaction, scope, event)?.event;
    if completed.sequence != activation.origin.completed_journal.0 {
        return Err(invalid("completion Journal coordinate mismatch"));
    }
    stage(super::CompactionFaultOperation::AfterCompletionInsert)?;
    let result = ProjectionCommitReceipt {
        operation_id: input.operation_id,
        candidate_fingerprint: fingerprint.clone(),
        checkpoint: ProjectionCheckpoint {
            candidate: input.candidate,
            fingerprint,
            activation,
            occupancy: input.occupancy,
        },
        completed,
    };
    let json = encode(&result, "compaction receipt")?;
    transaction.execute("INSERT INTO compaction_operations(operation_id,candidate_fingerprint,receipt_json,receipt_digest) VALUES(?1,?2,?3,?4)",params![result.operation_id.as_str(),result.candidate_fingerprint,json,digest(&json)]).map_err(|e|storage(e.to_string()))?;
    stage(super::CompactionFaultOperation::AfterReceiptInsert)?;
    Ok(result)
}

pub(super) fn history(
    connection: &Connection,
    scope: &ConversationId,
) -> Result<crate::durable::projection::HistoricalModelInput, ConversationStoreError> {
    history_at(connection, scope, None)
}

#[allow(clippy::too_many_lines)] // One bounded checkpoint/Tail compilation under the native read transaction.
pub(super) fn history_at(
    connection: &Connection,
    scope: &ConversationId,
    expected: Option<&crate::durable::projection::RequestHistoryCut>,
) -> Result<crate::durable::projection::HistoricalModelInput, ConversationStoreError> {
    use crate::durable::projection::{HistoricalModelInput, RequestHistoryCut};
    let mut read_cut = super::compaction_resources::cut(connection, scope)?;
    let checkpoint = head(connection)?;
    if let Some(expected) = expected {
        if expected.ledger > read_cut.ledger
            || checkpoint.as_ref().map(|p| &p.candidate.checkpoint_id)
                != expected.checkpoint_id.as_ref()
            || checkpoint
                .as_ref()
                .map_or(ProjectionActivationId(0), |p| p.activation.activation)
                != expected.activation
            || checkpoint
                .as_ref()
                .map_or(LedgerFrontier(0), |p| p.candidate.base_frontier)
                != expected.base_frontier
        {
            return Err(invalid(
                "request history cut disagrees with projection authority",
            ));
        }
        read_cut.ledger = expected.ledger;
    }
    let mut messages = Vec::new();
    let mut tool_associations = Vec::new();
    let mut pending_tail = std::collections::BTreeMap::new();
    let mut items = checkpoint
        .as_ref()
        .map_or_else(Vec::new, |p| p.candidate.items.clone());
    let base = checkpoint
        .as_ref()
        .map_or(LedgerFrontier(0), |p| p.candidate.base_frontier);
    if let Some(checkpoint) = &checkpoint {
        validate_current_dependencies(connection, scope, &checkpoint.candidate)?;
        let compiled = compile_checkpoint(&checkpoint.candidate, |source, body_required| {
            super::compaction_resources::dependency(
                connection,
                scope,
                &checkpoint.candidate.read_cut,
                source,
                body_required,
                ResourceLimits::default(),
            )
            .map_err(|e| e.to_string())
        })
        .map_err(invalid)?;
        for tool in &compiled.tools {
            let item = &checkpoint.candidate.items[tool.call.message.item_index as usize];
            let assistant = match &item.message {
                CheckpointMessage::Original(source) => source.clone(),
                CheckpointMessage::Derived(_) => item
                    .call_sources
                    .iter()
                    .find(|source| source.block == tool.call.block)
                    .ok_or_else(|| invalid("missing compiled Derived call source"))?
                    .assistant
                    .clone(),
            };
            tool_associations.push(crate::model::occurrence::HistoricalToolAssociation {
                call_message: tool.call.message.item_index,
                call_block: tool.call.block.0,
                result_message: tool.result.item_index,
                source: crate::model::occurrence::CanonicalCallSource {
                    assistant,
                    block: tool.canonical_source.block_index,
                },
                wire_id: crate::model::occurrence::ProviderWireCorrelationId(
                    tool.wire_id.to_string(),
                ),
            });
        }
        messages = compiled.messages;
    }
    let mut after = base;
    let mut bytes = crate::context::resources::bounded_encoded_len(
        &messages,
        ResourceLimits::default().total_bytes,
    )?;
    loop {
        let page = super::compaction_resources::messages(
            connection,
            scope,
            &read_cut,
            after,
            None,
            ResourceLimits::default(),
        )?;
        if page.rows.is_empty() {
            break;
        }
        bytes = bytes
            .checked_add(page.bytes)
            .ok_or_else(|| invalid("historical Tail byte overflow"))?;
        if bytes > ResourceLimits::default().total_bytes
            || messages.len() + page.rows.len() > ResourceLimits::default().total_items
        {
            return Err(invalid("historical Tail materialization exceeds bound"));
        }
        for (position, message) in page.rows {
            let index = u32::try_from(messages.len())
                .map_err(|_| invalid("request occurrence overflow"))?;
            match &message {
                crate::message::types::MessageBlock::Assistant(assistant) => {
                    for (block, content) in assistant.content.iter().enumerate() {
                        if let crate::message::types::AssistantContentBlock::ToolCall(call) =
                            content
                        {
                            let block = u32::try_from(block)
                                .map_err(|_| invalid("canonical call block overflow"))?;
                            let occurrence = crate::message::types::ToolCallOccurrenceRef::new(
                                assistant.id.clone(),
                                crate::message::types::ContentBlockIndex::new(block),
                            );
                            pending_tail.insert(
                                occurrence,
                                (
                                    index,
                                    block,
                                    crate::model::projected::CanonicalMessageRef::of(
                                        scope, &message,
                                    ),
                                    call.id.clone(),
                                    call.tool_id.clone(),
                                ),
                            );
                        }
                    }
                }
                crate::message::types::MessageBlock::Tool(result) => {
                    let (call_message, call_block, assistant, wire_id, tool_id) = pending_tail
                        .remove(&result.occurrence)
                        .ok_or_else(|| invalid("orphan canonical Tail ToolResult"))?;
                    if result.tool_call_id != wire_id || result.tool_id != tool_id {
                        return Err(invalid(
                            "Tail ToolResult disagrees with physical call occurrence",
                        ));
                    }
                    tool_associations.push(crate::model::occurrence::HistoricalToolAssociation {
                        call_message,
                        call_block,
                        result_message: index,
                        source: crate::model::occurrence::CanonicalCallSource {
                            assistant,
                            block: result.occurrence.block_index,
                        },
                        wire_id: crate::model::occurrence::ProviderWireCorrelationId(
                            wire_id.to_string(),
                        ),
                    });
                }
                crate::message::types::MessageBlock::User(_) => {}
            }
            after = LedgerFrontier(position);
            items.push(crate::context::checkpoint::CheckpointItem {
                message: CheckpointMessage::Original(
                    crate::model::projected::CanonicalMessageRef::of(scope, &message),
                ),
                call_sources: Vec::new(),
                result_call: None,
            });
            messages.push(message.into());
        }
    }
    if !pending_tail.is_empty() {
        return Err(invalid(
            "canonical Tail ends inside an incomplete Tool batch",
        ));
    }
    Ok(HistoricalModelInput {
        tool_associations,
        cut: RequestHistoryCut {
            ledger: read_cut.ledger,
            checkpoint_id: checkpoint
                .as_ref()
                .map(|p| p.candidate.checkpoint_id.clone()),
            activation: checkpoint
                .as_ref()
                .map_or(ProjectionActivationId(0), |p| p.activation.activation),
            base_frontier: base,
        },
        messages,
        items,
    })
}

pub(super) fn install_lineage(
    transaction: &Transaction<'_>,
    scope: &ConversationId,
    checkpoints: &[ProjectionCheckpoint],
) -> Result<(), ConversationStoreError> {
    let history: String = transaction
        .query_row(
            "SELECT history_digest FROM bootstrap_identity WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    transaction
        .execute(
            "UPDATE bootstrap_identity SET projection_count=0,projection_digest=?1 WHERE id=1",
            [inherited_count_digest(&history, 0)],
        )
        .map_err(|e| storage(e.to_string()))?;
    let mut last = None;
    let mut previous_checkpoint = None;
    let mut previous_ledger = LedgerFrontier(0);
    for checkpoint in checkpoints {
        let candidate = &checkpoint.candidate;
        if candidate.read_cut.conversation_id != *scope
            || checkpoint.activation.destination_installation.is_none()
            || candidate.fingerprint() != checkpoint.fingerprint
        {
            return Err(invalid("inherited projection scope or integrity mismatch"));
        }
        let actual_cut = super::compaction_resources::cut(transaction, scope)?;
        if checkpoint.activation.checkpoint_id != candidate.checkpoint_id
            || checkpoint.activation.activation.0
                != last.map_or(1, |a: ProjectionActivationId| a.0 + 1)
            || checkpoint.activation.ledger < previous_ledger
            || candidate.base_frontier > checkpoint.activation.ledger
            || checkpoint.activation.ledger > actual_cut.ledger
            || candidate.read_cut.ledger > actual_cut.ledger
            || candidate.read_cut.projection_activation > last.unwrap_or_default()
            || candidate.expected_head.as_ref() != previous_checkpoint.as_ref()
        {
            return Err(invalid(
                "inconsistent inherited projection activation boundary",
            ));
        }
        validate_dependencies(transaction, scope, candidate)?;
        let json = encode(candidate, "inherited checkpoint")?;
        transaction.execute("INSERT INTO projection_checkpoints(checkpoint_id,candidate_fingerprint,candidate_json,occupancy_json) VALUES(?1,?2,?3,?4)", params![candidate.checkpoint_id.as_str(),checkpoint.fingerprint,json,encode(&checkpoint.occupancy,"inherited occupancy")?]).map_err(|e|storage(e.to_string()))?;
        let activation = &checkpoint.activation;
        persist_activation(transaction, activation)?;
        let history: String = transaction
            .query_row(
                "SELECT history_digest FROM bootstrap_identity WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| storage(e.to_string()))?;
        transaction
            .execute(
                "UPDATE bootstrap_identity SET projection_count=?1,projection_digest=?2 WHERE id=1",
                params![
                    seq_to_i64(activation.activation.0)?,
                    inherited_count_digest(&history, activation.activation.0)
                ],
            )
            .map_err(|e| storage(e.to_string()))?;
        last = Some(activation.activation);
        previous_checkpoint = Some(candidate.checkpoint_id.clone());
        previous_ledger = activation.ledger;
    }
    if let Some(last) = last {
        transaction
            .execute(
                "INSERT INTO projection_head(id,activation) VALUES(1,?1)",
                [seq_to_i64(last.0)?],
            )
            .map_err(|e| storage(e.to_string()))?;
    }
    // Intentionally no local CompactionCompleted, request, usage or execution fact.
    Ok(())
}

pub(super) fn verify_lineage(
    connection: &Connection,
    checkpoints: &[ProjectionCheckpoint],
) -> Result<(), ConversationStoreError> {
    for expected in checkpoints {
        if checkpoint(connection, &expected.candidate.checkpoint_id)? != *expected {
            return Err(invalid("immutable inherited projection bootstrap changed"));
        }
    }
    Ok(())
}

/// Current model input owns its Derived bodies. A parent checkpoint is an
/// immutable provenance dependency, not an instruction to replay its historical
/// selection or hydrate its retired Originals. Lineage installation validates
/// each retained historical checkpoint independently for subsequent forks.
type ProvenanceHeader = (String, Option<String>, Option<String>, Option<u64>, u64);
pub(super) fn validate_current_dependencies(
    connection: &Connection,
    scope: &ConversationId,
    value: &CheckpointCandidate,
) -> Result<(), ConversationStoreError> {
    for item in &value.items {
        if let CheckpointMessage::Derived(derived) = &item.message {
            for source in &derived.checkpoint_sources {
                if source.conversation_id != *scope {
                    return Err(invalid("foreign checkpoint provenance"));
                }
                let row:Option<ProvenanceHeader> = connection.query_row("SELECT candidate_fingerprint,CASE WHEN octet_length(candidate_json)<=8388608 THEN rustx_content_digest(candidate_json) END,json_extract(CASE WHEN octet_length(candidate_json)<=8388608 THEN candidate_json END,'$.read_cut.conversation_id'),json_extract(CASE WHEN octet_length(candidate_json)<=8388608 THEN candidate_json END,'$.base_frontier'),a.activation FROM projection_checkpoints p JOIN projection_activations a ON a.checkpoint_id=p.checkpoint_id WHERE p.checkpoint_id=?1",[source.checkpoint_id.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e|storage(e.to_string()))?;
                let Some((fingerprint, digest, parent_scope, base, activation)) = row else {
                    return Err(invalid("missing immutable checkpoint provenance"));
                };
                if fingerprint != source.content_digest
                    || digest.as_ref() != Some(&fingerprint)
                    || parent_scope.as_deref() != Some(scope.as_str())
                    || base.is_none_or(|base| base > value.base_frontier.0)
                    || activation > value.read_cut.projection_activation.0
                {
                    return Err(invalid(
                        "corrupted, oversized or future checkpoint provenance",
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

pub(super) fn at_journal_cut(
    connection: &Connection,
    through: u64,
) -> Result<Option<ProjectionCheckpoint>, ConversationStoreError> {
    activation_at_journal_cut(connection, through)?
        .map(|activation| activated(connection, activation.activation.0))
        .transpose()
}

fn activation_at_journal_cut(
    connection: &Connection,
    through: u64,
) -> Result<Option<ProjectionActivation>, ConversationStoreError> {
    let id = journal_frontier(connection, through)?;
    if id == 0 {
        Ok(None)
    } else {
        activation_at(connection, ProjectionActivationId(id)).map(Some)
    }
}

/// Validate selection without repeating any dynamic semantic transformation.
/// The frozen bodies remain Agent-owned; this checks their immutable history origins and order.
pub(super) fn validate_request_selection(
    connection: &Connection,
    scope: &ConversationId,
    snapshot: &crate::model::RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    use crate::model::{ModelInputMessage, projected::FrozenModelInput};
    let history = history_at(connection, scope, Some(&snapshot.history_cut))?;
    let historical_len = history.messages.len();
    let mut expected = history.messages;
    let mut core_ids = Vec::new();
    for id in snapshot
        .contributions
        .iter()
        .map(|r| &r.message_id)
        .chain(snapshot.request_context_ids.iter())
    {
        if !core_ids.contains(id) {
            core_ids.push(id.clone());
        }
    }
    for id in core_ids {
        if expected.iter().any(|m| m.canonical_id() == Some(&id)) {
            continue;
        }
        let message = super::load_message(connection, &id)?;
        if !matches!(&message,crate::message::types::MessageBlock::User(user) if matches!(user.kind,crate::message::types::InboundKind::Context(_)))
        {
            return Err(invalid(
                "core request Context is not a canonical Context fact",
            ));
        }
        expected.push(ModelInputMessage::Canonical(message));
    }
    if let Some(carryover) = &snapshot.unresolved_output_carryover {
        let at = match &snapshot.unresolved_output_carryover_anchor {
            Some(crate::model::RequestOnlyInsertionAnchor::BeforeMessage(id)) => expected
                .iter()
                .position(|m| m.canonical_id() == Some(id))
                .ok_or_else(|| invalid("missing core carryover anchor"))?,
            Some(crate::model::RequestOnlyInsertionAnchor::AfterCanonical) => historical_len,
            None => return Err(invalid("unanchored core carryover")),
        };
        expected.insert(
            at,
            ModelInputMessage::RequestOnly(
                crate::model::input::RequestOnlyModelContext::UnresolvedOutputCarryover(
                    carryover.clone(),
                ),
            ),
        );
    }
    if snapshot.input.len() != expected.len() {
        return Err(invalid(
            "frozen request selection omits or adds historical/core items",
        ));
    }
    for (actual, expected) in snapshot.input.iter().zip(expected) {
        let valid = match (actual, expected) {
            (FrozenModelInput::Original(reference), ModelInputMessage::Canonical(message)) => {
                reference == &crate::model::projected::CanonicalMessageRef::of(scope, &message)
            }
            (FrozenModelInput::Derived(derived), ModelInputMessage::Canonical(message)) => {
                derived.sources
                    == [crate::model::projected::CanonicalMessageRef::of(
                        scope, &message,
                    )]
                    && derived.checkpoint_sources.is_empty()
                    && crate::model::transformations::valid(
                        &crate::model::projected::ProjectedMessage::from(&message),
                        &derived.message,
                        &[],
                        &derived.transformations,
                    )
            }
            (FrozenModelInput::Derived(derived), ModelInputMessage::Derived(expected)) => {
                derived.sources == expected.sources
                    && derived.checkpoint_sources == expected.checkpoint_sources
                    && crate::model::transformations::valid(
                        &expected.message,
                        &derived.message,
                        &expected.transformations,
                        &derived.transformations,
                    )
            }

            (FrozenModelInput::RequestOnly(actual), ModelInputMessage::RequestOnly(expected)) => {
                actual == &expected
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(
                "frozen request revives excluded history or changes historical ordering",
            ));
        }
    }
    Ok(())
}
