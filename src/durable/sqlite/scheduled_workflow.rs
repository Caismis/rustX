//! Workflow receipts share the target conversation's native writer and SQLite transaction.
use super::{SqliteConversationStore, decode, encode, storage};
use crate::durable::{ConversationStoreError, ScheduledWorkflowStore};
use crate::events::types::RuntimeEvent;
use crate::runtime::workflow::scheduled::{
    PinnedScheduledWorkflow,
    lifecycle::{
        ClaimedScheduledWorkflow, QuiescenceSource, ScheduledEffectKind, ScheduledEffectOwner,
        ScheduledWorkflowEffect, ScheduledWorkflowHead, ScheduledWorkflowOutcome,
        ScheduledWorkflowOwnerRetirement, ScheduledWorkflowQuiescence, ScheduledWorkflowRecovery,
        ScheduledWorkflowSettlement, ScheduledWorkflowStart, ScheduledWorkflowStartId,
        ScheduledWorkflowState, bounded_reason,
    },
};
use crate::runtime::workflow::{WorkflowNodeInstance, WorkflowRunId, WorkflowRunOrigin};
use crate::scheduler::{ScheduleBinding, ScheduleTriggerId};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

#[allow(clippy::needless_pass_by_value)] // Owned map_err adapter.
fn sql(e: rusqlite::Error) -> ConversationStoreError {
    storage(format!("scheduled Workflow: {e}"))
}
pub(super) fn create_schema(c: &Connection) -> Result<(), ConversationStoreError> {
    c.execute_batch("CREATE TABLE scheduled_workflow_starts(start_id TEXT PRIMARY KEY,schedule_ordinal INTEGER NOT NULL,state TEXT NOT NULL,accepted_at TEXT NOT NULL,settled_at TEXT,head_json TEXT NOT NULL,contract_json TEXT,args_json TEXT,result_json TEXT);
        CREATE INDEX scheduled_workflow_pending ON scheduled_workflow_starts(state,accepted_at,start_id);
        CREATE INDEX scheduled_workflow_schedule ON scheduled_workflow_starts(schedule_ordinal,state,start_id);
        CREATE INDEX scheduled_workflow_completed ON scheduled_workflow_starts(settled_at,start_id) WHERE state='settled' AND contract_json IS NOT NULL;
        CREATE TABLE scheduled_workflow_effects(start_id TEXT NOT NULL,node_key TEXT NOT NULL,ordinal INTEGER NOT NULL,effect_json TEXT NOT NULL,PRIMARY KEY(start_id,node_key),UNIQUE(start_id,ordinal),FOREIGN KEY(start_id) REFERENCES scheduled_workflow_starts(start_id));").map_err(sql)
}
pub(super) fn verify_schema(c: &Connection) -> Result<(), ConversationStoreError> {
    for (table, expected) in [
        (
            "scheduled_workflow_starts",
            &[
                "start_id",
                "schedule_ordinal",
                "state",
                "accepted_at",
                "settled_at",
                "head_json",
                "contract_json",
                "args_json",
                "result_json",
            ][..],
        ),
        (
            "scheduled_workflow_effects",
            &["start_id", "node_key", "ordinal", "effect_json"][..],
        ),
    ] {
        let columns = c
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .map_err(sql)?
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        if columns != expected {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid {table}"
            )));
        }
    }
    super::schema::shape::verify_unique_columns(c, "scheduled_workflow_starts", &["start_id"])?;
    super::schema::shape::verify_unique_columns(
        c,
        "scheduled_workflow_effects",
        &["start_id", "node_key"],
    )?;
    super::schema::shape::verify_unique_columns(
        c,
        "scheduled_workflow_effects",
        &["start_id", "ordinal"],
    )?;
    for (name, expected) in [
        (
            "scheduled_workflow_completed",
            "CREATE INDEX scheduled_workflow_completed ON scheduled_workflow_starts(settled_at,start_id) WHERE state='settled' AND contract_json IS NOT NULL",
        ),
        (
            "scheduled_workflow_pending",
            "CREATE INDEX scheduled_workflow_pending ON scheduled_workflow_starts(state,accepted_at,start_id)",
        ),
        (
            "scheduled_workflow_schedule",
            "CREATE INDEX scheduled_workflow_schedule ON scheduled_workflow_starts(schedule_ordinal,state,start_id)",
        ),
    ] {
        let actual: Option<String> = c
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1 AND type='index'",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?;
        if actual.as_deref() != Some(expected) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid {name}"
            )));
        }
    }
    let foreign_keys:Vec<(String,String,String)>=c.prepare("SELECT \"table\",\"from\",\"to\" FROM pragma_foreign_key_list('scheduled_workflow_effects')").map_err(sql)?.query_map([],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(sql)?.collect::<Result<_,_>>().map_err(sql)?;
    if foreign_keys
        != [(
            "scheduled_workflow_starts".into(),
            "start_id".into(),
            "start_id".into(),
        )]
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid scheduled effect ownership key".into(),
        ));
    }
    Ok(())
}
fn state(s: &ScheduledWorkflowState) -> &'static str {
    match s {
        ScheduledWorkflowState::Accepted { .. } => "accepted",
        ScheduledWorkflowState::Claimed { .. } => "claimed",
        ScheduledWorkflowState::Settled { .. } => "settled",
    }
}
pub(super) fn head(
    c: &Connection,
    id: &ScheduledWorkflowStartId,
) -> Result<Option<ScheduledWorkflowHead>, ConversationStoreError> {
    let row:Option<(String,Option<String>)>=c.query_row("SELECT state,CASE WHEN length(CAST(head_json AS BLOB))<=16384 THEN head_json END FROM scheduled_workflow_starts WHERE start_id=?1",[id.as_str()],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(sql)?;
    let Some((stored, json)) = row else {
        return Ok(None);
    };
    let h: ScheduledWorkflowHead = decode(
        &json.ok_or_else(|| storage("scheduled header exceeds native bound"))?,
        "scheduled start header",
    )?;
    if h.id != *id
        || ScheduledWorkflowStartId::for_trigger(&h.trigger_id) != h.id
        || h.run_id.invocation != 1
        || state(&h.state) != stored
        || h.run_id.conversation_id != h.binding.conversation_id
        || !matches!(&h.run_id.origin,WorkflowRunOrigin::Scheduled{start_id} if start_id==id)
        || h.trigger_id.schedule_id.conversation_id != h.binding.conversation_id
    {
        return Err(storage("scheduled header identity or state index differs"));
    }
    Ok(Some(h))
}

fn save(tx: &Transaction<'_>, h: &ScheduledWorkflowHead) -> Result<(), ConversationStoreError> {
    let json = encode(h, "scheduled start header")?;
    if json.len() > 16384 {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "scheduled start header".into(),
        ));
    }
    let settled = match &h.state {
        ScheduledWorkflowState::Settled { settled_at, .. } => Some(settled_at.to_rfc3339()),
        _ => None,
    };
    tx.execute("UPDATE scheduled_workflow_starts SET state=?2,settled_at=?3,head_json=?4 WHERE start_id=?1",params![h.id.as_str(),state(&h.state),settled,json]).map_err(sql)?;
    // Settlement changes outstanding Scheduler capacity, even after the definition was pruned.
    super::schedule::advance_generation(tx)?;
    if let ScheduledWorkflowState::Settled { settled_at, .. } = &h.state {
        super::schedule::prune(tx, *settled_at)?;
    }
    Ok(())
}
pub(super) fn accept_tx(
    tx: &Transaction<'_>,
    binding: &ScheduleBinding,
    trigger: ScheduleTriggerId,
    contract: &PinnedScheduledWorkflow,
    arguments: &serde_json::Value,
    now: DateTime<Utc>,
) -> Result<ScheduledWorkflowHead, ConversationStoreError> {
    if contract.binding != *binding {
        return Err(ConversationStoreError::InvalidReference(
            "scheduled start binding mismatch".into(),
        ));
    }
    let contract_json = encode(contract, "pinned scheduled Workflow")?;
    let args_json = encode(arguments, "scheduled Workflow arguments")?;
    if contract_json.len()
        > crate::runtime::workflow::scheduled::MAX_SCHEDULED_WORKFLOW_CONTRACT_BYTES
        || args_json.len() > 16384
    {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "scheduled Workflow contract or arguments".into(),
        ));
    }
    let id = ScheduledWorkflowStartId::for_trigger(&trigger);
    let h = ScheduledWorkflowHead {
        run_id: WorkflowRunId {
            conversation_id: binding.conversation_id.clone(),
            origin: WorkflowRunOrigin::Scheduled {
                start_id: id.clone(),
            },
            invocation: 1,
        },
        id,
        trigger_id: trigger,
        binding: binding.clone(),
        accepted_at: now,
        state: ScheduledWorkflowState::Accepted {
            waiting: None,
            retry_after: None,
        },
        cancellation_requested_at: None,
        recovery_blocked: None,
        native_quiescence_proven: false,
        result_digest: None,
    };
    tx.execute("INSERT INTO scheduled_workflow_starts(start_id,schedule_ordinal,state,accepted_at,settled_at,head_json,contract_json,args_json) VALUES(?1,?2,'accepted',?3,NULL,?4,?5,?6)",params![h.id.as_str(),h.trigger_id.schedule_id.ordinal,now.to_rfc3339(),encode(&h,"scheduled start header")?,contract_json,args_json]).map_err(sql)?;
    Ok(h)
}
fn require_claim(
    c: &Connection,
    claim: &ClaimedScheduledWorkflow,
) -> Result<ScheduledWorkflowHead, ConversationStoreError> {
    let h = head(c, &claim.start.head.id)?.ok_or_else(|| storage("scheduled claim disappeared"))?;
    if !matches!(&h.state,ScheduledWorkflowState::Claimed{owner,..} if owner==claim.owner()) {
        return Err(ConversationStoreError::InvalidReference(
            "scheduled execution no longer owns its claim".into(),
        ));
    }
    Ok(h)
}
fn node_key(node: &WorkflowNodeInstance) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(node).expect("native node identity"))
    )
}

impl ScheduledWorkflowStore for SqliteConversationStore {
    fn scheduled_workflow_get(
        &self,
        id: &ScheduledWorkflowStartId,
    ) -> Result<Option<ScheduledWorkflowStart>, ConversationStoreError> {
        let c = self.lock()?;
        let Some(h) = head(&c, id)? else {
            return Ok(None);
        };
        if h.binding.conversation_id != self.conversation_id {
            return Err(storage("foreign scheduled start"));
        }
        let (contract,args,result):(Option<String>,Option<String>,Option<String>)=c.query_row("SELECT CASE WHEN length(CAST(contract_json AS BLOB))<=65536 THEN contract_json END,CASE WHEN length(CAST(args_json AS BLOB))<=16384 THEN args_json END,CASE WHEN length(CAST(result_json AS BLOB))<=65536 THEN result_json END FROM scheduled_workflow_starts WHERE start_id=?1",[id.as_str()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(sql)?;
        let contract = contract
            .map(|json| decode(&json, "pinned scheduled Workflow"))
            .transpose()?;
        let arguments = args
            .map(|json| decode(&json, "scheduled arguments"))
            .transpose()?;
        if !matches!(h.state, ScheduledWorkflowState::Settled { .. })
            && (contract.is_none() || arguments.is_none())
        {
            return Err(storage(
                "nonterminal scheduled start lost its bounded contract",
            ));
        }
        Ok(Some(ScheduledWorkflowStart {
            head: h,
            contract,
            arguments,
            result: result
                .map(|json| decode(&json, "scheduled result"))
                .transpose()?,
        }))
    }
    fn scheduled_workflow_pending(
        &self,
        limit: u32,
    ) -> Result<Vec<ScheduledWorkflowHead>, ConversationStoreError> {
        if !(1..=256).contains(&limit) {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "scheduled start page".into(),
            ));
        }
        let c = self.lock()?;
        let ids:Vec<String>=c.prepare("SELECT start_id FROM scheduled_workflow_starts WHERE state IN ('accepted','claimed') ORDER BY accepted_at,start_id LIMIT ?1").map_err(sql)?.query_map([limit],|r|r.get(0)).map_err(sql)?.collect::<Result<_,_>>().map_err(sql)?;
        ids.into_iter()
            .map(|id| {
                let id = ScheduledWorkflowStartId::try_from(id).map_err(storage)?;
                head(&c, &id)?.ok_or_else(|| storage("pending scheduled identity disappeared"))
            })
            .collect()
    }
    fn scheduled_workflow_claim(
        &self,
        id: &ScheduledWorkflowStartId,
        expected: &PinnedScheduledWorkflow,
        owner: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedScheduledWorkflow>, ConversationStoreError> {
        if owner.is_empty() || owner.len() > 128 {
            return Err(ConversationStoreError::InvalidReference(
                "invalid scheduled execution owner".into(),
            ));
        }
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let Some(mut h) = head(&tx, id)? else {
            return Ok(None);
        };
        if !matches!(h.state, ScheduledWorkflowState::Accepted { .. })
            || h.cancellation_requested_at.is_some()
        {
            return Ok(None);
        }
        let (contract,args):(String,String)=tx.query_row("SELECT contract_json,args_json FROM scheduled_workflow_starts WHERE start_id=?1 AND length(CAST(contract_json AS BLOB))<=65536 AND length(CAST(args_json AS BLOB))<=16384",[id.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(sql)?;
        let persisted: PinnedScheduledWorkflow = decode(&contract, "scheduled contract")?;
        let mut left = persisted.clone();
        left.observed_resource_revision = expected.observed_resource_revision;
        if left != *expected || persisted.binding.conversation_id != self.conversation_id {
            return Err(ConversationStoreError::InvalidReference(
                "scheduled contract changed before claim".into(),
            ));
        }
        h.state = ScheduledWorkflowState::Claimed {
            owner: owner.into(),
            claimed_at: now,
            effects_started: false,
        };
        save(&tx, &h)?;
        crate::runtime::process_death::reach("before:scheduled_workflow_claim");
        tx.commit().map_err(sql)?;
        crate::runtime::process_death::reach("after:scheduled_workflow_claim");
        Ok(Some(ClaimedScheduledWorkflow {
            start: ScheduledWorkflowStart {
                head: h,
                contract: Some(persisted),
                arguments: Some(decode(&args, "scheduled arguments")?),
                result: None,
            },
        }))
    }
    fn scheduled_workflow_wait(
        &self,
        id: &ScheduledWorkflowStartId,
        reason: &str,
        retry_after: DateTime<Utc>,
    ) -> Result<(), ConversationStoreError> {
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        if let Some(mut h) = head(&tx, id)?
            && matches!(h.state, ScheduledWorkflowState::Accepted { .. })
        {
            h.state = ScheduledWorkflowState::Accepted {
                waiting: Some(bounded_reason(reason)),
                retry_after: Some(retry_after),
            };
            save(&tx, &h)?;
        }
        tx.commit().map_err(sql)
    }
    fn scheduled_workflow_reject(
        &self,
        id: &ScheduledWorkflowStartId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, ConversationStoreError> {
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let Some(mut h) = head(&tx, id)? else {
            return Ok(false);
        };
        if !matches!(h.state, ScheduledWorkflowState::Accepted { .. }) {
            return Ok(false);
        }
        h.state = ScheduledWorkflowState::Settled {
            outcome: ScheduledWorkflowOutcome::RejectedBeforeStart {
                reason: bounded_reason(reason),
            },
            settled_at: now,
        };
        h.native_quiescence_proven = true;
        save(&tx, &h)?;
        tx.commit().map_err(sql)?;
        Ok(true)
    }
    fn scheduled_workflow_cancel(
        &self,
        id: &ScheduledWorkflowStartId,
        now: DateTime<Utc>,
    ) -> Result<Option<ScheduledWorkflowHead>, ConversationStoreError> {
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let Some(mut h) = head(&tx, id)? else {
            return Ok(None);
        };
        if !matches!(h.state, ScheduledWorkflowState::Settled { .. }) {
            h.cancellation_requested_at.get_or_insert(now);
            if matches!(h.state, ScheduledWorkflowState::Accepted { .. }) {
                h.state = ScheduledWorkflowState::Settled {
                    outcome: ScheduledWorkflowOutcome::Cancelled,
                    settled_at: now,
                };
                h.native_quiescence_proven = true;
            }
            save(&tx, &h)?;
        }
        tx.commit().map_err(sql)?;
        Ok(Some(h))
    }
    fn scheduled_workflow_begin_effect(
        &self,
        claim: &ClaimedScheduledWorkflow,
        node: &WorkflowNodeInstance,
        kind: ScheduledEffectKind,
    ) -> Result<(), ConversationStoreError> {
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let mut h = require_claim(&tx, claim)?;
        if h.cancellation_requested_at.is_some() {
            return Err(ConversationStoreError::OperationCancelled);
        }
        if node.block.run != h.run_id {
            return Err(ConversationStoreError::InvalidReference(
                "foreign scheduled node".into(),
            ));
        }
        let ordinal:u64=tx.query_row("SELECT COALESCE(MAX(ordinal),0)+1 FROM scheduled_workflow_effects WHERE start_id=?1",[h.id.as_str()],|r|r.get(0)).map_err(sql)?;
        if ordinal > 4096 {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "scheduled effect count".into(),
            ));
        }
        let effect = ScheduledWorkflowEffect {
            ordinal,
            node: node.clone(),
            kind,
            owner: ScheduledEffectOwner::Preparing,
            physical_settlement_proven: false,
            diagnostic: None,
        };
        let json = encode(&effect, "scheduled effect")?;
        if json.len() > 4096 {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "scheduled effect identity".into(),
            ));
        }
        tx.execute("INSERT INTO scheduled_workflow_effects(start_id,node_key,ordinal,effect_json) VALUES(?1,?2,?3,?4)",params![h.id.as_str(),node_key(node),ordinal,json]).map_err(sql)?;
        if let ScheduledWorkflowState::Claimed {
            effects_started, ..
        } = &mut h.state
        {
            *effects_started = true;
        }
        save(&tx, &h)?;
        tx.commit().map_err(sql)
    }
    fn scheduled_workflow_effect_owner(
        &self,
        claim: &ClaimedScheduledWorkflow,
        node: &WorkflowNodeInstance,
        owner: ScheduledEffectOwner,
    ) -> Result<(), ConversationStoreError> {
        update_effect(self, &claim.start.head.id, claim.owner(), node, |effect| {
            effect.owner = owner;
        })
    }
    fn scheduled_workflow_effect_settled(
        &self,
        claim: &ClaimedScheduledWorkflow,
        node: &WorkflowNodeInstance,
        proven: bool,
        diagnostic: Option<&str>,
    ) -> Result<(), ConversationStoreError> {
        update_effect(self, &claim.start.head.id, claim.owner(), node, |effect| {
            effect.physical_settlement_proven = proven;
            effect.diagnostic = diagnostic.map(bounded_reason);
        })
    }
    fn scheduled_workflow_recovery(
        &self,
        id: &ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
    ) -> Result<Option<ScheduledWorkflowRecovery>, ConversationStoreError> {
        if !(1..=100).contains(&limit) || after > i64::MAX as u64 {
            return Err(ConversationStoreError::ResourceBoundExceeded(
                "scheduled recovery page".into(),
            ));
        }
        let c = self.lock()?;
        let Some(h) = head(&c, id)? else {
            return Ok(None);
        };
        let effects = c.prepare("SELECT ordinal,node_key,CASE WHEN length(CAST(effect_json AS BLOB))<=4096 THEN effect_json END FROM scheduled_workflow_effects WHERE start_id=?1 AND ordinal>?2 ORDER BY ordinal LIMIT ?3").map_err(sql)?
            .query_map(params![id.as_str(),after,limit],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).map_err(sql)?
            .map(|row| {
                let (ordinal,key,json)=row.map_err(sql)?;
                let effect:ScheduledWorkflowEffect=decode(&json.ok_or_else(||storage("scheduled effect exceeds native bound"))?,"scheduled effect")?;
                if effect.ordinal!=ordinal || node_key(&effect.node)!=key || effect.node.block.run!=h.run_id {
                    return Err(storage("scheduled effect identity differs"));
                }
                Ok(effect)
            }).collect::<Result<Vec<_>,ConversationStoreError>>()?;
        let next_effect_ordinal = effects
            .last()
            .filter(|_| effects.len() == limit as usize)
            .map(|e| e.ordinal);
        let blocked = !matches!(h.state, ScheduledWorkflowState::Settled { .. });
        let (has_result,result):(bool,Option<String>)=c.query_row("SELECT result_json IS NOT NULL,CASE WHEN length(CAST(result_json AS BLOB))<=65536 THEN result_json END FROM scheduled_workflow_starts WHERE start_id=?1",[id.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(sql)?;
        if has_result && result.is_none() {
            return Err(storage("scheduled Return exceeds native bound"));
        }
        let result = result
            .map(|json| {
                if h.result_digest.as_deref()
                    != Some(format!("{:x}", Sha256::digest(json.as_bytes())).as_str())
                {
                    return Err(storage("scheduled Return digest differs"));
                }
                decode(&json, "scheduled Return")
            })
            .transpose()?;
        Ok(Some(ScheduledWorkflowRecovery {
            start: h,
            effects,
            result,
            next_effect_ordinal,
            capacity_blocked: blocked,
            deletion_blocked: blocked,
        }))
    }
    #[allow(clippy::too_many_lines)] // One exhaustive native proof interpretation.
    fn scheduled_workflow_recover_effects(
        &self,
        id: &ScheduledWorkflowStartId,
        after: u64,
        limit: u32,
        retired: &ScheduledWorkflowOwnerRetirement,
    ) -> Result<(), ConversationStoreError> {
        if retired.start_id != *id {
            return Err(storage("foreign scheduled retirement proof"));
        }
        let Some(start) = self.scheduled_workflow_get(id)? else {
            return Ok(());
        };
        if !matches!(start.head.state, ScheduledWorkflowState::Claimed { .. }) {
            return Ok(());
        }
        let record = start;
        if retired.interactions_closed || retired.recovered_writer {
            self.close_scheduled_interactions(id)?;
        }
        let Some(page) = self.scheduled_workflow_recovery(id, after, limit)? else {
            return Ok(());
        };
        for effect in page.effects {
            if effect.physical_settlement_proven {
                continue;
            }
            let proven = match &effect.owner {
                ScheduledEffectOwner::Agent { activation } => {
                    let mut c = self.lock()?;
                    let tx = c.transaction().map_err(sql)?;
                    let ownership = super::journal::find_event_by_id(
                        &tx,
                        &crate::runtime::subagent::subagent_ownership_event_id(activation),
                    )?;
                    let owns=ownership.is_some_and(|event|matches!(event.event,RuntimeEvent::SubagentOwnershipCommitted{origin:crate::runtime::subagent::AgentActivationOrigin::Workflow{node_id},..} if *node_id==effect.node));
                    let terminal = super::journal::find_event_by_id(
                        &tx,
                        &crate::runtime::subagent::terminal_settlement_event_id(activation),
                    )?;
                    let physical = super::journal::find_event_by_id(
                        &tx,
                        &crate::runtime::subagent::physical_settlement_event_id(activation),
                    )?;
                    owns && terminal.as_ref().is_some_and(|event| {
                        matches!(event.event, RuntimeEvent::SubagentTerminalSettled { .. })
                    }) && (terminal.is_some_and(|event| {
                        matches!(
                            event.event,
                            RuntimeEvent::SubagentTerminalSettled {
                                physical_settlement_proven: true,
                                ..
                            }
                        )
                    }) || physical.is_some_and(|event| {
                        matches!(
                            event.event,
                            RuntimeEvent::SubagentPhysicalSettlementProven { .. }
                        )
                    }))
                }
                ScheduledEffectOwner::Model { call_id } => {
                    let cut = self.auxiliary_audit_cut()?;
                    self.load_auxiliary_call(call_id,cut)?.is_some_and(|call|{
                        let expected=format!("{:x}",Sha256::digest(serde_json::to_vec(&effect.node).expect("native node identity")));
                        matches!(call.business_source,crate::model::AuxiliaryBusinessSource::Business{namespace,reference} if namespace.as_str()=="workflow_node_instance" && reference.as_str()==expected) && call.settled_sequence.is_some() && call.outcome.is_some()
                    })
                }
                ScheduledEffectOwner::Preparing => match effect.kind {
                    ScheduledEffectKind::Agent => retired.agent_domain_quiescent,
                    ScheduledEffectKind::Tool | ScheduledEffectKind::Model => {
                        retired.recovered_writer
                    }
                    ScheduledEffectKind::Review => {
                        retired.interactions_closed || retired.recovered_writer
                    }
                },
                ScheduledEffectOwner::Review => {
                    retired.interactions_closed || retired.recovered_writer
                }
                ScheduledEffectOwner::Tool { .. } => {
                    let mut path = effect.node.block.definition.blocks.clone();
                    path.push(effect.node.node.clone());
                    retired.recovered_writer && record.contract.as_ref().is_some_and(|pin|pin.dependencies.iter().any(|dependency|matches!(dependency,crate::runtime::workflow::scheduled::ScheduledWorkflowDependency::Tool{path:at,selector:crate::capabilities::selection::ExactToolSelector::Builtin{name},..} if at==&path && matches!(name.as_str(),"read"|"read_image"|"present"|"write"|"edit"|"glob"|"grep"|"ask_user"))))
                }
            };
            if proven {
                let ScheduledWorkflowState::Claimed { owner, .. } = &record.head.state else {
                    unreachable!("checked native recovery state")
                };
                update_effect(self, id, owner, &effect.node, |row| {
                    row.physical_settlement_proven = true;
                    row.diagnostic = Some("exact native owner quiescence proven".into());
                })?;
            } else if effect.diagnostic.is_none() {
                let reason = match &effect.owner {
                    ScheduledEffectOwner::Preparing => {
                        "recovery_blocked: native preparation lacks a positive retirement proof; inspect the original native owner"
                    }
                    ScheduledEffectOwner::Agent { .. } => {
                        "recovery_blocked: exact Agent terminal publication or native physical settlement proof is missing"
                    }
                    ScheduledEffectOwner::Model { .. } => {
                        "recovery_blocked: exact Auxiliary Call has no confirmed native terminal audit"
                    }
                    ScheduledEffectOwner::Tool { .. } => {
                        "recovery_blocked: this Tool has no retained positive native physical proof; use its supported owner controls, never infer retirement from process absence"
                    }
                    ScheduledEffectOwner::Review => {
                        "recovery_blocked: the native interaction scope has not retired"
                    }
                };
                let ScheduledWorkflowState::Claimed { owner, .. } = &record.head.state else {
                    unreachable!("native recovery state")
                };
                update_effect(self, id, owner, &effect.node, |row| {
                    row.diagnostic = Some(reason.into());
                })?;
            }
        }
        Ok(())
    }
    fn scheduled_workflow_blocked(
        &self,
        id: &ScheduledWorkflowStartId,
        reason: &str,
    ) -> Result<(), ConversationStoreError> {
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        if let Some(mut h) = head(&tx, id)?
            && matches!(h.state, ScheduledWorkflowState::Claimed { .. })
        {
            let reason = bounded_reason(reason);
            if h.recovery_blocked.as_ref() != Some(&reason) {
                h.recovery_blocked = Some(reason);
                save(&tx, &h)?;
            }
        }
        tx.commit().map_err(sql)
    }
    fn scheduled_workflow_settle(
        &self,
        id: &ScheduledWorkflowStartId,
        outcome: ScheduledWorkflowOutcome,
        proof: ScheduledWorkflowQuiescence,
        result: Option<&serde_json::Value>,
        now: DateTime<Utc>,
    ) -> Result<ScheduledWorkflowSettlement, ConversationStoreError> {
        let mut c = self.lock()?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let Some(mut h) = head(&tx, id)? else {
            return Ok(ScheduledWorkflowSettlement::AlreadySettled);
        };
        let ScheduledWorkflowState::Claimed {
            effects_started, ..
        } = h.state
        else {
            return Ok(ScheduledWorkflowSettlement::AlreadySettled);
        };
        let unproven:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM scheduled_workflow_effects WHERE start_id=?1 AND coalesce(json_extract(effect_json,'$.physical_settlement_proven'),0)!=1)",[id.as_str()],|r|r.get(0)).map_err(sql)?;
        match proof.source {
            QuiescenceSource::RecoveredNoEffects if effects_started => {
                return Ok(ScheduledWorkflowSettlement::AwaitingNativeProof);
            }
            QuiescenceSource::RecoveredOwners | QuiescenceSource::NativeExecutor if unproven => {
                return Ok(ScheduledWorkflowSettlement::AwaitingNativeProof);
            }
            _ => {}
        }
        let pending_interactions:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM events r WHERE r.event_id GLOB ?1 AND NOT EXISTS(SELECT 1 FROM events s WHERE s.event_id=replace(r.event_id,'interaction-requested-event:','interaction-settled-event:')))",[format!("interaction-requested-event:scheduled:{id}:*")],|r|r.get(0)).map_err(sql)?;
        if pending_interactions {
            return Ok(ScheduledWorkflowSettlement::AwaitingNativeProof);
        }
        if matches!(outcome, ScheduledWorkflowOutcome::Completed) {
            if !matches!(proof.source, QuiescenceSource::NativeExecutor) {
                return Err(storage(
                    "recovery cannot manufacture successful Workflow output",
                ));
            }
            let value =
                result.ok_or_else(|| storage("completed Workflow lost its Return value"))?;
            let json = encode(value, "scheduled Workflow result")?;
            if json.len() > 65536 {
                return Err(ConversationStoreError::ResourceBoundExceeded(
                    "scheduled Return value".into(),
                ));
            }
            h.result_digest = Some(format!("{:x}", Sha256::digest(json.as_bytes())));
            tx.execute(
                "UPDATE scheduled_workflow_starts SET result_json=?2 WHERE start_id=?1",
                params![id.as_str(), json],
            )
            .map_err(sql)?;
        } else if result.is_some() {
            return Err(storage("failed Workflow cannot publish a Return value"));
        }
        h.state = ScheduledWorkflowState::Settled {
            outcome,
            settled_at: now,
        };
        h.native_quiescence_proven = true;
        h.recovery_blocked = None;
        save(&tx, &h)?;
        crate::runtime::process_death::reach("before:scheduled_workflow_settle");
        tx.commit().map_err(sql)?;
        crate::runtime::process_death::reach("after:scheduled_workflow_settle");
        Ok(ScheduledWorkflowSettlement::Settled)
    }
}
fn update_effect(
    store: &SqliteConversationStore,
    id: &ScheduledWorkflowStartId,
    owner: &str,
    node: &WorkflowNodeInstance,
    change: impl FnOnce(&mut ScheduledWorkflowEffect),
) -> Result<(), ConversationStoreError> {
    let mut c = store.lock()?;
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    let h = head(&tx, id)?.ok_or_else(|| storage("scheduled owner disappeared"))?;
    if !matches!(&h.state,ScheduledWorkflowState::Claimed{owner:held,..} if held==owner) {
        return Err(storage("scheduled effect owner changed"));
    }
    let json:String=tx.query_row("SELECT effect_json FROM scheduled_workflow_effects WHERE start_id=?1 AND node_key=?2 AND length(CAST(effect_json AS BLOB))<=4096",params![h.id.as_str(),node_key(node)],|r|r.get(0)).map_err(sql)?;
    let mut effect: ScheduledWorkflowEffect = decode(&json, "scheduled effect")?;
    change(&mut effect);
    let json = encode(&effect, "scheduled effect")?;
    if json.len() > 4096 {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "scheduled effect".into(),
        ));
    }
    tx.execute(
        "UPDATE scheduled_workflow_effects SET effect_json=?3 WHERE start_id=?1 AND node_key=?2",
        params![h.id.as_str(), node_key(node), json],
    )
    .map_err(sql)?;
    tx.commit().map_err(sql)
}

/// Durable Claim and required effect receipt, never best-effort Workflow events,
/// authorize an interaction without a Human Attempt/model turn.
pub(super) fn validate_interaction_owner(
    tx: &Transaction<'_>,
    conversation: &crate::runtime::identity::ConversationId,
    id: &crate::runtime::identity::InteractionId,
    subject: &crate::events::interaction::InteractionSubject,
    settlement: bool,
) -> Result<(), ConversationStoreError> {
    use crate::events::interaction::InteractionSubject;
    let node = match subject {
        InteractionSubject::Approval {
            invocation_id: crate::tools::types::ToolInvocationId::Workflow { node },
            ..
        }
        | InteractionSubject::Questionnaire {
            invocation_id: crate::tools::types::ToolInvocationId::Workflow { node },
            ..
        } => node.as_ref(),
        InteractionSubject::Review { review } => review.instance.as_ref(),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "scheduled interaction lacks a native Workflow node".into(),
            ));
        }
    };
    let WorkflowRunOrigin::Scheduled { start_id } = &node.block.run.origin else {
        return Err(ConversationStoreError::InvalidReference(
            "interaction lacks a real Scheduled origin".into(),
        ));
    };
    let h = head(tx, start_id)?
        .ok_or_else(|| storage("scheduled interaction has no admitted start"))?;
    if &node.block.run.conversation_id != conversation
        || h.run_id != node.block.run
        || !matches!(h.state, ScheduledWorkflowState::Claimed { .. })
        || !id.as_str().starts_with(&format!("scheduled:{start_id}:"))
    {
        return Err(ConversationStoreError::InvalidReference(
            "scheduled interaction differs from its durable Claim".into(),
        ));
    }
    if !settlement && h.cancellation_requested_at.is_some() {
        return Err(ConversationStoreError::OperationCancelled);
    }
    if !settlement && let InteractionSubject::Approval { invocation_id, .. } = subject {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                [super::journal::reference::native_approval_key(
                    invocation_id,
                )],
                |r| r.get(0),
            )
            .map_err(sql)?;
        if exists {
            return Err(storage(
                "scheduled native invocation already requested approval",
            ));
        }
    }
    let key = node_key(node);
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM scheduled_workflow_effects WHERE start_id=?1 AND node_key=?2)",params![start_id.as_str(),key],|r|r.get(0)).map_err(sql)?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(
            "scheduled interaction has no required native effect receipt".into(),
        ));
    }
    Ok(())
}

impl SqliteConversationStore {
    /// The old graph/requester is fenced out by the native caller before this
    /// cancellation audit is committed. This never reopens a prompt waiter.
    fn close_scheduled_interactions(
        &self,
        id: &ScheduledWorkflowStartId,
    ) -> Result<(), ConversationStoreError> {
        use crate::durable::inbox::ConversationInteractionAudit;
        let requests = {
            let c = self.lock()?;
            let mut query=c.prepare("SELECT r.event_json FROM events r WHERE r.event_id GLOB ?1 AND NOT EXISTS(SELECT 1 FROM events s WHERE s.event_id=replace(r.event_id,'interaction-requested-event:','interaction-settled-event:')) ORDER BY r.sequence LIMIT 100").map_err(sql)?;
            query
                .query_map(
                    [format!("interaction-requested-event:scheduled:{id}:*")],
                    |r| r.get::<_, String>(0),
                )
                .map_err(sql)?
                .map(|r| {
                    decode::<crate::events::types::RuntimeEventEnvelope>(
                        &r.map_err(sql)?,
                        "scheduled interaction request",
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        for mut request in requests {
            let crate::events::types::RuntimeEvent::InteractionRequested { interaction_id, .. } =
                &request.event
            else {
                return Err(storage("invalid native interaction index"));
            };
            request.event_id =
                crate::runtime::interaction::interaction_settled_event_id(interaction_id);
            request.event = crate::events::types::RuntimeEvent::InteractionSettled {
                interaction_id: interaction_id.clone(),
                settlement: crate::events::interaction::InteractionSettlement::Cancelled {
                    reason: crate::runtime::types::CancellationReason::RuntimeShutdown,
                },
            };
            request.sequence = 0;
            self.commit_interaction_settled(request)?;
        }
        Ok(())
    }
}
