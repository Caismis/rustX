//! Configuration requests outside any attachment: User/Workspace sources,
//! Session settings, MCP probes, reconciliation and adoption. Mutations
//! commit through the manager; reads materialize off the async executor.
use super::AppServerConnection;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::{
    domain, session_error, source_error, source_read_error, source_settings_error,
};

impl AppServerConnection {
    /// Serve one admitted configuration request.
    #[allow(clippy::too_many_lines)] // One arm per configuration method.
    pub(super) async fn configuration(&self, method: Method) -> Result<MethodResult, RpcError> {
        match method {
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
            Method::AdoptConfiguration {
                session_id,
                candidate,
                expected_binding,
            } => {
                let application = self
                    .host
                    .manager()
                    .adopt_configuration(&session_id, &candidate, expected_binding)
                    .map_err(|rejection| domain(ErrorData::ConfigurationAdoption { rejection }))?;
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
            Method::McpProbe {
                target,
                id,
                expected_revision,
            } => {
                let result = self
                    .host
                    .manager()
                    .probe_mcp(target, id, expected_revision)
                    .await
                    .map_err(source_error)?;
                Ok(MethodResult::McpProbe { result })
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
            Method::WorkflowCheck {
                target,
                id,
                document,
                context,
                explain,
            } => {
                let report = self
                    .host
                    .manager()
                    .check_workflow(target, id, *document, context, explain)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::WorkflowCheck {
                    report: Box::new(report),
                })
            }
            Method::SourcesWrite {
                target,
                expected_revision,
                mutation,
            } => {
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
