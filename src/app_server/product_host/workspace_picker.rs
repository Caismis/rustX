//! A bounded browse of an operator-authorized root through a private Session.
//! The transport owns the Client until Revoke and child settlement finish.

use crate::app_server::{RequestLane, host::AppServerHost, transport::MAX_MESSAGE_BYTES};
use crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint;
use futures_util::{SinkExt, StreamExt};
use rustx_execution_protocol::{
    MAX_DIRECTORY_NAMES, MAX_DIRECTORY_PAGE_ENTRIES, ReadError, is_normalized_absolute_path,
};
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

pub(crate) const PATH: &str = "/product-host/workspace-picker";
pub(crate) const SUBPROTOCOL: &str = "rustx.product-host.workspace-picker.v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Browse {
    root: PathBuf,
    directory: PathBuf,
    expected: Option<String>,
}

#[derive(Serialize)]
struct Listing {
    folders: Vec<String>,
    identity: String,
    revision: String,
}

fn directory_entry_limit() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        format!("Cannot browse the folder (limit {MAX_DIRECTORY_NAMES} directory entries)"),
    )
}

fn picker_page_error(error: io::Error) -> io::Error {
    let physical = error
        .get_ref()
        .and_then(|failure| failure.downcast_ref::<rustx_execution_server::PhysicalError>());
    if matches!(
        physical,
        Some(rustx_execution_server::PhysicalError(
            ReadError::DirectoryEntryLimit
        ))
    ) {
        return directory_entry_limit();
    }
    error
}

fn list(
    endpoint: &LocalExecutionEndpoint,
    request: &Browse,
    active: impl Fn() -> bool,
) -> io::Result<Listing> {
    if !active() {
        return Err(io::Error::other("Folder selection cancelled"));
    }
    let before = endpoint.entry_metadata(&request.directory)?;
    if !before.is_directory
        || before.is_symlink
        || request
            .expected
            .as_ref()
            .is_some_and(|expected| *expected != before.revision)
    {
        return Err(io::Error::other(
            "The folder changed. Browse to it again before adding it.",
        ));
    }
    let mut folders = Vec::new();
    if request.expected.is_none() {
        let mut offset = 0;
        loop {
            if !active() {
                return Err(io::Error::other("Folder selection cancelled"));
            }
            let page = endpoint
                .directory_page(
                    &request.directory,
                    offset,
                    u32::try_from(MAX_DIRECTORY_PAGE_ENTRIES).expect("directory page fits u32"),
                    Some(&before.revision),
                )
                .map_err(picker_page_error)?;
            for entry in page.entries {
                if entry.metadata.is_directory && !entry.metadata.is_symlink {
                    if folders.len() == MAX_DIRECTORY_NAMES {
                        return Err(directory_entry_limit());
                    }
                    folders.push(entry.file_name);
                }
            }
            let Some(next) = page.next_offset else { break };
            offset = u32::try_from(next).expect("verified directory cursor fits u32");
        }
    }
    Ok(Listing {
        folders,
        identity: before.identity,
        revision: before.revision,
    })
}

async fn browse(
    host: &AppServerHost,
    request: Browse,
    cancelled: CancellationToken,
) -> Result<Listing, String> {
    if !is_normalized_absolute_path(&request.root)
        || !is_normalized_absolute_path(&request.directory)
        || !request.directory.starts_with(&request.root)
    {
        return Err("Invalid Workspace directory".into());
    }
    let manager = host.manager();
    let grant = manager
        .environment
        .retirement(&request.root)
        .ok_or("Folder selection requires a configured environment root")?;
    let _slot = host
        .local_workspace_probes()
        .try_acquire()
        .map_err(|_| "Another local Workspace probe is still settling")?;
    let _operation = host
        .admit_request(RequestLane::Ordinary, |owner| owner)
        .map_err(|_| "Folder selection is unavailable while the Server drains or is at capacity")?;
    let stop = host.environment_stop();
    if cancelled.is_cancelled() || grant.is_cancelled() || stop.is_cancelled() {
        return Err("Folder selection cancelled".into());
    }
    let owner = manager
        .start_local_workspace_probe(request.root.clone())
        .await?;
    let endpoint = owner.scope().endpoint();
    let result = match endpoint {
        Ok(endpoint)
            if !cancelled.is_cancelled() && !grant.is_cancelled() && !stop.is_cancelled() =>
        {
            let grant = grant.clone();
            let stop = stop.clone();
            let cancelled = cancelled.clone();
            tokio::task::spawn_blocking(move || {
                list(&endpoint, &request, || {
                    !cancelled.is_cancelled() && !grant.is_cancelled() && !stop.is_cancelled()
                })
            })
            .await
            .map_err(|error| io::Error::other(error.to_string()))
            .and_then(std::convert::identity)
        }
        Ok(_) => Err(io::Error::other("Folder selection cancelled")),
        Err(error) => Err(error),
    };
    owner.shutdown().await?;
    if grant.is_cancelled() || stop.is_cancelled() {
        return Err("Folder selection cancelled".into());
    }
    result.map_err(|error| error.to_string())
}

pub(crate) async fn serve<S>(
    mut socket: WebSocketStream<S>,
    host: AppServerHost,
    authority: CancellationToken,
    shutdown: CancellationToken,
) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    use crate::app_server::transport::websocket::HANDSHAKE_TIMEOUT;
    let message = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        () = authority.cancelled() => return Ok(()),
        message = tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.next()) => message?,
    };
    let Some(Ok(Message::Text(text))) = message else {
        return Err(io::Error::other("Expected a folder selection request"));
    };
    let request = serde_json::from_str(&text).map_err(io::Error::other)?;
    let cancelled = CancellationToken::new();
    let work = browse(&host, request, cancelled.clone());
    tokio::pin!(work);
    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => None,
        () = authority.cancelled() => None,
        _ = socket.next() => None,
        result = &mut work => Some(result),
    };
    let Some(result) = result else {
        cancelled.cancel();
        if let Err(error) = work.await {
            tracing::debug!(%error, "Folder selection ended");
        }
        return Ok(());
    };
    let mut response = serde_json::to_string(&result).map_err(io::Error::other)?;
    if response.len() > MAX_MESSAGE_BYTES {
        response = r#"{"Err":"Folder listing exceeds the response limit"}"#.into();
    }
    tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        socket.send(Message::Text(response.into())),
    )
    .await?
    .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_reports_the_entry_limit_only_for_a_confirmed_overflow() {
        for (kind, diagnostic) in [
            (io::ErrorKind::PermissionDenied, "physical access denied"),
            (io::ErrorKind::AlreadyExists, "physical revision conflict"),
            (io::ErrorKind::Other, "physical operation unavailable"),
        ] {
            let result = picker_page_error(io::Error::new(kind, diagnostic));
            assert_eq!(result.kind(), kind);
            assert_eq!(result.to_string(), diagnostic);
        }
        let result = picker_page_error(io::Error::new(
            io::ErrorKind::FileTooLarge,
            rustx_execution_server::PhysicalError(ReadError::DirectoryEntryLimit),
        ));
        assert_eq!(result.kind(), io::ErrorKind::FileTooLarge);
        assert!(result.to_string().contains("limit 4096 directory entries"));
    }
}
