//! Configuration requests outside any attachment: User/Workspace sources,
//! Session settings, MCP probes and reconciliation. Mutations
//! commit through the manager; reads materialize off the async executor.
use std::sync::Arc;

use super::AppServerConnection;
use crate::app_server::host::ServerOperation;
use crate::app_server::protocol::{
    ErrorData, Method, MethodResult, RpcError, WorkspaceCatalogAuthority,
};
use crate::app_server::rpc_error::{
    domain, session_error, source_error, source_read_error, source_settings_error,
};
use crate::local_runtime::{
    configuration::settings::{SettingsError, SourceTarget},
    session::SessionId,
    session_runtime_manager::{
        LocalExecutionEndpoint, SessionRuntimeManager, SourceCatalogExecution,
    },
};

#[derive(Clone, Copy)]
enum PrivateCatalogAccess {
    ReadOnly,
    Tools,
}

fn workspace_catalog_endpoint(
    manager: &SessionRuntimeManager,
    target: &SourceTarget,
    execution: &SourceCatalogExecution,
) -> Result<Option<Arc<LocalExecutionEndpoint>>, RpcError> {
    let Some(workspace) = target.workspace() else {
        return if matches!(execution, SourceCatalogExecution::Session(Some(_))) {
            Err(domain(ErrorData::InvalidParams))
        } else {
            Ok(None)
        };
    };
    let Some(scope) = execution
        .scope(manager, Some(workspace))
        .map_err(|_| domain(ErrorData::ExecutionUnavailable))?
    else {
        return Ok(None);
    };
    let endpoint = scope
        .endpoint()
        .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
    if endpoint.workspace().as_os_str() != workspace.as_os_str() {
        return Err(domain(ErrorData::InvalidParams));
    }
    Ok(Some(endpoint))
}

impl AppServerConnection {
    async fn validate_catalog_session(
        &self,
        target: &crate::local_runtime::configuration::settings::SourceTarget,
        session: Option<&crate::local_runtime::session::SessionId>,
    ) -> Result<(), RpcError> {
        if let Some(session) = session {
            let selected = self
                .sessions
                .read_session_summary(session)
                .await
                .map_err(session_error)?;
            if target
                .workspace()
                .is_some_and(|directory| directory != selected.cwd)
            {
                return Err(domain(ErrorData::InvalidParams));
            }
        }
        Ok(())
    }

    async fn resolve_workflow_document(
        &self,
        target: &crate::local_runtime::configuration::settings::SourceTarget,
        reference: crate::app_server::workflow_documents::WorkflowDocumentRef,
    ) -> Result<Box<crate::workflow_file::WorkflowFileDocument>, RpcError> {
        let target = target.clone();
        let documents = self.workflow_documents.clone();
        tokio::task::spawn_blocking(move || documents.resolve(&target, &reference))
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
    }

    async fn with_catalog_execution<T, R>(
        &self,
        target: SourceTarget,
        session: Option<SessionId>,
        owner: ServerOperation,
        access: PrivateCatalogAccess,
        read: impl FnOnce(
            SessionRuntimeManager,
            SourceTarget,
            SourceCatalogExecution,
            Option<ServerOperation>,
        ) -> R
        + Send
        + 'static,
    ) -> Result<T, RpcError>
    where
        T: Send + 'static,
        R: Future<Output = Result<T, RpcError>> + Send + 'static,
    {
        self.validate_catalog_session(&target, session.as_ref())
            .await?;
        let manager = self.host.manager().clone();
        let probe = target
            .workspace()
            .filter(|_| session.is_none())
            .and_then(|workspace| {
                manager
                    .local_workspace_catalog_grant(workspace)
                    .map(|grant| (workspace.to_path_buf(), grant))
            });
        let Some((workspace, grant)) = probe else {
            return read(
                manager,
                target,
                SourceCatalogExecution::Session(session),
                Some(owner),
            )
            .await;
        };
        let unavailable = || domain(ErrorData::ExecutionUnavailable);
        let stop = self.host.environment_stop();
        let capacity = self.host.local_workspace_probes().clone();
        // The Host request limit bounds the queue. A queued catalog does not
        // spend its handshake or listing budget waiting for a prior probe.
        let slot = tokio::select! {
            biased;
            () = grant.cancelled() => return Err(unavailable()),
            () = stop.cancelled() => return Err(unavailable()),
            slot = capacity.acquire_owned() => slot.map_err(|_| unavailable())?,
        };
        // Once admitted, the request and probe permits cover the Client
        // through shutdown even if the observer disconnects.
        tokio::spawn(async move {
            let _owner = owner;
            let _slot = slot;
            if matches!(access, PrivateCatalogAccess::Tools) {
                let (manager, workspace) = (manager.clone(), workspace.clone());
                tokio::task::spawn_blocking(move || manager.require_workspace_settled(&workspace))
                    .await
                    .map_err(|_| unavailable())?
                    .map_err(|_| unavailable())?;
            }
            if grant.is_cancelled() || stop.is_cancelled() {
                return Err(unavailable());
            }
            let private = manager
                .start_local_workspace_probe(workspace)
                .await
                .map_err(|_| unavailable())?;
            let result = if grant.is_cancelled() || stop.is_cancelled() {
                Err(unavailable())
            } else {
                read(
                    manager,
                    target,
                    SourceCatalogExecution::Private(private.scope()),
                    None,
                )
                .await
            };
            private.shutdown().await.map_err(|_| unavailable())?;
            if grant.is_cancelled() || stop.is_cancelled() {
                return Err(unavailable());
            }
            result
        })
        .await
        .map_err(|_| domain(ErrorData::OperationFailed))?
    }

    async fn read_skill_catalog<T, R>(
        &self,
        target: SourceTarget,
        session: Option<SessionId>,
        owner: ServerOperation,
        read: impl FnOnce(
            SessionRuntimeManager,
            SourceTarget,
            Option<Arc<LocalExecutionEndpoint>>,
            Option<ServerOperation>,
        ) -> R
        + Send
        + 'static,
    ) -> Result<(T, WorkspaceCatalogAuthority), RpcError>
    where
        T: Send + 'static,
        R: Future<Output = Result<T, SettingsError>> + Send + 'static,
    {
        if target.workspace().is_none() && session.is_some() {
            return Err(domain(ErrorData::InvalidParams));
        }
        self.with_catalog_execution(
            target,
            session,
            owner,
            PrivateCatalogAccess::ReadOnly,
            move |manager, target, execution, owner| async move {
                let endpoint = workspace_catalog_endpoint(&manager, &target, &execution)?;
                let authority = if endpoint.is_some() {
                    WorkspaceCatalogAuthority::Client
                } else {
                    WorkspaceCatalogAuthority::Authoring
                };
                let result = read(manager, target, endpoint, owner)
                    .await
                    .map_err(source_error)?;
                Ok((result, authority))
            },
        )
        .await
    }

    async fn inspect_mcp_catalog(
        &self,
        target: SourceTarget,
        session: Option<SessionId>,
        id: crate::runtime::identity::McpServerId,
        revision: String,
        kind: Option<crate::tools::mcp::catalog::CatalogKind>,
        owner: ServerOperation,
    ) -> Result<MethodResult, RpcError> {
        let result = self
            .with_catalog_execution(
                target,
                session,
                owner,
                PrivateCatalogAccess::Tools,
                move |manager, target, execution, owner| async move {
                    manager
                        .inspect_mcp_source(target, execution, id, revision, kind, owner)
                        .await
                        .map_err(source_error)
                },
            )
            .await?;
        Ok(MethodResult::McpProbe { result })
    }

    /// Serve one admitted configuration request.
    #[allow(clippy::too_many_lines)] // One arm per configuration method.
    pub(super) async fn configuration(
        &self,
        method: Method,
        request_owner: crate::app_server::host::ServerOperation,
    ) -> Result<MethodResult, RpcError> {
        match method {
            Method::McpCatalog {
                target,
                execution_session,
                id,
                expected_revision,
                kind,
            } => {
                self.inspect_mcp_catalog(
                    target,
                    execution_session,
                    id,
                    expected_revision,
                    Some(kind),
                    request_owner,
                )
                .await
            }
            Method::SkillsList {
                target,
                execution_session,
                offset,
                limit,
                expected_revision,
            } => {
                let (page, authority) = self
                    .read_skill_catalog(
                        target,
                        execution_session,
                        request_owner,
                        move |manager, target, endpoint, owner| async move {
                            manager
                                .skill_page(
                                    target,
                                    endpoint,
                                    offset,
                                    limit,
                                    expected_revision,
                                    owner,
                                )
                                .await
                        },
                    )
                    .await?;
                Ok(MethodResult::Skills { page, authority })
            }
            Method::SkillsRead {
                target,
                execution_session,
                name,
                kind,
                offset,
                max_bytes,
                expected_revision,
            } => {
                let (content, _) = self
                    .read_skill_catalog(
                        target,
                        execution_session,
                        request_owner,
                        move |manager, target, endpoint, owner| async move {
                            manager
                                .skill_read(
                                    target,
                                    endpoint,
                                    name,
                                    kind,
                                    offset,
                                    max_bytes,
                                    expected_revision,
                                    owner,
                                )
                                .await
                        },
                    )
                    .await?;
                Ok(MethodResult::SkillContent { content })
            }
            Method::ConfigurationReconcile { target } => {
                let application = self
                    .host
                    .manager()
                    .reconcile_configuration(&target)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::ConfigurationReceipt {
                    scope: application.scope,
                    version: application.version,
                })
            }
            Method::SessionConfiguration { session_id } => {
                self.sessions
                    .read_session_summary(&session_id)
                    .await
                    .map_err(session_error)?;
                let application = self.host.manager().configuration_application(&session_id);
                let projection = tokio::task::spawn_blocking(move || {
                    application
                        .as_ref()
                        .map(crate::local_runtime::configuration::observation::session_application)
                        .transpose()
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SessionConfiguration { projection })
            }
            Method::SessionApplicationValueRead {
                session_id,
                reference,
                offset,
                max_bytes,
            } => {
                self.sessions
                    .read_session_summary(&session_id)
                    .await
                    .map_err(session_error)?;
                let application = self
                    .host
                    .manager()
                    .configuration_application(&session_id)
                    .ok_or_else(|| domain(ErrorData::ContentSourceUnavailable))?;
                let chunk = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::session_application_read(
                        &application,
                        &reference,
                        offset,
                        max_bytes,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SessionApplicationValueChunk { chunk })
            }
            Method::SourcesRead { target } => {
                let projection = self
                    .host
                    .manager()
                    .source_settings(&target)
                    .await
                    .map_err(source_settings_error)?;
                let projection = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::bounded(&projection)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|_| {
                    domain(ErrorData::ObservationUnavailable {
                        diagnostic: "source projection unavailable".into(),
                    })
                })?;
                Ok(MethodResult::SourceSettings {
                    projection: Box::new(projection),
                })
            }
            Method::SourcesPage {
                target,
                revision,
                field,
                offset,
                limit,
            } => {
                let source = self
                    .host
                    .manager()
                    .source_settings(&target)
                    .await
                    .map_err(source_settings_error)?;
                let page = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::page(
                        &source, &revision, field, offset, limit,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SourcePage { page })
            }
            Method::SourcesValueRead {
                target,
                reference,
                offset,
                max_bytes,
            } => {
                let source = self
                    .host
                    .manager()
                    .source_settings(&target)
                    .await
                    .map_err(source_settings_error)?;
                let chunk = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::read(
                        &source, &reference, offset, max_bytes,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SourceValueChunk { chunk })
            }
            Method::ModelProbe {
                target,
                model,
                source,
            } => {
                let outcome = self.host.manager().probe_model(target, model, source).await;
                Ok(MethodResult::ModelProbe { outcome })
            }
            Method::McpSourceRead {
                target,
                execution_session,
            } => {
                if target.workspace().is_none() {
                    return Err(domain(ErrorData::InvalidParams));
                }
                self.validate_catalog_session(&target, Some(&execution_session))
                    .await?;
                let Some(endpoint) = workspace_catalog_endpoint(
                    self.host.manager(),
                    &target,
                    &SourceCatalogExecution::Session(Some(execution_session)),
                )?
                else {
                    return Ok(MethodResult::McpSource {
                        authority:
                            crate::app_server::protocol::WorkspaceCatalogAuthority::Authoring,
                        source: None,
                    });
                };
                let source = tokio::task::spawn_blocking(move || {
                    let document =
                        crate::local_runtime::configuration::capture::read_bound_mcp(&endpoint)
                            .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
                    let path = endpoint
                        .workspace()
                        .join(crate::local_runtime::mcp_resources::WORKSPACE_DOCUMENT);
                    Ok::<_, RpcError>(
                        crate::local_runtime::configuration::settings::mcp_document_view(
                            path,
                            document.as_deref(),
                        ),
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::McpSource {
                    authority: crate::app_server::protocol::WorkspaceCatalogAuthority::Client,
                    source: Some(source),
                })
            }
            Method::McpProbe {
                target,
                execution_session,
                id,
                expected_revision,
            } => {
                self.inspect_mcp_catalog(
                    target,
                    execution_session,
                    id,
                    expected_revision,
                    None,
                    request_owner,
                )
                .await
            }
            Method::WorkflowDocumentBegin {
                target,
                byte_length,
                sha256,
            } => {
                let documents = self.workflow_documents.clone();
                let reference = tokio::task::spawn_blocking(move || {
                    documents.begin(target, byte_length, sha256)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowDocument { reference })
            }
            Method::WorkflowDocumentWrite {
                target,
                reference,
                offset,
                data,
            } => {
                let documents = self.workflow_documents.clone();
                tokio::task::spawn_blocking(move || {
                    documents.write(&target, &reference, offset, &data)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowDocumentAccepted {})
            }
            Method::WorkflowDocumentFinish { target, reference } => {
                let documents = self.workflow_documents.clone();
                tokio::task::spawn_blocking(move || documents.finish(&target, &reference))
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowDocumentAccepted {})
            }
            Method::WorkflowDocumentRead {
                target,
                reference,
                offset,
                max_bytes,
            } => {
                let documents = self.workflow_documents.clone();
                let chunk = tokio::task::spawn_blocking(move || {
                    documents.read(&target, &reference, offset, max_bytes)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowDocumentChunk { chunk })
            }
            Method::WorkflowDocumentRelease { target, reference } => {
                let documents = self.workflow_documents.clone();
                tokio::task::spawn_blocking(move || documents.release(&target, &reference))
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowDocumentAccepted {})
            }
            Method::WorkflowList { target } => {
                let library = self
                    .host
                    .manager()
                    .workflow_library(target)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::WorkflowLibrary { library })
            }
            Method::WorkflowRead { target, id } => {
                let source = self
                    .host
                    .manager()
                    .workflow_source(target, id)
                    .await
                    .map_err(source_settings_error)?;
                let documents = self.workflow_documents.clone();
                let source = tokio::task::spawn_blocking(move || {
                    use crate::local_runtime::workflow_authoring::WorkflowSource;
                    let document = source
                        .document
                        .as_deref()
                        .map(|document| documents.capture(&source.target, document).map(Box::new))
                        .transpose()?;
                    Ok::<_, RpcError>(WorkflowSource {
                        target: source.target,
                        id: source.id,
                        path: source.path,
                        relative_path: source.relative_path,
                        revision: source.revision,
                        present: source.present,
                        modified_ms: source.modified_ms,
                        document,
                        diagnostic: source.diagnostic,
                        vocabulary: source.vocabulary,
                    })
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowSource {
                    source: Box::new(source),
                })
            }

            Method::WorkflowModelChoices {
                target,
                query,
                expected_revision,
            } => {
                let choices = self
                    .host
                    .manager()
                    .workflow_model_choices(target, query, expected_revision)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::WorkflowModelChoices { choices })
            }
            Method::WorkflowToolCatalog {
                target,
                execution_session,
                source,
                cursor,
                expected_scope,
            } => {
                let catalog = self
                    .with_catalog_execution(
                        target,
                        execution_session,
                        request_owner,
                        PrivateCatalogAccess::Tools,
                        move |manager, target, execution, owner| async move {
                            let _owner = owner;
                            manager
                                .workflow_tool_catalog(
                                    target,
                                    execution,
                                    source,
                                    cursor,
                                    expected_scope,
                                )
                                .await
                                .map_err(source_error)
                        },
                    )
                    .await?;
                Ok(MethodResult::WorkflowToolCatalog { catalog })
            }
            Method::WorkflowAnalyze {
                target,
                id,
                document,
                generation,
                query,
            } => {
                let document = self.resolve_workflow_document(&target, *document).await?;
                let report = self
                    .host
                    .manager()
                    .analyze_workflow(target, id, *document, generation, query)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::WorkflowAnalysis {
                    report: Box::new(report),
                })
            }
            Method::WorkflowCheck {
                target,
                id,
                document,
                context,
                explain,
            } => {
                let document = self.resolve_workflow_document(&target, *document).await?;
                let report = self
                    .host
                    .manager()
                    .check_workflow(target.clone(), id, *document, context, explain)
                    .await
                    .map_err(source_settings_error)?;
                let documents = self.workflow_documents.clone();
                let report =
                    tokio::task::spawn_blocking(move || documents.capture_report(target, &report))
                        .await
                        .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowCheck { report })
            }
            Method::WorkflowCheckReportRead {
                target,
                reference,
                offset,
                max_bytes,
            } => {
                let documents = self.workflow_documents.clone();
                let chunk = tokio::task::spawn_blocking(move || {
                    documents.read_report(&target, &reference, offset, max_bytes)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowCheckReportChunk { chunk })
            }
            Method::WorkflowCheckReportRelease { target, reference } => {
                let documents = self.workflow_documents.clone();
                tokio::task::spawn_blocking(move || documents.release_report(&target, &reference))
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))??;
                Ok(MethodResult::WorkflowCheckReportReleased {})
            }
            Method::SourcesWrite {
                target,
                expected_revision,
                mutation,
            } => {
                use crate::local_runtime::configuration::settings::SourceMutation;
                use crate::local_runtime::workflow_authoring::WorkflowEdit;
                let mutation = match mutation {
                    SourceMutation::GlobalGuidance { document } => {
                        SourceMutation::GlobalGuidance { document }
                    }
                    SourceMutation::RepairConfig { document } => {
                        SourceMutation::RepairConfig { document }
                    }
                    SourceMutation::Mcp { id, authored } => SourceMutation::Mcp { id, authored },
                    SourceMutation::Config { mutation } => SourceMutation::Config { mutation },
                    SourceMutation::Agent { name, authored } => {
                        SourceMutation::Agent { name, authored }
                    }
                    SourceMutation::SkillDisplay {
                        name,
                        authored,
                        upload,
                    } => SourceMutation::SkillDisplay {
                        name,
                        authored,
                        upload,
                    },
                    SourceMutation::SkillDelete { name } => SourceMutation::SkillDelete { name },
                    SourceMutation::ResourceTransfer {
                        from,
                        operation,
                        resource,
                    } => SourceMutation::ResourceTransfer {
                        from,
                        operation,
                        resource,
                    },
                    SourceMutation::Workflow { id, edit } => SourceMutation::Workflow {
                        id,
                        edit: match edit {
                            WorkflowEdit::Create {
                                description,
                                display,
                            } => WorkflowEdit::Create {
                                description,
                                display,
                            },
                            WorkflowEdit::Replace { document } => WorkflowEdit::Replace {
                                document: self
                                    .resolve_workflow_document(&target, *document)
                                    .await?,
                            },
                            WorkflowEdit::Copy {
                                source,
                                source_revision,
                            } => WorkflowEdit::Copy {
                                source,
                                source_revision,
                            },
                            WorkflowEdit::Delete => WorkflowEdit::Delete,
                        },
                    },
                };
                let receipt = self
                    .host
                    .manager()
                    .write_source_settings(&target, expected_revision, mutation)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::SourcesWritten {
                    revision: receipt.revision,
                    configuration_revision: receipt.configuration_revision,
                })
            }
            Method::SessionInputsWrite {
                session_id,
                expected_revision,
                session_config,
                session_resources,
            } => {
                let revision = self
                    .host
                    .manager()
                    .write_session_inputs(
                        session_id,
                        expected_revision,
                        session_config,
                        session_resources,
                        request_owner,
                    )
                    .await
                    .map_err(super::super::rpc_error::session_input_error)?;
                Ok(MethodResult::SessionInputsWritten { revision })
            }
            Method::SettingsRead { session_id } => {
                let (revision, settings) = self
                    .sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                let projection = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::session_settings(
                        revision, &settings,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::Settings { projection })
            }
            Method::SettingsValueRead {
                session_id,
                reference,
                offset,
                max_bytes,
            } => {
                let (revision, settings) = self
                    .sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                let chunk = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::session_settings_read(
                        revision, &settings, &reference, offset, max_bytes,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SettingsValueChunk { chunk })
            }
            // `Family::of` routes only configuration requests here.
            _ => Err(domain(ErrorData::OperationFailed)),
        }
    }
}
