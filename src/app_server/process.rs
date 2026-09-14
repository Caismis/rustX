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

async fn serve_transport(
    options: Options,
    manager: SessionRuntimeManager,
    shutdown: CancellationToken,
) -> io::Result<()> {
    if options.listen == "stdio" {
        let _connection = manager
            .admit_connection(false)
            .ok_or_else(|| io::Error::other("server draining"))?;
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
}

async fn run(options: Options) -> Result<(), String> {
    // All user-scoped owners and signal listeners exist before readiness.
    let manager = compose(&options)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| error.to_string())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| error.to_string())?;
    let shutdown = CancellationToken::new();
    let transport_manager = manager.clone();
    let transport_stop = shutdown.clone();
    let stdio_mode = options.listen == "stdio";
    let mut serving = tokio::spawn(serve_transport(options, transport_manager, transport_stop));
    let reaper_manager = manager.clone();
    let reaper_stop = CancellationToken::new();
    let stop = reaper_stop.clone();
    let reaper = tokio::spawn(async move { reaper_manager.run_idle_reaper(stop).await });
    let mut transport_finished = false;
    let mut transport_failure = None;
    loop {
        tokio::select! {
            result = &mut serving, if !transport_finished => {
                transport_finished = true;
                match result {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => { transport_failure = Some(error.to_string()); },
                    Err(error) => { transport_failure = Some(error.to_string()); },
                }
                if !stdio_mode { break; }
                // EOF is only detachment. An owned child must receive SIGTERM
                // (or SIGINT) from its owner to request semantic drain.
                eprintln!("rustx app-server: stdio detached; awaiting explicit owner shutdown");
            },
            _ = interrupt.recv() => break,
            _ = terminate.recv() => break,
        }
    }
    manager.begin_drain();
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(manager.policy().shutdown_deadline_ms);
    reaper_stop.cancel();
    eprintln!("rustx app-server: Draining; new semantic admission closed");
    let graceful = async {
        let (failures, reaper_result) = tokio::join!(manager.drain(), reaper);
        shutdown.cancel();
        if !transport_finished {
            match (&mut serving).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => transport_failure = Some(error.to_string()),
                Err(error) => transport_failure = Some(error.to_string()),
            }
        }
        if !failures.is_empty() {
            return Err(format!(
                "runtime settlement failed: {}",
                failures.join("; ")
            ));
        }
        if let Some(error) = transport_failure {
            return Err(format!("transport failed: {error}"));
        }
        reaper_result.map_err(|error| format!("idle reaper failed: {error}"))?;
        manager.finish_drain().map_err(|error| error.to_string())
    };
    tokio::pin!(graceful);
    tokio::select! {
        result = &mut graceful => result,
        () = tokio::time::sleep_until(deadline) => {
            force_exit("drain deadline exceeded", &manager)
        },
        _ = interrupt.recv() => force_exit("second termination request", &manager),
        _ = terminate.recv() => force_exit("second termination request", &manager),
    }
}

// Executor destruction may wait on blocking operations. Forced host exit
// bypasses destruction; it neither claims nor fabricates semantic settlement.
fn force_exit(phase: &str, manager: &SessionRuntimeManager) -> ! {
    eprintln!(
        "rustx app-server: forced host termination: {phase}; runtime settlement unproven: {:?}",
        manager.forced_resources(phase == "drain deadline exceeded")
    );
    std::process::exit(3)
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
