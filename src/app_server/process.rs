//! Standalone user-scoped composition, independent of the local Session launcher.
use super::host::AppServerHost;
use super::{
    connection::AppServerConnection,
    transport::{stdio, websocket},
};
use crate::local_runtime::{
    configuration::{UserConfigManager, UserConfigSources},
    launch::HostEnvironment,
    session_controller::SessionController,
    session_runtime_manager::{RuntimeResidencyPolicy, SessionRuntimeManager},
};
use std::os::unix::fs::OpenOptionsExt;
use std::{
    io::{self, Read},
    os::fd::AsFd,
    path::PathBuf,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

/// App Server lexical arguments deliberately exclude ordinary Session launch flags.
#[derive(Debug, clap::Args)]
pub(crate) struct AppServerArgs {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    runtime_root: Option<PathBuf>,
    /// Explicit transport: stdio or ws://IP:PORT
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    listen: String,
    /// Dedicated WebSocket credential file; forbidden for stdio
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// Alternative capability source: hexadecimal SHA-256 digest
    #[arg(long, conflicts_with_all = ["token_file", "shared_secret_file"])]
    token_sha256: Option<String>,
    /// Trusted host's HS256 signing secret; alternative to capability mode
    #[arg(long, conflicts_with = "token_file")]
    shared_secret_file: Option<PathBuf>,
    #[arg(long, requires = "shared_secret_file")]
    issuer: Option<String>,
    #[arg(long, requires = "shared_secret_file")]
    audience: Option<String>,
    #[arg(long, requires = "shared_secret_file")]
    max_clock_skew_seconds: Option<u64>,
    /// Separate Product Host-only file-read credential; never the browser token.
    #[arg(long)]
    product_host_token_file: Option<PathBuf>,
}
impl AppServerArgs {
    pub(crate) fn into_request(self) -> Request {
        Request {
            config: self.config,
            root: self.runtime_root,
            listen: self.listen,
            token: self.token_file,
            token_sha256: self.token_sha256,
            shared_secret: self.shared_secret_file,
            issuer: self.issuer,
            audience: self.audience,
            max_clock_skew_seconds: self.max_clock_skew_seconds,
            product_host_token: self.product_host_token_file,
        }
    }
}

/// Native process binding intent. Transport validation and composition stay here.
#[derive(Debug)]
pub struct Request {
    config: Option<PathBuf>,
    root: Option<PathBuf>,
    listen: String,
    token: Option<PathBuf>,
    token_sha256: Option<String>,
    shared_secret: Option<PathBuf>,
    issuer: Option<String>,
    audience: Option<String>,
    max_clock_skew_seconds: Option<u64>,
    product_host_token: Option<PathBuf>,
}

// These roots are exposed by SourceSettings in the JSON protocol. This is
// App Server policy, not a restriction on native configuration or CLI paths.
fn validate_wire_bindings(sources: &UserConfigSources) -> Result<(), String> {
    for (name, path) in [
        ("config", &sources.config_path),
        ("runtime-root", &sources.runtime_root),
        ("home/resource root", &sources.home_directory),
    ] {
        if path.to_str().is_none() {
            return Err(format!(
                "App Server {name} binding requires a lossless UTF-8 representation for the JSON protocol"
            ));
        }
    }
    Ok(())
}

fn compose(options: &Request) -> Result<AppServerHost, String> {
    let host = HostEnvironment::capture()?;
    for (name, path) in [
        ("--config", options.config.as_ref()),
        ("--runtime-root", options.root.as_ref()),
    ] {
        if path.is_some_and(|path| !path.is_absolute()) {
            return Err(format!("{name} requires an absolute process binding"));
        }
    }
    let absolute = |path: &PathBuf| path.clone();
    let config_path = options
        .config
        .as_ref()
        .map_or_else(|| host.config_directory.join("rustx.toml"), absolute);
    let sources = UserConfigSources {
        config_path,
        runtime_root: options.root.clone().unwrap_or(host.state_directory),
        home_directory: host.home_directory,
    };
    validate_wire_bindings(&sources)?;
    // Explicit selection is required; only the omitted canonical default may
    // be absent. Parsing and canonical source binding remain in the shared owner.
    if options.config.is_some()
        && !std::fs::metadata(&sources.config_path).is_ok_and(|metadata| metadata.is_file())
    {
        return Err("explicit user settings source must be an existing readable TOML file".into());
    }
    let configuration =
        UserConfigManager::bootstrap(sources, None).map_err(|error| error.to_string())?;
    // Canonical native bindings can differ through symlinks. Check those too,
    // before opening storage or admitting any protocol traffic.
    validate_wire_bindings(configuration.source_bindings())?;
    let sessions =
        SessionController::open(configuration.runtime_root()).map_err(|error| error.to_string())?;
    // Automatic repair of the bound User source precedes process policy
    // parsing and every transport. Its outcome is a ledger observation the
    // first client reads; an unrepairable source keeps its strict failure.
    // No Workspace is selected yet, so the scope is the User alone.
    configuration.repair_source(
        &crate::local_runtime::configuration::settings::SourceTarget::User,
        &crate::local_runtime::configuration::repair::RepairScope::user(),
    );
    let policy = configuration.app_server_policy()?;
    let manager = SessionRuntimeManager::new(
        sessions,
        configuration,
        crate::credentials::CredentialSnapshot::capture(),
        crate::local_runtime::composition::LocalRuntimeDependencies::default(),
        RuntimeResidencyPolicy {
            max_resident_runtimes: policy.max_resident_runtimes,
            idle_grace_ms: policy.idle_grace_ms,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(AppServerHost::new(manager, policy))
}

/// Read an owner-only private credential without following symlinks.
fn private_credential(path: &std::path::Path, lane: &str) -> io::Result<websocket::Credential> {
    use std::os::unix::fs::MetadataExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other(format!(
            "{lane} credential requires an owner-only regular file"
        )));
    }
    let mut token = String::new();
    file.take(130).read_to_string(&mut token)?;
    let token = token.strip_suffix('\n').unwrap_or(&token);
    if !(43..=128).contains(&token.len())
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Product Host credential must contain 43..128 base64url characters",
        ));
    }
    websocket::Credential::new(token.to_owned())
}

fn connection_credential(options: &Request) -> io::Result<websocket::Credential> {
    fn read(path: &std::path::Path) -> io::Result<String> {
        if !path.is_absolute() {
            return Err(io::Error::other("authentication paths must be absolute"));
        }
        let mut value = String::new();
        std::fs::File::open(path)?
            .take(4098)
            .read_to_string(&mut value)?;
        if value.len() > 4097 {
            return Err(io::Error::other("authentication file exceeds limit"));
        }
        Ok(value.strip_suffix('\n').unwrap_or(&value).to_owned())
    }
    match (
        &options.token,
        &options.token_sha256,
        &options.shared_secret,
    ) {
        (Some(path), None, None) => websocket::Credential::new(read(path)?),
        (None, Some(digest), None) => websocket::Credential::from_sha256(digest),
        (None, None, Some(path)) => websocket::Credential::signed(
            read(path)?.as_bytes(),
            options.issuer.clone(),
            options.audience.clone(),
            options.max_clock_skew_seconds.unwrap_or(30),
        ),
        _ => Err(io::Error::other(
            "WebSocket requires exactly one of --token-file, --token-sha256 or --shared-secret-file",
        )),
    }
}

async fn serve_transport(
    options: Request,
    host: AppServerHost,
    shutdown: CancellationToken,
) -> io::Result<()> {
    if options.listen == "stdio" {
        let _connection = host
            .admit_connection(false)
            .ok_or_else(|| io::Error::other("server draining"))?;
        // Tokio's io::stdin uses an uncancellable blocking read. Reactor-owned
        // pipe FDs instead let disconnect/shutdown drop every pending I/O.
        let input = std::io::stdin().as_fd().try_clone_to_owned()?;
        let output = std::io::stdout().as_fd().try_clone_to_owned()?;
        let reader = inherited_reader(input)?;
        let writer = inherited_writer(output)?;
        let connection = AppServerConnection::new(host);
        stdio::serve(Arc::new(connection), reader, writer, shutdown.clone()).await
    } else {
        let address: std::net::SocketAddr = options
            .listen
            .strip_prefix("ws://")
            .ok_or_else(|| io::Error::other("listen must be stdio or ws://IP:PORT"))?
            .parse()
            .map_err(io::Error::other)?;
        let credential = connection_credential(&options)?;
        if let Some(path) = &options.product_host_token {
            let product_host = private_credential(path, "Product Host")?;
            if credential.same_secret(&product_host) {
                return Err(io::Error::other(
                    "transport and Product Host credentials must differ",
                ));
            }
            host.bind_product_host(Some(product_host));
        }
        let listener = tokio::net::TcpListener::bind(address).await?;
        eprintln!(
            "rustx app-server protocol v{} (runtime-client v{})",
            super::protocol::APP_SERVER_PROTOCOL_VERSION,
            crate::runtime_client::types::RUNTIME_CLIENT_PROTOCOL_VERSION
        );
        eprintln!("rustx app-server listening ws://{}", listener.local_addr()?);
        websocket::serve(listener, host, credential, shutdown.clone()).await
    }
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

#[allow(clippy::too_many_lines)] // One process supervision and bounded shutdown sequence.
async fn run(options: Request) -> Result<(), String> {
    // Transport selection is native process policy, checked before composition.
    if options.listen == "stdio" {
        if options.token.is_some()
            || options.token_sha256.is_some()
            || options.shared_secret.is_some()
            || options.product_host_token.is_some()
        {
            return Err("stdio does not accept network authentication flags".into());
        }
    } else {
        options
            .listen
            .strip_prefix("ws://")
            .ok_or("listen must be stdio or ws://IP:PORT")?
            .parse::<std::net::SocketAddr>()
            .map_err(|error| error.to_string())?;
        // Validate before composition/listening; including loopback, there is
        // no unauthenticated network configuration.
        connection_credential(&options).map_err(|error| error.to_string())?;
    }
    // All user-scoped owners and signal listeners exist before readiness.
    let host = compose(&options)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| error.to_string())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| error.to_string())?;
    let shutdown = CancellationToken::new();
    let transport_host = host.clone();
    let transport_stop = shutdown.clone();
    let stdio_mode = options.listen == "stdio";
    let mut serving = tokio::spawn(serve_transport(options, transport_host, transport_stop));
    let reaper_host = host.clone();
    let reaper_stop = CancellationToken::new();
    let stop = reaper_stop.clone();
    let reaper = tokio::spawn(async move { reaper_host.manager().run_idle_reaper(stop).await });
    // External edits of supported sources enter native application without a
    // client; the watcher stops with the reaper at drain.
    let watch_manager = host.manager().clone();
    let watch_stop = reaper_stop.clone();
    let source_watch = tokio::spawn(async move {
        if let Err(error) = watch_manager.run_source_watch(watch_stop).await {
            eprintln!(
                "rustx app-server: configuration source watch unavailable ({error}); \
                 external edits apply after configuration/reconcile"
            );
        }
    });
    let schedule_manager = host.manager().clone();
    let mut scheduler = tokio::spawn(async move { schedule_manager.run_schedule_driver().await });
    let mut scheduler_early_result = None;
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
            result = &mut scheduler => {
                scheduler_early_result=Some(result);
                transport_failure=Some("Scheduler driver stopped before process drain".into());
                break;
            },
            _ = interrupt.recv() => break,
            _ = terminate.recv() => break,
        }
    }
    host.begin_drain();
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(host.policy().shutdown_deadline_ms);
    reaper_stop.cancel();
    eprintln!("rustx app-server: Draining; new semantic admission closed");
    let graceful = async {
        let scheduler_completion = async {
            match scheduler_early_result.take() {
                Some(result) => result,
                None => (&mut scheduler).await,
            }
        };
        let (failures, reaper_result, scheduler_result, _) =
            tokio::join!(host.drain(), reaper, scheduler_completion, source_watch);
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
        scheduler_result
            .map_err(|error| format!("Scheduler driver failed: {error}"))?
            .map_err(|error| error.to_string())?;
        host.finish_drain()
    };
    tokio::pin!(graceful);
    tokio::select! {
        result = &mut graceful => result,
        () = tokio::time::sleep_until(deadline) => {
            force_exit("drain deadline exceeded", &host)
        },
        _ = interrupt.recv() => force_exit("second termination request", &host),
        _ = terminate.recv() => force_exit("second termination request", &host),
    }
}

// Executor destruction may wait on blocking operations. Forced host exit
// bypasses destruction; it neither claims nor fabricates semantic settlement.
fn force_exit(phase: &str, host: &AppServerHost) -> ! {
    eprintln!(
        "rustx app-server: forced host termination: {phase}; runtime settlement unproven: {:?}",
        host.forced_resources(phase == "drain deadline exceeded")
    );
    std::process::exit(3)
}

/// Run the explicit App Server command. Diagnostics never reach protocol stdout.
pub async fn run_process(options: Request) -> i32 {
    match run(options).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rustx app-server: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_product_host_credential_rejects_weak_or_truncated_values() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for length in [43, 128] {
            for suffix in ["", "\n"] {
                std::fs::write(file.path(), format!("{}{suffix}", "a".repeat(length))).unwrap();
                assert!(super::private_credential(file.path(), "Product Host").is_ok());
            }
        }
        for token in [
            String::new(),
            "a".to_owned(),
            "a".repeat(42),
            "a".repeat(129),
            "a".repeat(4096),
            format!("{}+", "a".repeat(43)),
            format!("{}=", "a".repeat(43)),
            format!("{}\nextra", "a".repeat(128)),
        ] {
            std::fs::write(file.path(), token).unwrap();
            assert!(super::private_credential(file.path(), "Product Host").is_err());
        }
    }

    #[test]
    fn cli04_app_server_omitted_bindings_stay_absent() {
        let crate::local_runtime::cli::Command::AppServer(request) =
            crate::local_runtime::cli::parse_command(
                ["app-server", "--listen=stdio"].map(str::to_owned),
            )
            .unwrap()
        else {
            panic!("App Server intent")
        };
        assert!(request.config.is_none() && request.root.is_none() && request.token.is_none());
        assert_eq!(request.listen, "stdio");
    }

    #[test]
    fn app_server_paths_and_listen_survive_public_cli_conversion() {
        for value in [" /tmp/rustx path ", "/tmp/rustx path ", " "] {
            let crate::local_runtime::cli::Command::AppServer(request) =
                crate::local_runtime::cli::parse_command(
                    [
                        "app-server",
                        "--config",
                        value,
                        "--runtime-root",
                        value,
                        "--token-file",
                        value,
                        "--listen",
                        " stdio ",
                    ]
                    .map(str::to_owned),
                )
                .unwrap()
            else {
                panic!("App Server intent")
            };
            for path in [request.config, request.root, request.token] {
                assert_eq!(path.unwrap().as_os_str(), std::ffi::OsStr::new(value));
            }
            assert_eq!(request.listen, " stdio ");
        }
    }
}
