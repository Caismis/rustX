//! Ordinary trusted-client routing to the existing native environment owners.
use super::AppServerConnection;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::domain;
use base64::Engine as _;

pub(in crate::app_server) fn error(error: std::io::Error) -> RpcError {
    let kind = error.kind();
    if let Some(inner) = error.into_inner()
        && inner.is::<crate::local_runtime::environment::files::CommitUnknown>()
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
    #[allow(clippy::too_many_lines)] // One exhaustive arm per native environment operation.
    pub(super) async fn environment(
        &self,
        method: Method,
        owner: crate::app_server::host::ServerOperation,
    ) -> Result<MethodResult, RpcError> {
        match method {
            Method::CommandExec(params) => {
                let received = self.commands.start(
                    params,
                    self.host.manager().environment.clone(),
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
                        Method::FsReadFile {
                            path,
                            offset,
                            max_bytes,
                            expected_revision,
                        } => environment
                            .read(&path, offset, max_bytes, expected_revision.as_deref())
                            .map(|chunk| MethodResult::FileChunk { chunk }),
                        Method::FsWriteFile {
                            path,
                            data_base64,
                            expected_revision,
                        } => {
                            if data_base64.len() > 699_052 {
                                return Err(domain(ErrorData::InvalidParams));
                            }
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(data_base64)
                                .map_err(|_| domain(ErrorData::InvalidParams))?;
                            environment
                                .write(&path, &bytes, expected_revision.as_deref())
                                .map(|metadata| MethodResult::FileMetadata { metadata })
                        }
                        Method::FsReadDirectory {
                            path,
                            offset,
                            limit,
                            expected_revision,
                        } => environment
                            .list(&path, offset, limit, expected_revision.as_deref())
                            .map(|page| MethodResult::DirectoryPage { page }),
                        Method::FsGetMetadata { path } => environment
                            .metadata(&path)
                            .map(|metadata| MethodResult::FileMetadata { metadata }),
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
