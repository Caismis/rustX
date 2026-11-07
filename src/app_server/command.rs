//! Connection-owned standalone commands using the native supervision kernel.
use super::protocol::{ErrorData, NotificationMethod, RequestId, RpcError};
use super::rpc_error::domain;
use crate::runtime::CancellationSignal;
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, ProgramInvocation, SupervisedCommandRunner, SupervisedCommandSpec,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecParams {
    #[schemars(length(min = 1, max = 256))]
    pub command: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub env: BTreeMap<String, Option<String>>,
    pub process_id: Option<String>,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub stream_stdin: bool,
    #[serde(default)]
    pub stream_stdout_stderr: bool,
    #[schemars(range(min = 1, max = 262_144))]
    pub output_bytes_cap: Option<usize>,
    #[schemars(range(min = 1, max = 3_600_000))]
    pub timeout_ms: Option<u32>,
    pub size: Option<TerminalSize>,
}
impl std::fmt::Debug for ExecParams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecParams")
            .field("process_id", &self.process_id)
            .field("tty", &self.tty)
            .field("environment_keys", &self.env.keys())
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSize {
    #[schemars(range(min = 1, max = 1000))]
    pub rows: u16,
    #[schemars(range(min = 1, max = 1000))]
    pub cols: u16,
}
impl TerminalSize {
    fn validate(self) -> Result<Self, RpcError> {
        if self.rows == 0 || self.cols == 0 || self.rows > 1000 || self.cols > 1000 {
            return Err(domain(ErrorData::InvalidParams));
        }
        Ok(self)
    }
    fn native(self) -> nix::pty::Winsize {
        nix::pty::Winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteParams {
    pub process_id: String,
    pub delta_base64: Option<String>,
    #[serde(default)]
    pub close_stdin: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub status: ExecStatus,
    pub stdout_base64: String,
    pub stderr_base64: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecStatus {
    Completed,
    TimedOut,
    Terminated,
    ProcessControlFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputDelta {
    pub process_id: String,
    pub sequence: u64,
    pub stream: OutputStream,
    pub delta_base64: String,
    pub cap_reached: bool,
}

enum Control {
    Write(Vec<u8>, bool, oneshot::Sender<Result<(), RpcError>>),
    Resize(TerminalSize, oneshot::Sender<Result<(), RpcError>>),
}
struct Entry {
    cancellation: CancellationSignal,
    control: mpsc::Sender<Control>,
}
#[derive(Default)]
struct State {
    closed: bool,
    active: BTreeMap<String, Entry>,
}
pub(super) struct Commands(Mutex<State>, tokio::sync::watch::Sender<u64>);
impl Default for Commands {
    fn default() -> Self {
        Self(Mutex::default(), tokio::sync::watch::channel(0).0)
    }
}
impl Commands {
    #[cfg(test)]
    pub(crate) async fn admitted_for_test(&self, id: &str) {
        self.1
            .subscribe()
            .wait_for(|_| self.0.lock().expect("commands").active.contains_key(id))
            .await
            .unwrap();
    }
    pub fn close(&self) {
        let mut state = self.0.lock().expect("connection commands");
        state.closed = true;
        for entry in state.active.values() {
            entry.cancellation.cancel();
        }
    }
    pub fn start(
        self: &Arc<Self>,
        params: ExecParams,
        environment: Arc<crate::local_runtime::environment::Environment>,
        events: Arc<super::service_events::Events>,
        owner: super::host::ServerOperation,
    ) -> Result<oneshot::Receiver<Result<ExecResult, RpcError>>, RpcError> {
        validate(&params)?;
        let process_id = params
            .process_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let cancellation = CancellationSignal::new();
        let (control, controls) = mpsc::channel(2);
        let mut state = self.0.lock().expect("connection commands");
        if state.closed || state.active.len() >= 4 || state.active.contains_key(&process_id) {
            return Err(domain(ErrorData::EnvironmentCapacityOrConflict));
        }
        state.active.insert(
            process_id.clone(),
            Entry {
                cancellation: cancellation.clone(),
                control,
            },
        );
        self.1.send_modify(|revision| *revision += 1);
        let (sender, received) = oneshot::channel();
        let commands = self.clone();
        tokio::spawn(async move {
            let _owner = owner;
            let result = run(params, environment, events, cancellation, controls).await;
            commands
                .0
                .lock()
                .expect("connection commands")
                .active
                .remove(&process_id);
            let _ = sender.send(result);
        });
        Ok(received)
    }
    fn control(&self, process_id: &str) -> Result<mpsc::Sender<Control>, RpcError> {
        let state = self.0.lock().expect("connection commands");
        if state.closed {
            return Err(domain(ErrorData::ConnectionClosed));
        }
        state
            .active
            .get(process_id)
            .map(|entry| entry.control.clone())
            .ok_or_else(|| domain(ErrorData::EnvironmentResourceNotFound))
    }
    pub async fn write(&self, params: WriteParams) -> Result<(), RpcError> {
        if params
            .delta_base64
            .as_ref()
            .is_some_and(|data| data.len() > 87_384)
            || (params.delta_base64.is_none() && !params.close_stdin)
        {
            return Err(domain(ErrorData::InvalidParams));
        }
        let bytes = params
            .delta_base64
            .map(|data| base64::engine::general_purpose::STANDARD.decode(data))
            .transpose()
            .map_err(|_| domain(ErrorData::InvalidParams))?
            .unwrap_or_default();
        if bytes.len() > 64 * 1024 {
            return Err(domain(ErrorData::InvalidParams));
        }
        let (sent, received) = oneshot::channel();
        self.control(&params.process_id)?
            .try_send(Control::Write(bytes, params.close_stdin, sent))
            .map_err(|_| domain(ErrorData::EnvironmentCapacityOrConflict))?;
        received
            .await
            .map_err(|_| domain(ErrorData::EnvironmentResourceNotFound))?
    }
    pub async fn resize(&self, process_id: &str, size: TerminalSize) -> Result<(), RpcError> {
        let (sent, received) = oneshot::channel();
        self.control(process_id)?
            .try_send(Control::Resize(size.validate()?, sent))
            .map_err(|_| domain(ErrorData::EnvironmentCapacityOrConflict))?;
        received
            .await
            .map_err(|_| domain(ErrorData::EnvironmentResourceNotFound))?
    }
    pub fn terminate(&self, process_id: &str) -> Result<(), RpcError> {
        let state = self.0.lock().expect("connection commands");
        if state.closed {
            return Err(domain(ErrorData::ConnectionClosed));
        }
        state
            .active
            .get(process_id)
            .ok_or_else(|| domain(ErrorData::EnvironmentResourceNotFound))?
            .cancellation
            .cancel();
        Ok(())
    }
}
fn validate(params: &ExecParams) -> Result<(), RpcError> {
    if params.command.is_empty()
        || params.command.len() > 256
        || params.command[0].is_empty()
        || params.command.iter().any(|part| part.contains('\0'))
        || params.command.iter().map(String::len).sum::<usize>() > 64 * 1024
        || params.env.len() > 128
        || params.env.iter().any(|(key, value)| {
            key.len() > 256
                || value
                    .as_ref()
                    .is_some_and(|value| value.len() > 8192 || value.contains('\0'))
        })
        || params
            .output_bytes_cap
            .is_some_and(|cap| cap == 0 || cap > 262_144)
        || params
            .timeout_ms
            .is_some_and(|timeout| timeout == 0 || timeout > 3_600_000)
        || params.size.is_some() && !params.tty
        || (params.tty || params.stream_stdin || params.stream_stdout_stderr)
            && params.process_id.is_none()
        || params
            .process_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || !RequestId::is_valid_string(id))
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    if let Some(size) = params.size {
        size.validate()?;
    }
    crate::tools::environment::ToolEnvironment::from_authorized(
        params
            .env
            .iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone()))),
    )
    .map_err(|_| domain(ErrorData::InvalidParams))?;
    Ok(())
}

struct Terminal {
    master: tokio::io::unix::AsyncFd<OwnedFd>,
    slave: Option<OwnedFd>,
    path: PathBuf,
}
impl Terminal {
    fn open(size: TerminalSize) -> std::io::Result<Self> {
        let pty = nix::pty::openpty(Some(&size.native()), None)?;
        let path = nix::unistd::ttyname(&pty.slave)?;
        nix::fcntl::fcntl(
            &pty.master,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )?;
        Ok(Self {
            master: tokio::io::unix::AsyncFd::new(pty.master)?,
            slave: Some(pty.slave),
            path,
        })
    }
    async fn read(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let mut ready = self.master.readable().await?;
            match ready
                .try_io(|fd| rustix::io::read(fd.get_ref(), &mut *buffer).map_err(Into::into))
            {
                Ok(Err(error)) if error.raw_os_error() == Some(libc::EIO) => return Ok(0),
                Ok(result) => return result,
                Err(_) => {}
            }
        }
    }
    async fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            let mut ready = self.master.writable().await?;
            if let Ok(result) =
                ready.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(Into::into))
            {
                let count = result?;
                if count == 0 {
                    return Err(std::io::ErrorKind::WriteZero.into());
                }
                return Ok(count);
            }
        }
    }
    fn resize(&self, size: TerminalSize) -> std::io::Result<()> {
        rustix::termios::tcsetwinsize(
            self.master.get_ref(),
            rustix::termios::Winsize {
                ws_row: size.rows,
                ws_col: size.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
        Ok(())
    }
    fn eof(&self) -> std::io::Result<[u8; 3]> {
        use nix::sys::termios::{InputFlags, LocalFlags, SpecialCharacterIndices as C, tcgetattr};
        let attributes = tcgetattr(self.master.get_ref())?;
        let eof = attributes.control_chars[C::VEOF as usize];
        let conflicts = |characters: &[C]| {
            characters
                .iter()
                .any(|character| attributes.control_chars[*character as usize] == eof)
        };
        #[cfg(target_os = "macos")]
        let platform_conflict = attributes
            .local_flags
            .intersects(LocalFlags::ISIG | LocalFlags::IEXTEN)
            && conflicts(&[C::VDSUSP, C::VSTATUS]);
        #[cfg(target_os = "linux")]
        let platform_conflict =
            eof.is_ascii_uppercase() && attributes.input_flags.contains(InputFlags::IUCLC);
        if !attributes.local_flags.contains(LocalFlags::ICANON)
            || attributes.local_flags.contains(LocalFlags::EXTPROC)
            || platform_conflict
            || eof == libc::_POSIX_VDISABLE
            || eof == b'\n'
            || eof == b'\r'
                && attributes
                    .input_flags
                    .intersects(InputFlags::ICRNL | InputFlags::IGNCR)
            || eof >= 128 && attributes.input_flags.contains(InputFlags::ISTRIP)
            || conflicts(&[C::VERASE, C::VKILL, C::VEOL, C::VEOL2])
            || attributes.local_flags.contains(LocalFlags::ISIG)
                && conflicts(&[C::VINTR, C::VQUIT, C::VSUSP])
            || attributes.local_flags.contains(LocalFlags::IEXTEN)
                && conflicts(&[C::VLNEXT, C::VWERASE, C::VREPRINT, C::VDISCARD])
            || attributes.input_flags.contains(InputFlags::IXON)
                && conflicts(&[C::VSTART, C::VSTOP])
        {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        // Pending VLNEXT may quote the first byte. A further VEOF releases a
        // pending canonical line, then the final VEOF ends the following read.
        Ok([eof; 3])
    }
}
struct PendingInput {
    bytes: Vec<u8>,
    offset: usize,
    close: bool,
    receipt: oneshot::Sender<Result<(), RpcError>>,
}
#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    total: usize,
    truncated: bool,
}
impl Output {
    fn push(
        &mut self,
        bytes: &[u8],
        cap: usize,
        stream: OutputStream,
        params: &ExecParams,
        events: &super::service_events::Events,
        sequence: &mut u64,
    ) -> Result<Option<oneshot::Receiver<()>>, RpcError> {
        let available = cap.saturating_sub(self.total);
        let retained = &bytes[..bytes.len().min(available)];
        let newly_truncated = !self.truncated && bytes.len() > available;
        self.truncated |= newly_truncated;
        self.total = self.total.saturating_add(bytes.len());
        if params.tty || params.stream_stdout_stderr {
            if retained.is_empty() && !newly_truncated {
                return Ok(None);
            }
            *sequence += 1;
            events
                .emit(NotificationMethod::CommandOutputDelta(OutputDelta {
                    process_id: params
                        .process_id
                        .clone()
                        .expect("validated streaming identity"),
                    sequence: *sequence,
                    stream,
                    delta_base64: base64::engine::general_purpose::STANDARD.encode(retained),
                    cap_reached: newly_truncated,
                }))
                .map(Some)
                .map_err(|()| domain(ErrorData::ConnectionClosed))
        } else {
            self.bytes.extend_from_slice(retained);
            Ok(None)
        }
    }
}

#[allow(clippy::too_many_lines)] // One actor owns the finite process, I/O, controls and terminal fence.
async fn run(
    params: ExecParams,
    environment: Arc<crate::local_runtime::environment::Environment>,
    events: Arc<super::service_events::Events>,
    cancellation: CancellationSignal,
    mut controls: mpsc::Receiver<Control>,
) -> Result<ExecResult, RpcError> {
    let requested_cwd = params.cwd.clone();
    let (cwd, cwd_identity, root_retired) =
        tokio::task::spawn_blocking(move || environment.directory(&requested_cwd))
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(super::connection::environment_error)?;
    if cancellation.is_cancelled() || root_retired.is_cancelled() || events.closed.is_cancelled() {
        return Err(domain(ErrorData::ConnectionClosed));
    }
    let temporary = tempfile::tempdir().map_err(|_| domain(ErrorData::OperationFailed))?;
    let input_socket = temporary.path().join("stdin");
    let mut terminal = params
        .tty
        .then(|| Terminal::open(params.size.unwrap_or(TerminalSize { rows: 24, cols: 80 })))
        .transpose()
        .map_err(|_| domain(ErrorData::OperationFailed))?;
    let listener = if !params.tty && params.stream_stdin {
        Some(
            tokio::net::UnixListener::bind(&input_socket)
                .map_err(|_| domain(ErrorData::OperationFailed))?,
        )
    } else {
        None
    };
    let tool_environment = crate::tools::environment::ToolEnvironment::from_authorized(
        params
            .env
            .iter()
            .filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone()))),
    )
    .map_err(|_| domain(ErrorData::InvalidParams))?;
    let spec = SupervisedCommandSpec {
        command: "standalone argv".into(),
        cwd: cwd.clone(),
        command_environment: tool_environment.child_environment(&cwd),
        timeout: Some(std::time::Duration::from_millis(u64::from(
            params.timeout_ms.unwrap_or(60_000),
        ))),
        cancellation: cancellation.clone(),
    };
    let invocation = ProgramInvocation {
        argv: params.command.clone(),
        stdin_socket: listener.as_ref().map(|_| input_socket),
        terminal: terminal.as_ref().map(|terminal| terminal.path.clone()),
        cwd_identity,
    };
    let (mut runner, mut stdout, mut stderr) =
        SupervisedCommandRunner::spawn_program(&spec, None, Some(&invocation))
            .map_err(|_| domain(ErrorData::OperationFailed))?;
    let settlement = tokio::spawn(async move { runner.settle().await });
    tokio::pin!(settlement);
    let mut input: Option<tokio::net::UnixStream> = None;
    let mut input_closed = !params.tty && !params.stream_stdin;
    let mut pending_input: Option<PendingInput> = None;
    let mut termination = None;
    let mut stdout_output = Output::default();
    let mut stderr_output = Output::default();
    let mut sequence = 0;
    let mut last_receipt = None;
    let mut output_failed = false;
    let mut buffer = vec![0u8; 8192];
    let mut error_buffer = vec![0u8; 8192];
    let mut terminal_buffer = vec![0u8; 8192];
    let cap = params.output_bytes_cap.unwrap_or(64 * 1024);
    let mut terminal_eof = terminal.is_none();
    loop {
        if termination.is_some() && stdout.is_none() && stderr.is_none() && terminal_eof {
            break;
        }
        tokio::select! {
            result = &mut settlement, if termination.is_none() => {
                termination = Some(result.map_err(|_| domain(ErrorData::OutcomeUnknown))?);
                if let Some(terminal) = &mut terminal { terminal.slave.take(); }
                input.take();
                input_closed = true;
                if let Some(pending) = pending_input.take() {
                    let _ = pending.receipt.send(Err(domain(ErrorData::OutcomeUnknown)));
                }
            },
            () = root_retired.cancelled(), if !cancellation.is_cancelled() => cancellation.cancel(),
            () = events.closed.cancelled(), if !cancellation.is_cancelled() => cancellation.cancel(),
            accepted = async { listener.as_ref().expect("stdin listener").accept().await }, if listener.is_some() && input.is_none() && !input_closed && termination.is_none() => {
                if let Ok((stream, _)) = accepted { input = Some(stream); } else { cancellation.cancel(); input_closed = true; }
            },
            read = async { stdout.as_mut().expect("stdout").read(&mut buffer).await }, if stdout.is_some() => {
                match read {
                    Ok(0) => { stdout.take(); },
                    Ok(count) => match stdout_output.push(&buffer[..count], cap, OutputStream::Stdout, &params, &events, &mut sequence) { Ok(Some(receipt)) => last_receipt = Some(receipt), Ok(None) => {}, Err(_) => { output_failed = true; cancellation.cancel(); } },
                    Err(_) => { stdout.take(); output_failed = true; cancellation.cancel(); },
                }
            },
            read = async { stderr.as_mut().expect("stderr").read(&mut error_buffer).await }, if stderr.is_some() => {
                match read {
                    Ok(0) => { stderr.take(); },
                    Ok(count) => match stderr_output.push(&error_buffer[..count], cap, OutputStream::Stderr, &params, &events, &mut sequence) { Ok(Some(receipt)) => last_receipt = Some(receipt), Ok(None) => {}, Err(_) => { output_failed = true; cancellation.cancel(); } },
                    Err(_) => { stderr.take(); output_failed = true; cancellation.cancel(); },
                }
            },
            read = async { terminal.as_ref().expect("terminal").read(&mut terminal_buffer).await }, if !terminal_eof => {
                match read {
                    Ok(0) => terminal_eof = true,
                    Ok(count) => match stdout_output.push(&terminal_buffer[..count], cap, OutputStream::Stdout, &params, &events, &mut sequence) { Ok(Some(receipt)) => last_receipt = Some(receipt), Ok(None) => {}, Err(_) => { output_failed = true; cancellation.cancel(); } },
                    Err(_) => { terminal_eof = true; output_failed = true; cancellation.cancel(); },
                }
            },
            written = async {
                let pending = pending_input.as_ref().expect("pending stdin");
                let bytes = &pending.bytes[pending.offset..];
                if let Some(terminal) = &terminal { terminal.write(bytes).await }
                else { input.as_mut().expect("connected stdin").write(bytes).await }
            }, if pending_input.is_some() && termination.is_none() => {
                let pending = pending_input.as_mut().expect("pending stdin");
                match written {
                    Ok(count) if count > 0 => {
                        pending.offset += count;
                        if pending.offset == pending.bytes.len() {
                            let pending = pending_input.take().expect("completed stdin");
                            if pending.close { input_closed = true; input.take(); }
                            let _ = pending.receipt.send(Ok(()));
                        }
                    },
                    _ => {
                        let pending = pending_input.take().expect("failed stdin");
                        let _ = pending.receipt.send(Err(domain(ErrorData::OutcomeUnknown)));
                        input_closed = true;
                        input.take();
                        cancellation.cancel();
                    },
                }
            },
            control = controls.recv(), if termination.is_none() && (input.is_some() || terminal.is_some() || input_closed) => {
                match control {
                    Some(Control::Write(mut bytes, close, receipt)) => {
                        if input_closed {
                            let _ = receipt.send(Err(domain(ErrorData::EnvironmentResourceNotFound)));
                        } else if pending_input.is_some() {
                            let _ = receipt.send(Err(domain(ErrorData::EnvironmentCapacityOrConflict)));
                        } else {
                            if let Some(terminal) = &terminal && close {
                                if let Ok(eof) = terminal.eof() { bytes.extend_from_slice(&eof); }
                                else {
                                    let _ = receipt.send(Err(domain(ErrorData::InvalidParams)));
                                    continue;
                                }
                            }
                            if bytes.is_empty() {
                                if close { input_closed = true; input.take(); }
                                let _ = receipt.send(Ok(()));
                            } else {
                                pending_input = Some(PendingInput { bytes, offset: 0, close, receipt });
                            }
                        }
                    },
                    Some(Control::Resize(size, receipt)) => { let _ = receipt.send(terminal.as_ref().ok_or_else(|| domain(ErrorData::InvalidParams)).and_then(|terminal| terminal.resize(size).map_err(|_| domain(ErrorData::OperationFailed)))); },
                    None => cancellation.cancel(),
                }
            },
        }
    }
    if let Some(receipt) = last_receipt {
        events
            .emitted(receipt)
            .await
            .map_err(|()| domain(ErrorData::ConnectionClosed))?;
    }
    let termination = termination.expect("native process settled");
    let status = if output_failed {
        ExecStatus::ProcessControlFailed
    } else {
        match termination.intent {
            ProcessOutcomeIntent::Completed => ExecStatus::Completed,
            ProcessOutcomeIntent::Cancelled => ExecStatus::Terminated,
            ProcessOutcomeIntent::TimedOut => ExecStatus::TimedOut,
            ProcessOutcomeIntent::ProcessControlFailed(_) => ExecStatus::ProcessControlFailed,
        }
    };
    Ok(ExecResult {
        exit_code: termination.exit_status.and_then(|status| status.code()),
        status,
        stdout_base64: base64::engine::general_purpose::STANDARD.encode(stdout_output.bytes),
        stderr_base64: base64::engine::general_purpose::STANDARD.encode(stderr_output.bytes),
        stdout_truncated: stdout_output.truncated,
        stderr_truncated: stderr_output.truncated,
    })
}
