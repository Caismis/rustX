//! Bounded auxiliary audit reads at a captured cut.

use super::{
    AUXILIARY_AUDIT_PAGE_LIMIT_MAX, AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
    AUXILIARY_CALL_REQUESTS_SQL, AuditFactRow, AuxiliaryAuditCut, AuxiliaryAuditFact,
    AuxiliaryAuditPage, AuxiliaryAuditRecord, AuxiliaryCallAudit, AuxiliaryCallId,
    AuxiliaryRequestAudit, ConversationStoreError, ModelRequestOwner, OptionalExtension, RequestId,
    SqliteConversationStore, audit_fact_row, audit_record_bytes, audit_total_at_connection,
    bounded_encoded_len, call_audit_at_tx, event_at_connection, has_call_journal_fact_through_tx,
    has_request_journal_fact_through_tx, journal_prefix, owner_row_query, params,
    request_audit_at_tx, request_row_query, seq_to_i64, sequence_from_i64, storage,
    validate_audit_fact_event, validate_owner_row_tx,
};

impl SqliteConversationStore {
    pub(in crate::durable::sqlite) fn auxiliary_audit_cut(
        &self,
    ) -> Result<AuxiliaryAuditCut, ConversationStoreError> {
        let connection = self.lock()?;
        Ok(AuxiliaryAuditCut {
            journal: journal_prefix(&connection)?,
        })
    }

    pub(in crate::durable::sqlite) fn auxiliary_audit_total(
        &self,
        cut: AuxiliaryAuditCut,
    ) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        audit_total_at_connection(&connection, cut)
    }

    pub(in crate::durable::sqlite) fn read_auxiliary_audit(
        &self,
        cut: AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<AuxiliaryAuditPage, ConversationStoreError> {
        if limit == 0 || limit > AUXILIARY_AUDIT_PAGE_LIMIT_MAX {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary audit limit must be 1..={AUXILIARY_AUDIT_PAGE_LIMIT_MAX}"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("auxiliary audit snapshot: {error}")))?;
        // Pin one WAL snapshot for the cut, cursor and related authority rows.
        let total = audit_total_at_connection(&transaction, cut)?;
        let after = after_offset.unwrap_or(0);
        if after > total {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit offset is after its captured cut".into(),
            ));
        }
        let mut previous_journal = 0;
        if after > 0 {
            let anchor_sequence: Option<i64> = transaction
                .query_row(
                    "SELECT sequence FROM auxiliary_audit_facts
                     WHERE audit_offset=?1 AND sequence<=?2",
                    params![
                        i64::try_from(after)
                            .map_err(|_| storage("auxiliary audit cursor overflow"))?,
                        seq_to_i64(cut.journal)?,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("auxiliary audit cursor anchor: {error}")))?;
            let anchor_sequence = anchor_sequence.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary audit cursor does not name a visible dense record".into(),
                )
            })?;
            let anchor = audit_fact_row(&transaction, sequence_from_i64(anchor_sequence)?)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary audit cursor anchor has no indexed row".into(),
                    )
                })?;
            if anchor.audit_offset != after {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary audit cursor anchor has a conflicting offset".into(),
                ));
            }
            previous_journal = anchor.sequence;
            let event = event_at_connection(&transaction, anchor.sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary audit cursor anchor has no Journal fact".into(),
                )
            })?;
            validate_audit_fact_event(&event, &anchor)?;
            match anchor.kind.as_str() {
                "call_started" | "call_settled" => {
                    if call_audit_at_tx(&transaction, &self.conversation_id, &anchor.call_id, cut)?
                        .is_none()
                    {
                        return Err(ConversationStoreError::InvalidReference(
                            "auxiliary audit cursor Call anchor has no audit projection".into(),
                        ));
                    }
                }
                "request_started" | "request_settled" | "retry_scheduled" => {
                    let request_id = anchor.request_id.as_ref().ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary audit cursor request anchor has no RequestId".into(),
                        )
                    })?;
                    if request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?
                        .is_none()
                    {
                        return Err(ConversationStoreError::InvalidReference(
                            "auxiliary audit cursor request anchor has no audit projection".into(),
                        ));
                    }
                }
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "auxiliary audit cursor anchor has an unknown kind".into(),
                    ));
                }
            }
        }
        let requested =
            i64::try_from(limit + 1).map_err(|_| storage("auxiliary audit page limit overflow"))?;
        let mut statement = transaction
            .prepare(
                "SELECT sequence,audit_offset,kind,call_id,request_id,ordinal
                 FROM auxiliary_audit_facts
                 WHERE audit_offset>?1 AND audit_offset<=?4 AND sequence<=?2
                 ORDER BY audit_offset LIMIT ?3",
            )
            .map_err(|error| storage(format!("auxiliary audit page query: {error}")))?;
        let rows = statement
            .query_map(
                params![
                    i64::try_from(after).map_err(|_| storage("auxiliary audit cursor overflow"))?,
                    seq_to_i64(cut.journal)?,
                    requested,
                    i64::try_from(total).map_err(|_| storage("auxiliary audit total overflow"))?,
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .map_err(|error| storage(format!("auxiliary audit page rows: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("collect auxiliary audit page: {error}")))?;
        let has_more = rows.len() > limit;
        let selected = &rows[..rows.len().min(limit)];
        if total > after && selected.is_empty() {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit total has no corresponding indexed facts".into(),
            ));
        }
        let mut records = Vec::with_capacity(selected.len());
        let mut expected_offset = after.checked_add(1).ok_or_else(|| {
            ConversationStoreError::InvalidReference("auxiliary audit cursor overflow".into())
        })?;
        for (sequence, audit_offset, kind, call_id, request_id, ordinal) in selected {
            let row = AuditFactRow {
                sequence: sequence_from_i64(*sequence)?,
                audit_offset: u64::try_from(*audit_offset).map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary audit offset is negative".into(),
                    )
                })?,
                kind: kind.clone(),
                call_id: AuxiliaryCallId::parse(call_id.clone())
                    .map_err(ConversationStoreError::InvalidReference)?,
                request_id: request_id.clone().map(RequestId::new),
                ordinal: ordinal
                    .map(|value| {
                        u32::try_from(value).map_err(|_| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary audit ordinal is out of range".into(),
                            )
                        })
                    })
                    .transpose()?,
            };
            if row.audit_offset != expected_offset {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary audit offsets are not dense and advancing".into(),
                ));
            }
            if row.sequence <= previous_journal {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary audit records are not in Journal order".into(),
                ));
            }
            expected_offset = expected_offset.checked_add(1).ok_or_else(|| {
                ConversationStoreError::InvalidReference("auxiliary audit offset overflow".into())
            })?;
            previous_journal = row.sequence;
            let event = event_at_connection(&transaction, row.sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary audit fact has no Journal event".into(),
                )
            })?;
            validate_audit_fact_event(&event, &row)?;
            let fact = match row.kind.as_str() {
                "call_started" => AuxiliaryAuditFact::CallStarted(
                    call_audit_at_tx(&transaction, &self.conversation_id, &row.call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary Call start fact has no audit row".into(),
                            )
                        })?,
                ),
                "call_settled" => AuxiliaryAuditFact::CallSettled(
                    call_audit_at_tx(&transaction, &self.conversation_id, &row.call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary Call settlement fact has no audit row".into(),
                            )
                        })?,
                ),
                "request_started" | "request_settled" | "retry_scheduled" => {
                    let request_id = row.request_id.as_ref().ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary request audit fact has no RequestId".into(),
                        )
                    })?;
                    let audit =
                        request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?
                            .ok_or_else(|| {
                                ConversationStoreError::InvalidReference(
                                    "auxiliary request audit fact has no request row".into(),
                                )
                            })?;
                    if row.ordinal != Some(audit.ordinal) {
                        return Err(ConversationStoreError::InvalidReference(
                            "auxiliary audit ordinal disagrees with its request".into(),
                        ));
                    }
                    if row.kind == "request_started" {
                        AuxiliaryAuditFact::RequestStarted(audit)
                    } else if row.kind == "retry_scheduled" {
                        AuxiliaryAuditFact::RetryScheduled(audit)
                    } else {
                        AuxiliaryAuditFact::RequestSettled(audit)
                    }
                }
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "unknown auxiliary audit fact kind".into(),
                    ));
                }
            };
            let record = AuxiliaryAuditRecord {
                audit_offset: row.audit_offset,
                journal_sequence: row.sequence,
                fact,
            };
            audit_record_bytes(&record)?;
            records.push(record);
        }
        let next_offset = if has_more {
            Some(records.last().expect("nonempty page").audit_offset)
        } else {
            None
        };
        Ok(AuxiliaryAuditPage {
            cut,
            total,
            records,
            next_offset,
        })
    }

    pub(in crate::durable::sqlite) fn load_auxiliary_submission(
        &self,
        correlation: &str,
    ) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError> {
        if correlation.is_empty() || correlation.len() > 128 {
            return Err(ConversationStoreError::InvalidReference(
                "invalid auxiliary correlation".into(),
            ));
        }
        let mut connection = self.lock()?;
        #[cfg(test)]
        {
            let hook = lookup_hooks().lock().unwrap().remove(correlation);
            if let Some(hook) = hook {
                hook();
            }
        }

        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let call: Option<String> = transaction.query_row(
            "SELECT json_extract(event_json,'$.event.call_id') FROM events INDEXED BY events_auxiliary_submission_idx WHERE json_extract(event_json,'$.event.type')='auxiliary_call_started' AND json_extract(event_json,'$.event.business_source.kind')='submission' AND json_extract(event_json,'$.event.business_source.identity.correlation')=?1",
            [correlation], |row| row.get(0),
        ).optional().map_err(|error| storage(error.to_string()))?;
        let Some(call) = call else {
            return Ok(None);
        };
        let call =
            AuxiliaryCallId::parse(call).map_err(ConversationStoreError::InvalidReference)?;
        call_audit_at_tx(
            &transaction,
            &self.conversation_id,
            &call,
            AuxiliaryAuditCut {
                journal: journal_prefix(&transaction)?,
            },
        )?
        .map_or_else(
            || {
                Err(ConversationStoreError::InvalidReference(
                    "submission has no local Call index".into(),
                ))
            },
            |call| Ok(Some(call)),
        )
    }

    pub(in crate::durable::sqlite) fn load_auxiliary_call(
        &self,
        call_id: &AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError> {
        let connection = self.lock()?;
        let current = journal_prefix(&connection)?;
        if cut.journal > current {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit cut is in the future".into(),
            ));
        }
        let audit = call_audit_at_tx(&connection, &self.conversation_id, call_id, cut)?;
        if audit.is_none() {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|error| storage(format!("auxiliary Call audit probe: {error}")))?;
            if has_call_journal_fact_through_tx(&transaction, call_id, cut.journal)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} has Journal evidence without its audit index"
                )));
            }
        }
        Ok(audit)
    }

    pub(in crate::durable::sqlite) fn read_auxiliary_call_requests(
        &self,
        call_id: &AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuxiliaryRequestAudit>, ConversationStoreError> {
        if !(1..=AUXILIARY_AUDIT_PAGE_LIMIT_MAX).contains(&limit) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary Call request limit must be 1..={AUXILIARY_AUDIT_PAGE_LIMIT_MAX}"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("auxiliary Call request snapshot: {error}")))?;
        if cut.journal > journal_prefix(&transaction)? {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit cut is in the future".into(),
            ));
        }
        if after_started_sequence.is_some_and(|after| after == 0 || after > cut.journal) {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call request cursor is outside its captured cut".into(),
            ));
        }
        let call = call_audit_at_tx(&transaction, &self.conversation_id, call_id, cut)?;
        if call.is_none() {
            if after_started_sequence.is_some()
                || has_call_journal_fact_through_tx(&transaction, call_id, cut.journal)?
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request lookup has no visible Call authority".into(),
                ));
            }
            return Ok(Vec::new());
        }
        let mut next_ordinal = Some(0_u32);
        if let Some(after) = after_started_sequence {
            let anchor = audit_fact_row(&transaction, after)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor has no audit fact".into(),
                )
            })?;
            if anchor.kind != "request_started" || anchor.call_id != *call_id {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor must name this Call's request start".into(),
                ));
            }
            let request_id = anchor.request_id.as_ref().ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor has no request identity".into(),
                )
            })?;
            let request =
                request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call request cursor has no visible request authority".into(),
                        )
                    })?;
            if request.call_id != *call_id
                || request.started_sequence != after
                || Some(request.ordinal) != anchor.ordinal
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request cursor disagrees with its request".into(),
                ));
            }
            next_ordinal = request.ordinal.checked_add(1);
        }
        let rows = transaction
            .prepare_cached(AUXILIARY_CALL_REQUESTS_SQL)
            .map_err(|error| storage(format!("auxiliary Call request lookup: {error}")))?
            .query_map(
                params![
                    call_id.as_str(),
                    seq_to_i64(after_started_sequence.unwrap_or(0))?,
                    seq_to_i64(cut.journal)?,
                    i64::try_from(limit)
                        .map_err(|_| storage("auxiliary request limit overflow"))?,
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .map_err(|error| storage(format!("auxiliary Call request query: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("auxiliary Call request row: {error}")))?;
        let mut requests = Vec::with_capacity(rows.len());
        for (sequence, request_id, ordinal) in rows {
            let request = request_audit_at_tx(
                &transaction,
                &self.conversation_id,
                &RequestId::new(request_id),
                cut,
            )?
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary Call request fact has no visible request authority".into(),
                )
            })?;
            if request.call_id != *call_id
                || request.started_sequence != sequence_from_i64(sequence)?
                || request.ordinal != ordinal
                || Some(ordinal) != next_ordinal
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request facts disagree with sequential request authority"
                        .into(),
                ));
            }
            bounded_encoded_len(
                &request,
                AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
                "auxiliary Call request audit",
            )?;
            next_ordinal = ordinal.checked_add(1);
            requests.push(request);
        }
        if requests.len() < limit
            && let Some(ordinal) = next_ordinal
        {
            let indexed_start: Option<i64> = transaction
                .query_row(
                    "SELECT started_sequence FROM auxiliary_requests
                     WHERE call_id=?1 AND ordinal=?2
                     UNION ALL
                     SELECT start_sequence FROM request_owners
                     INDEXED BY request_owners_source_idx
                     WHERE owner_kind='auxiliary' AND call_id=?1 AND auxiliary_ordinal=?2
                     ORDER BY 1 LIMIT 1",
                    params![call_id.as_str(), ordinal],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    storage(format!("auxiliary Call request completeness: {error}"))
                })?;
            if indexed_start
                .map(sequence_from_i64)
                .transpose()?
                .is_some_and(|sequence| sequence <= cut.journal)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request page is missing indexed request evidence".into(),
                ));
            }
        }
        Ok(requests)
    }

    pub(in crate::durable::sqlite) fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryRequestAudit>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("auxiliary request audit snapshot: {error}")))?;
        // The request, Call and owner rows must describe one committed state.
        if cut.journal > journal_prefix(&transaction)? {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit cut is in the future".into(),
            ));
        }
        let audit = request_audit_at_tx(&transaction, &self.conversation_id, request_id, cut)?;
        if audit.is_none() {
            if let Some(owner) = owner_row_query(&transaction, request_id)? {
                validate_owner_row_tx(&transaction, &self.conversation_id, request_id, &owner)?;
                if matches!(owner.owner, ModelRequestOwner::Agent { .. }) {
                    return Ok(None);
                }
                if request_row_query(&transaction, "request_id=?1", [request_id.as_str()])?
                    .is_some_and(|request| request.started_sequence > cut.journal)
                {
                    return Ok(None);
                }
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {request_id} owner has no request index"
                )));
            }
            if has_request_journal_fact_through_tx(&transaction, request_id, cut.journal)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {request_id} has Journal evidence without its audit index"
                )));
            }
        }
        Ok(audit)
    }
}

#[cfg(test)]
type LookupHook = Box<dyn FnOnce() + Send>;
#[cfg(test)]
fn lookup_hooks() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, LookupHook>> {
    static HOOKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, LookupHook>>,
    > = std::sync::OnceLock::new();
    HOOKS.get_or_init(std::sync::Mutex::default)
}
#[cfg(test)]
pub(crate) fn install_lookup_hook(correlation: String, hook: LookupHook) {
    assert!(
        lookup_hooks()
            .lock()
            .unwrap()
            .insert(correlation, hook)
            .is_none()
    );
}
