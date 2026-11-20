//! Ordinary trusted-client routing to the existing native environment owners.
use super::AppServerConnection;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::domain;
use base64::Engine as _;

pub(in crate::app_server) fn error(error: std::io::Error) -> RpcError {
    let kind = error.kind();
    if let Some(inner) = error.into_inner()
        && (inner.is::<crate::local_runtime::environment::files::CommitUnknown>()
            || inner.is::<rustx_execution_server::WriteOutcomeUnknown>())
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
        query: impl FnOnce(
            &crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint,
        ) -> std::io::Result<MethodResult>
        + Send
        + 'static,
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
            Method::FsWatch { watch_id, path } => {
                self.watches
                    .watch(
                        watch_id,
                        path,
                        self.host.manager().environment.clone(),
                        self.service_events.clone(),
                        self.host.file_observers(),
                        owner,
                    )
                    .await?;
                Ok(MethodResult::EnvironmentAccepted {})
            }
            Method::FsUnwatch { watch_id } => {
                self.watches.unwatch(&watch_id)?;
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
                let manager = self.host.manager().clone();
                let closed = self.service_events.closed.clone();
                // Detached blocking work retains admission through the
                // physical commit. Dropping a transport waiter cannot
                // free its capacity.
                tokio::task::spawn_blocking(move || {
                    let _owner = owner;
                    if closed.is_cancelled() {
                        return Err(domain(ErrorData::ConnectionClosed));
                    }
                    // Non-creating lookup: no endpoint means no admitted
                    // Client — absence is unavailability, never a
                    // fallback to Server-local file I/O.
                    let endpoint = manager
                        .execution_endpoint(&session_id)
                        .map_err(|_| domain(ErrorData::ExecutionUnavailable))?;
                    // Logical authorization only: the admitted workspace
                    // must sit under a configured root, checked before the
                    // prepare and again inside the commit gate.
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
                    let metadata = endpoint
                        .write_file(
                            std::path::Path::new(&path),
                            &bytes,
                            expected_revision.as_deref(),
                            |_| {
                                // The last caller gate, invoked on the
                                // retained prepared original immediately
                                // before the commit frame is written:
                                // the same grant must still cover the
                                // admitted workspace and the caller must
                                // still be connected.
                                if !authorized() {
                                    return Err(std::io::Error::new(
                                        std::io::ErrorKind::PermissionDenied,
                                        "environment root unauthorized",
                                    ));
                                }
                                if closed.is_cancelled() {
                                    return Err(std::io::Error::new(
                                        std::io::ErrorKind::ConnectionAborted,
                                        "connection closed",
                                    ));
                                }
                                Ok(())
                            },
                        )
                        .map_err(|io| match io.kind() {
                            std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe => {
                                domain(ErrorData::ExecutionUnavailable)
                            }
                            std::io::ErrorKind::WouldBlock => {
                                domain(ErrorData::EnvironmentCapacityOrConflict)
                            }
                            std::io::ErrorKind::ConnectionAborted => {
                                domain(ErrorData::ConnectionClosed)
                            }
                            _ => error(io),
                        })?;
                    // A known-successful write is durable: it is never
                    // reported as cancelled or rolled back past this point.
                    Ok(MethodResult::FileMetadata { metadata })
                })
                .await
                .map_err(|_| domain(ErrorData::OutcomeUnknown))?
            }
            method => {
                let environment = self.host.manager().environment.clone();
                let closed = self.service_events.closed.clone();
                // Detached blocking work retains admission through the physical
                // commit. Dropping a transport waiter cannot free its capacity.
                tokio::task::spawn_blocking(move || {
                    let _owner = owner;
                    if closed.is_cancelled() {
                        return Err(domain(ErrorData::ConnectionClosed));
                    }
                    match method {
                        Method::FsCreateDirectory { path, recursive } => environment
                            .create_directory(&path, recursive)
                            .map(|()| MethodResult::EnvironmentAccepted {}),
                        Method::FsRemove {
                            path,
                            recursive,
                            force,
                        } => environment
                            .remove(&path, recursive, force)
                            .map(|()| MethodResult::EnvironmentAccepted {}),
                        Method::FsCopy {
                            source_path,
                            destination_path,
                            recursive,
                        } => environment
                            .copy(&source_path, &destination_path, recursive)
                            .map(|()| MethodResult::EnvironmentAccepted {}),
                        _ => return Err(domain(ErrorData::InvalidParams)),
                    }
                    .map_err(error)
                })
                .await
                .map_err(|_| domain(ErrorData::OutcomeUnknown))?
            }
        }
    }
}
