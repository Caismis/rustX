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

pub const USAGE: &str = "usage: rustx app-server [--models <models.toml>] [--runtime-root <path>] --listen <stdio|ws://IP:PORT> [--token-file <path>]\nUser settings: $XDG_CONFIG_HOME/rustx/settings.toml (default ~/.config/rustx/settings.toml).\n--models and --runtime-root override canonical user TOML source bindings.\nWebSocket requires a dedicated base64url token file. stdio requires owned pipes.";

struct Options {
    models: Option<PathBuf>,
    root: Option<PathBuf>,
    listen: String,
    token: Option<PathBuf>,
}
impl Options {
    fn parse(arguments: Vec<String>) -> Result<Self, String> {
        let mut options = Self {
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
    let configuration = UserConfigManager::bootstrap(
        UserConfigSources {
            settings: host.config_directory.join("settings.toml"),
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
            // pipe FDs instead let disconnect/shutdown drop every pending I/O.
            let input = std::io::stdin().as_fd().try_clone_to_owned()?;
            let output = std::io::stdout().as_fd().try_clone_to_owned()?;
            let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(input)?;
            let writer = tokio::net::unix::pipe::Sender::from_owned_fd(output)?;
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
