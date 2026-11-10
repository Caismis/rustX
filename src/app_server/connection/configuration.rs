//! Configuration requests outside any attachment: User/Workspace sources,
//! Session settings, MCP probes and reconciliation. Mutations
//! commit through the manager; reads materialize off the async executor.
use super::AppServerConnection;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::{
    domain, session_error, source_error, source_read_error, source_settings_error,
};

impl AppServerConnection {
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
                id,
                expected_revision,
                kind,
            } => {
                let result = self
                    .host
                    .manager()
                    .mcp_catalog(target, id, expected_revision, kind, request_owner)
                    .await
                    .map_err(source_error)?;
                Ok(MethodResult::McpProbe { result })
            }
            Method::SkillsList {
                target,
                offset,
                limit,
                expected_revision,
            } => {
                let page = self
                    .host
                    .manager()
                    .skill_page(target, offset, limit, expected_revision, request_owner)
                    .await
                    .map_err(source_error)?;
                Ok(MethodResult::Skills { page })
            }
            Method::SkillsRead {
                target,
                name,
                kind,
                offset,
                max_bytes,
                expected_revision,
            } => {
                let content = self
                    .host
                    .manager()
                    .skill_read(
                        target,
                        name,
                        kind,
                        offset,
                        max_bytes,
                        expected_revision,
                        request_owner,
                    )
                    .await
                    .map_err(source_error)?;
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
            Method::McpProbe {
                target,
                id,
                expected_revision,
            } => {
                let result = self
                    .host
                    .manager()
                    .probe_mcp(target, id, expected_revision, request_owner)
                    .await
                    .map_err(source_error)?;
                Ok(MethodResult::McpProbe { result })
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
                source,
                cursor,
                expected_scope,
            } => {
                let catalog = self
                    .host
                    .manager()
                    .workflow_tool_catalog(target, source, cursor, expected_scope)
                    .await
                    .map_err(source_error)?;
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
