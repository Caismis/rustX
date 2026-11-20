//! Ordinary trusted-client routing to the existing native environment owners.
use super::AppServerConnection;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::domain;
use base64::Engine as _;
use rustx_execution_protocol::TreeMutation;

type ExecutionEndpoint = crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint;

pub(in crate::app_server) fn error(error: std::io::Error) -> RpcError {
    let kind = error.kind();
    if let Some(inner) = error.into_inner()
        && (inner.is::<rustx_execution_server::WriteOutcomeUnknown>()
            || inner.is::<rustx_execution_server::WatchOutcomeUnknown>())
    {
        return domain(ErrorData::OutcomeUnknown);
    }
    let data = match kind {
        std::io::ErrorKind::InvalidInput => ErrorData::InvalidParams,
        std::io::ErrorKind::PermissionDenied => ErrorData::EnvironmentUnauthorized,
        std::io::ErrorKind::NotFound => ErrorData::EnvironmentResourceNotFound,
        std::io::ErrorKind::AlreadyExists => ErrorData::EnvironmentCapacityOrConflict,
        _ => ErrorData::OperationFailed,
    };
    // Native OS diagnostics may contain private paths or credential-bearing
    // authored strings. The public error is closed and never echoes them.
    domain(data)
}
impl AppServerConnection {
    /// The shared dispatch gate every file query routed to a Session's
    /// admitted Execution Client passes: the non-creating endpoint
    /// lookup (absence is `ExecutionUnavailable`, never a fallback), the
    /// admitted-workspace authorization against configured environment
    /// roots checked before *and* after the exchange — a policy change
    /// mid-exchange never reaches the caller — the detached-blocking
    /// shape that keeps admission through a dropped waiter, and the
    /// shared error mapping. Nothing here opens or stats the workspace.
    async fn execution_file_query(
        &self,
        session_id: crate::local_runtime::session::SessionId,
        owner: crate::app_server::host::ServerOperation,
        query: impl FnOnce(&ExecutionEndpoint) -> std::io::Result<MethodResult> + Send + 'static,
    ) -> Result<MethodResult, RpcError> {
        let manager = self.host.manager().clone();
        let closed = self.service_events.closed.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = owner;
            if closed.is_cancelled() {
                return Err(domain(ErrorData::ConnectionClosed));
            }
            // Non-creating lookup: no endpoint means no admitted
            // Client — absence is unavailability, not an error a
            // fallback could hide.
            let endpoint = manager
                .execution_endpoint(&session_id)
                .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
            // Logical authorization only: the admitted workspace
            // must sit under a configured root. A configured
            // subtree grant never widens upward to the whole
            // workspace, and nothing here opens or stats it.
            let authorized = || {
                manager
                    .process_policy()
                    .environment_roots
                    .iter()
                    .any(|root| endpoint.workspace().starts_with(root))
            };
            if !authorized() {
                return Err(domain(ErrorData::EnvironmentUnauthorized));
            }
            let result = query(&endpoint).map_err(|io| match io.kind() {
                std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe => {
                    domain(ErrorData::ExecutionUnavailable)
                }
                std::io::ErrorKind::WouldBlock => domain(ErrorData::EnvironmentCapacityOrConflict),
                _ => error(io),
            })?;
            if !authorized() {
                return Err(domain(ErrorData::EnvironmentUnauthorized));
            }
            if closed.is_cancelled() {
                return Err(domain(ErrorData::ConnectionClosed));
            }
            Ok(result)
        })
        .await
        .map_err(|_| domain(ErrorData::OutcomeUnknown))?
    }

    #[allow(clippy::too_many_lines)] // One exhaustive arm per native environment operation.
    pub(super) async fn environment(
        &self,
        method: Method,
        owner: crate::app_server::host::ServerOperation,
    ) -> Result<MethodResult, RpcError> {
        match method {
            Method::CommandExec(params) => {
                let received = self.commands.start(
                    &params,
                    self.host.manager().clone(),
                    self.service_events.clone(),
                    owner,
                )?;
                received
                    .await
                    .map_err(|_| domain(ErrorData::OutcomeUnknown))?
                    .map(|result| MethodResult::CommandExec { result })
            }
            Method::CommandWrite(params) => {
                self.commands.write(params).await?;
                Ok(MethodResult::EnvironmentAccepted {})
            }
            Method::CommandResize { process_id, size } => {
                self.commands.resize(&process_id, size).await?;
                Ok(MethodResult::EnvironmentAccepted {})
            }
            Method::CommandTerminate { process_id } => {
                self.commands.terminate(&process_id)?;
                Ok(MethodResult::EnvironmentAccepted {})
            }
            Method::FsWatch {
                session_id,
                watch_id,
                path,
            } => {
                let manager = self.host.manager();
                let endpoint = manager
                    .execution_endpoint(&session_id)
                    .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
                let root_grant = manager
                    .environment
                    .retirement(endpoint.workspace())
                    .ok_or_else(|| domain(ErrorData::EnvironmentUnauthorized))?;
                self.watches
                    .watch(
                        watch_id,
                        super::super::filesystem::WatchSource {
                            endpoint,
                            root_grant,
                            path: path.into(),
                        },
                        self.service_events.clone(),
                        self.host.file_observers(),
                        owner,
                    )
                    .await?;
                Ok(MethodResult::EnvironmentAccepted {})
            }
            Method::FsUnwatch { watch_id } => {
                self.watches.unwatch(&watch_id).await?;
                Ok(MethodResult::EnvironmentAccepted {})
            }
            Method::FsReadFile {
                session_id,
                path,
                offset,
                max_bytes,
                expected_revision,
            } => {
                // Parameter gate before any work is enqueued — the schema
                // range and this bound are the same physical limit.
                if !(1..=rustx_execution_protocol::MAX_RAW_CHUNK_BYTES).contains(&max_bytes) {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let bounded =
                    u32::try_from(max_bytes).map_err(|_| domain(ErrorData::InvalidParams))?;
                self.execution_file_query(session_id, owner, move |endpoint| {
                    let range = endpoint.read_range(
                        std::path::Path::new(&path),
                        offset,
                        bounded,
                        expected_revision.as_deref(),
                    )?;
                    Ok(MethodResult::FileChunk {
                        chunk: rustx_confined_fs::FileChunk {
                            data_base64: base64::engine::general_purpose::STANDARD
                                .encode(range.bytes),
                            offset: range.offset,
                            next_offset: range.next_offset,
                            metadata: rustx_confined_fs::FileMetadata {
                                is_directory: false,
                                is_file: true,
                                is_symlink: false,
                                size: range.metadata.size.get(),
                                revision: range.metadata.revision,
                                identity: range.metadata.identity,
                                modified_at_ms: range.metadata.modified_at_ms,
                            },
                        },
                    })
                })
                .await
            }
            Method::FsGetMetadata { session_id, path } => {
                self.execution_file_query(session_id, owner, move |endpoint| {
                    endpoint
                        .entry_metadata(std::path::Path::new(&path))
                        .map(|metadata| MethodResult::FileMetadata { metadata })
                })
                .await
            }
            Method::FsReadDirectory {
                session_id,
                path,
                offset,
                limit,
                expected_revision,
            } => {
                // Parameter gate before enqueue — the schema range and
                // this bound are the same physical page limits.
                if !(1..=rustx_execution_protocol::MAX_DIRECTORY_PAGE_ENTRIES).contains(&limit)
                    || offset > rustx_execution_protocol::MAX_DIRECTORY_NAMES
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (offset, limit) = (
                    u32::try_from(offset).map_err(|_| domain(ErrorData::InvalidParams))?,
                    u32::try_from(limit).map_err(|_| domain(ErrorData::InvalidParams))?,
                );
                self.execution_file_query(session_id, owner, move |endpoint| {
                    endpoint
                        .directory_page(
                            std::path::Path::new(&path),
                            offset,
                            limit,
                            expected_revision.as_deref(),
                        )
                        .map(|page| MethodResult::DirectoryPage { page })
                })
                .await
            }
            Method::FsWriteFile {
                session_id,
                path,
                data_base64,
                expected_revision,
            } => {
                // The encoded bound gates before any decode: an encoding
                // that cannot fit inside the decoded cap is refused up
                // front; the decoded bytes are bounded again after.
                if data_base64.len() > rustx_execution_protocol::MAX_RAW_CHUNK_BYTES.div_ceil(3) * 4
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .map_err(|_| domain(ErrorData::InvalidParams))?;
                if bytes.len() > rustx_execution_protocol::MAX_RAW_CHUNK_BYTES {
                    return Err(domain(ErrorData::InvalidParams));
                }
                self.execution_file_mutation(session_id, owner, move |endpoint, authorize| {
                    endpoint
                        .write_file(
                            std::path::Path::new(&path),
                            &bytes,
                            expected_revision.as_deref(),
                            |_| authorize(),
                        )
                        .map(|metadata| MethodResult::FileMetadata { metadata })
                })
                .await
            }
            Method::FsCreateDirectory {
                session_id,
                path,
                recursive,
            } => {
                self.execution_tree_mutation(
                    session_id,
                    owner,
                    TreeMutation::CreateDirectory {
                        path: path.into(),
                        recursive,
                    },
                )
                .await
            }
            Method::FsRemove {
                session_id,
                path,
                recursive,
                force,
            } => {
                self.execution_tree_mutation(
                    session_id,
                    owner,
                    TreeMutation::Remove {
                        path: path.into(),
                        recursive,
                        force,
                    },
                )
                .await
            }
            Method::FsCopy {
                session_id,
                source_path,
                destination_path,
                recursive,
            } => {
                self.execution_tree_mutation(
                    session_id,
                    owner,
                    TreeMutation::Copy {
                        source: source_path.into(),
                        destination: destination_path.into(),
                        recursive,
                    },
                )
                .await
            }
            _ => Err(domain(ErrorData::InvalidParams)),
        }
    }

    async fn execution_tree_mutation(
        &self,
        session_id: crate::local_runtime::session::SessionId,
        owner: crate::app_server::host::ServerOperation,
        mutation: TreeMutation,
    ) -> Result<MethodResult, RpcError> {
        self.execution_file_mutation(session_id, owner, move |endpoint, authorize| {
            endpoint.mutate_tree(mutation, authorize)?;
            Ok(MethodResult::EnvironmentAccepted {})
        })
        .await
    }

    async fn execution_file_mutation(
        &self,
        session_id: crate::local_runtime::session::SessionId,
        owner: crate::app_server::host::ServerOperation,
        dispatch: impl FnOnce(
            &ExecutionEndpoint,
            &dyn Fn() -> std::io::Result<()>,
        ) -> std::io::Result<MethodResult>
        + Send
        + 'static,
    ) -> Result<MethodResult, RpcError> {
        use std::io::{Error, ErrorKind};
        let manager = self.host.manager().clone();
        let closed = self.service_events.closed.clone();
        // Retain App Server admission until the physical receipt or an
        // uncertain outcome, even if the UI drops its response waiter.
        tokio::task::spawn_blocking(move || {
            let _owner = owner;
            if closed.is_cancelled() {
                return Err(domain(ErrorData::ConnectionClosed));
            }
            let endpoint = manager
                .execution_endpoint(&session_id)
                .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
            let authorize = || {
                if !manager
                    .process_policy()
                    .environment_roots
                    .iter()
                    .any(|root| endpoint.workspace().starts_with(root))
                {
                    return Err(Error::new(
                        ErrorKind::PermissionDenied,
                        "environment root unauthorized",
                    ));
                }
                if closed.is_cancelled() {
                    return Err(Error::new(
                        ErrorKind::ConnectionAborted,
                        "connection closed",
                    ));
                }
                Ok(())
            };
            let map_error = |source: Error| match source.kind() {
                ErrorKind::NotConnected | ErrorKind::BrokenPipe => {
                    domain(ErrorData::ExecutionUnavailable)
                }
                ErrorKind::WouldBlock => domain(ErrorData::EnvironmentCapacityOrConflict),
                ErrorKind::ConnectionAborted => domain(ErrorData::ConnectionClosed),
                _ => error(source),
            };
            authorize().map_err(map_error)?;
            dispatch(&endpoint, &authorize).map_err(map_error)
        })
        .await
        .map_err(|_| domain(ErrorData::OutcomeUnknown))?
    }
}
