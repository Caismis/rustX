//! Bounded historical reads of a Session's committed lineage: history
//! windows, trace observations, checkpoint pages, content and statistics.
//!
//! Every read is fenced by the native observation cut of the selected node,
//! so a client cut never reaches past what the Store committed.
use super::AppServerConnection;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::{client_error, domain, manager_error, session_error};
use crate::runtime_client::types::RuntimeClientError;

impl AppServerConnection {
    /// Serve one admitted historical read.
    #[allow(clippy::too_many_lines)] // One arm per historical read.
    pub(super) async fn history(&self, method: Method) -> Result<MethodResult, RpcError> {
        match method {
            Method::SessionHistory {
                session_id,
                node_id,
                at,
                limit,
            } => {
                if limit == 0 || limit > 32 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, upper) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), None)
                    .await?;
                let at = history_anchor(at, upper.as_ref())?;
                let (conversation_id, window) = self
                    .sessions
                    .read_history(&session_id, Some(&node_id), at, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionHistory {
                    conversation_id,
                    window,
                })
            }
            Method::SessionTraceHistory {
                session_id,
                node_id,
                before,
                limit,
            } => {
                if limit == 0 || limit > crate::runtime_client::trace::TRACE_PAGE_LIMIT {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (conversation_id, page) = self
                    .sessions
                    .read_trace(&session_id, node_id.as_ref(), move |projection| {
                        projection.page(before.as_ref(), limit)
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionTraceHistory {
                    conversation_id,
                    page,
                })
            }
            Method::SessionTraceHistoryDetail {
                session_id,
                node_id,
                record_id,
            } => {
                if record_id.len() > 256 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, cut) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), None)
                    .await?;
                let (conversation_id, page) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        let cut = match cut {
                            Some(cut) => cut,
                            None => crate::runtime_client::trace_observation::history_cut(store)?,
                        };
                        crate::runtime_client::trace_observation::history_detail(
                            store, &cut, &record_id,
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionTraceHistoryDetail {
                    conversation_id,
                    page: page.map_err(client_error)?,
                })
            }
            Method::SessionTracePage {
                session_id,
                node_id,
                cut,
                domain: collection,
                offset,
                limit,
            } => {
                if limit == 0 || limit > 32 || offset > 9_007_199_254_740_991 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&cut))
                    .await?;
                let (_, page) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::history_page(
                            store,
                            &cut,
                            &collection,
                            offset,
                            limit,
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointPage {
                    page: page.map_err(client_error)?,
                })
            }
            Method::SessionTraceValueRead {
                session_id,
                node_id,
                cut,
                domain: collection,
                item_offset,
                path,
                offset,
                max_bytes,
            } => {
                if path.len() > 32
                    || max_bytes == 0
                    || max_bytes > 65_536
                    || offset > 9_007_199_254_740_991
                    || item_offset > 9_007_199_254_740_991
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&cut))
                    .await?;
                let (_, chunk) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::history_value_read(
                            store,
                            &cut,
                            &collection,
                            item_offset,
                            &path,
                            (offset, max_bytes),
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointValueChunk {
                    chunk: chunk.map_err(client_error)?,
                })
            }
            Method::SessionTraceContentRead {
                session_id,
                node_id,
                cut,
                content_ref,
                offset,
                max_bytes,
            } => {
                self.history_content_read(
                    session_id,
                    node_id,
                    cut,
                    content_ref,
                    (offset, max_bytes),
                )
                .await
            }
            Method::SessionHistoryContentRead {
                session_id,
                node_id,
                cut,
                content_ref,
                offset,
                max_bytes,
            } => {
                // The source is the exact native historical page cut, not an ACK
                // or a new observation frontier. Reuse native cold/Live cut checks.
                let cut = history_checkpoint_cut(cut);
                return self
                    .history_content_read(
                        session_id,
                        node_id,
                        cut,
                        content_ref,
                        (offset, max_bytes),
                    )
                    .await;
            }
            Method::SessionHistoryPage {
                session_id,
                node_id,
                cut,
                domain: collection,
                offset,
                limit,
            } => {
                if limit == 0
                    || limit > 32
                    || offset > 9_007_199_254_740_991
                    || collection.len() > 1024
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let evidence = history_checkpoint_cut(cut);
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&evidence))
                    .await?;
                let (_, page) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::validate_history_cut(
                            store, &evidence,
                        )?;
                        crate::runtime_client::history_observation::page(
                            store,
                            &evidence,
                            &collection,
                            offset,
                            limit,
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointPage {
                    page: page.map_err(client_error)?,
                })
            }
            Method::SessionHistoryValueRead {
                session_id,
                node_id,
                cut,
                domain: collection,
                item_offset,
                path,
                offset,
                max_bytes,
            } => {
                if path.len() > 32
                    || path.iter().any(|field| field.len() > 256)
                    || collection.len() > 1024
                    || max_bytes == 0
                    || max_bytes > 65_536
                    || offset > 9_007_199_254_740_991
                    || item_offset > 9_007_199_254_740_991
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let evidence = history_checkpoint_cut(cut);
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&evidence))
                    .await?;
                let (_, chunk) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::validate_history_cut(
                            store, &evidence,
                        )?;
                        crate::runtime_client::history_observation::value_read(
                            store,
                            &evidence,
                            &collection,
                            item_offset,
                            &path,
                            (offset, max_bytes),
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointValueChunk {
                    chunk: chunk.map_err(client_error)?,
                })
            }
            Method::SessionStatistics {
                session_id,
                node_id,
            } => {
                let reading = self
                    .sessions
                    .read_statistics(&session_id, node_id.as_ref())
                    .await
                    .map_err(session_error)?
                    .map_err(client_error)?;
                Ok(MethodResult::SessionStatistics {
                    conversation_id: reading.conversation_id,
                    statistics: reading.statistics,
                    occupancy: reading.occupancy,
                    read_cut: reading.read_cut,
                })
            }
            Method::SessionEditorRead {
                locator,
                block_index,
                offset,
                max_bytes,
            } => {
                crate::app_server::editor_read::read(
                    self.sessions.clone(),
                    locator,
                    block_index,
                    offset,
                    max_bytes,
                )
                .await
            }
            Method::SessionNodeOriginHistory {
                session_id,
                node_id,
                limit,
            } => {
                if limit == 0 || limit > 32 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (_destination, origin) = self
                    .sessions
                    .acquire_read_origin(&session_id, &node_id)
                    .await
                    .map_err(session_error)?;
                let (_, upper) = self
                    .history_trace_cut(&origin.source_session, Some(&origin.source_node), None)
                    .await?;
                let at = history_anchor(
                    crate::durable::reading::ConversationWindowAt::Canonical {
                        locator: origin.source_locator,
                        cut: None,
                    },
                    upper.as_ref(),
                )?;
                let (conversation_id, window) = self
                    .sessions
                    .read_history(&origin.source_session, Some(&origin.source_node), at, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionHistory {
                    conversation_id,
                    window,
                })
            }
            // `Family::of` routes only historical reads here.
            _ => Err(domain(ErrorData::OperationFailed)),
        }
    }

    async fn history_content_read(
        &self,
        session_id: crate::local_runtime::session::SessionId,
        node_id: Option<crate::local_runtime::session::SessionNodeId>,
        cut: crate::observation::cut::CheckpointReadCut,
        content_ref: crate::observation::content::ContentRef,
        range: (u64, u32),
    ) -> Result<MethodResult, RpcError> {
        let (offset, max_bytes) = range;
        if offset > 9_007_199_254_740_991
            || max_bytes == 0
            || max_bytes > 65_536
            || matches!(
                content_ref.source,
                crate::observation::content::ContentSource::LivePublicationPrefix { .. }
            )
        {
            return Err(domain(ErrorData::InvalidParams));
        }
        let (node_id, _) = self
            .history_trace_cut(&session_id, node_id.as_ref(), Some(&cut))
            .await?;
        let (_, chunk) = self
            .sessions
            .read_trace_observation(&session_id, Some(&node_id), move |store| {
                // Native source validation checks Conversation ownership,
                // immutable cut, representation and committed source frontier.
                crate::runtime_client::trace_observation::validate_history_cut(store, &cut)?;
                let source = cut
                    .source_cut
                    .as_ref()
                    .ok_or(RuntimeClientError::ReadCutExpired)?;
                store
                    .validate_content_source_cut(&content_ref, source)
                    .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
                store
                    .content_read(&content_ref, offset, max_bytes)
                    .map_err(RuntimeClientError::content_read_error)
            })
            .await
            .map_err(session_error)?;
        Ok(MethodResult::ContentChunk {
            chunk: chunk.map_err(client_error)?,
        })
    }

    async fn history_trace_cut(
        &self,
        session: &crate::local_runtime::session::SessionId,
        node: Option<&crate::local_runtime::session::SessionNodeId>,
        requested: Option<&crate::observation::cut::CheckpointReadCut>,
    ) -> Result<
        (
            crate::local_runtime::session::SessionNodeId,
            Option<crate::observation::cut::CheckpointReadCut>,
        ),
        RpcError,
    > {
        let selected = match node {
            Some(node) => node.clone(),
            None => {
                self.sessions
                    .read_session_summary(session)
                    .await
                    .map_err(session_error)?
                    .active_node
            }
        };
        let node = self
            .sessions
            .observe_node(session, &selected)
            .await
            .map_err(session_error)?;
        if requested.is_some_and(|cut| {
            cut.conversation_id != node.conversation_id || cut.activation_id.is_some()
        }) {
            return Err(domain(ErrorData::ReadCutExpired));
        }
        let manager = self.host.manager().clone();
        let upper = tokio::task::spawn_blocking(move || {
            manager.history_observation_cut(&node.conversation_id)
        })
        .await
        .map_err(|_| domain(ErrorData::OperationFailed))?
        .map_err(manager_error)?;
        if let (Some(requested), Some(upper)) = (requested, &upper) {
            let source = requested
                .source_cut
                .as_ref()
                .ok_or_else(|| domain(ErrorData::ReadCutExpired))?;
            let native = upper
                .source_cut
                .as_ref()
                .ok_or_else(|| domain(ErrorData::ReadCutExpired))?;
            if requested.through > upper.through
                || requested.revision > upper.revision
                || !source.read.reconstructible_from(&native.read)
                || source.inherited_through > native.inherited_through
            {
                return Err(domain(ErrorData::ReadCutExpired));
            }
        }
        Ok((selected, upper))
    }
}

fn history_checkpoint_cut(
    cut: crate::durable::observation::ObservationReadCut,
) -> crate::observation::cut::CheckpointReadCut {
    crate::observation::cut::CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        conversation_id: cut.read.conversation_id.clone(),
        activation_id: None,
        revision: 0,
        through: cut.read.journal,
        source_cut: Some(cut),
        domain_revisions: std::collections::BTreeMap::new(),
        collection_cuts: std::collections::BTreeMap::new(),
    }
}

fn history_anchor(
    at: crate::durable::reading::ConversationWindowAt,
    upper: Option<&crate::observation::cut::CheckpointReadCut>,
) -> Result<crate::durable::reading::ConversationWindowAt, RpcError> {
    use crate::durable::reading::ConversationWindowAt as At;
    if matches!(&at, At::Canonical { locator: crate::observation::content::CanonicalContentLocator::ByMessageId { message_id }, .. } if message_id.as_str().len()>256)
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    let Some(upper) = upper else { return Ok(at) };
    let cut = &upper
        .source_cut
        .as_ref()
        .ok_or_else(|| domain(ErrorData::ReadCutExpired))?
        .read;
    let check = |selected: &crate::durable::reading::ConversationReadCut| {
        if selected.reconstructible_from(cut) {
            Ok(())
        } else {
            Err(domain(ErrorData::ReadCutExpired))
        }
    };
    Ok(match at {
        At::Latest => At::Older {
            before: crate::durable::TranscriptCursor::new(
                cut.transcript
                    .checked_add(1)
                    .ok_or_else(|| domain(ErrorData::ReadCutExpired))?,
            ),
            cut: Some(cut.clone()),
        },
        At::Older { before, cut: None } => At::Older {
            before,
            cut: Some(cut.clone()),
        },
        At::Older {
            before,
            cut: Some(selected),
        } => {
            check(&selected)?;
            At::Older {
                before,
                cut: Some(selected),
            }
        }
        At::Canonical { locator, cut: None } => At::Canonical {
            locator,
            cut: Some(cut.clone()),
        },
        At::Canonical {
            locator,
            cut: Some(selected),
        } => {
            check(&selected)?;
            At::Canonical {
                locator,
                cut: Some(selected),
            }
        }
        At::Message { id, cut: None } => At::Message {
            id,
            cut: Some(cut.clone()),
        },
        At::Message {
            id,
            cut: Some(selected),
        } => {
            check(&selected)?;
            At::Message {
                id,
                cut: Some(selected),
            }
        }
        At::Newer {
            after,
            cut: selected,
        } => {
            check(&selected)?;
            At::Newer {
                after,
                cut: selected,
            }
        }
        At::Turn { id, cut: selected } => {
            check(&selected)?;
            At::Turn { id, cut: selected }
        }
        At::TurnOrdinal {
            ordinal,
            cut: selected,
        } => {
            check(&selected)?;
            At::TurnOrdinal {
                ordinal,
                cut: selected,
            }
        }
    })
}
