//! Operations on one exact attachment's live runtime. The connection owner
//! has already admitted the request and started the native operation; this
//! family runs inside that operation and maps its typed outcome to the wire.
use std::sync::Arc;

use super::Route;
use crate::app_server::host::AppServerHost;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError, UserInputBlock};
use crate::app_server::rpc_error::{
    client_error, domain, model_selection_error, session_error, source_read_error,
};
use crate::local_runtime::session_controller::SessionController;
use crate::runtime_client::types::{RuntimeClientError, RuntimeClientResult};

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // Explicit native operation owners.
pub(super) async fn dispatch_runtime(
    method: Method,
    route: Arc<Route>,
    authority: Arc<crate::runtime_client::host::ClientInner>,
    changed: Arc<tokio::sync::Notify>,
    sessions: SessionController,
    manager: crate::local_runtime::session_runtime_manager::SessionRuntimeManager,
    configuration: Option<
        crate::local_runtime::session_runtime_manager::ConfigurationObservationSource,
    >,
    upload_host: AppServerHost,
    observation_cancel: Option<crate::app_server::admission::ObservationControl>,
) -> Result<MethodResult, RpcError> {
    match method {
        Method::ConfigurationGet { target } => {
            let projection = tokio::task::spawn_blocking(move || {
                let view = configuration
                    .as_ref()
                    .and_then(|source| source.read(&manager, &target.session_id))
                    .ok_or("configuration source unavailable".to_owned())?;
                crate::local_runtime::configuration::effective_observation::bounded(&view)
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(|error| source_read_error(&error))?;
            Ok(MethodResult::EffectiveConfiguration {
                projection: Box::new(projection),
            })
        }
        Method::ConfigurationValueRead {
            target,
            reference,
            offset,
            max_bytes,
        } => {
            let chunk = tokio::task::spawn_blocking(move || {
                let view = configuration
                    .as_ref()
                    .and_then(|source| source.read(&manager, &target.session_id))
                    .ok_or("configuration source unavailable".to_owned())?;
                crate::local_runtime::configuration::effective_observation::read(
                    &view, &reference, offset, max_bytes,
                )
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(|error| source_read_error(&error))?;
            Ok(MethodResult::ConfigurationValueChunk { chunk })
        }
        Method::ConfigurationPage {
            target,
            revision,
            field,
            offset,
            limit,
        } => {
            let page = tokio::task::spawn_blocking(move || {
                let view = configuration
                    .as_ref()
                    .and_then(|source| source.read(&manager, &target.session_id))
                    .ok_or("configuration source unavailable".to_owned())?;
                crate::local_runtime::configuration::effective_observation::page(
                    &view, &revision, field, offset, limit,
                )
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(|error| source_read_error(&error))?;
            Ok(MethodResult::ConfigurationPage { page })
        }
        Method::AgentArtifactRead {
            agent_id,
            artifact_id,
            ..
        } => Ok(MethodResult::ArtifactBytes {
            data: authority
                .agent_artifact_read(&agent_id, &artifact_id)
                .map_err(client_error)?,
        }),
        Method::ArtifactRead {
            target: _,
            artifact_id,
        } => Ok(MethodResult::ArtifactBytes {
            data: authority
                .artifact_read(&artifact_id)
                .map_err(client_error)?,
        }),
        Method::SessionUploadPrepare {
            target: _,
            operation_id,
            files,
        } => {
            let transfer = crate::app_server::upload_transfer::prepare(
                &upload_host,
                route,
                operation_id,
                files,
            )
            .await
            .map_err(|_| domain(ErrorData::InvalidParams))?;
            Ok(MethodResult::UploadPrepared { transfer })
        }
        Method::SessionUploadStatus {
            target,
            operation_id,
        } => {
            let outcome = crate::app_server::upload_transfer::status(
                &upload_host,
                &target.session_id,
                &operation_id,
            )
            .await
            .map_err(|_| domain(ErrorData::InvalidParams))?;
            Ok(MethodResult::UploadStatus { outcome })
        }
        Method::ModelGet { target: _ } => read_domain(authority, "model".into()).await,
        Method::ModelCatalog { target: _ } => {
            read_domain(authority, "model_catalog.models".into()).await
        }
        Method::ModelSet { target, config } => {
            let (_, settings_revision) = manager
                .set_model(&target.session_id, *config)
                .await
                .map_err(model_selection_error)?;
            Ok(MethodResult::ModelSet { settings_revision })
        }
        Method::ModelSelect { target, choice } => {
            let (_, settings_revision) = manager
                .select_catalog_model(&target.session_id, choice)
                .await
                .map_err(model_selection_error)?;
            Ok(MethodResult::ModelSet { settings_revision })
        }
        Method::Capability { target: _ } => read_domain(authority, "capabilities".into()).await,
        Method::Projections { after, limit, .. } => {
            native_result(authority.projection_page(after, limit))
        }
        Method::ProjectionDetail {
            checkpoint_id,
            offset,
            limit,
            ..
        } => native_result(authority.projection_detail(checkpoint_id, offset, limit)),
        Method::Trace {
            target: _,
            records,
            before,
            limit,
        } => native_result(authority.trace_page(before, limit, records)),
        Method::TraceDetail {
            target: _,
            record_id,
        } => native_result(authority.trace_detail(record_id)),
        Method::TraceLocateTool { locator, .. } => {
            native_result(authority.trace_locate_tool(locator).await)
        }
        Method::Transcript {
            target: _,
            at,
            limit,
        } => Ok(MethodResult::TranscriptWindow {
            window: authority
                .conversation_window(&at, limit)
                .map_err(client_error)?,
        }),
        Method::ConversationTurns {
            target: _,
            offset,
            limit,
        } => Ok(MethodResult::ConversationTurns {
            page: authority
                .conversation_turns(offset, limit)
                .map_err(client_error)?,
        }),
        Method::Goal {
            control: crate::goal::GoalControl::Show,
            ..
        } => read_domain(authority, "goal".into()).await,
        Method::Goal { target: _, control } => native_result(authority.goal_control(control)),
        Method::JobStatus { job_id, .. } => {
            read_domain(authority, format!("jobs.{}", job_id.as_str())).await
        }
        Method::JobList { .. } => read_domain(authority, "jobs".into()).await,
        Method::JobWait { job_id, .. } => native_result(authority.job_wait(&job_id).await),
        Method::JobCancel { job_id, .. } => native_result(authority.job_cancel_intent(&job_id)),
        Method::AgentObserve {
            agent_id, after, ..
        } => {
            let control = observation_cancel.expect("Observe owns synchronous request admission");
            let read = authority.agent_observe_with_deadline(&agent_id, after, control.deadline);
            let cancel = control.cancel;
            {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(domain(ErrorData::ObservationCancelled)),
                    result = read => native_result(result),
                }
            }
        }
        Method::AgentStatus { agent_id, .. } => {
            read_domain(authority, format!("agents.{}", agent_id.as_str())).await
        }
        Method::AgentList { .. } => read_domain(authority, "agents".into()).await,
        Method::AgentSendMessage {
            target,
            agent_id,
            message,
            attachments,
        } => {
            authority.agent_status(&agent_id).map_err(client_error)?;
            let uploads = manager
                .session_controller()
                .uploaded_content(&target.session_id, &attachments)
                .await
                .map_err(|_| domain(ErrorData::InvalidParams))?;
            let references = uploads
                .into_iter()
                .filter_map(|block| match block {
                    crate::message::types::UserContentBlock::UploadedFile(file) => Some(file),
                    _ => None,
                })
                .collect();
            native_result(
                authority
                    .agent_send_message(&agent_id, message, references)
                    .await,
            )
        }
        Method::AgentWait { agent_id, .. } => native_result(authority.agent_wait(&agent_id).await),
        Method::AgentInterrupt { agent_id, .. } => {
            native_result(authority.agent_interrupt_intent(&agent_id))
        }
        Method::AgentStatistics { agent_id, .. } => {
            native_result(authority.agent_statistics(&agent_id).await)
        }
        Method::AgentTrace {
            agent_id,
            before,
            limit,
            records,
            ..
        } => native_result(
            authority
                .agent_trace(&agent_id, before, limit, records)
                .await,
        ),
        Method::AgentTraceDetail {
            agent_id,
            record_id,
            ..
        } => native_result(authority.agent_trace_detail(&agent_id, record_id).await),
        Method::AgentTraceLocateTool {
            agent_id, locator, ..
        } => native_result(authority.agent_trace_locate_tool(&agent_id, locator).await),
        Method::AgentTranscript {
            agent_id,
            at,
            limit,
            ..
        } => match authority.agent_transcript_window(&agent_id, &at, limit) {
            Err(RuntimeClientError::RuntimeFailure { .. }) => {
                Err(domain(ErrorData::AgentHistoryUnavailable { agent_id }))
            }
            result => result
                .map(|window| MethodResult::TranscriptWindow { window })
                .map_err(client_error),
        },
        Method::AgentTurns {
            agent_id,
            offset,
            limit,
            ..
        } => match authority.agent_turns(&agent_id, offset, limit) {
            Err(RuntimeClientError::RuntimeFailure { .. }) => {
                Err(domain(ErrorData::AgentHistoryUnavailable { agent_id }))
            }
            result => result
                .map(|page| MethodResult::ConversationTurns { page })
                .map_err(client_error),
        },
        Method::CompactContext {
            target: _,
            request_id,
        } => native_result(authority.compact_context(Some(request_id)).await),
        Method::SessionBoundaries {
            target: _,
            offset,
            limit,
        } => {
            let (surface_revision, boundaries, next_offset) = authority
                .user_message_boundaries(offset, limit)
                .map_err(client_error)?;
            Ok(MethodResult::Boundaries {
                surface_revision,
                boundaries,
                next_offset,
            })
        }
        Method::ConversationCheckpointValueRead {
            target: _,
            agent_id,
            cut,
            domain: collection,
            item_offset,
            path,
            offset,
            max_bytes,
        } => {
            if path.len() > 32 || path.iter().any(|part| part.len() > 256) || collection.len() > 256
            {
                return Err(domain(ErrorData::InvalidParams));
            }
            let chunk = if let Some(agent_id) = agent_id {
                authority
                    .agent_checkpoint_value_read(
                        &agent_id,
                        &cut,
                        &collection,
                        item_offset,
                        &path,
                        (offset, max_bytes),
                    )
                    .await
            } else {
                authority
                    .checkpoint_value_read_async(
                        &cut,
                        &collection,
                        item_offset,
                        &path,
                        (offset, max_bytes),
                    )
                    .await
            }
            .map_err(client_error)?;
            Ok(MethodResult::CheckpointValueChunk { chunk })
        }
        Method::ConversationContentRead {
            target: _,
            agent_id,
            content_ref,
            offset,
            max_bytes,
        } => {
            let chunk = if let Some(agent_id) = agent_id {
                authority
                    .agent_content_read(&agent_id, &content_ref, offset, max_bytes)
                    .await
            } else {
                tokio::task::spawn_blocking(move || {
                    authority.content_read(&content_ref, offset, max_bytes)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            .map_err(client_error)?;
            Ok(MethodResult::ContentChunk { chunk })
        }
        Method::ConversationCheckpointPage {
            target: _,
            agent_id,
            cut,
            domain: collection,
            offset,
            limit,
        } => {
            let page = if let Some(agent_id) = agent_id {
                authority
                    .agent_checkpoint_page(&agent_id, &cut, &collection, offset, limit)
                    .await
            } else {
                authority
                    .checkpoint_page_async(&cut, &collection, offset, limit)
                    .await
            }
            .map_err(client_error)?;
            Ok(MethodResult::CheckpointPage { page })
        }
        Method::SessionCheckpoint {
            target: _,
            trace_records,
        } => {
            let checkpoint = tokio::task::spawn_blocking(move || {
                authority.checkpoint_with_trace(&trace_records)
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(client_error)?;
            Ok(MethodResult::Checkpoint {
                checkpoint: Box::new(checkpoint),
            })
        }
        Method::SessionSubscribe {
            target: _,
            after_cursor,
        } => {
            let registration = route.registration.replace(
                || {
                    route
                        .attachment
                        .subscribe_events(after_cursor)
                        .map_err(client_error)
                },
                after_cursor,
            )?;
            changed.notify_one();
            Ok(MethodResult::Subscribed {
                registration_id: registration.id,
                target: route.target.clone(),
                after_cursor,
            })
        }
        Method::TurnStart { target, content } | Method::TurnSteer { target, content } => {
            let receipts: Vec<_> = content
                .iter()
                .filter_map(|block| match block {
                    UserInputBlock::Upload(receipt) => Some(receipt.clone()),
                    UserInputBlock::Text(_) => None,
                })
                .collect();
            let mut uploads = sessions
                .uploaded_content(&target.session_id, &receipts)
                .await
                .map_err(session_error)?
                .into_iter();
            let mut canonical = Vec::with_capacity(content.len());
            for block in content {
                match block {
                    UserInputBlock::Text(text) => {
                        canonical.push(crate::message::types::UserContentBlock::Text(text));
                    }
                    UserInputBlock::Upload(_) => {
                        canonical.push(uploads.next().expect("validated receipt collection"));
                    }
                }
            }
            native_result(authority.submit_session_inbound(canonical))
        }
        Method::InboundEdit {
            target: _,
            expected,
            text,
        } => Ok(MethodResult::InboundMutation {
            outcome: authority
                .edit_pending(&expected, &text)
                .map_err(client_error)?,
        }),
        Method::InboundRemove {
            target: _,
            expected,
        } => Ok(MethodResult::InboundMutation {
            outcome: authority.remove_pending(&expected).map_err(client_error)?,
        }),
        Method::TurnCancel { target: _ } => native_result(authority.cancel_current_attempt()),
        Method::InteractionRespond {
            target: _,
            interaction,
            response,
        } => native_result(authority.respond_interaction(&interaction, response).await),
        Method::InteractionCancel {
            target: _,
            interaction,
        } => native_result(authority.cancel_interaction(&interaction).await),

        // `Family::of` routes only runtime operations here.
        _ => Err(domain(ErrorData::OperationFailed)),
    }
}

async fn read_domain(
    authority: Arc<crate::runtime_client::host::ClientInner>,
    collection: String,
) -> Result<MethodResult, RpcError> {
    native_result(
        tokio::task::spawn_blocking(move || authority.domain_page(&collection, 0, 32))
            .await
            .map_err(|_| {
                domain(ErrorData::ObservationUnavailable {
                    diagnostic: "domain materialization interrupted".into(),
                })
            })?,
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep the single native-result carrier mapping together"
)]
fn native_result(
    result: Result<RuntimeClientResult, RuntimeClientError>,
) -> Result<MethodResult, RpcError> {
    Ok(match result.map_err(client_error)? {
        RuntimeClientResult::DomainPage { page } => MethodResult::DomainPage { page },
        RuntimeClientResult::CheckpointValueChunk { chunk } => {
            MethodResult::CheckpointValueChunk { chunk }
        }
        RuntimeClientResult::InteractionFields { .. }
        | RuntimeClientResult::InteractionValueChunk { .. } => {
            return Err(domain(ErrorData::OperationFailed));
        }
        RuntimeClientResult::ContextCompacted { compaction } => {
            MethodResult::ContextCompacted { compaction }
        }
        RuntimeClientResult::ProjectionPage { page } => MethodResult::Projections { page },
        RuntimeClientResult::ProjectionDetail { detail } => {
            MethodResult::ProjectionDetail { detail }
        }
        RuntimeClientResult::TracePage { page } => MethodResult::Trace { page },
        RuntimeClientResult::TraceToolLocation { location } => {
            MethodResult::TraceToolLocation { location }
        }
        RuntimeClientResult::TranscriptPage { .. } => {
            unreachable!("App Server transcript reads use cut-bound windows")
        }
        RuntimeClientResult::TranscriptWindow { window } => {
            MethodResult::TranscriptWindow { window: *window }
        }
        RuntimeClientResult::ConversationTurns { page } => MethodResult::ConversationTurns { page },
        RuntimeClientResult::GoalMutation { reference } => MethodResult::GoalMutation { reference },
        RuntimeClientResult::JobCancellationAccepted { job_id } => {
            MethodResult::JobCancellationAccepted { job_id }
        }
        RuntimeClientResult::AgentInterruptionAccepted {
            agent_id,
            activation_id,
        } => MethodResult::AgentInterruptionAccepted {
            agent_id,
            activation_id,
        },
        RuntimeClientResult::JobOutcome { job_id, state } => {
            MethodResult::JobOutcome { job_id, state }
        }
        RuntimeClientResult::AgentObserve { observation } => {
            MethodResult::AgentObserve { observation }
        }
        RuntimeClientResult::AgentStatistics { metrics } => {
            MethodResult::AgentStatistics { metrics }
        }
        RuntimeClientResult::AgentMessage { accepted } => MethodResult::AgentMessage {
            agent_id: accepted.agent_id,
            activation_id: accepted.activation_id,
            resumed: accepted.resumed,
        },
        RuntimeClientResult::AgentWait {
            agent_id,
            activation_id,
            outcome,
        } => MethodResult::AgentWait {
            agent_id,
            activation_id,
            outcome,
        },

        RuntimeClientResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } => MethodResult::InboundAccepted {
            message_id,
            inbound_sequence,
        },
        RuntimeClientResult::AttemptCancellationAccepted { attempt_id } => {
            MethodResult::CancellationAccepted { attempt_id }
        }
        RuntimeClientResult::InteractionResponseAccepted { interaction } => {
            MethodResult::InteractionSettled { interaction }
        }

        _ => return Err(domain(ErrorData::OperationFailed)),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn job_publication_abandonment_preserves_typed_failure_and_identity() {
        let job_id = crate::runtime::identity::ToolExecutionId::new(
            "exec_0199c989-03a0-7000-8000-000000000001",
        );
        let error = super::native_result(Err(
            crate::runtime_client::types::RuntimeClientError::JobPublicationAbandoned {
                job_id: job_id.clone(),
            },
        ))
        .unwrap_err();
        assert_eq!(
            error.data,
            Some(super::ErrorData::JobPublicationAbandoned { job_id })
        );
        assert!(error.message.contains("no durable terminal result"));
    }

    #[test]
    fn job_listing_preserves_bounded_page_metadata_on_the_wire() {
        let page = crate::runtime_client::checkpoint::CheckpointPage {
            collections: Vec::new(),
            cursor: crate::runtime_client::types::RuntimeClientCursor::new(0),
            read_cut: crate::observation::cut::CheckpointReadCut {
                canonical_foreground: None,
                live_publication: None,
                collection_cuts: std::collections::BTreeMap::new(),
                conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                ),
                activation_id: None,
                revision: 0,
                through: 0,
                source_cut: None,
                domain_revisions: std::collections::BTreeMap::default(),
            },
            domain: "jobs".into(),
            total: 67,
            items: Vec::new(),
            next_offset: Some(32),
        };
        let result = super::native_result(Ok(
            crate::runtime_client::types::RuntimeClientResult::DomainPage { page },
        ))
        .unwrap();
        let super::MethodResult::DomainPage { page } = result else {
            panic!("page");
        };
        assert_eq!(page.total, 67);
        assert_eq!(page.next_offset, Some(32));
    }
}
