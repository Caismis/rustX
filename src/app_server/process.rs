//! Standalone user-scoped composition, independent of the local Session launcher.
use super::{
    connection::AppServerConnection,
    transport::{stdio, websocket},
};
use crate::local_runtime::{
    configuration::{UserConfigManager, UserConfigSources},
    launch::HostEnvironment,
    session_controller::SessionController,
    session_runtime_manager::SessionRuntimeManager,
};
use std::{
    io::{self, Read},
    os::fd::AsFd,
    path::PathBuf,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const USAGE: &str = "usage: rustx app-server [--user-settings <settings.toml>] [--models <models.toml>] [--runtime-root <path>] --listen <stdio|ws://IP:PORT> [--token-file <path>]\nUser settings default: $XDG_CONFIG_HOME/rustx/settings.toml (default ~/.config/rustx/settings.toml).\n--user-settings fixes the user TOML source for this process; relative CLI paths use launch cwd.\n--models and --runtime-root override canonical user TOML source bindings.\nWebSocket requires a dedicated base64url token file. stdio requires owned pipes.";

struct Options {
    settings: Option<PathBuf>,
    models: Option<PathBuf>,
    root: Option<PathBuf>,
    listen: String,
    token: Option<PathBuf>,
}
impl Options {
    fn parse(arguments: Vec<String>) -> Result<Self, String> {
        let mut options = Self {
            settings: None,
            models: None,
            root: None,
            listen: String::new(),
            token: None,
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            if !seen.insert(flag.clone()) {
                return Err("duplicate option".into());
            }
            let value = arguments
                .next()
                .filter(|value| !value.is_empty())
                .ok_or("option requires a value")?;
            match flag.as_str() {
                "--user-settings" => options.settings = Some(value.into()),
                "--models" => options.models = Some(value.into()),
                "--runtime-root" => options.root = Some(value.into()),
                "--listen" => options.listen = value,
                "--token-file" => options.token = Some(value.into()),
                _ => return Err("unknown App Server option".into()),
            }
        }
        if options.listen.is_empty() || (options.listen == "stdio" && options.token.is_some()) {
            return Err("invalid transport selection".into());
        }
        Ok(options)
    }
}

fn compose(options: &Options) -> Result<SessionRuntimeManager, String> {
    let host = HostEnvironment::capture()?;
    let absolute = |path: &PathBuf| {
        if path.is_absolute() {
            path.clone()
        } else {
            host.launch_directory.join(path)
        }
    };
    let settings = options
        .settings
        .as_ref()
        .map_or_else(|| host.config_directory.join("settings.toml"), absolute);
    // Explicit selection is required; only the omitted canonical default may
    // be absent. Parsing and canonical source binding remain in the shared owner.
    if options.settings.is_some()
        && !std::fs::metadata(&settings).is_ok_and(|metadata| metadata.is_file())
    {
        return Err("explicit user settings source must be an existing readable TOML file".into());
    }
    let configuration = UserConfigManager::bootstrap(
        UserConfigSources {
            settings,
            models: host.config_directory.join("models.toml"),
            runtime_root: host.state_directory.join("app-server"),
            home_directory: host.home_directory,
            config_directory: host.config_directory,
            state_directory: host.state_directory,
        },
        options.models.as_ref().map(absolute),
        options.root.as_ref().map(absolute),
    )
    .map_err(|error| error.to_string())?;
    configuration.validate_catalog()?;
    let sessions =
        SessionController::open(configuration.runtime_root()).map_err(|error| error.to_string())?;
    SessionRuntimeManager::new(
        sessions,
        configuration,
        crate::credentials::CredentialSnapshot::capture(),
        crate::local_runtime::composition::LocalRuntimeDependencies::default(),
    )
    .map_err(|error| error.to_string())
}

/// The kind of descriptor a launcher inherited to this process.
///
/// A child-process launcher supplies whatever its platform layer creates. Rust's
/// own `Command` creates anonymous pipes; libuv — and therefore Node, Electron
/// and everything built on them — creates `AF_UNIX` socket pairs. Both are
/// ordinary byte streams the owner reads and writes; refusing one of them would
/// make the transport unusable from a whole class of hosts for no semantic
/// reason.
fn descriptor_kind(fd: std::os::fd::BorrowedFd<'_>) -> io::Result<nix::sys::stat::SFlag> {
    let status = nix::sys::stat::fstat(fd)?;
    Ok(nix::sys::stat::SFlag::from_bits_truncate(status.st_mode)
        .intersection(nix::sys::stat::SFlag::S_IFMT))
}

fn unix_stream(fd: std::os::fd::OwnedFd) -> io::Result<tokio::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::from(fd);
    stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(stream)
}

fn unsupported(kind: nix::sys::stat::SFlag) -> io::Error {
    io::Error::other(format!(
        "stdio requires a pipe or a socket on stdin and stdout, not {kind:?}"
    ))
}

/// Binds the inherited protocol input to the reactor.
fn inherited_reader(
    fd: std::os::fd::OwnedFd,
) -> io::Result<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
    match descriptor_kind(fd.as_fd())? {
        nix::sys::stat::SFlag::S_IFIFO => Ok(Box::new(
            tokio::net::unix::pipe::Receiver::from_owned_fd(fd)?,
        )),
        nix::sys::stat::SFlag::S_IFSOCK => Ok(Box::new(unix_stream(fd)?)),
        kind => Err(unsupported(kind)),
    }
}

/// Binds the inherited protocol output to the reactor.
fn inherited_writer(
    fd: std::os::fd::OwnedFd,
) -> io::Result<Box<dyn tokio::io::AsyncWrite + Unpin + Send>> {
    match descriptor_kind(fd.as_fd())? {
        nix::sys::stat::SFlag::S_IFIFO => {
            Ok(Box::new(tokio::net::unix::pipe::Sender::from_owned_fd(fd)?))
        }
        nix::sys::stat::SFlag::S_IFSOCK => Ok(Box::new(unix_stream(fd)?)),
        kind => Err(unsupported(kind)),
    }
}

async fn run(options: Options) -> Result<(), String> {
    // All user-scoped owners and signal listeners exist before readiness.
    let manager = compose(&options)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| error.to_string())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| error.to_string())?;
    let shutdown = CancellationToken::new();
    let serving = async {
        if options.listen == "stdio" {
            // Tokio's io::stdin uses an uncancellable blocking read. Reactor-owned
            // descriptors instead let disconnect/shutdown drop every pending I/O.
            let reader = inherited_reader(std::io::stdin().as_fd().try_clone_to_owned()?)?;
            let writer = inherited_writer(std::io::stdout().as_fd().try_clone_to_owned()?)?;
            stdio::serve(
                Arc::new(AppServerConnection::new(manager)),
                reader,
                writer,
                shutdown.clone(),
            )
            .await
        } else {
            let address: std::net::SocketAddr = options
                .listen
                .strip_prefix("ws://")
                .ok_or_else(|| io::Error::other("listen must be stdio or ws://IP:PORT"))?
                .parse()
                .map_err(io::Error::other)?;
            let path = options
                .token
                .ok_or_else(|| io::Error::other("WebSocket requires --token-file"))?;
            let mut token = String::new();
            std::fs::File::open(path)?
                .take(130)
                .read_to_string(&mut token)?;
            if token.len() > 129 {
                return Err(io::Error::other("transport token file exceeds limit"));
            }
            let credential =
                websocket::Credential::new(token.strip_suffix('\n').unwrap_or(&token).to_owned())?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            eprintln!("rustx app-server listening ws://{}", listener.local_addr()?);
            websocket::serve(listener, manager, credential, shutdown.clone()).await
        }
    };
    tokio::pin!(serving);
    let result = tokio::select! {
        result = &mut serving => result,
        _ = interrupt.recv() => { shutdown.cancel(); serving.await },
        _ = terminate.recv() => { shutdown.cancel(); serving.await },
    };
    result.map_err(|error| error.to_string())
}

/// Run the explicit App Server command. Diagnostics never reach protocol stdout.
pub async fn run_process(arguments: Vec<String>) -> i32 {
    if arguments == ["--help"] {
        eprintln!("{USAGE}");
        return 0;
    }
    let options = match Options::parse(arguments) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("rustx app-server: {error}\n{USAGE}");
            return 2;
        }
    };
    match run(options).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rustx app-server: {error}");
            2
        }
    }
}
